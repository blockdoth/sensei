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
