mod adapter;
mod header;
pub mod read;

use thiserror::Error;

/// Public reexport
pub use crate::adapters::iwl::adapter::IwlAdapter;
