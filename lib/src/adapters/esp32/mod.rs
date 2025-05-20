//! ESP32 Wi-Fi Module
//!
//! This module provides support for parsing and interacting with ESP32 CSI data,
//!
//! - adapter: Contains the logic for interfacing with ESP32 devices.
pub mod adapter;

/// Re-export of the main adapter interface for ESP32 devices.
///
/// This allows external crates/modules to access the adapter functionality
/// via adapters::esp32::ESP32Adapter instead of the longer path adapters::esp32::adapter::ESP32Adapter.
pub use crate::adapters::esp32::adapter::ESP32Adapter;
