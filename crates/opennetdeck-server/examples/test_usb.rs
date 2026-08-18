use nusb::transfer::{ControlIn, ControlType, Recipient};
use opennetdeck_protocol::models::is_streamdeck_vendor;
use std::time::Duration;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let devices = nusb::list_devices().await?;
    for dev in devices {
        if is_streamdeck_vendor(dev.vendor_id()) {
            println!("Testing {:?}...", dev.product_string());
            let handle = dev.open().await?;
            let interface = handle.detach_and_claim_interface(0).await?;

            for report_id in [0x08, 0x06, 0x03, 0xa1, 0x80] {
                let control = ControlIn {
                    control_type: ControlType::Class,
                    recipient: Recipient::Interface,
                    request: 0x01,
                    value: (0x03 << 8) | (report_id as u16),
                    index: 0,
                    length: 32,
                };
                match interface
                    .control_in(control, Duration::from_millis(500))
                    .await
                {
                    Ok(data) => println!(
                        "Report 0x{:02x}: Ok ({} bytes): {:02x?}",
                        report_id,
                        data.len(),
                        data
                    ),
                    Err(e) => println!("Report 0x{:02x}: Err: {}", report_id, e),
                }
            }
        }
    }
    Ok(())
}
