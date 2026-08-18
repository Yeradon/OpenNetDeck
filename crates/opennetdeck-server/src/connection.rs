use bytes::{Buf, BytesMut};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::time::{interval, Instant};
use tracing::{debug, error, info, warn};

use opennetdeck_protocol::{
    build_cora_push_frame, build_device_info_payload, build_firmware_version_payload,
    build_keepalive_ack_frame, build_keepalive_probe_frame, build_mac_address_payload,
    build_serial_number_payload, is_keepalive_probe, parse_primary_query, CoraFlags, CoraFrame,
    CoraHeader, CoraHidOp, PrimaryFeatureCommand, KEEPALIVE_INTERVAL_MS, PRODUCT_ID_NETWORK_DOCK,
    TIMEOUT_DURATION_MS, VENDOR_ID_ELGATO,
};

use crate::dock::{DockState, ServerMode};
use crate::usb::device::StreamDeckUsbHandle;

pub struct PrimaryConnection {
    socket: TcpStream,
    peer_addr: SocketAddr,
    local_addr: SocketAddr,
    state: DockState,
    connection_id: u8,
    usb_device: Option<StreamDeckUsbHandle>,
}

impl PrimaryConnection {
    pub fn new(
        socket: TcpStream,
        peer_addr: SocketAddr,
        state: DockState,
        usb_device: Option<StreamDeckUsbHandle>,
    ) -> Self {
        let local_addr = socket.local_addr().unwrap_or(peer_addr);
        let connection_id = state.next_connection_id();
        Self {
            socket,
            peer_addr,
            local_addr,
            state,
            connection_id,
            usb_device,
        }
    }

    pub async fn run(self) {
        let PrimaryConnection {
            socket,
            peer_addr,
            local_addr,
            state,
            connection_id,
            usb_device,
        } = self;

        let mode = state.config().await.mode;
        info!(
            peer = %peer_addr,
            local = %local_addr,
            conn_id = connection_id,
            mode = ?mode,
            "=== NEW CONNECTION on Primary Port (5343) ==="
        );

        let (mut reader, mut writer) = socket.into_split();
        let (tx, mut rx) = mpsc::channel::<Vec<u8>>(128);

        // Spawn socket writer task
        let writer_peer = peer_addr;
        let writer_task = tokio::spawn(async move {
            while let Some(msg) = rx.recv().await {
                info!(
                    peer = %writer_peer,
                    len = msg.len(),
                    preview_hex = ?&msg[..msg.len().min(32)],
                    "--> Sent packet to client"
                );
                if let Err(e) = writer.write_all(&msg).await {
                    debug!(peer = %writer_peer, "Failed to write data to client: {}", e);
                    break;
                }
            }
        });

        // 1. Send immediate Keepalive Probe upon connection
        let mut probe_buf = [0u8; 16 + 32];
        if let Ok(len) = build_keepalive_probe_frame(connection_id, 1, &mut probe_buf) {
            let _ = tx.send(probe_buf[..len].to_vec()).await;
        }

        // 2. In Direct mode, spawn USB input reader for physical button/dial events
        let (input_tx, mut input_rx) = mpsc::channel::<Vec<u8>>(128);
        let usb_reader_handle = if mode == ServerMode::Direct {
            if let Some(ref dev) = usb_device {
                let (dummy_tx, _) = mpsc::channel(1);
                Some(dev.spawn_input_reader(input_tx, dummy_tx))
            } else {
                None
            }
        } else {
            None
        };

        let direct_input_tx = tx.clone();
        let direct_input_task = tokio::spawn(async move {
            while let Some(raw_report) = input_rx.recv().await {
                info!(
                    len = raw_report.len(),
                    preview_hex = ?&raw_report[..raw_report.len().min(16)],
                    "--> [5343] Forwarding direct physical USB input event to client"
                );
                let header =
                    CoraHeader::new(CoraFlags::NONE, CoraHidOp::Write, 0, raw_report.len());

                let frame = CoraFrame::new(header, &raw_report);
                if direct_input_tx.send(frame.to_vec()).await.is_err() {
                    break;
                }
            }
        });

        // 3. In Dock mode, spawn hotplug change listener task for live USB bus insertion/removal
        let mut hotplug_rx = state.subscribe_hotplug();
        let hotplug_tx = tx.clone();
        let hotplug_peer = peer_addr;
        let hotplug_task = tokio::spawn(async move {
            while let Ok(child) = hotplug_rx.recv().await {
                let mut payload = [0u8; 128];
                child.build_payload(true, child.slot_index, &mut payload);

                let mut out = [0u8; 16 + 128];
                if let Ok(len) = build_cora_push_frame(&payload, &mut out) {
                    info!(
                        peer = %hotplug_peer,
                        slot = child.slot_index,
                        connected = child.connected,
                        full_hex = ?&payload[..],
                        "--> Pushing live child device hotplug event (0x01, 0x0b) to client"
                    );
                    if hotplug_tx.send(out[..len].to_vec()).await.is_err() {
                        break;
                    }
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

        let handler = FrameHandler {
            state: state.clone(),
            peer_addr,
            local_addr,
            _connection_id: connection_id,
            mode,
            usb_device: usb_device.clone(),
            has_sent_initial_hotplug: Arc::new(AtomicBool::new(false)),
        };

        loop {
            tokio::select! {
                _ = ping_interval.tick() => {
                    if last_received.elapsed() > timeout_duration {
                        warn!(peer = %peer_addr, conn_id = connection_id, "Connection timed out (no data for >5s), closing");
                        break;
                    }

                    // Transmit periodic keepalive probe packet
                    let mut probe_buf = [0u8; 16 + 32];
                    message_seq = message_seq.wrapping_add(1);
                    if let Ok(len) = build_keepalive_probe_frame(connection_id, message_seq, &mut probe_buf) {
                        if tx.send(probe_buf[..len].to_vec()).await.is_err() {
                            break;
                        }
                    }
                }

                read_res = reader.read(&mut raw_read_chunk) => {
                    match read_res {
                        Ok(0) => {
                            info!(peer = %peer_addr, conn_id = connection_id, "Client disconnected from primary port");
                            break;
                        }
                        Ok(n) => {
                            last_received = Instant::now();
                            info!(
                                peer = %peer_addr,
                                len = n,
                                hex = ?&raw_read_chunk[..n.min(32)],
                                "<-- Raw data received from client on 5343"
                            );

                            read_buf.extend_from_slice(&raw_read_chunk[..n]);

                            while !read_buf.is_empty() {
                                match CoraFrame::decode(&read_buf) {
                                    Ok(Some((frame, consumed))) => {
                                        handler.handle_frame(&frame, &tx).await;
                                        read_buf.advance(consumed);
                                    }
                                    Ok(None) => break,
                                    Err(err) => {
                                        warn!(peer = %peer_addr, error = ?err, "CORA decode error, looking for next magic");
                                        if let Some(pos) = CoraFrame::find_magic(&read_buf) {
                                            if pos > 0 {
                                                debug!(peer = %peer_addr, skipped = pos, "Resynchronizing stream to next CORA magic");
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
                            error!(peer = %peer_addr, "Socket read error: {}", e);
                            break;
                        }
                    }
                }
            }
        }

        if let Some(h) = usb_reader_handle {
            h.abort();
        }
        direct_input_task.abort();
        hotplug_task.abort();
        writer_task.abort();
        info!(peer = %peer_addr, conn_id = connection_id, "Primary port connection closed");
    }
}

struct FrameHandler {
    state: DockState,
    peer_addr: SocketAddr,
    local_addr: SocketAddr,
    _connection_id: u8,
    mode: ServerMode,
    usb_device: Option<StreamDeckUsbHandle>,
    has_sent_initial_hotplug: Arc<AtomicBool>,
}

impl FrameHandler {
    async fn handle_frame(&self, frame: &CoraFrame<'_>, tx: &mpsc::Sender<Vec<u8>>) {
        info!(
            peer = %self.peer_addr,
            op = ?frame.header.hid_op,
            flags = format_args!("0x{:04x}", frame.header.flags.bits()),
            msg_id = frame.header.message_id,
            payload_len = frame.payload.len(),
            payload_preview = ?&frame.payload[..frame.payload.len().min(16)],
            "<-- Decoded incoming CORA frame"
        );

        // 1. Keepalive probe handling (probe from client)
        if let Some(conn_id) = is_keepalive_probe(frame) {
            info!(peer = %self.peer_addr, conn_id = conn_id, "Handling client keepalive probe, replying ACK");
            let mut ack_buf = [0u8; 16 + 32];
            if let Ok(len) =
                build_keepalive_ack_frame(conn_id, frame.header.message_id, &mut ack_buf)
            {
                let _ = tx.send(ack_buf[..len].to_vec()).await;
            }
            return;
        }

        // 2. Direct mode: Forward image writes and set_reports to USB
        if self.mode == ServerMode::Direct {
            let is_verbatim = frame.header.flags.contains(CoraFlags::VERBATIM);

            if frame.header.hid_op == CoraHidOp::Write
                && !frame.payload.is_empty()
                && frame.payload[0] != 0x03
                && frame.payload[0] != 0x01
            {
                if let Some(ref dev) = self.usb_device {
                    info!(peer = %self.peer_addr, len = frame.payload.len(), "Forwarding direct image Write to USB Stream Deck");
                    let _ = dev.write_out(frame.payload).await;
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
                return;
            }

            if frame.header.hid_op == CoraHidOp::SendReport {
                if let Some(ref dev) = self.usb_device {
                    info!(peer = %self.peer_addr, len = frame.payload.len(), "Forwarding direct SetReport to USB Stream Deck");
                    let _ = dev.set_feature_report(frame.payload).await;
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
        }

        // 3. Handle Feature Report queries
        let cmd = parse_primary_query(frame.payload);
        if let Some(cmd) = cmd {
            info!(peer = %self.peer_addr, cmd = ?cmd, msg_id = frame.header.message_id, mode = ?self.mode, "Handling Feature Query");

            let mut resp_payload = vec![0u8; 1024];
            resp_payload[0] = 0x03;
            resp_payload[1] = cmd.as_u8();
            let mut custom_resp_len: Option<usize> = None;

            match cmd {
                PrimaryFeatureCommand::GetDeviceInfo => {
                    if self.mode == ServerMode::Direct {
                        if let Some(ref dev) = self.usb_device {
                            let _ = build_device_info_payload(
                                dev.vendor_id(),
                                dev.product_id(),
                                &mut resp_payload,
                            );
                        } else {
                            let _ = build_device_info_payload(
                                VENDOR_ID_ELGATO,
                                PRODUCT_ID_NETWORK_DOCK,
                                &mut resp_payload,
                            );
                        }
                    } else {
                        let _ = build_device_info_payload(
                            VENDOR_ID_ELGATO,
                            PRODUCT_ID_NETWORK_DOCK,
                            &mut resp_payload,
                        );
                    }
                }
                PrimaryFeatureCommand::GetFirmwareVersion => {
                    let config = self.state.config().await;
                    let _ =
                        build_firmware_version_payload(&config.firmware_version, &mut resp_payload);
                }
                PrimaryFeatureCommand::GetSerialNumber => {
                    if self.mode == ServerMode::Direct {
                        if let Some(ref dev) = self.usb_device {
                            let _ =
                                build_serial_number_payload(dev.serial_number(), &mut resp_payload);
                        } else {
                            let config = self.state.config().await;
                            let _ = build_serial_number_payload(
                                &config.serial_number,
                                &mut resp_payload,
                            );
                        }
                    } else {
                        let config = self.state.config().await;
                        let _ =
                            build_serial_number_payload(&config.serial_number, &mut resp_payload);
                    }
                }
                PrimaryFeatureCommand::GetMacAddress => {
                    let config = self.state.config().await;
                    let _ = build_mac_address_payload(config.mac_address, &mut resp_payload);
                }
                PrimaryFeatureCommand::GetChildDeviceInfo => {
                    let requested_slot = if frame.payload.len() >= 3 {
                        frame.payload[2]
                    } else {
                        0
                    };
                    let child = self.state.child_device_at(requested_slot).await;
                    let mut p = [0u8; 128];
                    child.build_payload(false, requested_slot, &mut p);
                    info!(
                        peer = %self.peer_addr,
                        slot = requested_slot,
                        connected = child.connected,
                        full_child_hex = ?&p[..],
                        "Returning 0x1c ChildDeviceInfo (128 bytes)"
                    );
                    resp_payload[..128].copy_from_slice(&p);
                    custom_resp_len = Some(128);
                }
                PrimaryFeatureCommand::Other(0x87) => {
                    info!(peer = %self.peer_addr, "Populating 0x87 Network Configuration report");
                    resp_payload[2] = 0x01; // Static / active configuration
                    resp_payload[3] = 0x01; // Online
                    if let std::net::IpAddr::V4(ipv4) = self.local_addr.ip() {
                        resp_payload[4..8].copy_from_slice(&ipv4.octets());
                        resp_payload[8..12].copy_from_slice(&[255, 255, 255, 0]);
                        resp_payload[12..16].copy_from_slice(&ipv4.octets());
                    }
                    let primary_port = self.state.config().await.primary_port;
                    resp_payload[16..18].copy_from_slice(&primary_port.to_le_bytes());
                }
                PrimaryFeatureCommand::Other(0x8f) => {
                    // Opcode 0x8f: Standard dock hardware descriptor
                    info!(peer = %self.peer_addr, "Populating 0x8f Dock Hardware info");
                    resp_payload[0] = 0x03;
                    resp_payload[1] = 0x8f;
                }
                PrimaryFeatureCommand::Other(0x1a) => {
                    info!(peer = %self.peer_addr, "Responding to 0x1a Watchdog/Keepalive polling feature query");
                    resp_payload[0] = 0x03;
                    resp_payload[1] = 0x1a;

                    // Trigger child hotplug notification on confirmed idle poll for all active slots
                    if self.mode == ServerMode::Dock
                        && !self.has_sent_initial_hotplug.swap(true, Ordering::SeqCst)
                    {
                        let children = self.state.all_active_children().await;
                        for child in children {
                            let push_tx = tx.clone();
                            let push_peer = self.peer_addr;
                            tokio::spawn(async move {
                                let mut p = [0u8; 128];
                                child.build_payload(true, child.slot_index, &mut p);
                                let mut out = [0u8; 16 + 128];
                                if let Ok(len) = build_cora_push_frame(&p, &mut out) {
                                    info!(
                                        peer = %push_peer,
                                        slot = child.slot_index,
                                        full_hex = ?&p[..],
                                        "--> Triggering child hotplug push (0x01, 0x0b) on confirmed idle state"
                                    );
                                    let _ = push_tx.send(out[..len].to_vec()).await;
                                }
                            });
                        }
                    }
                }
                PrimaryFeatureCommand::Other(opcode) => {
                    info!(peer = %self.peer_addr, opcode = format_args!("0x{:02x}", opcode), "Responding to feature query");
                    resp_payload[0] = 0x03;
                    resp_payload[1] = opcode;

                    if self.mode == ServerMode::Direct {
                        if let Some(ref dev) = self.usb_device {
                            if let Ok(data) = dev.get_feature_report(opcode, 1024).await {
                                if data.len() >= 2 && data[0] == 0x03 && data[1] == opcode {
                                    resp_payload[..data.len().min(1024)]
                                        .copy_from_slice(&data[..data.len().min(1024)]);
                                } else if !data.is_empty() && data[0] == opcode {
                                    let copy_len = data.len().min(1023);
                                    resp_payload[1..1 + copy_len]
                                        .copy_from_slice(&data[..copy_len]);
                                } else if !data.is_empty() {
                                    let copy_len = data.len().min(1022);
                                    resp_payload[2..2 + copy_len]
                                        .copy_from_slice(&data[..copy_len]);
                                }
                            }
                        }
                    }
                }
            };

            let resp_len = custom_resp_len.unwrap_or(if frame.payload.len() >= 1024 {
                1024
            } else {
                128
            });
            let mut out_frame = vec![0u8; 16 + resp_len];
            let header = CoraHeader::new(
                CoraFlags::RESULT,
                frame.header.hid_op,
                frame.header.message_id,
                resp_len,
            );
            if let Ok(total) =
                CoraFrame::new(header, &resp_payload[..resp_len]).encode(&mut out_frame)
            {
                let _ = tx.send(out_frame[..total].to_vec()).await;
            }
        }
    }
}
