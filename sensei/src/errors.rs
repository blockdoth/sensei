//! This module contains all the errors thrown by the Sensei application

use lib::errors::{ExperimentError, NetworkError, TaskError};
use lib::network::rpc_message::{DataMsg, HostId};
use lib::network::tcp::ChannelMsg;
use thiserror::Error;

use crate::orchestrator::state::OrgUpdate;
use crate::orchestrator::{ExperimentChannelMsg, RegistryChannelMsg};

#[derive(Error, Debug)]
pub enum OrchestratorError {
    /// Network Error
    #[error("Network Error")]
    NetworkError(#[from] NetworkError),

    /// Tokio mpsc send error for OrgUpdate
    #[error("Tokio mpsc send error (OrgUpdate): {0}")]
    MpscOrgUpdate(#[from] tokio::sync::mpsc::error::SendError<OrgUpdate>),

    /// Tokio mpsc send error for bool
    #[error("Tokio mpsc send error (bool): {0}")]
    MpscBool(#[from] tokio::sync::mpsc::error::SendError<bool>),

    /// Tokio mpsc send error for ExperimentChannelMsg
    #[error("Tokio mpsc send error (ExperimentChannelMsg): {0}")]
    MpscExperimentChannelMsg(#[from] tokio::sync::mpsc::error::SendError<ExperimentChannelMsg>),

    /// Tokio mpsc send error for RegistryChannelMsg
    #[error("Tokio mpsc send error (RegistryChannelMsg): {0}")]
    MpscRegistryChannelMsg(#[from] tokio::sync::mpsc::error::SendError<RegistryChannelMsg>),

    /// Tokio watch send error for OrgUpdate
    #[error("Tokio watch send error (OrgUpdate): {0}")]
    WatchOrgUpdate(#[from] tokio::sync::watch::error::SendError<OrgUpdate>),

    /// Tokio watch send error for bool
    #[error("Tokio watch send error (bool): {0}")]
    WatchBool(#[from] tokio::sync::watch::error::SendError<bool>),

    /// Tokio watch receive error
    #[error("Tokio watch receive error: {0}")]
    WatchRecv(#[from] tokio::sync::watch::error::RecvError),
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
pub enum RegistryError {
    /// No such host
    #[error("No such host")]
    NoSuchHost,

    #[error("Network Error")]
    NetworkError(#[from] NetworkError),
}
