mod adapter;
mod header;
pub mod read;

use thiserror::Error;

/// Public reexport
pub use crate::adapters::iwl::adapter::IwlAdapter;

/// Specific errors of the Nexmon Adapter
#[derive(Error, Debug)]
pub enum IwlAdapterError {
    #[error("Insufficient bytes to reconstruct header")]
    IncompleteHeader,

    #[error("Incomplete packet (missing payload bytes)")]
    IncompletePacket,

    #[error("Invalid code: {0}")]
    InvalidCode(u8),

    #[error("Invalid beamforming matrix size: {0}")]
    InvalidMatrixSize(usize),

    #[error("Invalid antenna specification; Receive antennas: {num_rx}, streams: {num_streams}")]
    InvalidAntennaSpec { num_rx: usize, num_streams: usize },

    #[error("Invalid sequence number: {0}")]
    InvalidSequenceNumber(u16),
}
