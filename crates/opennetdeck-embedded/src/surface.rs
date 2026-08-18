//! Generic Stream Deck hardware/surface interface trait for bare-metal MCUs.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceEvent {
    KeyDown { key_index: u8 },
    KeyUp { key_index: u8 },
    DialRotate { dial_index: u8, delta: i8 },
    DialPress { dial_index: u8 },
    DialRelease { dial_index: u8 },
    TouchTap { dial_index: u8, x: u16, y: u16 },
}

/// Generic interface that an MCU board implementation provides for Stream Deck hardware.
pub trait StreamDeckSurface {
    /// Vendor ID (default 0x0fd9).
    fn vendor_id(&self) -> u16 {
        0x0fd9
    }

    /// Product ID (e.g. 0x0084 for Plus, 0x0080 for MK.2, 0x0063 for Mini).
    fn product_id(&self) -> u16;

    /// Device model display name.
    fn model_name(&self) -> &str;

    /// Serial number string.
    fn serial_number(&self) -> &str;

    /// Firmware version string.
    fn firmware_version(&self) -> &str {
        "1.0.0.0"
    }

    /// Number of physical keys.
    fn key_count(&self) -> usize;

    /// Number of rotary encoders.
    fn dial_count(&self) -> usize {
        0
    }

    /// Set brightness (0..=100%).
    fn set_brightness(&mut self, percentage: u8) {
        let _ = percentage;
    }

    /// Write an image chunk for a key or LCD display.
    fn write_image_chunk(&mut self, key_index: u8, is_last: bool, chunk_index: u16, data: &[u8]) {
        let _ = (key_index, is_last, chunk_index, data);
    }

    /// Poll for any pending button, encoder, or touch event.
    fn poll_event(&mut self) -> Option<SurfaceEvent> {
        None
    }
}

/// A default virtual Stream Deck Plus surface for boards with or without physical keys.
pub struct VirtualStreamDeckPlus<'a> {
    pub serial: &'a str,
    pub firmware: &'a str,
    pub brightness: u8,
}

impl<'a> VirtualStreamDeckPlus<'a> {
    pub fn new(serial: &'a str) -> Self {
        Self {
            serial,
            firmware: "2.0.3.5",
            brightness: 100,
        }
    }
}

impl<'a> StreamDeckSurface for VirtualStreamDeckPlus<'a> {
    fn product_id(&self) -> u16 {
        0x0084 // Stream Deck +
    }

    fn model_name(&self) -> &str {
        "Stream Deck +"
    }

    fn serial_number(&self) -> &str {
        self.serial
    }

    fn firmware_version(&self) -> &str {
        self.firmware
    }

    fn key_count(&self) -> usize {
        8
    }

    fn dial_count(&self) -> usize {
        4
    }

    fn set_brightness(&mut self, percentage: u8) {
        self.brightness = percentage;
    }
}
