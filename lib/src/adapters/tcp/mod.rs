//! TCP Adapter Module
//!
//! This module is basically a wrappter that looks at the sourceType of the Datamsg
//! And matches to the corresponding adapter
//!
//! - `adapter`: Mactching the correct adapter
pub mod adapter;

use thiserror::Error;

/// Re-export of the main adapter interface for Tcp
///
/// This allows external crates/modules to access the adapter functionality
/// via `adapters::tcp:TCPAdapter`
pub use crate::adapters::tcp::adapter::TCPAdapter;
