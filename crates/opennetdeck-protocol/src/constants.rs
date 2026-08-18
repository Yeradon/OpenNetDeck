//! Protocol constants for Elgato Network Dock and the CORA protocol.

/// The 4-byte magic signature present at the start of every CORA packet: 0x43, 0x93, 0x8a, 0x41.
pub const CORA_MAGIC: [u8; 4] = [0x43, 0x93, 0x8a, 0x41];

/// Default TCP port for the primary Network Dock control service.
pub const DEFAULT_PRIMARY_TCP_PORT: u16 = 5343;

/// Default TCP port assigned for downstream child Stream Deck USB bridging.
pub const DEFAULT_SECONDARY_TCP_PORT: u16 = 5344;

/// Elgato USB Vendor ID (0x0fd9 = 4057).
pub const VENDOR_ID_ELGATO: u16 = 0x0fd9;

/// Corsair USB Vendor ID (0x1b1c).
pub const VENDOR_ID_CORSAIR: u16 = 0x1b1c;

/// Special product ID reported by the Stream Deck Network Dock.
pub const PRODUCT_ID_NETWORK_DOCK: u16 = 0xffff;

/// Device type identifier reported in mDNS TXT record (`dt=215`).
pub const DEVICE_TYPE_NETWORK_DOCK: u8 = 215;

/// Connection timeout duration in milliseconds before dropping an inactive client.
pub const TIMEOUT_DURATION_MS: u64 = 5000;

/// Default keepalive ping interval in milliseconds.
pub const KEEPALIVE_INTERVAL_MS: u64 = 2500;

/// Size of the CORA message header in bytes.
pub const CORA_HEADER_SIZE: usize = 16;
