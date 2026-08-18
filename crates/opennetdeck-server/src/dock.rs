use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};

use opennetdeck_protocol::{ChildDeviceInfo, DEFAULT_PRIMARY_TCP_PORT, DEFAULT_SECONDARY_TCP_PORT};

use crate::usb::device::StreamDeckUsbHandle;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ServerMode {
    #[default]
    Dock,
    Direct,
}

impl std::str::FromStr for ServerMode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "dock" => Ok(Self::Dock),
            "direct" => Ok(Self::Direct),
            _ => Err(format!(
                "Invalid mode: '{}'. Expected 'dock' or 'direct'",
                s
            )),
        }
    }
}

/// Hardware and network configuration for the simulated/running dock.
#[derive(Debug, Clone)]
pub struct DockConfig {
    pub serial_number: String,
    pub firmware_version: String,
    pub mac_address: [u8; 6],
    pub primary_port: u16,
    pub secondary_port: u16,
    pub mode: ServerMode,
}

impl Default for DockConfig {
    fn default() -> Self {
        Self {
            serial_number: "DL01A1A00001".to_string(),
            firmware_version: "1.0.0.0".to_string(),
            mac_address: [0x00, 0x1A, 0x7D, 0xDA, 0x71, 0x01],
            primary_port: DEFAULT_PRIMARY_TCP_PORT,
            secondary_port: DEFAULT_SECONDARY_TCP_PORT,
            mode: ServerMode::Dock,
        }
    }
}

/// Internal dock state shared across connection handlers.
pub struct DockStateInner {
    pub config: DockConfig,
    pub child_device: Option<ChildDeviceInfo>,
    pub usb_device: Option<StreamDeckUsbHandle>,
}

#[derive(Clone)]
pub struct DockState {
    inner: Arc<RwLock<DockStateInner>>,
    hotplug_tx: broadcast::Sender<ChildDeviceInfo>,
    conn_counter: Arc<AtomicU8>,
}

impl DockState {
    pub fn new(config: DockConfig) -> Self {
        let (hotplug_tx, _) = broadcast::channel(32);
        Self {
            inner: Arc::new(RwLock::new(DockStateInner {
                config,
                child_device: None,
                usb_device: None,
            })),
            hotplug_tx,
            conn_counter: Arc::new(AtomicU8::new(1)),
        }
    }

    /// Allocate a sequential connection ID for a new client (1..=254).
    pub fn next_connection_id(&self) -> u8 {
        let mut id = self.conn_counter.fetch_add(1, Ordering::Relaxed);
        if id == 0 {
            id = self.conn_counter.fetch_add(1, Ordering::Relaxed);
        }
        id
    }

    /// Subscribe to hotplug change notifications.
    pub fn subscribe_hotplug(&self) -> broadcast::Receiver<ChildDeviceInfo> {
        self.hotplug_tx.subscribe()
    }

    /// Retrieve the current dock configuration.
    pub async fn config(&self) -> DockConfig {
        let guard = self.inner.read().await;
        guard.config.clone()
    }

    /// Get current downstream child device info.
    pub async fn child_device(&self) -> ChildDeviceInfo {
        let guard = self.inner.read().await;
        guard
            .child_device
            .clone()
            .unwrap_or_else(ChildDeviceInfo::disconnected)
    }

    /// Get current active USB device handle if connected.
    pub async fn usb_device(&self) -> Option<StreamDeckUsbHandle> {
        let guard = self.inner.read().await;
        guard.usb_device.clone()
    }

    /// Update downstream child device and USB handle state.
    pub async fn set_device(
        &self,
        child: Option<ChildDeviceInfo>,
        usb_dev: Option<StreamDeckUsbHandle>,
    ) {
        let to_broadcast = {
            let mut guard = self.inner.write().await;
            guard.child_device = child.clone();
            guard.usb_device = usb_dev;
            child.unwrap_or_else(ChildDeviceInfo::disconnected)
        };

        let _ = self.hotplug_tx.send(to_broadcast);
    }
}
