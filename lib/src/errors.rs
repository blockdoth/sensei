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
}
