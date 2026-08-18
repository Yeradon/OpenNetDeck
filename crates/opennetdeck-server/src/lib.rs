pub mod bridge;
pub mod connection;
pub mod discovery;
pub mod dock;
pub mod server;
pub mod usb;

pub use bridge::SecondaryPortBridge;
pub use discovery::DiscoveryService;
pub use dock::{DockConfig, DockState};
pub use server::PrimaryPortServer;
pub use usb::UsbWatcher;
