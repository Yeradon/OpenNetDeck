use std::collections::HashMap;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};

use opennetdeck_protocol::{ChildDeviceInfo, DEFAULT_PRIMARY_TCP_PORT, DEFAULT_SECONDARY_TCP_PORT};

use crate::usb::device::StreamDeckUsbHandle;

pub const MAX_CHILD_SLOTS: usize = 8;

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

/// Information about an allocated child Stream Deck slot.
#[derive(Clone)]
pub struct SlotEntry {
    pub slot_index: u8,
    pub port: u16,
    pub serial: String,
    pub child_info: ChildDeviceInfo,
    pub usb_device: Option<StreamDeckUsbHandle>,
}

/// Internal dock state shared across connection handlers.
pub struct DockStateInner {
    pub config: DockConfig,
    pub slots: [Option<SlotEntry>; MAX_CHILD_SLOTS],
    pub sticky_map: HashMap<String, u8>,
}

#[derive(Clone)]
pub struct DockState {
    inner: Arc<RwLock<DockStateInner>>,
    hotplug_tx: broadcast::Sender<ChildDeviceInfo>,
    conn_counter: Arc<AtomicU8>,
}

impl DockState {
    pub fn new(config: DockConfig) -> Self {
        const INIT_SLOT: Option<SlotEntry> = None;
        let (hotplug_tx, _) = broadcast::channel(64);
        Self {
            inner: Arc::new(RwLock::new(DockStateInner {
                config,
                slots: [INIT_SLOT; MAX_CHILD_SLOTS],
                sticky_map: HashMap::new(),
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

    /// Get downstream child device info for a specific slot index (0..MAX_CHILD_SLOTS).
    pub async fn child_device_at(&self, slot_index: u8) -> ChildDeviceInfo {
        let guard = self.inner.read().await;
        let idx = slot_index as usize;
        if idx < MAX_CHILD_SLOTS {
            if let Some(ref entry) = guard.slots[idx] {
                return entry.child_info.clone();
            }
        }
        ChildDeviceInfo::disconnected(slot_index)
    }

    /// Get all currently active connected child devices across all slots.
    pub async fn all_active_children(&self) -> Vec<ChildDeviceInfo> {
        let guard = self.inner.read().await;
        guard
            .slots
            .iter()
            .filter_map(|s| s.as_ref().map(|entry| entry.child_info.clone()))
            .collect()
    }

    /// Get the first active physical USB handle (for single/direct mode).
    pub async fn first_usb_device(&self) -> Option<StreamDeckUsbHandle> {
        let guard = self.inner.read().await;
        guard
            .slots
            .iter()
            .find_map(|s| s.as_ref().and_then(|entry| entry.usb_device.clone()))
    }

    /// Allocate or re-bind a physical USB Stream Deck to a sticky slot.
    pub async fn attach_device(
        &self,
        device: StreamDeckUsbHandle,
        vid: u16,
        pid: u16,
        model_name: &str,
    ) -> (u8, u16) {
        let mut guard = self.inner.write().await;
        let serial = device.serial_number().to_string();

        // 1. Check if device has a sticky slot reservation
        let target_slot = if let Some(&prev_slot) = guard.sticky_map.get(&serial) {
            let idx = prev_slot as usize;
            if idx < MAX_CHILD_SLOTS && guard.slots[idx].is_none() {
                idx
            } else {
                find_lowest_free_slot(&guard.slots)
            }
        } else {
            find_lowest_free_slot(&guard.slots)
        };

        let slot_index = target_slot as u8;
        let port = guard
            .config
            .secondary_port
            .saturating_add(slot_index as u16);

        let child_info =
            ChildDeviceInfo::connected(slot_index, vid, pid, model_name, &serial, port);

        guard.slots[target_slot] = Some(SlotEntry {
            slot_index,
            port,
            serial: serial.clone(),
            child_info: child_info.clone(),
            usb_device: Some(device),
        });
        guard.sticky_map.insert(serial, slot_index);

        let _ = self.hotplug_tx.send(child_info);
        (slot_index, port)
    }

    /// Detach a device by serial number, freeing the slot while keeping sticky reservation.
    pub async fn detach_device_by_serial(&self, serial: &str) -> Option<(u8, ChildDeviceInfo)> {
        let mut guard = self.inner.write().await;
        let mut found = None;

        for i in 0..MAX_CHILD_SLOTS {
            if let Some(ref entry) = guard.slots[i] {
                if entry.serial == serial {
                    let slot_index = entry.slot_index;
                    let disconnected = ChildDeviceInfo::disconnected(slot_index);
                    guard.slots[i] = None;
                    found = Some((slot_index, disconnected));
                    break;
                }
            }
        }

        if let Some((_, ref disconnected)) = found {
            let _ = self.hotplug_tx.send(disconnected.clone());
        }

        found
    }

    /// Helper for testing/manual state overrides.
    pub async fn set_device_for_slot(
        &self,
        slot_index: u8,
        child: Option<ChildDeviceInfo>,
        usb_dev: Option<StreamDeckUsbHandle>,
    ) {
        let mut guard = self.inner.write().await;
        let idx = slot_index as usize;
        if idx < MAX_CHILD_SLOTS {
            if let Some(c) = child {
                guard.slots[idx] = Some(SlotEntry {
                    slot_index,
                    port: c.tcp_port,
                    serial: c.serial_as_str().unwrap_or("").to_string(),
                    child_info: c.clone(),
                    usb_device: usb_dev,
                });
                let _ = self.hotplug_tx.send(c);
            } else {
                guard.slots[idx] = None;
                let disconnected = ChildDeviceInfo::disconnected(slot_index);
                let _ = self.hotplug_tx.send(disconnected);
            }
        }
    }
}

fn find_lowest_free_slot(slots: &[Option<SlotEntry>; MAX_CHILD_SLOTS]) -> usize {
    for (i, slot) in slots.iter().enumerate() {
        if slot.is_none() {
            return i;
        }
    }
    0 // Fallback to slot 0 if all occupied
}
