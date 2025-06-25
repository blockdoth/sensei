use lib::errors::{ConfigError, ExperimentError, NetworkError, TaskError};
use thiserror::Error;

use lib::network::rpc_message::{DataMsg, HostId};
use lib::network::tcp::ChannelMsg;

///! This module contains all the errors thrown by the Sensei application

#[derive(Error, Debug)]
pub enum OrchestratorError<T> {
    /// Could not execute something.
    #[error("Execution Error")]
    ExecutionError,

    /// Network Error
    #[error("Network Error")]
    NetworkError(#[from] NetworkError),

    /// Send Error
    #[error("An error caused by Tokio")]
    GenericSendError,

    /// Mpsc error
    #[error("Error caused by Tokio mpsc")]
    MpscError(#[from] tokio::sync::mpsc::error::SendError<T>),
}


/// Errors occurring at the application/config level.
#[derive(Debug, Error)]
pub enum AppError {
    /// I/O error during application execution.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// Failed to parse a YAML configuration.
    #[error("Failed with config parsing: {0}")]
    YamlError(#[from] serde_yaml::Error),

    /// Configuration is invalid or incomplete.
    #[error("Configuration error: {0}")]
    ConfigError(String),

    /// No such host
    #[error("No such host exists")]
    NoSuchHost,

    /// Experiment error
    #[error("Experiment Error")]
    ExperimentError(#[from] ExperimentError),

    /// Tokio was unable to send the message
    #[error("Message could not be sent due to a broadcast error")]
    TokioBroadcastSendingError(#[from] tokio::sync::broadcast::error::SendError<(DataMsg, HostId)>),

    /// Tokio was unable to send the message
    #[error("Message could not be sent due to a Watch error")]
    TokioWatchSendingError(#[from] tokio::sync::watch::error::SendError<ChannelMsg>),

    /// TaskError
    #[error("An error executing the tasks")]
    TaskError(#[from] TaskError),
}

#[derive(Error, Debug)]
pub enum CommandError {
    /// No command associated with the base string.
    #[error("Could not find a command associated with the base string.")]
    NoSuchCommand,

    /// The command is missing an argument.
    #[error("The command is missing an argument")]
    MissingArgument,

    /// The command argument is invalid.
    #[error("The command argument is invalid.")]
    InvalidArgument,

    /// There was an error in parsing a config.
    #[error("There was an error in parsing a config.")]
    ConfigError(#[from] ConfigError),
}

#[derive(Error, Debug)]
pub enum RegistryError {
    /// A generic error only intended to be used in development
    #[error("A generic error. Should not be used in finalized features")]
    GenericError,

    /// No such host
    #[error("No such host")]
    NoSuchHost,

    // /// Network Error
    // #[error("Network Error")]
    // NetworkError(#[from] Box<NetworkError>),
    /// Netowrk Error
    #[error("Network Error")]
    NetworkError(#[from] NetworkError),

    /// No Standalone
    #[error("The registry cannot be ran as a standalone process.")]
    NoStandalone,
}
