use std::num::{ParseFloatError, ParseIntError};

use thiserror::Error;

use crate::network::rpc_message::{DataMsg, HostId};
use crate::network::tcp::ChannelMsg;

/// Errors that can occur during network communication with sources or clients.
#[derive(Error, Debug)]
pub enum NetworkError {
    /// I/O-related errors
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// Tokio was unable to send the message
    #[error("Message could not be sent due to a broadcast error")]
    TokioBroadcastSendingError(#[from] tokio::sync::broadcast::error::SendError<(DataMsg, HostId)>),

    /// Tokio was unable to send the message
    #[error("Message could not be sent due to a Watch error")]
    TokioWatchSendingError(#[from] tokio::sync::watch::error::SendError<ChannelMsg>),

    /// Tokio was unable to receive a message
    #[error("Message could not be received due to a Watch error")]
    TokioWatchRecvError(#[from] tokio::sync::watch::error::RecvError),

    /// Communication operation timed out.
    #[error("Communication timed out")]
    Timeout(#[from] tokio::time::error::Elapsed),

    /// Processing Error
    #[error("Processing Error")]
    ProcessingError(#[from] Box<dyn std::error::Error + Send>),

    /// Failed during serialization or deserialization.
    #[error("Error during (De)Serialization")]
    Serialization,

    /// Could not establish a connection to the target.
    #[error("Unable to connect to target")]
    UnableToConnect,

    /// Source connection was closed by the remote end.
    #[error("Source closed (from source side)")]
    Closed,

    /// Attempted to connect a client that is already connected.
    #[error("This client is already connected")]
    AlreadyConnected,

    /// The response could not be parsed.
    #[error("Message could not be parsed")]
    MessageError,

    /// Other error type
    #[error("An error occurred")]
    Other,

    /// Thrown when a connection that does not exist is referenced
    #[error("That connection does not exist")]
    NoSuchConnection,
}

/// Generic application-level error for unimplemented functionality.
#[derive(Error, Debug)]
pub enum SenseiError {
    /// Indicates a function or feature is not yet implemented.
    #[error("Not implemented: {0}")]
    NotImplemented(String),
}

/// Errors related to handling or processing data sources.
#[derive(Error, Debug)]
pub enum DataSourceError {
    /// A general-purpose error with custom message.
    #[error("Generic error: {0}")]
    GenericError(String),

    /// I/O-related error (e.g., device read/write failure).
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// Serial port-specific error.
    #[error("Serial port error: {0}")]
    Serial(#[from] serialport::Error),

    /// Failed to parse a received data packet.
    #[error("Couldnt parse packet: {0}")]
    ParsingError(String),

    #[error("Tcp source error: {0}")]
    NetworkError(#[from] Box<NetworkError>),

    /// Packet was incomplete, likely due to a bug in the source handler.
    #[error("Incomplete packet (Source handler bug)")]
    IncompletePacket,

    /// Conversion from byte slice to number array failed.
    #[error("Array conversion failed: {0}")]
    ArrayToNumber(#[from] std::array::TryFromSliceError),

    /// Error during controller application to the data source.
    #[error("Controller failed: {0}")]
    Controller(String),

    /// Tried to use a feature or function that isn't implemented.
    #[error("Tried to use unimplemented feature: {0}")]
    NotImplemented(String),

    #[error("Not connected: {0}")]
    NotConnected(String),

    /// Insufficient privileges to access the source.
    #[error("Permission denied: application lacks sufficient privileges. See `README.md` for details on permissions.")]
    PermissionDenied,

    /// Attempted to read from the source before it was started.
    #[error("Read before starting (must call `start` before)")]
    ReadBeforeStart,

    #[error("Do not use method read_buf")]
    ReadBuf,
}

/// Common error enum for all CSI adapters (IWL, ESP32, Csv).
#[derive(Error, Debug)]
pub enum CsiAdapterError {
    /// Error from IWL adapter.
    #[error("IWL Adapter Error: {0}")]
    IWL(#[from] IwlAdapterError),

    /// Error from ESP32 adapter.
    #[error("ESP32 Adapter Error: {0}")]
    ESP32(#[from] Esp32AdapterError),

    /// Input to adapter was invalid or unexpected.
    #[error("Invalid input, give a raw frame")]
    InvalidInput,

    /// Error from Csv adapter.
    #[error("Csv Adapter Error: {0}")]
    Csv(#[from] CsvAdapterError),

    /// Error whilst parsing to int
    #[error("Could not convert to int: {0}")]
    IntConversionError(#[from] ParseIntError),

    /// Error whilst parsing to float
    #[error("Could not convert '{input}' to float: {err}")]
    FloatConversionError {
        #[source]
        err: ParseFloatError,
        input: String,
    },
}

/// Specific errors of the Csv Adapter
#[derive(Error, Debug)]
pub enum CsvAdapterError {
    #[error("Invalid number of columns in Csv row: {0}")]
    InvalidData(String),
}

/// Errors specific to the ESP32 CSI adapter.
#[derive(Error, Debug)]
pub enum Esp32AdapterError {
    /// CSI payload is too short to be valid.
    #[error("Payload too short: expected at least {expected} bytes, got {actual}")]
    PayloadTooShort { expected: usize, actual: usize },

    /// Failed to parse ESP32 CSI data.
    #[error("ESP32 CSI data parsing error: {0}")]
    ParseError(String),
}

/// Errors specific to the IWL CSI adapter.
#[derive(Error, Debug)]
pub enum IwlAdapterError {
    /// CSI packet header was incomplete.
    #[error("Insufficient bytes to reconstruct header")]
    IncompleteHeader,

    /// CSI packet body was incomplete.
    #[error("Incomplete packet (missing payload bytes)")]
    IncompletePacket,

    /// Unexpected or invalid packet code.
    #[error("Invalid code: {0}")]
    InvalidCode(u8),

    /// Matrix size in CSI payload is incorrect or invalid.
    #[error("Invalid beamforming matrix size: {0}")]
    InvalidMatrixSize(usize),

    /// Invalid configuration of RX antennas or streams.
    #[error("Invalid antenna specification; Receive antennas: {num_rx}, streams: {num_streams}")]
    InvalidAntennaSpec { num_rx: usize, num_streams: usize },

    /// CSI sequence number was invalid or unexpected.
    #[error("Invalid sequence number: {0}")]
    InvalidSequenceNumber(u16),
}

/// Errors specific to the Atheros CSI adapter.
#[derive(Error, Debug)]
pub enum AtherosAdapterError {
    /// CSI packet header was incomplete.
    #[error("Insufficient bytes to reconstruct header")]
    IncompleteHeader,

    /// CSI packet body was incomplete.
    #[error("Incomplete packet (missing payload bytes)")]
    IncompletePacket,

    /// Unexpected or invalid packet code.
    #[error("Invalid code: {0}")]
    InvalidCode(u8),

    /// Matrix size in CSI payload is incorrect or invalid.
    #[error("Invalid beamforming matrix size: {0}")]
    InvalidMatrixSize(usize),

    /// Invalid configuration of RX antennas or streams.
    #[error("Invalid antenna specification; Receive antennas: {num_rx}, streams: {num_streams}")]
    InvalidAntennaSpec { num_rx: usize, num_streams: usize },

    /// CSI sequence number was invalid or unexpected.
    #[error("Invalid sequence number: {0}")]
    InvalidSequenceNumber(u16),
}

/// Errors for sources using file-backed input.
#[derive(Error, Debug)]
pub enum FileSourceError {
    /// Underlying I/O error occurred.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// CSI adapter failed while reading the file.
    #[error("CSI adapter error: {0}")]
    Adapter(#[from] CsiAdapterError),
}

/// Errors from a stream-based CSI adapter source.
#[derive(Error, Debug)]
pub enum AdapterStreamError {
    /// CSI adapter reported a failure.
    #[error("CSI adapter error: {0}")]
    Adapter(#[from] CsiAdapterError),

    /// Adapter expected a RawFrame but received something else.
    #[error("Expected RawFrame but received non-RawFrame DataMsg variant")]
    InvalidInput,
}

/// Generic error used in low-level raw source tasks.
#[derive(Error, Debug)]
pub enum RawSourceTaskError {
    /// Unspecified or general-purpose task failure.
    #[error("Generic RawSourceTask Error")]
    GenericError,
}

/// Errors that may occur while applying a controller to a data source.
#[derive(Error, Debug)]
pub enum ControllerError {
    /// I/O operation failed (e.g., writing config).
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// Required configuration file was not found.
    #[error("Expected file {0} to exist, but it didnt.")]
    FileNotPresent(String),

    /// External script execution failed.
    #[error("Script failed with error: {0}")]
    ScriptError(String),

    /// Data source failed during reconfiguration.
    #[error("Encountered error at data source during reconfiguration: {0}")]
    DataSource(#[from] DataSourceError),

    /// Controller parameters were invalid or missing.
    #[error("Given invalid parameters: {0}")]
    InvalidParams(String),

    /// Failed to (de)serialize controller parameters.
    #[error("(De-) Serialization returned an error: {0}")]
    Serialization(#[from] serde_json::Error),

    /// A required parameter was missing from the configuration.
    #[error("Missing parameter: {0}")]
    MissingParameter(String),

    #[error("Command failed to execute")]
    CommandFailed { command_name: String, details: String },

    #[error("Invalid datasource")]
    InvalidDataSource(String),
    /// Could not determine the wireless PHY name.
    #[error("Failed to extract PhyName due to string conversions")]
    PhyName,

    /// Generic error when executing controller logic.
    #[error("Controller execution error: {0}")]
    Execution(String),
}

/// Errors that can occur in data sinks.
#[derive(Error, Debug)]
pub enum SinkError {
    /// Underlying I/O error (e.g., writing to file).
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// Generic error from sink operations.
    #[error("Error: {0}")]
    Generic(#[from] SenseiError),

    /// Sink's channel was closed unexpectedly.
    #[error("Channel closed; Sink disconnected.")]
    Disconnected,

    /// Data serialization to output format failed.
    #[error("Error: {0}")]
    Serialize(String),

    /// Error related to tcp sinks
    #[error("Error from tcp sink: {0}")]
    NetworkError(#[from] Box<NetworkError>),
}

/// Top-level task errors used across Sensei's runtime.
#[derive(Error, Debug)]
pub enum TaskError {
    /// Underlying I/O error (e.g., writing to file).
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    // Error when you trying to serialie
    #[error("Serialization error: {0}")]
    Serde(#[from] serde_yaml::Error),

    /// Not implemented error
    #[error("Not Implemented")]
    NotImplemented,

    /// Failure occurred in sink-related code.
    #[error("Sink Error: {0}")]
    SinkError(#[from] SinkError),

    /// Error occurred in a data source.
    #[error("Data Source Error: {0}")]
    DataSourceError(#[from] DataSourceError),

    /// Error occurred in a controller.
    #[error("Controller Error: {0}")]
    ControllerError(#[from] ControllerError),

    /// Error when you try to stop task and join the threads
    #[error("Error when trying to stop task")]
    JoinError(String),

    /// This happens when the controller type doesn't match the source type
    #[error("Controller doesn't correspond to the source")]
    IncorrectController,

    /// This happens when the adapter type doesn't match the adapter type
    #[error("Adapter doesn't correspond to the  source")]
    IncorrectAdapter,

    /// This happens when the device id of the sink doesn't match the one from task
    /// Specifically for the tcp sink
    #[error("Incorrect device_id for sink, according to config")]
    WrongSinkDid,
}

#[derive(Error, Debug)]
pub enum ConfigError {
    /// Underlying I/O error (e.g., writing to file).
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    // Error when you trying to serialize
    #[error("Serialization error: {0}")]
    Serde(#[from] serde_yaml::Error),

    /// The configuration is invalid.
    #[error("The config is invalid")]
    InvalidConfig(String),
}

#[derive(Error, Debug)]
pub enum ExperimentError {
    /// Could not execute experiment
    #[error("Execution Error")]
    ExecutionError,

    /// TaskError
    #[error("An error executing the tasks")]
    TaskError(#[from] TaskError),
}

// Allow conversion from Box<NetworkError> to NetworkError
impl From<Box<NetworkError>> for NetworkError {
    fn from(err: Box<NetworkError>) -> Self {
        *err
    }
}
