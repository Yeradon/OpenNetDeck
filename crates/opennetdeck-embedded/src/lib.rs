//! Generic bare-metal MCU (no_std) runtime for OpenNetDeck.
//!
//! Compatible with Embassy, smoltcp, esp-wifi, and standard embedded-io-async streams.

#![no_std]

#[cfg(feature = "alloc")]
extern crate alloc;

pub mod bridge;
pub mod hub;
pub mod surface;

pub use bridge::EmbeddedDeckBridge;
pub use hub::EmbeddedDockHub;
pub use surface::{StreamDeckSurface, SurfaceEvent, VirtualStreamDeckPlus};
