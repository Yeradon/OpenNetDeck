use opennetdeck_protocol::models::{is_streamdeck_vendor, match_streamdeck_model};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("Scanning for Stream Deck USB devices...");
    let devices = nusb::list_devices().await?;
    for dev in devices {
        if is_streamdeck_vendor(dev.vendor_id()) {
            let model = match_streamdeck_model(dev.vendor_id(), dev.product_id());
            println!(
                "Found StreamDeck! VID: 0x{:04x}, PID: 0x{:04x}, Serial: {:?}, Product: {:?}, Model: {:?}",
                dev.vendor_id(),
                dev.product_id(),
                dev.serial_number(),
                dev.product_string(),
                model.map(|m| m.name())
            );

            // Test opening device
            match dev.open().await {
                Ok(handle) => {
                    println!("Successfully opened USB device handle!");
                    println!("Device speed: {:?}", handle.speed());
                    match handle.detach_and_claim_interface(0).await {
                        Ok(interface) => {
                            println!(
                                "Successfully detached kernel driver and claimed USB interface 0!"
                            );
                            for ep in interface.descriptors() {
                                println!("Endpoint descriptor: {:?}", ep);
                            }
                        }
                        Err(e) => {
                            println!("Note: detach_and_claim_interface returned: {}", e);
                        }
                    }
                }
                Err(e) => {
                    println!("Failed to open device: {}", e);
                }
            }
        }
    }
    Ok(())
}
