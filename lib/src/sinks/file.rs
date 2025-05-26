use crate::errors::SinkError;
use crate::network::rpc_message::DataMsg;
use crate::sinks::Sink;
use async_trait::async_trait;
use tokio::fs::File;
use tokio::io::AsyncWriteExt;

/// Configuration for a YAML-based file sink.
///
/// This defines the output path.
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
pub struct FileConfig {
    /// Path to the output file.
    pub file: String,
}

/// A sink that writes each `DataMsg` as a YAML document to a file.
///
/// Messages are serialized using `serde_yaml` and separated by `---` for readability.
pub struct FileSink {
    file: File,
}

impl FileSink {
    /// Creates a new `FileSink` with the specifiedpath.
    ///
    /// # Errors
    ///
    /// Returns a `SinkError::Io` if the file cannot be created.
    pub async fn new(config: FileConfig) -> Result<Self, SinkError> {
        log::trace!("Creating YAML file sink (file: {})", config.file);
        let file = File::create(config.file).await.map_err(SinkError::Io)?;
        Ok(FileSink { file })
    }
}

#[async_trait]
impl Sink for FileSink {
    // already opened
    async fn open(&mut self, data: DataMsg) -> Result<(), SinkError> {
        Ok(())
    }
    // in rust file is closed whenever it goes out scope
    async fn close(&mut self, data: DataMsg) -> Result<(), SinkError> {
        Ok(())
    }
    /// Serializes the message to YAML and writes it to the file, followed by a document separator.
    ///
    /// # Errors
    ///
    /// - Returns `SinkError::Serialize` if YAML serialization fails.
    /// - Returns `SinkError::Io` if writing to the file fails.
    async fn provide(&mut self, data: DataMsg) -> Result<(), SinkError> {
        let serialized =
            serde_yaml::to_string(&data).map_err(|e| SinkError::Serialize(e.to_string()))?;
        self.file
            .write_all(serialized.as_bytes())
            .await
            .map_err(SinkError::Io)?;
        self.file
            .write_all(b"\n---\n")
            .await
            .map_err(SinkError::Io)?;
        Ok(())
    }
}
