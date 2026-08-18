//! Generic embedded Primary Dock Hub engine for TCP 5343.

use embedded_io_async::{Read, Write};
use log::{debug, info, warn};
use opennetdeck_protocol::{
    build_cora_push_frame, build_cora_response_frame, build_device_info_payload,
    build_firmware_version_payload, build_keepalive_ack_frame, build_keepalive_probe_frame,
    build_mac_address_payload, build_serial_number_payload, is_keepalive_probe,
    parse_primary_query, ChildDeviceInfo, CoraFrame, PrimaryFeatureCommand,
    PRODUCT_ID_NETWORK_DOCK, VENDOR_ID_ELGATO,
};

pub struct EmbeddedDockHub<'a> {
    pub serial: &'a str,
    pub firmware: &'a str,
    pub mac_address: [u8; 6],
    pub ip_address: [u8; 4],
    pub primary_port: u16,
    pub secondary_port: u16,
}

impl<'a> EmbeddedDockHub<'a> {
    pub fn new(
        serial: &'a str,
        firmware: &'a str,
        mac_address: [u8; 6],
        ip_address: [u8; 4],
        primary_port: u16,
        secondary_port: u16,
    ) -> Self {
        Self {
            serial,
            firmware,
            mac_address,
            ip_address,
            primary_port,
            secondary_port,
        }
    }

    /// Process a client connection session over any generic async I/O stream.
    pub async fn handle_connection<S: Read + Write>(
        &self,
        stream: &mut S,
        active_children: &[ChildDeviceInfo],
    ) -> Result<(), S::Error> {
        info!(
            "Primary Dock Hub connection established on port {}",
            self.primary_port
        );

        // 1. Transmit initial Keepalive probe
        let mut probe = [0u8; 48];
        if let Ok(len) = build_keepalive_probe_frame(1, 1, &mut probe) {
            stream.write_all(&probe[..len]).await?;
        }

        // 2. Transmit initial hotplug notifications for all active slots
        for child in active_children {
            let mut hotplug_payload = [0u8; 128];
            child.build_payload(true, child.slot_index, &mut hotplug_payload);
            let mut push_frame = [0u8; 144];
            if let Ok(len) = build_cora_push_frame(&hotplug_payload, &mut push_frame) {
                stream.write_all(&push_frame[..len]).await?;
            }
        }

        let mut rx_buffer = [0u8; 2048];
        let mut tx_buffer = [0u8; 2048];
        let mut rx_offset = 0;

        loop {
            let n = match stream.read(&mut rx_buffer[rx_offset..]).await {
                Ok(0) => {
                    info!("Primary Dock client disconnected");
                    break;
                }
                Ok(n) => n,
                Err(e) => {
                    warn!("Stream read error on primary dock: {:?}", e);
                    return Err(e);
                }
            };

            rx_offset += n;
            let mut consumed_total = 0;
            let mut slice = &rx_buffer[..rx_offset];

            while !slice.is_empty() {
                match CoraFrame::decode(slice) {
                    Ok(Some((frame, consumed))) => {
                        self.handle_frame(&frame, active_children, stream, &mut tx_buffer)
                            .await?;
                        slice = &slice[consumed..];
                        consumed_total += consumed;
                    }
                    Ok(None) => break,
                    Err(_) => {
                        if let Some(pos) = CoraFrame::find_magic(slice) {
                            if pos > 0 {
                                slice = &slice[pos..];
                                consumed_total += pos;
                            } else {
                                slice = &slice[1..];
                                consumed_total += 1;
                            }
                        } else {
                            consumed_total = rx_offset;
                            break;
                        }
                    }
                }
            }

            if consumed_total > 0 {
                rx_buffer.copy_within(consumed_total..rx_offset, 0);
                rx_offset -= consumed_total;
            }
        }

        Ok(())
    }

    async fn handle_frame<S: Read + Write>(
        &self,
        frame: &CoraFrame<'_>,
        active_children: &[ChildDeviceInfo],
        stream: &mut S,
        tx_buf: &mut [u8; 2048],
    ) -> Result<(), S::Error> {
        // Keepalive probe handling
        if let Some(conn_id) = is_keepalive_probe(frame) {
            let mut ack_buf = [0u8; 48];
            if let Ok(len) =
                build_keepalive_ack_frame(conn_id, frame.header.message_id, &mut ack_buf)
            {
                stream.write_all(&ack_buf[..len]).await?;
            }
            return Ok(());
        }

        let cmd = parse_primary_query(frame.payload);
        if let Some(cmd) = cmd {
            debug!("Handling primary feature command: {:?}", cmd);
            let mut resp_payload = [0u8; 1024];
            resp_payload[0] = 0x03;
            resp_payload[1] = cmd.as_u8();
            let mut resp_len = 1024;

            match cmd {
                PrimaryFeatureCommand::GetDeviceInfo => {
                    let _ = build_device_info_payload(
                        VENDOR_ID_ELGATO,
                        PRODUCT_ID_NETWORK_DOCK,
                        &mut resp_payload,
                    );
                }
                PrimaryFeatureCommand::GetFirmwareVersion => {
                    let _ = build_firmware_version_payload(self.firmware, &mut resp_payload);
                }
                PrimaryFeatureCommand::GetSerialNumber => {
                    let _ = build_serial_number_payload(self.serial, &mut resp_payload);
                }
                PrimaryFeatureCommand::GetMacAddress => {
                    let _ = build_mac_address_payload(self.mac_address, &mut resp_payload);
                }
                PrimaryFeatureCommand::GetChildDeviceInfo => {
                    let requested_slot = if frame.payload.len() >= 3 {
                        frame.payload[2]
                    } else {
                        0
                    };
                    let mut p = [0u8; 128];

                    if let Some(child) = active_children
                        .iter()
                        .find(|c| c.slot_index == requested_slot)
                    {
                        child.build_payload(false, requested_slot, &mut p);
                    } else {
                        ChildDeviceInfo::disconnected(requested_slot).build_payload(
                            false,
                            requested_slot,
                            &mut p,
                        );
                    }

                    resp_payload[..128].copy_from_slice(&p);
                    resp_len = 128;
                }
                PrimaryFeatureCommand::Other(0x87) => {
                    resp_payload[2] = 0x01; // Static config
                    resp_payload[3] = 0x01; // Online
                    resp_payload[4..8].copy_from_slice(&self.ip_address);
                    resp_payload[8..12].copy_from_slice(&[255, 255, 255, 0]);
                    resp_payload[12..16].copy_from_slice(&self.ip_address);
                    resp_payload[16..18].copy_from_slice(&self.primary_port.to_le_bytes());
                }
                PrimaryFeatureCommand::Other(0x8f) => {
                    resp_payload[0] = 0x03;
                    resp_payload[1] = 0x8f;
                }
                PrimaryFeatureCommand::Other(0x1a) => {
                    resp_payload[0] = 0x03;
                    resp_payload[1] = 0x1a;
                }
                PrimaryFeatureCommand::Other(opcode) => {
                    resp_payload[0] = 0x03;
                    resp_payload[1] = opcode;
                }
            }

            if let Ok(out_len) = build_cora_response_frame(
                frame.header.message_id,
                &resp_payload[..resp_len],
                tx_buf,
            ) {
                stream.write_all(&tx_buf[..out_len]).await?;
            }
        }

        Ok(())
    }
}
