use crate::constants::*;
use crate::cora::*;
use crate::keepalive::*;
use crate::reports::*;

#[test]
fn test_cora_header_encode_decode() {
    let header = CoraHeader {
        flags: CoraFlags::REQ_ACK.union(CoraFlags::VERBATIM),
        hid_op: CoraHidOp::GetReport,
        reserved: 0,
        message_id: 0x12345678,
        payload_len: 42,
    };

    let mut buf = [0u8; CORA_HEADER_SIZE];
    header.encode(&mut buf);

    assert_eq!(&buf[0..4], &CORA_MAGIC);
    assert_eq!(u16::from_le_bytes([buf[4], buf[5]]), 0x8000 | 0x4000);
    assert_eq!(buf[6], CoraHidOp::GetReport.as_u8());
    assert_eq!(
        u32::from_le_bytes([buf[8], buf[9], buf[10], buf[11]]),
        0x12345678
    );
    assert_eq!(u32::from_le_bytes([buf[12], buf[13], buf[14], buf[15]]), 42);

    let parsed = CoraHeader::parse(&buf).unwrap();
    assert_eq!(parsed, header);
}

#[test]
fn test_cora_frame_decode() {
    let payload = [0x03, 0x80, 0x00, 0x01];
    let header = CoraHeader::new(CoraFlags::NONE, CoraHidOp::SendReport, 100, payload.len());
    let frame = CoraFrame::new(header, &payload);

    let mut out = [0u8; 64];
    let written = frame.encode(&mut out).unwrap();
    assert_eq!(written, CORA_HEADER_SIZE + payload.len());

    let (decoded, consumed) = CoraFrame::decode(&out[..written]).unwrap().unwrap();
    assert_eq!(consumed, written);
    assert_eq!(decoded.header.message_id, 100);
    assert_eq!(decoded.payload, &payload);
}

#[test]
fn test_keepalive_probe_and_ack() {
    let conn_id = 7;
    let msg_id = 999;
    let mut probe_buf = [0u8; 64];
    let probe_len = build_keepalive_probe_frame(conn_id, msg_id, &mut probe_buf).unwrap();

    let (frame, _) = CoraFrame::decode(&probe_buf[..probe_len]).unwrap().unwrap();
    assert_eq!(is_keepalive_probe(&frame), Some(conn_id));
    assert_eq!(is_keepalive_ack(&frame), None);

    let mut ack_buf = [0u8; 64];
    let ack_len = build_keepalive_ack_frame(conn_id, msg_id, &mut ack_buf).unwrap();
    let (ack_frame, _) = CoraFrame::decode(&ack_buf[..ack_len]).unwrap().unwrap();
    assert_eq!(is_keepalive_ack(&ack_frame), Some(conn_id));
    assert_eq!(is_keepalive_probe(&ack_frame), None);
}

#[test]
fn test_device_info_0x80() {
    let mut payload = [0u8; 16];
    build_device_info_payload(VENDOR_ID_ELGATO, PRODUCT_ID_NETWORK_DOCK, &mut payload).unwrap();

    assert_eq!(payload[0], 0x03);
    assert_eq!(payload[1], 0x80);
    let vid = u16::from_le_bytes([payload[12], payload[13]]);
    let pid = u16::from_le_bytes([payload[14], payload[15]]);
    assert_eq!(vid, VENDOR_ID_ELGATO);
    assert_eq!(pid, PRODUCT_ID_NETWORK_DOCK);
}

#[test]
fn test_firmware_version_0x83() {
    let mut payload = [0u8; 16];
    build_firmware_version_payload("1.0.42", &mut payload).unwrap();

    assert_eq!(payload[0], 0x03);
    assert_eq!(payload[1], 0x83);
    let str_slice = &payload[8..14];
    assert_eq!(core::str::from_utf8(str_slice).unwrap(), "1.0.42");
}

#[test]
fn test_serial_number_0x84() {
    let mut payload = [0u8; 32];
    let written = build_serial_number_payload("DL12A3B45678", &mut payload).unwrap();

    assert_eq!(payload[0], 0x03);
    assert_eq!(payload[1], 0x84);
    let len = payload[3] as usize;
    assert_eq!(len, 12);
    assert_eq!(
        core::str::from_utf8(&payload[4..4 + len]).unwrap(),
        "DL12A3B45678"
    );
    assert_eq!(written, 4 + len);
}

#[test]
fn test_mac_address_0x85() {
    let mac = [0x00, 0x11, 0x22, 0x33, 0x44, 0x55];
    let mut payload = [0u8; 10];
    let written = build_mac_address_payload(mac, &mut payload).unwrap();

    assert_eq!(payload[0], 0x03);
    assert_eq!(payload[1], 0x85);
    assert_eq!(&payload[4..10], &mac);
    assert_eq!(written, 10);
}

#[test]
fn test_child_device_info_0x1c_and_hotplug() {
    let child =
        ChildDeviceInfo::connected(0, 0x0fd9, 0x0080, "Stream Deck MK.2", "CL12A1A99999", 5344);

    let mut query_payload = [0u8; 128];
    child.build_payload(false, 0, &mut query_payload);
    assert_eq!(query_payload[0], 0x03);
    assert_eq!(query_payload[1], 0x1c);
    assert_eq!(&query_payload[2..4], &[124, 0]);
    assert_eq!(query_payload[5], 0x00);

    let parsed = ChildDeviceInfo::parse(&query_payload).unwrap().unwrap();
    assert_eq!(parsed.slot_index, 0);
    assert_eq!(parsed.vendor_id, 0x0fd9);
    assert_eq!(parsed.product_id, 0x0080);
    assert_eq!(parsed.tcp_port, 5344);
    assert_eq!(parsed.serial_as_str().unwrap(), "CL12A1A99999");

    let mut push_payload = [0u8; 128];
    child.build_payload(true, 0, &mut push_payload);
    assert_eq!(push_payload[0], 0x01);
    assert_eq!(push_payload[1], 0x0b);
    assert_eq!(&push_payload[2..4], &[124, 0]);
    assert_eq!(push_payload[5], 0x00);
    let parsed_push = ChildDeviceInfo::parse(&push_payload).unwrap().unwrap();
    assert_eq!(parsed_push.slot_index, 0);
    assert_eq!(parsed_push.tcp_port, 5344);
}
