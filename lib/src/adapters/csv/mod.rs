//! The Csv adapter expects the following format:
//! ```csv
//! timestamp,sequence_number,num_cores,num_antennas,num_subcarriers,rssi,csi
//! 3418319.67224585,23074,2,1,1,"98,28","(-0.9306254246008658-0.7152900288177244j),(-0.9533073248869779-0.8846499863951724j)"
//! ```

pub mod adapter;

/// Public export
pub use crate::adapters::csv::adapter::CsvAdapter;
