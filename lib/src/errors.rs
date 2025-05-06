use thiserror::Error;

#[derive(Error, Debug)]
pub enum SenseiError {
    #[error("Not implemented: {0}")]
    NotImplemented(String),
}

#[derive(Error, Debug)]
pub enum CommunicationError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("(De-) Serialization returned an error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("Source closed (from source side)")]
    Closed,

    #[error("This client is already connected")]
    AlreadyConnected,

    #[error("Communication timed out")]
    Timeout(#[from] tokio::time::error::Elapsed),
}

#[derive(Error, Debug)]
pub enum DataSourceError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[cfg(feature = "pcap_source")]
    #[error("PCAP error: {0}")]
    Pcap(#[from] pcap::Error),

    #[error("Read before starting (must call `start` before)")]
    ReadBeforeStart,

    #[error("Specified unknown interface: {0}")]
    UnknownInterface(String),

    #[error("Source closed (from source side)")]
    Closed,

    #[error("Error in underlying communication: {0}")]
    Communication(#[from] CommunicationError),

    #[error("Controller failed: {0}")]
    Controller(String),

    #[error("Tried to use unimplemented feature: {0}")]
    NotImplemented(String),

    #[error("Couldnt parse packet: {0}")]
    ParsingError(String),

    #[error("Permission denied: application lacks sufficient privileges. See `README.md` for details on permissions.")]
    PermissionDenied,

    #[error("Incomplete packet (Source handler bug)")]
    IncompletePacket,

    #[error("Array conversion failed: {0}")]
    ArrayToNumber(#[from] std::array::TryFromSliceError),
}

#[derive(Error, Debug)]
pub enum SinkError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Failed to send to queue: {0}")]
    Queue(#[from] tokio::sync::mpsc::error::SendError<crate::csi_types::CsiData>),

    #[error("Error: {0}")]
    Generic(#[from] SenseiError),

    #[error("Channel closed; Sink disconnected.")]
    Disconnected,
}

#[derive(Error, Debug)]
pub enum SourceHandlerError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Unknown service {0}")]
    UnkownService(String),

    #[error("Service unavailable (queue closed, likely an error at source)")]
    ServiceUnavailable,

    #[error("Service already exists")]
    ServiceExists,

    #[error("Controller failed: {0}")]
    ControllerError(#[from] ControllerError),

    #[error("Error from source: {0}")]
    SourceError(#[from] DataSourceError),

    #[error("Failed to await stop signal: {0}")]
    Stop(#[from] tokio::sync::watch::error::RecvError),
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

#[derive(Error, Debug)]
pub enum ServerError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("(De-) Serialization returned an error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("Dispatch failed, error: {0}")]
    Dispatch(#[from] SourceHandlerError),

    #[error("Invalid message length {0}, likely forgot framing!")]
    MessageTooLarge(usize),
}
