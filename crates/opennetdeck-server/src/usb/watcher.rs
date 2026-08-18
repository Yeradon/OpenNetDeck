use std::net::{IpAddr, SocketAddr};
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

use opennetdeck_protocol::models::match_streamdeck_model;
use opennetdeck_protocol::ChildDeviceInfo;

use crate::bridge::SecondaryPortBridge;
use crate::dock::DockState;
use crate::usb::device::StreamDeckUsbHandle;

pub struct UsbWatcher {
    state: DockState,
    bind_ip: IpAddr,
    secondary_port: u16,
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
            secondary_port,
            override_pid,
        }
    }

    pub async fn run(self) {
        info!("Starting USB device hotplug watcher...");

        loop {
            match StreamDeckUsbHandle::open_first().await {
                Ok(Some(device)) => {
                    let serial = device.serial_number().to_string();
                    let vid = device.vendor_id();
                    let pid = self.override_pid.unwrap_or_else(|| device.product_id());
                    let model_name = match_streamdeck_model(vid, pid)
                        .map(|m| m.name())
                        .or_else(|| device.model().map(|m| m.name()))
                        .unwrap_or("Stream Deck");

                    info!(
                        serial = %serial,
                        model = %model_name,
                        vid = format_args!("0x{:04x}", vid),
                        pid = format_args!("0x{:04x}", pid),
                        port = self.secondary_port,
                        "Stream Deck hardware connected, starting secondary TCP bridge"
                    );

                    // Register child and USB handle with primary dock state
                    let child_info = ChildDeviceInfo::connected(
                        vid,
                        pid,
                        model_name,
                        &serial,
                        self.secondary_port,
                    );
                    self.state
                        .set_device(Some(child_info), Some(device.clone()))
                        .await;

                    // Run the secondary bridge server for this device
                    let bridge_bind_addr = SocketAddr::new(self.bind_ip, self.secondary_port);
                    let (disconnect_tx, mut disconnect_rx) = mpsc::channel::<()>(1);
                    let bridge = SecondaryPortBridge::new(bridge_bind_addr, device, disconnect_tx);

                    tokio::select! {
                        res = bridge.run() => {
                            if let Err(e) = res {
                                error!("Secondary bridge server exited with error: {}", e);
                            }
                        }
                        _ = disconnect_rx.recv() => {
                            warn!(serial = %serial, "USB device disconnected");
                        }
                    }

                    // Reset child device state upon disconnect
                    self.state.set_device(None, None).await;
                    info!("Secondary bridge stopped, resuming USB scan");
                }
                Ok(None) => {
                    tokio::time::sleep(Duration::from_millis(1500)).await;
                }
                Err(e) => {
                    debug!("USB scan check: {}", e);
                    tokio::time::sleep(Duration::from_millis(1500)).await;
                }
            }
        }
    }
}
