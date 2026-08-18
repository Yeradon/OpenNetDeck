//! Generic embedded Secondary Stream Deck Bridge for TCP 5344.

use embedded_io_async::{Read, Write};
use log::{info, warn};
use opennetdeck_protocol::{
    build_device_info_payload, build_keepalive_ack_frame, is_keepalive_ack, is_keepalive_probe,
    ChildDeviceInfo, CoraFlags, CoraFrame, CoraHeader, CoraHidOp,
};

use crate::surface::{StreamDeckSurface, SurfaceEvent};

pub struct EmbeddedDeckBridge<'a, D: StreamDeckSurface> {
    pub surface: &'a mut D,
    pub slot_index: u8,
    pub port: u16,
}

impl<'a, D: StreamDeckSurface> EmbeddedDeckBridge<'a, D> {
    pub fn new(surface: &'a mut D, slot_index: u8, port: u16) -> Self {
        Self {
            surface,
            slot_index,
            port,
        }
    }

    /// Process a child Stream Deck connection session over any generic async I/O stream.
    pub async fn handle_connection<S: Read + Write>(
        &mut self,
        stream: &mut S,
    ) -> Result<(), S::Error> {
        info!(
            "Child Stream Deck connection established on port {}",
            self.port
        );

        let mut rx_buffer = [0u8; 2048];
        let mut tx_buffer = [0u8; 2048];
        let mut rx_offset = 0;

        loop {
            let n = match stream.read(&mut rx_buffer[rx_offset..]).await {
                Ok(0) => {
                    info!("Child Stream Deck client disconnected");
                    break;
                }
                Ok(n) => n,
                Err(e) => {
                    warn!("Stream read error on child deck: {:?}", e);
                    return Err(e);
                }
            };

            rx_offset += n;
            let mut consumed_total = 0;
            let mut slice = &rx_buffer[..rx_offset];

            while !slice.is_empty() {
                match CoraFrame::decode(slice) {
                    Ok(Some((frame, consumed))) => {
                        self.handle_incoming_frame(&frame, stream, &mut tx_buffer)
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

            // Check if there are any hardware surface events to push
            if let Some(event) = self.surface.poll_event() {
                let mut report = [0u8; 512];
                let report_len = encode_surface_event(&event, &mut report);
                if report_len > 0 {
                    let header = CoraHeader::new(CoraFlags::NONE, CoraHidOp::Write, 0, report_len);
                    let push_frame = CoraFrame::new(header, &report[..report_len]);
                    let msg = push_frame.to_vec();
                    stream.write_all(&msg).await?;
                }
            }
        }

        Ok(())
    }

    async fn handle_incoming_frame<S: Read + Write>(
        &mut self,
        frame: &CoraFrame<'_>,
        stream: &mut S,
        _tx_buf: &mut [u8; 2048],
    ) -> Result<(), S::Error> {
        // 1. Keepalive
        if let Some(conn_id) = is_keepalive_probe(frame) {
            let mut ack_buf = [0u8; 48];
            if let Ok(len) =
                build_keepalive_ack_frame(conn_id, frame.header.message_id, &mut ack_buf)
            {
                stream.write_all(&ack_buf[..len]).await?;
            }
            return Ok(());
        }

        if is_keepalive_ack(frame).is_some() {
            return Ok(());
        }

        let is_verbatim = frame.header.flags.contains(CoraFlags::VERBATIM);

        // 2. Feature Queries (GetReport, or Write with 0x03)
        if frame.header.hid_op == CoraHidOp::GetReport
            || (frame.header.hid_op == CoraHidOp::Write
                && !frame.payload.is_empty()
                && frame.payload[0] == 0x03)
        {
            let report_id = if frame.payload[0] == 0x03 && frame.payload.len() >= 2 {
                frame.payload[1]
            } else {
                frame.payload[0]
            };

            let mut resp_payload = [0u8; 1024];
            let mut resp_len = if frame.payload.len() >= 1024 {
                1024
            } else {
                32
            };
            resp_payload[0] = report_id;

            match report_id {
                0x05 => {
                    // Firmware version
                    let fw = self.surface.firmware_version().as_bytes();
                    let len = fw.len().min(30);
                    resp_payload[1] = len as u8;
                    resp_payload[6..6 + len].copy_from_slice(&fw[..len]);
                }
                0x06 => {
                    // Serial number
                    let sn = self.surface.serial_number().as_bytes();
                    let len = sn.len().min(30);
                    resp_payload[1] = len as u8;
                    resp_payload[2..2 + len].copy_from_slice(&sn[..len]);
                }
                0x0b => {
                    // Capabilities report
                    resp_payload[1] = 0x02;
                    resp_payload[2] = 0x30;
                    resp_payload[3] = 0x01;
                }
                0x80 => {
                    let _ = build_device_info_payload(
                        self.surface.vendor_id(),
                        self.surface.product_id(),
                        &mut resp_payload,
                    );
                    resp_len = 16;
                }
                0x1c => {
                    let child = ChildDeviceInfo::connected(
                        self.slot_index,
                        self.surface.vendor_id(),
                        self.surface.product_id(),
                        self.surface.model_name(),
                        self.surface.serial_number(),
                        self.port,
                    );
                    let mut buf128 = [0u8; 128];
                    child.build_payload(false, self.slot_index, &mut buf128);
                    resp_payload[..128].copy_from_slice(&buf128);
                    resp_len = 128;
                }
                _ => {
                    resp_payload[0] = 0x03;
                    resp_payload[1] = report_id;
                }
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
                resp_len,
            );
            let resp_frame = CoraFrame::new(header, &resp_payload[..resp_len]);
            stream.write_all(&resp_frame.to_vec()).await?;
            return Ok(());
        }

        // 3. Feature Writes (SendReport: brightness, reset)
        if frame.header.hid_op == CoraHidOp::SendReport {
            if frame.payload.len() >= 3 && frame.payload[0] == 0x03 && frame.payload[1] == 0x08 {
                let brightness = frame.payload[2];
                self.surface.set_brightness(brightness);
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
                stream.write_all(&resp_frame.to_vec()).await?;
            }
            return Ok(());
        }

        // 4. Output Writes (Write: Image / LCD drawing chunks)
        if frame.header.hid_op == CoraHidOp::Write {
            if frame.payload.len() >= 8
                && frame.payload[0] == 0x02
                && (frame.payload[1] == 0x07 || frame.payload[1] == 0x0c)
            {
                let key_index = frame.payload[2];
                let is_last = frame.payload[3] != 0;
                let chunk_index = u16::from_le_bytes([frame.payload[6], frame.payload[7]]);
                let img_data = &frame.payload[8..];
                self.surface
                    .write_image_chunk(key_index, is_last, chunk_index, img_data);
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
                stream.write_all(&resp_frame.to_vec()).await?;
            }
        }

        Ok(())
    }
}

fn encode_surface_event(event: &SurfaceEvent, out: &mut [u8; 512]) -> usize {
    out.fill(0);
    out[0] = 0x01; // Report ID 1

    match event {
        SurfaceEvent::KeyDown { key_index } => {
            out[1] = 0x00; // Key event
            out[2] = 0x08; // 8 keys
            let idx = (*key_index as usize).min(7);
            out[4 + idx] = 0x01;
            512
        }
        SurfaceEvent::KeyUp { key_index: _ } => {
            out[1] = 0x00; // Key event
            out[2] = 0x08;
            512
        }
        SurfaceEvent::DialRotate { dial_index, delta } => {
            out[1] = 0x03; // Dial event
            out[2] = *dial_index;
            out[3] = *delta as u8;
            512
        }
        SurfaceEvent::DialPress { dial_index } => {
            out[1] = 0x03;
            out[2] = *dial_index;
            out[3] = 0x01;
            512
        }
        SurfaceEvent::DialRelease { dial_index } => {
            out[1] = 0x03;
            out[2] = *dial_index;
            out[3] = 0x00;
            512
        }
        SurfaceEvent::TouchTap { dial_index, x, y } => {
            out[1] = 0x02; // Touch strip event
            out[2] = *dial_index;
            out[4..6].copy_from_slice(&x.to_le_bytes());
            out[6..8].copy_from_slice(&y.to_le_bytes());
            512
        }
    }
}
