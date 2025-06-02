//! Intel Wireless Link (IWL) Adapter Module
//!
//! This module provides support for parsing and interacting with CSI (Channel State Information)
//! data obtained from Intel Wireless Link (IWL) devices. It includes:
//!
//! - `adapter`: Logic for interfacing with the IWL hardware and processing CSI frames.
//! - `header`: Structures and parsers for interpreting IWL-specific packet headers.
pub mod adapter;
mod header;

/// Re-export of the main adapter interface for IWL devices.
///
/// This allows external crates/modules to access the adapter functionality
/// via `adapters::iwl::IwlAdapter` instead of the longer path `adapters::iwl::adapter::IwlAdapter`.
pub use crate::adapters::iwl::adapter::IwlAdapter;
