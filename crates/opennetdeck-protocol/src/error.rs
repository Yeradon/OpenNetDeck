//! Protocol error definitions.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtocolError {
    /// Buffer is smaller than the required header or payload size.
    BufferTooSmall,
    /// The stream does not begin with the valid CORA magic bytes.
    InvalidMagic,
    /// Unknown or unsupported HID opcode received.
    UnknownHidOp(u8),
    /// Message header specifies a payload larger than supported or available.
    PayloadLengthExceeded,
    /// Header was received only partially.
    IncompleteHeader,
    /// Frame was received only partially.
    IncompleteFrame { expected: usize, available: usize },
    /// Report ID in query or response was not recognized.
    UnknownReportId(u8),
    /// Payload formatting or offset layout is invalid.
    InvalidPayloadLayout,
    /// Serial string or version string is not valid ASCII / UTF-8.
    InvalidStringEncoding,
}

#[cfg(feature = "std")]
impl std::error::Error for ProtocolError {}

#[cfg(feature = "std")]
impl core::fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::BufferTooSmall => write!(f, "Buffer is too small for the operation"),
            Self::InvalidMagic => write!(f, "Invalid CORA magic header bytes"),
            Self::UnknownHidOp(op) => write!(f, "Unknown HID opcode: {:#04x}", op),
            Self::PayloadLengthExceeded => {
                write!(f, "Payload length exceeds maximum allowable size")
            }
            Self::IncompleteHeader => write!(f, "Incomplete CORA header received"),
            Self::IncompleteFrame {
                expected,
                available,
            } => {
                write!(
                    f,
                    "Incomplete frame: expected {} bytes, got {}",
                    expected, available
                )
            }
            Self::UnknownReportId(id) => write!(f, "Unknown report ID: {:#04x}", id),
            Self::InvalidPayloadLayout => write!(f, "Invalid report payload layout or offsets"),
            Self::InvalidStringEncoding => write!(f, "String payload is not valid ASCII"),
        }
    }
}
