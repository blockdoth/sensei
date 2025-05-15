use crate::errors::SinkError;
use crate::network::DataMsg;
use async_trait::async_trait;

pub mod file;

/// Sink trait for handling data messages.
#[async_trait]
pub trait Sink: Send {
    /// Consume a DataMsg and process it (e.g., write to file, send over network).
    async fn provide(&mut self, data: DataMsg) -> Result<(), SinkError>;
}

/// Possible sink configurations.
#[derive(serde::Deserialize, Debug, Clone)]
#[serde(tag = "type")]
pub enum SinkConfig {
    File(file::FileConfig),
    // add other sink types here
}

/// Factory to create a boxed SinkHandler from a config.
pub async fn create_sink(config: SinkConfig) -> Result<Box<dyn SinkHandler>, SinkError> {
    match config {
        SinkConfig::File(cfg) => {
            let sink = file::FileSink::new(cfg).await?;
            Ok(Box::new(sink))
        }
    }
}

