//! CORA packet framing, header serialization, and codec operations.

use crate::constants::{CORA_HEADER_SIZE, CORA_MAGIC};
use crate::error::ProtocolError;

#[cfg(feature = "alloc")]
use alloc::vec::Vec;

/// CORA message flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CoraFlags(pub u16);

impl CoraFlags {
    pub const NONE: Self = Self(0x0000);
    pub const RESULT: Self = Self(0x0100);
    pub const ACK_NAK: Self = Self(0x0200);
    pub const REQ_ACK: Self = Self(0x4000);
    pub const VERBATIM: Self = Self(0x8000);

    #[inline]
    pub const fn contains(&self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }

    #[inline]
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    #[inline]
    pub const fn bits(&self) -> u16 {
        self.0
    }
}

/// HID operation code transmitted in the CORA header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum CoraHidOp {
    /// Pass-through data write (e.g. key image buffer, LCD graphics).
    Write = 0,
    /// Send HID feature report.
    SendReport = 1,
    /// Query HID feature report.
    GetReport = 2,
}

impl CoraHidOp {
    pub const fn from_u8(value: u8) -> Result<Self, ProtocolError> {
        match value {
            0 => Ok(Self::Write),
            1 => Ok(Self::SendReport),
            2 => Ok(Self::GetReport),
            other => Err(ProtocolError::UnknownHidOp(other)),
        }
    }

    pub const fn as_u8(&self) -> u8 {
        *self as u8
    }
}

/// 16-byte fixed-size CORA header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CoraHeader {
    pub flags: CoraFlags,
    pub hid_op: CoraHidOp,
    pub reserved: u8,
    pub message_id: u32,
    pub payload_len: u32,
}

impl CoraHeader {
    pub fn new(flags: CoraFlags, hid_op: CoraHidOp, message_id: u32, payload_len: usize) -> Self {
        Self {
            flags,
            hid_op,
            reserved: 0,
            message_id,
            payload_len: payload_len as u32,
        }
    }

    /// Parse header from a 16-byte slice.
    pub fn parse(buf: &[u8]) -> Result<Self, ProtocolError> {
        if buf.len() < CORA_HEADER_SIZE {
            return Err(ProtocolError::IncompleteHeader);
        }

        if buf[0..4] != CORA_MAGIC {
            return Err(ProtocolError::InvalidMagic);
        }

        let flags = CoraFlags(u16::from_le_bytes([buf[4], buf[5]]));
        let hid_op = CoraHidOp::from_u8(buf[6])?;
        let reserved = buf[7];
        let message_id = u32::from_le_bytes([buf[8], buf[9], buf[10], buf[11]]);
        let payload_len = u32::from_le_bytes([buf[12], buf[13], buf[14], buf[15]]);

        Ok(Self {
            flags,
            hid_op,
            reserved,
            message_id,
            payload_len,
        })
    }

    /// Encode header into a 16-byte buffer.
    pub fn encode(&self, out: &mut [u8; CORA_HEADER_SIZE]) {
        out[0..4].copy_from_slice(&CORA_MAGIC);
        out[4..6].copy_from_slice(&self.flags.bits().to_le_bytes());
        out[6] = self.hid_op.as_u8();
        out[7] = self.reserved;
        out[8..12].copy_from_slice(&self.message_id.to_le_bytes());
        out[12..16].copy_from_slice(&self.payload_len.to_le_bytes());
    }
}

/// A parsed CORA frame referencing the underlying payload slice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoraFrame<'a> {
    pub header: CoraHeader,
    pub payload: &'a [u8],
}

impl<'a> CoraFrame<'a> {
    pub fn new(header: CoraHeader, payload: &'a [u8]) -> Self {
        Self { header, payload }
    }

    /// Locate the first occurrence of `CORA_MAGIC` within `buf`.
    pub fn find_magic(buf: &[u8]) -> Option<usize> {
        if buf.len() < 4 {
            return None;
        }
        buf.windows(4).position(|w| w == CORA_MAGIC)
    }

    /// Try to decode a complete CORA frame from a byte buffer.
    /// Returns `Ok(Some((frame, consumed_bytes)))` on success,
    /// `Ok(None)` if more data is required, or `Err(ProtocolError)`.
    pub fn decode(buf: &'a [u8]) -> Result<Option<(CoraFrame<'a>, usize)>, ProtocolError> {
        if buf.len() < CORA_HEADER_SIZE {
            return Ok(None);
        }

        // Verify magic bytes
        if buf[0..4] != CORA_MAGIC {
            return Err(ProtocolError::InvalidMagic);
        }

        let header = CoraHeader::parse(&buf[..CORA_HEADER_SIZE])?;
        let total_len = CORA_HEADER_SIZE + (header.payload_len as usize);

        if buf.len() < total_len {
            return Ok(None);
        }

        let payload = &buf[CORA_HEADER_SIZE..total_len];
        Ok(Some((CoraFrame { header, payload }, total_len)))
    }

    /// Encode this frame into a destination buffer. Returns number of bytes written.
    pub fn encode(&self, out: &mut [u8]) -> Result<usize, ProtocolError> {
        let total_len = CORA_HEADER_SIZE + self.payload.len();
        if out.len() < total_len {
            return Err(ProtocolError::BufferTooSmall);
        }

        let mut header_buf = [0u8; CORA_HEADER_SIZE];
        let mut header = self.header;
        header.payload_len = self.payload.len() as u32;
        header.encode(&mut header_buf);

        out[0..CORA_HEADER_SIZE].copy_from_slice(&header_buf);
        out[CORA_HEADER_SIZE..total_len].copy_from_slice(self.payload);

        Ok(total_len)
    }

    #[cfg(feature = "alloc")]
    pub fn to_vec(&self) -> Vec<u8> {
        let mut vec = Vec::with_capacity(CORA_HEADER_SIZE + self.payload.len());
        let mut header_buf = [0u8; CORA_HEADER_SIZE];
        let mut header = self.header;
        header.payload_len = self.payload.len() as u32;
        header.encode(&mut header_buf);

        vec.extend_from_slice(&header_buf);
        vec.extend_from_slice(self.payload);
        vec
    }
}
