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
