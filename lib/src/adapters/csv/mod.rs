//! The CSV adapter expects the following format:
//! ```csv
//! timestamp,sequence_number,num_cores,num_antennas,num_subcarriers,rssi,csi
//! 3418319.67224585,23074,2,1,1,"98,28","(-0.9306254246008658-0.7152900288177244j),(-0.9533073248869779-0.8846499863951724j)"
//! ```

mod adapter;

use thiserror::Error;

/// Public reexport
pub use crate::adapters::csv::adapter::CSVAdapter;

/// Specific errors of the Nexmon Adapter
#[derive(Error, Debug)]
pub enum CSVAdapterError {
    #[error("Insufficient bytes to reconstruct header")]
    IncompleteHeader,

    #[error("Incomplete packet (missing payload bytes)")]
    IncompletePacket,

    #[error("Invalid code: {0}")]
    InvalidCode(u8),

    #[error("Invalid sequence number: {0}")]
    InvalidSequenceNumber(u16),

    #[error("Invalid number of columns in CSV row: {0}")]
    InvalidData(String),
}
