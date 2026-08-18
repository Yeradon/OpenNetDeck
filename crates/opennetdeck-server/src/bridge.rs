use bytes::{Buf, BytesMut};
use std::net::SocketAddr;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio::time::{interval, Instant};
use tracing::{debug, error, info, warn};

use opennetdeck_protocol::{
    build_device_info_payload, build_keepalive_ack_frame, build_keepalive_probe_frame,
    is_keepalive_ack, is_keepalive_probe, ChildDeviceInfo, CoraFlags, CoraFrame, CoraHeader,
    CoraHidOp, KEEPALIVE_INTERVAL_MS, TIMEOUT_DURATION_MS,
};

use crate::usb::device::StreamDeckUsbHandle;

pub struct SecondaryPortBridge {
    bind_addr: SocketAddr,
    device: StreamDeckUsbHandle,
    disconnect_tx: mpsc::Sender<()>,
}

impl SecondaryPortBridge {
    pub fn new(
        bind_addr: SocketAddr,
        device: StreamDeckUsbHandle,
        disconnect_tx: mpsc::Sender<()>,
    ) -> Self {
        Self {
            bind_addr,
            device,
            disconnect_tx,
        }
    }

    pub async fn run(self) -> anyhow::Result<()> {
        let listener = TcpListener::bind(self.bind_addr).await?;
        info!(
            addr = %self.bind_addr,
            serial = %self.device.serial_number(),
            "Secondary Bridge TCP server listening for child Stream Deck connections (5344)"
        );

        loop {
            match listener.accept().await {
                Ok((socket, peer_addr)) => {
                    let device = self.device.clone();
                    let disconnect_tx = self.disconnect_tx.clone();
                    tokio::spawn(async move {
                        let conn =
                            SecondaryConnection::new(socket, peer_addr, device, disconnect_tx);
                        conn.run().await;
                    });
                }
                Err(e) => {
                    error!("Error accepting secondary bridge connection: {}", e);
                }
            }
        }
    }
}

pub struct SecondaryConnection {
    socket: TcpStream,
    peer_addr: SocketAddr,
    device: StreamDeckUsbHandle,
    disconnect_tx: mpsc::Sender<()>,
}

impl SecondaryConnection {
    pub fn new(
        socket: TcpStream,
        peer_addr: SocketAddr,
        device: StreamDeckUsbHandle,
        disconnect_tx: mpsc::Sender<()>,
    ) -> Self {
        Self {
            socket,
            peer_addr,
            device,
            disconnect_tx,
        }
    }

    pub async fn run(self) {
        let SecondaryConnection {
            socket,
            peer_addr,
            device,
            disconnect_tx,
        } = self;

        info!(peer = %peer_addr, "=== NEW CONNECTION on Secondary Bridge Port (5344) ===");

        let (mut reader, mut writer) = socket.into_split();
        let (tx, mut rx) = mpsc::channel::<Vec<u8>>(128);

        // 1. Writer task: flush outgoing packets to network socket
        let writer_peer = peer_addr;
        let writer_task = tokio::spawn(async move {
            while let Some(msg) = rx.recv().await {
                info!(
                    peer = %writer_peer,
                    len = msg.len(),
                    preview_hex = ?&msg[..msg.len().min(32)],
                    "--> [5344] Sent packet to child client"
                );
                if let Err(e) = writer.write_all(&msg).await {
                    debug!(peer = %writer_peer, "Failed to write data to child client: {}", e);
                    break;
                }
            }
        });

        // 3. USB -> TCP input forwarder: listen to physical hardware events
        let (input_tx, mut input_rx) = mpsc::channel::<Vec<u8>>(128);
        let usb_reader_handle = device.spawn_input_reader(input_tx, disconnect_tx.clone());

        let input_forward_tx = tx.clone();
        let input_forward_task = tokio::spawn(async move {
            while let Some(raw_report) = input_rx.recv().await {
                info!(
                    len = raw_report.len(),
                    preview_hex = ?&raw_report[..raw_report.len().min(16)],
                    "--> [5344] Forwarding physical USB input event to child client"
                );
                let header =
                    CoraHeader::new(CoraFlags::NONE, CoraHidOp::Write, 0, raw_report.len());

                let frame = CoraFrame::new(header, &raw_report);
                let msg = frame.to_vec();
                if input_forward_tx.send(msg).await.is_err() {
                    break;
                }
            }
        });

        let mut read_buf = BytesMut::with_capacity(65536);
        let mut raw_read_chunk = [0u8; 16384];
        let mut ping_interval = interval(Duration::from_millis(KEEPALIVE_INTERVAL_MS));
        ping_interval.tick().await;

        let mut last_received = Instant::now();
        let timeout_duration = Duration::from_millis(TIMEOUT_DURATION_MS);
        let mut message_seq: u32 = 1;
        let conn_id: u8 = 1;

        loop {
            tokio::select! {
                _ = ping_interval.tick() => {
                    if last_received.elapsed() > timeout_duration {
                        warn!(peer = %peer_addr, "Secondary bridge client timed out, closing");
                        break;
                    }

                    // Transmit periodic keepalive probe to refresh client's QDeadlineTimer
                    let mut probe_buf = [0u8; 16 + 32];
                    message_seq = message_seq.wrapping_add(1);
                    if let Ok(len) = build_keepalive_probe_frame(conn_id, message_seq, &mut probe_buf) {
                        if tx.send(probe_buf[..len].to_vec()).await.is_err() {
                            break;
                        }
                    }
                }

                read_res = reader.read(&mut raw_read_chunk) => {
                    match read_res {
                        Ok(0) => {
                            info!(peer = %peer_addr, "Secondary bridge client disconnected");
                            break;
                        }
                        Ok(n) => {
                            last_received = Instant::now();
                            info!(
                                peer = %peer_addr,
                                len = n,
                                hex = ?&raw_read_chunk[..n.min(32)],
                                "<-- [5344] Raw data received from child client"
                            );

                            read_buf.extend_from_slice(&raw_read_chunk[..n]);

                            while !read_buf.is_empty() {
                                match CoraFrame::decode(&read_buf) {
                                    Ok(Some((frame, consumed))) => {
                                        Self::handle_incoming_frame(&device, peer_addr, &frame, &tx).await;
                                        read_buf.advance(consumed);
                                    }
                                    Ok(None) => break,
                                    Err(_) => {
                                        if let Some(pos) = CoraFrame::find_magic(&read_buf) {
                                            if pos > 0 {
                                                read_buf.advance(pos);
                                            } else {
                                                read_buf.advance(1);
                                            }
                                        } else {
                                            read_buf.clear();
                                            break;
                                        }
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            error!(peer = %peer_addr, "Secondary bridge socket read error: {}", e);
                            break;
                        }
                    }
                }
            }
        }

        usb_reader_handle.abort();
        input_forward_task.abort();
        writer_task.abort();
        info!(peer = %peer_addr, "Secondary child bridge connection closed");
    }

    async fn handle_incoming_frame(
        device: &StreamDeckUsbHandle,
        peer_addr: SocketAddr,
        frame: &CoraFrame<'_>,
        tx: &mpsc::Sender<Vec<u8>>,
    ) {
        info!(
            peer = %peer_addr,
            op = ?frame.header.hid_op,
            flags = format_args!("0x{:04x}", frame.header.flags.bits()),
            msg_id = frame.header.message_id,
            payload_len = frame.payload.len(),
            payload_preview = ?&frame.payload[..frame.payload.len().min(16)],
            "<-- [5344] Decoded incoming CORA frame"
        );

        // 1. Keepalive handling
        if let Some(conn_id) = is_keepalive_probe(frame) {
            let mut ack_buf = [0u8; 16 + 32];
            if let Ok(len) =
                build_keepalive_ack_frame(conn_id, frame.header.message_id, &mut ack_buf)
            {
                let _ = tx.send(ack_buf[..len].to_vec()).await;
            }
            return;
        }

        if is_keepalive_ack(frame).is_some() {
            debug!(peer = %peer_addr, "Secondary bridge keepalive ACK received");
            return;
        }

        let is_verbatim = frame.header.flags.contains(CoraFlags::VERBATIM);

        // 2. Feature Report Queries (GetReport, or Write with [0x03, <report_id>])
        if frame.header.hid_op == CoraHidOp::GetReport
            || (frame.header.hid_op == CoraHidOp::Write
                && !frame.payload.is_empty()
                && frame.payload[0] == 0x03)
        {
            if frame.payload.is_empty() {
                return;
            }

            let report_id = if frame.payload[0] == 0x03 && frame.payload.len() >= 2 {
                frame.payload[1]
            } else {
                frame.payload[0]
            };

            info!(
                peer = %peer_addr,
                report_id = format_args!("0x{:02x}", report_id),
                is_verbatim,
                msg_id = frame.header.message_id,
                "Handling secondary GetReport query"
            );

            // Virtual queries on child port
            if report_id == 0x1c {
                let model_name = device.model().map(|m| m.name()).unwrap_or("Stream Deck");
                let requested_slot = if frame.payload.len() >= 3 {
                    frame.payload[2]
                } else {
                    0
                };
                let child = ChildDeviceInfo::connected(
                    requested_slot,
                    device.vendor_id(),
                    device.product_id(),
                    model_name,
                    device.serial_number(),
                    5344,
                );
                let resp_len = if frame.payload.len() >= 1024 {
                    1024
                } else {
                    128
                };
                let mut p = vec![0u8; resp_len];
                let mut buf128 = [0u8; 128];
                child.build_payload(false, requested_slot, &mut buf128);
                p[..128].copy_from_slice(&buf128);

                let resp_flags = if is_verbatim {
                    CoraFlags::VERBATIM.union(CoraFlags::RESULT)
                } else {
                    CoraFlags::RESULT
                };

                let header = CoraHeader::new(
                    resp_flags,
                    frame.header.hid_op,
                    frame.header.message_id,
                    p.len(),
                );
                let resp_frame = CoraFrame::new(header, &p);
                let _ = tx.send(resp_frame.to_vec()).await;
                return;
            }

            if report_id == 0x80 {
                let resp_len = if frame.payload.len() >= 1024 {
                    1024
                } else {
                    16
                };
                let mut p = vec![0u8; resp_len];
                let _ = build_device_info_payload(device.vendor_id(), device.product_id(), &mut p);

                let resp_flags = if is_verbatim {
                    CoraFlags::VERBATIM.union(CoraFlags::RESULT)
                } else {
                    CoraFlags::RESULT
                };
                let header = CoraHeader::new(
                    resp_flags,
                    frame.header.hid_op,
                    frame.header.message_id,
                    p.len(),
                );
                let resp_frame = CoraFrame::new(header, &p);
                let _ = tx.send(resp_frame.to_vec()).await;
                return;
            }

            // Otherwise, forward directly to physical USB Stream Deck
            let req_len = if frame.payload.len() >= 1024 {
                1024
            } else {
                32
            };
            match device.get_feature_report(report_id, req_len).await {
                Ok(mut report_data) => {
                    if frame.payload.len() >= 1024 && report_data.len() < 1024 {
                        report_data.resize(1024, 0);
                    }
                    let resp_flags = if is_verbatim {
                        CoraFlags::VERBATIM.union(CoraFlags::RESULT)
                    } else {
                        CoraFlags::RESULT
                    };
                    let header = CoraHeader::new(
                        resp_flags,
                        frame.header.hid_op,
                        frame.header.message_id,
                        report_data.len(),
                    );
                    let resp_frame = CoraFrame::new(header, &report_data);
                    let _ = tx.send(resp_frame.to_vec()).await;
                }
                Err(e) => {
                    warn!(peer = %peer_addr, report_id = format_args!("0x{:02x}", report_id), "USB GetReport error, falling back to dummy response: {}", e);
                    let mut fallback = vec![0u8; req_len];
                    fallback[0] = 0x03;
                    fallback[1] = report_id;
                    let resp_flags = if is_verbatim {
                        CoraFlags::VERBATIM.union(CoraFlags::RESULT)
                    } else {
                        CoraFlags::RESULT
                    };
                    let header = CoraHeader::new(
                        resp_flags,
                        frame.header.hid_op,
                        frame.header.message_id,
                        fallback.len(),
                    );
                    let resp_frame = CoraFrame::new(header, &fallback);
                    let _ = tx.send(resp_frame.to_vec()).await;
                }
            }
            return;
        }

        // 3. Send Feature Report (SendReport)
        if frame.header.hid_op == CoraHidOp::SendReport {
            debug!(
                peer = %peer_addr,
                len = frame.payload.len(),
                "Forwarding SetReport to physical USB device"
            );
            if let Err(e) = device.set_feature_report(frame.payload).await {
                error!(peer = %peer_addr, "Failed USB SetReport: {}", e);
            }
            if frame.header.flags.contains(CoraFlags::REQ_ACK) || frame.header.message_id != 0 {
                let resp_flags = if is_verbatim {
                    CoraFlags::VERBATIM.union(CoraFlags::RESULT)
                } else {
                    CoraFlags::RESULT
                };
                let ack_payload = if !frame.payload.is_empty() {
                    &frame.payload[..1]
                } else {
                    &[0x03]
                };
                let header = CoraHeader::new(
                    resp_flags,
                    frame.header.hid_op,
                    frame.header.message_id,
                    ack_payload.len(),
                );
                let resp_frame = CoraFrame::new(header, ack_payload);
                let _ = tx.send(resp_frame.to_vec()).await;
            }
            return;
        }

        // 4. Output / Image Write (Write)
        if frame.header.hid_op == CoraHidOp::Write {
            info!(
                peer = %peer_addr,
                len = frame.payload.len(),
                msg_id = frame.header.message_id,
                flags = format_args!("0x{:04x}", frame.header.flags.bits()),
                "Forwarding image chunk (Write) to physical USB OUT endpoint"
            );
            if let Err(e) = device.write_out(frame.payload).await {
                error!(peer = %peer_addr, "Failed USB write_out: {}", e);
            }
            if frame.header.flags.contains(CoraFlags::REQ_ACK) || frame.header.message_id != 0 {
                let resp_flags = if is_verbatim {
                    CoraFlags::VERBATIM.union(CoraFlags::RESULT)
                } else {
                    CoraFlags::RESULT
                };
                let ack_payload = if !frame.payload.is_empty() {
                    &frame.payload[..1]
                } else {
                    &[0x02]
                };
                let header = CoraHeader::new(
                    resp_flags,
                    frame.header.hid_op,
                    frame.header.message_id,
                    ack_payload.len(),
                );
                let resp_frame = CoraFrame::new(header, ack_payload);
                let _ = tx.send(resp_frame.to_vec()).await;
            }
        }
    }
}
