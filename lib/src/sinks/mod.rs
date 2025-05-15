use crate::errors::SinkError;
use crate::errors::TaskError;
use crate::network::DataMsg;
use async_trait::async_trait;
use crate::FromConfig;

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

#[async_trait::async_trait]
impl FromConfig<SinkConfig> for dyn Sink {
    type Error = SinkError;

    async fn from_config(config: SinkConfig) -> Result<Box<Self>, TaskError> {
        match config {
            SinkConfig::File(cfg) => {
                let sink = file::FileSink::new(cfg).await?;
                Ok(Box::new(sink))
            }
        }
    }
}

