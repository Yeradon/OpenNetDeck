//! Stream Deck hardware models, product IDs, and capabilities.

use crate::constants::{VENDOR_ID_CORSAIR, VENDOR_ID_ELGATO};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamDeckModel {
    Original,
    OriginalV2,
    OriginalMK2,
    OriginalMK2Scissor,
    Mini,
    XL,
    Plus,
    Pedal,
    Neo,
    Studio,
    Module6,
    Module15,
    Module32,
    PlusXL,
    GalleonK100,
}

impl StreamDeckModel {
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Original => "Stream Deck",
            Self::OriginalV2 => "Stream Deck (v2)",
            Self::OriginalMK2 => "Stream Deck MK.2",
            Self::OriginalMK2Scissor => "Stream Deck MK.2 (Scissor)",
            Self::Mini => "Stream Deck Mini",
            Self::XL => "Stream Deck XL",
            Self::Plus => "Stream Deck +",
            Self::Pedal => "Stream Deck Pedal",
            Self::Neo => "Stream Deck Neo",
            Self::Studio => "Stream Deck Studio",
            Self::Module6 => "Stream Deck 6 Module",
            Self::Module15 => "Stream Deck 15 Module",
            Self::Module32 => "Stream Deck 32 Module",
            Self::PlusXL => "Stream Deck + XL",
            Self::GalleonK100 => "Corsair Galleon K100",
        }
    }

    pub const fn key_count(&self) -> usize {
        match self {
            Self::Original
            | Self::OriginalV2
            | Self::OriginalMK2
            | Self::OriginalMK2Scissor
            | Self::Module15 => 15,
            Self::Mini | Self::Module6 => 6,
            Self::XL | Self::Module32 => 32,
            Self::Plus => 8,
            Self::Pedal => 3,
            Self::Neo => 8,
            Self::Studio => 32,
            Self::PlusXL => 16,
            Self::GalleonK100 => 6,
        }
    }

    pub const fn encoder_count(&self) -> usize {
        match self {
            Self::Plus => 4,
            Self::PlusXL => 4,
            Self::Studio => 2,
            _ => 0,
        }
    }

    pub const fn has_lcd_strip(&self) -> bool {
        matches!(self, Self::Plus | Self::PlusXL | Self::Neo)
    }
}

pub fn is_streamdeck_vendor(vendor_id: u16) -> bool {
    vendor_id == VENDOR_ID_ELGATO || vendor_id == VENDOR_ID_CORSAIR
}

pub fn match_streamdeck_model(vendor_id: u16, product_id: u16) -> Option<StreamDeckModel> {
    if vendor_id == VENDOR_ID_ELGATO {
        match product_id {
            0x0060 => Some(StreamDeckModel::Original),
            0x006d => Some(StreamDeckModel::OriginalV2),
            0x0080 => Some(StreamDeckModel::OriginalMK2),
            0x00a5 => Some(StreamDeckModel::OriginalMK2Scissor),
            0x0063 | 0x0090 | 0x00b3 => Some(StreamDeckModel::Mini),
            0x006c | 0x008f => Some(StreamDeckModel::XL),
            0x0084 => Some(StreamDeckModel::Plus),
            0x0086 => Some(StreamDeckModel::Pedal),
            0x009a => Some(StreamDeckModel::Neo),
            0x00aa => Some(StreamDeckModel::Studio),
            0x00b8 => Some(StreamDeckModel::Module6),
            0x00b9 => Some(StreamDeckModel::Module15),
            0x00ba => Some(StreamDeckModel::Module32),
            0x00c6 => Some(StreamDeckModel::PlusXL),
            _ => None,
        }
    } else if vendor_id == VENDOR_ID_CORSAIR {
        match product_id {
            0x2b18 => Some(StreamDeckModel::GalleonK100),
            _ => None,
        }
    } else {
        None
    }
}
