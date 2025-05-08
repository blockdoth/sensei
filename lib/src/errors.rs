use crate::adapters::iwl::IwlAdapterError;
use thiserror::Error;

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