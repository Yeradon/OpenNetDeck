use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, SocketAddr};
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

use opennetdeck_protocol::models::{is_streamdeck_vendor, match_streamdeck_model};

use crate::bridge::SecondaryPortBridge;
use crate::dock::DockState;
use crate::usb::device::StreamDeckUsbHandle;

struct ActiveBridge {
    slot_index: u8,
    port: u16,
    task_handle: tokio::task::JoinHandle<()>,
}

pub struct UsbWatcher {
    state: DockState,
    bind_ip: IpAddr,
    _secondary_port: u16,
    override_pid: Option<u16>,
}

impl UsbWatcher {
    pub fn new(
        state: DockState,
        bind_ip: IpAddr,
        secondary_port: u16,
        override_pid: Option<u16>,
    ) -> Self {
        Self {
            state,
            bind_ip,
            _secondary_port: secondary_port,
            override_pid,
        }
    }

    pub async fn run(self) {
        info!("Starting multi-device USB hotplug watcher...");

        let mut active_bridges: HashMap<String, ActiveBridge> = HashMap::new();
        let (global_disconnect_tx, mut global_disconnect_rx) = mpsc::channel::<String>(32);

        loop {
            // 1. Process any USB I/O disconnect signals from active bridges
            while let Ok(disconnected_serial) = global_disconnect_rx.try_recv() {
                if let Some(bridge) = active_bridges.remove(&disconnected_serial) {
                    bridge.task_handle.abort();
                    self.state
                        .detach_device_by_serial(&disconnected_serial)
                        .await;
                    info!(
                        serial = %disconnected_serial,
                        slot = bridge.slot_index,
                        port = bridge.port,
                        "Stream Deck hardware disconnected (USB transfer error)"
                    );
                }
            }

            // 2. Scan USB bus for attached Stream Deck hardware
            match nusb::list_devices().await {
                Ok(devices) => {
                    let mut seen_serials = HashSet::new();

                    for dev in devices {
                        if is_streamdeck_vendor(dev.vendor_id()) {
                            let serial = dev
                                .serial_number()
                                .map(|s| s.to_string())
                                .unwrap_or_else(|| "UNKNOWN_SERIAL".to_string());

                            seen_serials.insert(serial.clone());

                            if let std::collections::hash_map::Entry::Vacant(vacant) =
                                active_bridges.entry(serial.clone())
                            {
                                match StreamDeckUsbHandle::open_from_info(&dev).await {
                                    Ok(claimed_dev) => {
                                        let vid = claimed_dev.vendor_id();
                                        let pid = self
                                            .override_pid
                                            .unwrap_or_else(|| claimed_dev.product_id());
                                        let model_name = match_streamdeck_model(vid, pid)
                                            .map(|m| m.name())
                                            .or_else(|| claimed_dev.model().map(|m| m.name()))
                                            .unwrap_or("Stream Deck");

                                        let (slot_index, port) = self
                                            .state
                                            .attach_device(
                                                claimed_dev.clone(),
                                                vid,
                                                pid,
                                                model_name,
                                            )
                                            .await;

                                        info!(
                                            serial = %serial,
                                            model = %model_name,
                                            slot = slot_index,
                                            port = port,
                                            "Stream Deck attached: spawned dedicated secondary bridge"
                                        );

                                        let bridge_addr = SocketAddr::new(self.bind_ip, port);
                                        let (slot_dc_tx, mut slot_dc_rx) = mpsc::channel::<()>(1);
                                        let bridge = SecondaryPortBridge::new(
                                            bridge_addr,
                                            claimed_dev,
                                            slot_dc_tx,
                                        );

                                        let serial_clone = serial.clone();
                                        let forward_dc = global_disconnect_tx.clone();
                                        let task_handle = tokio::spawn(async move {
                                            tokio::select! {
                                                res = bridge.run() => {
                                                    if let Err(e) = res {
                                                        error!(
                                                            serial = %serial_clone,
                                                            port = port,
                                                            "Bridge listener error: {}", e
                                                        );
                                                    }
                                                }
                                                _ = slot_dc_rx.recv() => {
                                                    let _ = forward_dc.send(serial_clone).await;
                                                }
                                            }
                                        });

                                        vacant.insert(ActiveBridge {
                                            slot_index,
                                            port,
                                            task_handle,
                                        });
                                    }
                                    Err(e) => {
                                        warn!(serial = %serial, "Failed to claim attached Stream Deck: {}", e);
                                    }
                                }
                            }
                        }
                    }

                    // 3. Remove devices no longer detected on USB bus
                    let removed_serials: Vec<String> = active_bridges
                        .keys()
                        .filter(|s| !seen_serials.contains(*s))
                        .cloned()
                        .collect();

                    for serial in removed_serials {
                        if let Some(bridge) = active_bridges.remove(&serial) {
                            bridge.task_handle.abort();
                            self.state.detach_device_by_serial(&serial).await;
                            info!(
                                serial = %serial,
                                slot = bridge.slot_index,
                                port = bridge.port,
                                "Stream Deck unplugged from USB bus"
                            );
                        }
                    }
                }
                Err(e) => {
                    debug!("USB device scan error: {}", e);
                }
            }

            tokio::time::sleep(Duration::from_millis(1000)).await;
        }
    }
}
