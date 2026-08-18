//! Keepalive probing and acknowledgment handling.

use crate::constants::CORA_HEADER_SIZE;
use crate::cora::{CoraFlags, CoraFrame, CoraHeader, CoraHidOp};
use crate::error::ProtocolError;

#[cfg(feature = "alloc")]
use alloc::vec::Vec;

pub const KEEPALIVE_PROBE_LEN: usize = 32;
pub const KEEPALIVE_ACK_LEN: usize = 32;

/// Checks if a frame is a keepalive probe from the dock/device.
/// If valid, returns the `connection_id` embedded at payload index 5.
pub fn is_keepalive_probe(frame: &CoraFrame) -> Option<u8> {
    if frame.payload.len() >= 6 && frame.payload[0] == 0x01 && frame.payload[1] == 0x0a {
        Some(frame.payload[5])
    } else {
        None
    }
}

/// Checks if a frame is a keepalive ACK response from a connected client.
/// Supports both CORA standard with ACK_NAK flag and Elgato client Write format.
pub fn is_keepalive_ack(frame: &CoraFrame) -> Option<u8> {
    if frame.payload.len() >= 3 && frame.payload[0] == 0x03 && frame.payload[1] == 0x1a {
        Some(frame.payload[2])
    } else {
        None
    }
}

/// Build a 32-byte keepalive probe packet payload for a given connection id.
pub fn build_keepalive_probe_payload(connection_id: u8, out: &mut [u8; KEEPALIVE_PROBE_LEN]) {
    out.fill(0);
    out[0] = 0x01;
    out[1] = 0x0a;
    out[5] = connection_id;
}

/// Build a complete CORA frame for a keepalive probe.
pub fn build_keepalive_probe_frame(
    connection_id: u8,
    message_id: u32,
    out: &mut [u8],
) -> Result<usize, ProtocolError> {
    let mut payload = [0u8; KEEPALIVE_PROBE_LEN];
    build_keepalive_probe_payload(connection_id, &mut payload);

    let header = CoraHeader::new(
        CoraFlags::NONE,
        CoraHidOp::Write,
        message_id,
        KEEPALIVE_PROBE_LEN,
    );

    let frame = CoraFrame::new(header, &payload);
    frame.encode(out)
}

/// Build a 32-byte keepalive ACK packet payload.
pub fn build_keepalive_ack_payload(connection_id: u8, out: &mut [u8; KEEPALIVE_ACK_LEN]) {
    out.fill(0);
    out[0] = 0x03;
    out[1] = 0x1a; // 26
    out[2] = connection_id;
}

/// Build a complete CORA frame for a keepalive ACK.
pub fn build_keepalive_ack_frame(
    connection_id: u8,
    message_id: u32,
    out: &mut [u8],
) -> Result<usize, ProtocolError> {
    let mut payload = [0u8; KEEPALIVE_ACK_LEN];
    build_keepalive_ack_payload(connection_id, &mut payload);

    let header = CoraHeader::new(
        CoraFlags::ACK_NAK,
        CoraHidOp::Write,
        message_id,
        KEEPALIVE_ACK_LEN,
    );

    let frame = CoraFrame::new(header, &payload);
    frame.encode(out)
}

#[cfg(feature = "alloc")]
pub fn build_keepalive_probe_vec(connection_id: u8, message_id: u32) -> Vec<u8> {
    let mut buf = [0u8; CORA_HEADER_SIZE + KEEPALIVE_PROBE_LEN];
    build_keepalive_probe_frame(connection_id, message_id, &mut buf).unwrap();
    buf.to_vec()
}

#[cfg(feature = "alloc")]
pub fn build_keepalive_ack_vec(connection_id: u8, message_id: u32) -> Vec<u8> {
    let mut buf = [0u8; CORA_HEADER_SIZE + KEEPALIVE_ACK_LEN];
    build_keepalive_ack_frame(connection_id, message_id, &mut buf).unwrap();
    buf.to_vec()
}
