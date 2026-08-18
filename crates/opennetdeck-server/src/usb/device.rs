use nusb::transfer::{
    ControlIn, ControlOut, ControlType, In, Interrupt, Out, Recipient, TransferError,
};
use nusb::Interface;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, Mutex};
use tracing::{debug, error, info, warn};

use opennetdeck_protocol::models::{match_streamdeck_model, StreamDeckModel};

#[derive(Debug, thiserror::Error)]
pub enum UsbDeviceError {
    #[error("USB device error: {0}")]
    Device(#[from] nusb::Error),
    #[error("USB transfer error: {0}")]
    Transfer(#[from] TransferError),
    #[error("USB I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Device not connected")]
    NotConnected,
}

/// Active handle to a physical Stream Deck USB device.
#[derive(Clone)]
pub struct StreamDeckUsbHandle {
    inner: Arc<StreamDeckUsbInner>,
}

struct StreamDeckUsbInner {
    pub vendor_id: u16,
    pub product_id: u16,
    pub serial_number: String,
    pub model: Option<StreamDeckModel>,
    pub interface: Interface,
    pub ep_in_addr: u8,
    pub ep_out_addr: Option<u8>,
    pub max_packet_size: usize,
    pub write_lock: Mutex<()>,
}

impl StreamDeckUsbHandle {
    pub async fn open_first() -> Result<Option<Self>, UsbDeviceError> {
        let devices = nusb::list_devices().await?;
        for dev in devices {
            if opennetdeck_protocol::models::is_streamdeck_vendor(dev.vendor_id()) {
                let model = match_streamdeck_model(dev.vendor_id(), dev.product_id());
                info!(
                    vid = dev.vendor_id(),
                    pid = dev.product_id(),
                    model = ?model.map(|m| m.name()),
                    "Found Stream Deck USB device, opening..."
                );

                let handle = match dev.open().await {
                    Ok(h) => h,
                    Err(e) => {
                        warn!("Failed to open USB device: {}", e);
                        continue;
                    }
                };

                let interface = match handle.detach_and_claim_interface(0).await {
                    Ok(i) => i,
                    Err(e) => {
                        error!(
                            "Failed to detach kernel driver and claim interface 0: {}",
                            e
                        );
                        continue;
                    }
                };

                let mut ep_in_addr = 0x81;
                let mut ep_out_addr = Some(0x02);
                let mut max_packet_size = 512;

                for alt in interface.descriptors() {
                    for ep in alt.endpoints() {
                        if ep.direction() == nusb::transfer::Direction::In {
                            ep_in_addr = ep.address();
                            max_packet_size = ep.max_packet_size();
                        } else if ep.direction() == nusb::transfer::Direction::Out {
                            ep_out_addr = Some(ep.address());
                        }
                    }
                }

                // Query serial number via USB descriptor or standard feature report 0x06 / 0x84 / 0x03
                let serial_number = dev
                    .serial_number()
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| "UNKNOWN_SERIAL".to_string());

                info!(
                    serial = %serial_number,
                    ep_in = format_args!("0x{:02x}", ep_in_addr),
                    ep_out = ?ep_out_addr.map(|a| format!("0x{:02x}", a)),
                    "Successfully claimed Stream Deck device"
                );

                return Ok(Some(Self {
                    inner: Arc::new(StreamDeckUsbInner {
                        vendor_id: dev.vendor_id(),
                        product_id: dev.product_id(),
                        serial_number,
                        model,
                        interface,
                        ep_in_addr,
                        ep_out_addr,
                        max_packet_size,
                        write_lock: Mutex::new(()),
                    }),
                }));
            }
        }
        Ok(None)
    }

    pub fn vendor_id(&self) -> u16 {
        self.inner.vendor_id
    }

    pub fn product_id(&self) -> u16 {
        self.inner.product_id
    }

    pub fn serial_number(&self) -> &str {
        &self.inner.serial_number
    }

    pub fn model(&self) -> Option<StreamDeckModel> {
        self.inner.model
    }

    /// Read a Feature Report from the physical device using a USB Control In transfer.
    pub async fn get_feature_report(
        &self,
        report_id: u8,
        length: usize,
    ) -> Result<Vec<u8>, UsbDeviceError> {
        let _guard = self.inner.write_lock.lock().await;
        let control = ControlIn {
            control_type: ControlType::Class,
            recipient: Recipient::Interface,
            request: 0x01, // GET_REPORT
            value: (0x03 << 8) | (report_id as u16),
            index: 0,
            length: length.max(32) as u16,
        };

        let result = self
            .inner
            .interface
            .control_in(control, Duration::from_millis(1500))
            .await;

        match result {
            Ok(bytes) => Ok(bytes),
            Err(e) => Err(UsbDeviceError::Transfer(e)),
        }
    }

    /// Send a Feature Report to the physical device using a USB Control Out transfer.
    pub async fn set_feature_report(&self, data: &[u8]) -> Result<(), UsbDeviceError> {
        if data.is_empty() {
            return Ok(());
        }
        let _guard = self.inner.write_lock.lock().await;
        let report_id = data[0];
        let control = ControlOut {
            control_type: ControlType::Class,
            recipient: Recipient::Interface,
            request: 0x09, // SET_REPORT
            value: (0x03 << 8) | (report_id as u16),
            index: 0,
            data,
        };

        let result = self
            .inner
            .interface
            .control_out(control, Duration::from_millis(1500))
            .await;

        match result {
            Ok(_) => Ok(()),
            Err(e) => Err(UsbDeviceError::Transfer(e)),
        }
    }

    /// Write raw output data (e.g. image chunks) to the device's OUT endpoint.
    pub async fn write_out(&self, data: &[u8]) -> Result<(), UsbDeviceError> {
        if data.is_empty() {
            return Ok(());
        }
        let _guard = self.inner.write_lock.lock().await;

        if let Some(ep_out_addr) = self.inner.ep_out_addr {
            let mut ep = self
                .inner
                .interface
                .endpoint::<Interrupt, Out>(ep_out_addr)
                .or_else(|_| self.inner.interface.endpoint::<Interrupt, Out>(ep_out_addr))?;

            let mut req = ep.allocate(data.len());
            req.extend_from_slice(data);
            ep.submit(req);

            match ep.next_complete().await.status {
                Ok(_) => Ok(()),
                Err(e) => Err(UsbDeviceError::Transfer(e)),
            }
        } else {
            // Devices without dedicated OUT endpoint use control transfers (report 0x02)
            let control = ControlOut {
                control_type: ControlType::Class,
                recipient: Recipient::Interface,
                request: 0x09,
                value: (0x02 << 8) | (data[0] as u16),
                index: 0,
                data,
            };
            self.inner
                .interface
                .control_out(control, Duration::from_millis(1500))
                .await
                .map(|_| ())
                .map_err(UsbDeviceError::Transfer)
        }
    }

    /// Spawns a background task continuously polling the Interrupt IN endpoint.
    /// Emits raw input reports to `tx`.
    pub fn spawn_input_reader(
        &self,
        tx: mpsc::Sender<Vec<u8>>,
        disconnect_tx: mpsc::Sender<()>,
    ) -> tokio::task::JoinHandle<()> {
        let interface = self.inner.interface.clone();
        let ep_in_addr = self.inner.ep_in_addr;
        let max_packet_size = self.inner.max_packet_size;

        tokio::spawn(async move {
            let mut ep_in = match interface.endpoint::<Interrupt, In>(ep_in_addr) {
                Ok(ep) => ep,
                Err(e) => {
                    error!(
                        "Failed to open interrupt IN endpoint 0x{:02x}: {}",
                        ep_in_addr, e
                    );
                    return;
                }
            };

            debug!(
                ep = format_args!("0x{:02x}", ep_in_addr),
                "Starting USB interrupt IN reader loop"
            );

            loop {
                let buf = ep_in.allocate(max_packet_size);
                ep_in.submit(buf);

                let completion = ep_in.next_complete().await;
                match completion.status {
                    Ok(_) => {
                        let actual_len = completion.actual_len;
                        if actual_len > 0 {
                            let data = completion.buffer[..actual_len].to_vec();
                            if tx.send(data).await.is_err() {
                                debug!("Input report channel closed, exiting reader loop");
                                break;
                            }
                        }
                    }
                    Err(e) => {
                        error!("Error reading interrupt IN endpoint: {}", e);
                        if let TransferError::Disconnected = e {
                            let _ = disconnect_tx.send(()).await;
                            break;
                        }
                        tokio::time::sleep(Duration::from_millis(50)).await;
                    }
                }
            }
        })
    }
}
