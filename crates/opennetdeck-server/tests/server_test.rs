use bytes::{Buf, BytesMut};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::timeout;

use opennetdeck_protocol::{
    build_keepalive_ack_frame, ChildDeviceInfo, CoraFlags, CoraFrame, CoraHeader, CoraHidOp,
    PRODUCT_ID_NETWORK_DOCK, VENDOR_ID_ELGATO,
};
use opennetdeck_server::{DockConfig, DockState, PrimaryPortServer};

async fn read_next_frame(client: &mut TcpStream, read_buf: &mut BytesMut) -> (CoraHeader, Vec<u8>) {
    let mut chunk = [0u8; 2048];
    loop {
        if !read_buf.is_empty() {
            if let Ok(Some((frame, consumed))) = CoraFrame::decode(read_buf) {
                let header = frame.header;
                let payload = frame.payload.to_vec();
                read_buf.advance(consumed);
                return (header, payload);
            }
        }

        let n = timeout(Duration::from_secs(3), client.read(&mut chunk))
            .await
            .expect("Timeout waiting for data")
            .expect("Read error");
        assert!(n > 0, "Socket closed prematurely");
        read_buf.extend_from_slice(&chunk[..n]);
    }
}

async fn read_next_response(
    client: &mut TcpStream,
    read_buf: &mut BytesMut,
    expected_msg_id: u32,
) -> (CoraHeader, Vec<u8>) {
    loop {
        let (header, payload) = read_next_frame(client, read_buf).await;
        if header.message_id == expected_msg_id {
            return (header, payload);
        }
    }
}

#[tokio::test]
async fn test_primary_port_server_protocol_lifecycle() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let local_addr = listener.local_addr().unwrap();
    drop(listener);

    let config = DockConfig {
        serial_number: "TEST_SERIAL_123".to_string(),
        firmware_version: "2.1.0.0".to_string(),
        mac_address: [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF],
        primary_port: local_addr.port(),
        secondary_port: 5344,
        mode: opennetdeck_server::dock::ServerMode::Dock,
    };

    let state = DockState::new(config);
    let server_state = state.clone();

    let server = PrimaryPortServer::new(local_addr, server_state);
    tokio::spawn(async move {
        let _ = server.run().await;
    });

    tokio::time::sleep(Duration::from_millis(50)).await;

    let mut client = TcpStream::connect(local_addr).await.unwrap();
    let mut read_buf = BytesMut::with_capacity(4096);

    // 1. Receive initial keepalive probe
    let (header, payload) = read_next_frame(&mut client, &mut read_buf).await;
    assert_eq!(payload[0], 0x01);
    assert_eq!(payload[1], 0x0a);
    let conn_id = payload[5];

    // 2. Respond with Keepalive ACK
    let mut ack_buf = [0u8; 64];
    let ack_len = build_keepalive_ack_frame(conn_id, header.message_id, &mut ack_buf).unwrap();
    client.write_all(&ack_buf[..ack_len]).await.unwrap();

    // 3. Query Device Info (0x80)
    let get_device_info_query = [0x03, 0x80];
    let header = CoraHeader::new(
        CoraFlags::NONE,
        CoraHidOp::GetReport,
        101,
        get_device_info_query.len(),
    );
    let mut query_frame_buf = [0u8; 64];
    let q_len = CoraFrame::new(header, &get_device_info_query)
        .encode(&mut query_frame_buf)
        .unwrap();
    client.write_all(&query_frame_buf[..q_len]).await.unwrap();

    let (_resp_hdr, resp_payload) = read_next_response(&mut client, &mut read_buf, 101).await;
    assert_eq!(resp_payload[0], 0x03);
    assert_eq!(resp_payload[1], 0x80);
    let vid = u16::from_le_bytes([resp_payload[12], resp_payload[13]]);
    let pid = u16::from_le_bytes([resp_payload[14], resp_payload[15]]);
    assert_eq!(vid, VENDOR_ID_ELGATO);
    assert_eq!(pid, PRODUCT_ID_NETWORK_DOCK);

    // 4. Query Firmware Version (0x83)
    let get_fw_query = [0x03, 0x83];
    let header = CoraHeader::new(
        CoraFlags::NONE,
        CoraHidOp::GetReport,
        102,
        get_fw_query.len(),
    );
    let q_len = CoraFrame::new(header, &get_fw_query)
        .encode(&mut query_frame_buf)
        .unwrap();
    client.write_all(&query_frame_buf[..q_len]).await.unwrap();

    let (_resp_hdr, resp_payload) = read_next_response(&mut client, &mut read_buf, 102).await;
    let fw_str = std::str::from_utf8(&resp_payload[8..15]).unwrap();
    assert_eq!(fw_str, "2.1.0.0");

    // 5. Query Serial Number (0x84)
    let get_sn_query = [0x03, 0x84];
    let header = CoraHeader::new(
        CoraFlags::NONE,
        CoraHidOp::GetReport,
        103,
        get_sn_query.len(),
    );
    let q_len = CoraFrame::new(header, &get_sn_query)
        .encode(&mut query_frame_buf)
        .unwrap();
    client.write_all(&query_frame_buf[..q_len]).await.unwrap();

    let (_resp_hdr, resp_payload) = read_next_response(&mut client, &mut read_buf, 103).await;
    let sn_len = resp_payload[3] as usize;
    let sn_str = std::str::from_utf8(&resp_payload[4..4 + sn_len]).unwrap();
    assert_eq!(sn_str, "TEST_SERIAL_123");

    // 6. Query MAC Address (0x85)
    let get_mac_query = [0x03, 0x85];
    let header = CoraHeader::new(
        CoraFlags::NONE,
        CoraHidOp::GetReport,
        104,
        get_mac_query.len(),
    );
    let q_len = CoraFrame::new(header, &get_mac_query)
        .encode(&mut query_frame_buf)
        .unwrap();
    client.write_all(&query_frame_buf[..q_len]).await.unwrap();

    let (_resp_hdr, resp_payload) = read_next_response(&mut client, &mut read_buf, 104).await;
    assert_eq!(&resp_payload[4..10], &[0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF]);

    // 7. Query Child Device (0x1c) when disconnected
    let get_child_query = [0x03, 0x1c];
    let header = CoraHeader::new(
        CoraFlags::NONE,
        CoraHidOp::GetReport,
        105,
        get_child_query.len(),
    );
    let q_len = CoraFrame::new(header, &get_child_query)
        .encode(&mut query_frame_buf)
        .unwrap();
    client.write_all(&query_frame_buf[..q_len]).await.unwrap();

    let (_resp_hdr, resp_payload) = read_next_response(&mut client, &mut read_buf, 105).await;
    assert_eq!(resp_payload[0], 0x03);
    assert_eq!(resp_payload[1], 0x1c);
    assert_eq!(resp_payload[4], 0x00); // Disconnected

    // 8. Trigger simulated child device connection on Slot 0 and observe push notification
    let child_0 = ChildDeviceInfo::connected(
        0,
        0x0fd9,
        0x0080,
        "Stream Deck MK.2",
        "STREAMDECK_MK2_SN",
        5344,
    );
    state
        .set_device_for_slot(0, Some(child_0.clone()), None)
        .await;

    // Read hotplug push notification for Slot 0
    loop {
        let (_hdr, payload) = read_next_frame(&mut client, &mut read_buf).await;
        if payload.len() >= 128 && payload[0] == 0x01 && payload[1] == 0x0b {
            let parsed_child = ChildDeviceInfo::parse(&payload).unwrap().unwrap();
            assert_eq!(parsed_child.slot_index, 0);
            assert_eq!(parsed_child.vendor_id, 0x0fd9);
            assert_eq!(parsed_child.product_id, 0x0080);
            assert_eq!(parsed_child.tcp_port, 5344);
            assert_eq!(parsed_child.serial_as_str().unwrap(), "STREAMDECK_MK2_SN");
            break;
        }
    }

    // 9. Trigger simulated second child device connection on Slot 1 and observe push notification
    let child_1 = ChildDeviceInfo::connected(
        1,
        0x0fd9,
        0x0084,
        "Stream Deck +",
        "STREAMDECK_PLUS_SN",
        5345,
    );
    state
        .set_device_for_slot(1, Some(child_1.clone()), None)
        .await;

    // Read hotplug push notification for Slot 1
    loop {
        let (_hdr, payload) = read_next_frame(&mut client, &mut read_buf).await;
        if payload.len() >= 128 && payload[0] == 0x01 && payload[1] == 0x0b {
            let parsed_child = ChildDeviceInfo::parse(&payload).unwrap().unwrap();
            if parsed_child.slot_index == 1 {
                assert_eq!(parsed_child.vendor_id, 0x0fd9);
                assert_eq!(parsed_child.product_id, 0x0084);
                assert_eq!(parsed_child.tcp_port, 5345);
                assert_eq!(parsed_child.serial_as_str().unwrap(), "STREAMDECK_PLUS_SN");
                break;
            }
        }
    }

    // 10. Query Child Device Info for Slot 1 (0x1c with slot=1)
    let get_child_1_query = [0x03, 0x1c, 1];
    let header = CoraHeader::new(
        CoraFlags::NONE,
        CoraHidOp::GetReport,
        106,
        get_child_1_query.len(),
    );
    let q_len = CoraFrame::new(header, &get_child_1_query)
        .encode(&mut query_frame_buf)
        .unwrap();
    client.write_all(&query_frame_buf[..q_len]).await.unwrap();

    let (_resp_hdr, resp_payload) = read_next_response(&mut client, &mut read_buf, 106).await;
    assert_eq!(resp_payload[0], 0x03);
    assert_eq!(resp_payload[1], 0x1c);
    assert_eq!(resp_payload[4], 0x02); // Connected
    assert_eq!(resp_payload[5], 1); // Slot 1
    let parsed_slot1 = ChildDeviceInfo::parse(&resp_payload).unwrap().unwrap();
    assert_eq!(parsed_slot1.slot_index, 1);
    assert_eq!(parsed_slot1.product_id, 0x0084);
    assert_eq!(parsed_slot1.tcp_port, 5345);

    // 11. Send Watchdog / Health Query (0x1a) and verify response
    let get_health_query = [0x03, 0x1a];
    let header = CoraHeader::new(
        CoraFlags::NONE,
        CoraHidOp::Write,
        107,
        get_health_query.len(),
    );
    let q_len = CoraFrame::new(header, &get_health_query)
        .encode(&mut query_frame_buf)
        .unwrap();
    client.write_all(&query_frame_buf[..q_len]).await.unwrap();

    let (resp_hdr, resp_payload) = read_next_response(&mut client, &mut read_buf, 107).await;
    assert_eq!(resp_hdr.flags, CoraFlags::RESULT);
    assert_eq!(resp_payload[0], 0x03);
    assert_eq!(resp_payload[1], 0x1a);
}
