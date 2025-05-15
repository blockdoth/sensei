use crate::adapters::{csv::CSVAdapter, csv::CSVAdapterError};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum NetworkError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Error during (De)Serialization")]
    Serialization,

    #[error("Unable to connect to target")]
    UnableToConnect,

    #[error("Source closed (from source side)")]
    Closed,

    #[error("This client is already connected")]
    AlreadyConnected,

    #[error("Communication timed out")]
    Timeout(#[from] tokio::time::error::Elapsed),
}

#[derive(Error, Debug)]
pub enum SenseiError {
    #[error("Not implemented: {0}")]
    NotImplemented(String),
}

#[derive(Error, Debug)]
pub enum DataSourceError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Couldnt parse packet: {0}")]
    ParsingError(String),

    #[error("Incomplete packet (Source handler bug)")]
    IncompletePacket,

    #[error("Array conversion failed: {0}")]
    ArrayToNumber(#[from] std::array::TryFromSliceError),

    #[error("Controller failed: {0}")]
    Controller(String),

    #[error("Tried to use unimplemented feature: {0}")]
    NotImplemented(String),

    #[error(
        "Permission denied: application lacks sufficient privileges. See `README.md` for details on permissions."
    )]
    PermissionDenied,

    #[error("Read before starting (must call `start` before)")]
    ReadBeforeStart,
}

#[derive(Debug, Error)]
pub enum AppError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Failed with config parsing: {0}")]
    YamlError(#[from] serde_yaml::Error),

    #[error("Configuration error: {0}")]
    ConfigError(String),
}

// Common error for all adapters
#[derive(Error, Debug)]
pub enum CsiAdapterError {
    #[error("IWL Adapter Error: {0}")]
    IWL(#[from] IwlAdapterError),

    #[error("Invalid input, give a raw frame")]
    InvalidInput,

    #[error("CSV Adapter Error: {0}")]
    CSV(#[from] CSVAdapterError),
}

/// Specific errors of the Iwl adapter
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

/// Specific errors of the Atheros Adapter
#[derive(Error, Debug)]
pub enum AtherosAdapterError {
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

#[derive(Error, Debug)]
pub enum FileSourceError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("CSI adapter error: {0}")]
    Adapter(#[from] CsiAdapterError),
}

#[derive(Error, Debug)]
pub enum AdapterStreamError {
    #[error("CSI adapter error: {0}")]
    Adapter(#[from] CsiAdapterError),

    #[error("Expected RawFrame but received non-RawFrame DataMsg variant")]
    InvalidInput,
}

#[derive(Error, Debug)]
pub enum RawSourceTaskError {
    #[error("Generic RawSourceTask Error")]
    GenericError,
}

#[derive(Error, Debug)]
pub enum ControllerError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Expected file {0} to exist, but it didnt.")]
    FileNotPresent(String),

    #[error("Script failed with error: {0}")]
    ScriptError(String),

    #[error("Encountered error at data source during reconfiguration: {0}")]
    DataSource(#[from] DataSourceError),

    #[error("Given invalid parameters: {0}")]
    InvalidParams(String),

    #[error("(De-) Serialization returned an error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("Missing parameter: {0}")]
    MissingParameter(String),

    #[error("Failed to extract PhyName due to string conversions")]
    PhyName,
}
