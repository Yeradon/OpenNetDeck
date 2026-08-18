//! Primary port feature reports, queries, and hotplug notifications.

use crate::cora::{CoraFlags, CoraFrame, CoraHeader, CoraHidOp};
use crate::error::ProtocolError;

/// Primary dock feature command opcodes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PrimaryFeatureCommand {
    /// 0x80: Request Dock Device Identity (Vendor ID and Product ID).
    GetDeviceInfo = 0x80,
    /// 0x83: Request AP2 Firmware Version string.
    GetFirmwareVersion = 0x83,
    /// 0x84: Request Dock Serial Number string.
    GetSerialNumber = 0x84,
    /// 0x85: Request Dock Network MAC Address.
    GetMacAddress = 0x85,
    /// 0x1c: Request Attached Downstream Child Device Info (`Device2Info`).
    GetChildDeviceInfo = 0x1c,
    /// Unknown or unsupported command opcode.
    Other(u8),
}

impl PrimaryFeatureCommand {
    pub fn from_u8(value: u8) -> Self {
        match value {
            0x80 => Self::GetDeviceInfo,
            0x83 => Self::GetFirmwareVersion,
            0x84 => Self::GetSerialNumber,
            0x85 => Self::GetMacAddress,
            0x1c => Self::GetChildDeviceInfo,
            other => Self::Other(other),
        }
    }

    pub fn as_u8(&self) -> u8 {
        match self {
            Self::GetDeviceInfo => 0x80,
            Self::GetFirmwareVersion => 0x83,
            Self::GetSerialNumber => 0x84,
            Self::GetMacAddress => 0x85,
            Self::GetChildDeviceInfo => 0x1c,
            Self::Other(v) => *v,
        }
    }
}

/// Parse the feature report command requested in a CORA payload.
pub fn parse_primary_query(payload: &[u8]) -> Option<PrimaryFeatureCommand> {
    if payload.is_empty() {
        return None;
    }
    if payload[0] == 0x03 && payload.len() >= 2 {
        Some(PrimaryFeatureCommand::from_u8(payload[1]))
    } else {
        Some(PrimaryFeatureCommand::from_u8(payload[0]))
    }
}

/// Build `0x80` Device Info report payload.
pub fn build_device_info_payload(
    vendor_id: u16,
    product_id: u16,
    out: &mut [u8],
) -> Result<usize, ProtocolError> {
    if out.len() < 16 {
        return Err(ProtocolError::BufferTooSmall);
    }
    out[..16].fill(0);
    out[0] = 0x03;
    out[1] = 0x80;
    out[12..14].copy_from_slice(&vendor_id.to_le_bytes());
    out[14..16].copy_from_slice(&product_id.to_le_bytes());
    Ok(16)
}

/// Build `0x83` Firmware Version AP2 report payload.
pub fn build_firmware_version_payload(
    version: &str,
    out: &mut [u8],
) -> Result<usize, ProtocolError> {
    if out.len() < 16 {
        return Err(ProtocolError::BufferTooSmall);
    }
    out[..16].fill(0);
    out[0] = 0x03;
    out[1] = 0x83;

    let ver_bytes = version.as_bytes();
    let copy_len = ver_bytes.len().min(8);
    out[8..8 + copy_len].copy_from_slice(&ver_bytes[..copy_len]);
    Ok(16)
}

/// Build `0x84` Serial Number report payload.
pub fn build_serial_number_payload(serial: &str, out: &mut [u8]) -> Result<usize, ProtocolError> {
    let serial_bytes = serial.as_bytes();
    let length = serial_bytes.len().min(32);
    let total_len = 4 + length;

    if out.len() < total_len {
        return Err(ProtocolError::BufferTooSmall);
    }
    out[..total_len].fill(0);
    out[0] = 0x03;
    out[1] = 0x84;
    out[2] = 0x00;
    out[3] = length as u8;
    out[4..4 + length].copy_from_slice(&serial_bytes[..length]);

    Ok(total_len)
}

/// Build `0x85` MAC Address report payload.
pub fn build_mac_address_payload(mac: [u8; 6], out: &mut [u8]) -> Result<usize, ProtocolError> {
    if out.len() < 10 {
        return Err(ProtocolError::BufferTooSmall);
    }
    out[..10].fill(0);
    out[0] = 0x03;
    out[1] = 0x85;
    out[2] = 0x00;
    out[3] = 0x00;
    out[4..10].copy_from_slice(&mac);

    Ok(10)
}

/// Child Stream Deck attachment info (`Device2Info`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChildDeviceInfo {
    pub connected: bool,
    pub vendor_id: u16,
    pub product_id: u16,
    pub product_name: [u8; 32],
    pub product_name_len: usize,
    pub serial_number: [u8; 32],
    pub serial_len: usize,
    pub tcp_port: u16,
}

impl ChildDeviceInfo {
    pub fn disconnected() -> Self {
        Self {
            connected: false,
            vendor_id: 0,
            product_id: 0,
            product_name: [0u8; 32],
            product_name_len: 0,
            serial_number: [0u8; 32],
            serial_len: 0,
            tcp_port: 0,
        }
    }

    pub fn connected(
        vendor_id: u16,
        product_id: u16,
        product_name: &str,
        serial: &str,
        tcp_port: u16,
    ) -> Self {
        let mut name_buf = [0u8; 32];
        let name_bytes = product_name.as_bytes();
        let name_len = name_bytes.len().min(31);
        name_buf[..name_len].copy_from_slice(&name_bytes[..name_len]);

        let mut serial_buf = [0u8; 32];
        let bytes = serial.as_bytes();
        let len = bytes.len().min(31);
        serial_buf[..len].copy_from_slice(&bytes[..len]);

        Self {
            connected: true,
            vendor_id,
            product_id,
            product_name: name_buf,
            product_name_len: name_len,
            serial_number: serial_buf,
            serial_len: len,
            tcp_port,
        }
    }

    pub fn serial_as_str(&self) -> Result<&str, ProtocolError> {
        core::str::from_utf8(&self.serial_number[..self.serial_len])
            .map_err(|_| ProtocolError::InvalidStringEncoding)
    }

    /// Parse a 128-byte `Device2Info` buffer from the device.
    pub fn parse(buf: &[u8]) -> Result<Option<Self>, ProtocolError> {
        if buf.len() < 128 {
            return Err(ProtocolError::BufferTooSmall);
        }

        // Byte 4: status (0x02 = connected)
        if buf[4] != 0x02 {
            return Ok(None);
        }

        let vendor_id = u16::from_le_bytes([buf[26], buf[27]]);
        let product_id = u16::from_le_bytes([buf[28], buf[29]]);

        let name_slice = &buf[32..64];
        let first_null_name = name_slice
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(name_slice.len());
        let mut product_name = [0u8; 32];
        let product_name_len = first_null_name.min(32);
        product_name[..product_name_len].copy_from_slice(&name_slice[..product_name_len]);

        let serial_slice = &buf[94..126];
        let first_null = serial_slice
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(serial_slice.len());
        let mut serial_number = [0u8; 32];
        let serial_len = first_null.min(32);
        serial_number[..serial_len].copy_from_slice(&serial_slice[..serial_len]);

        let tcp_port = u16::from_le_bytes([buf[126], buf[127]]);

        Ok(Some(Self {
            connected: true,
            vendor_id,
            product_id,
            product_name,
            product_name_len,
            serial_number,
            serial_len,
            tcp_port,
        }))
    }

    /// Build a 128-byte `Device2Info` payload with slot correlation and Elgato C++ metadata.
    pub fn build_payload(&self, is_push_notification: bool, slot_index: u8, out: &mut [u8; 128]) {
        out.fill(0);
        if is_push_notification {
            out[0] = 0x01;
            out[1] = 0x0b;
        } else {
            out[0] = 0x03;
            out[1] = 0x1c;
        }
        // Length of subsequent payload bytes (128 - 4 = 124 bytes)
        out[2..4].copy_from_slice(&(124u16).to_le_bytes());

        if self.connected {
            out[4] = 0x02; // Connected status
            out[5] = slot_index; // Internal Slot / Unit index

            // Vendor ID and Product ID
            out[26..28].copy_from_slice(&self.vendor_id.to_le_bytes());
            out[28..30].copy_from_slice(&self.product_id.to_le_bytes());

            // bcdDevice revision (0x0100 = 1.00)
            out[30] = 0x00;
            out[31] = 0x01;

            // Product Name string at offset 32..64
            let name_len = self.product_name_len.min(31);
            out[32..32 + name_len].copy_from_slice(&self.product_name[..name_len]);

            // Manufacturer string at offset 64..94
            let mfg = b"Elgato";
            out[64..64 + mfg.len()].copy_from_slice(mfg);

            // Serial number at offset 94..126
            let len = self.serial_len.min(31);
            out[94..94 + len].copy_from_slice(&self.serial_number[..len]);

            // TCP Port at offset 126..128
            out[126..128].copy_from_slice(&self.tcp_port.to_le_bytes());
        } else {
            out[4] = 0x00; // Disconnected status
        }
    }
}

/// Helper to wrap any payload buffer into a complete CORA response frame.
pub fn build_cora_response_frame(
    message_id: u32,
    payload: &[u8],
    out: &mut [u8],
) -> Result<usize, ProtocolError> {
    let header = CoraHeader::new(
        CoraFlags::RESULT,
        CoraHidOp::GetReport,
        message_id,
        payload.len(),
    );
    let frame = CoraFrame::new(header, payload);
    frame.encode(out)
}

/// Helper to wrap an unsolicited push event (e.g. child device hotplug) into a CORA frame.
pub fn build_cora_push_frame(payload: &[u8], out: &mut [u8]) -> Result<usize, ProtocolError> {
    let header = CoraHeader::new(CoraFlags::NONE, CoraHidOp::Write, 0, payload.len());
    let frame = CoraFrame::new(header, payload);
    frame.encode(out)
}
