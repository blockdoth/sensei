use async_trait::async_trait;
use tokio::fs::File;
use tokio::io::AsyncWriteExt;

use crate::ToConfig;
use crate::errors::{SinkError, TaskError};
use crate::network::rpc_message::DataMsg;
use crate::sinks::{Sink, SinkConfig};

/// Configuration for a YAML-based file sink.
///
/// This defines the output path.
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, PartialEq)]
pub struct FileConfig {
    /// Path to the output file.
    pub file: String,
}

/// A sink that writes each `DataMsg` as a YAML document to a file.
///
/// Messages are serialized using `serde_yaml` and separated by `---` for readability.
pub struct FileSink {
    config: FileConfig,
    file: File,
}

impl FileSink {
    /// Creates a new `FileSink` with the specifiedpath.
    ///
    /// # Errors
    ///
    /// Returns a `SinkError::Io` if the file cannot be created.
    pub async fn new(cg: FileConfig) -> Result<Self, SinkError> {
        log::trace!("Creating YAML file sink (file: {})", cg.file);
        let file = File::create(cg.clone().file).await.map_err(SinkError::Io)?;
        Ok(FileSink { config: cg.clone(), file })
    }
}

#[async_trait]
impl Sink for FileSink {
    // already opened
    /// Open the connection to the file sink, this method is just for the trait
    ///
    /// # Errors
    ///
    /// Returns a ['SinkError'] if the operation fails (e.g., I/O failure)
    async fn open(&mut self) -> Result<(), SinkError> {
        Ok(())
    }

    // in rust file is closed whenever it goes out scope

    /// Closes the connection to the file sink, this method is just for the trait
    ///
    /// # Errors
    ///
    /// Returns a ['SinkError'] if the operation fails (e.g., I/O failure)
    async fn close(&mut self) -> Result<(), SinkError> {
        Ok(())
    }

    /// Serializes the message to YAML and writes it to the file, followed by a document separator.
    ///
    /// # Errors
    ///
    /// - Returns `SinkError::Serialize` if YAML serialization fails.
    /// - Returns `SinkError::Io` if writing to the file fails.
    async fn provide(&mut self, data: DataMsg) -> Result<(), SinkError> {
        let serialized = serde_yaml::to_string(&data).map_err(|e| SinkError::Serialize(e.to_string()))?;
        self.file.write_all(serialized.as_bytes()).await.map_err(SinkError::Io)?;
        self.file.write_all(b"\n---\n").await.map_err(SinkError::Io)?;
        Ok(())
    }
}

#[async_trait::async_trait]
impl ToConfig<SinkConfig> for FileSink {
    /// Converts the current `FileSink` instance into its corresponding configuration representation.
    ///
    /// This method implements the `ToConfig` trait for `FileSink`, allowing the runtime instance
    /// to be converted back into a `SinkConfig::File` variant. This is useful for persisting
    /// the current state or for exporting configuration to a file (e.g., YAML or JSON).
    ///
    /// # Returns
    /// - `Ok(SinkConfig::File)` containing the internal configuration of the `FileSink`.
    /// - `Err(TaskError)` if any failure occurs during the conversion (though this implementation
    ///   does not currently produce an error).
    async fn to_config(&self) -> Result<SinkConfig, TaskError> {
        Ok(SinkConfig::File(self.config.clone()))
    }
}

#[cfg(test)]
mod tests {
    use tempfile::NamedTempFile;
    use tokio::fs;
    use tokio::io::AsyncReadExt;

    use super::*;
    use crate::network::rpc_message::SourceType;

    #[tokio::test]
    async fn test_new_and_to_config() {
        let tmp = NamedTempFile::new().unwrap();
        let path = tmp.path().to_string_lossy().to_string();

        let config = FileConfig { file: path.clone() };
        let sink = FileSink::new(config.clone()).await.unwrap();

        let back_to_config = sink.to_config().await.unwrap();
        assert_eq!(back_to_config, SinkConfig::File(config));
    }

    #[tokio::test]
    async fn test_creates_file() {
        // Define a temporary file path in the temp directory
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("test.yaml");
        let file_path_str = file_path.to_string_lossy().to_string();

        // Ensure file doesn't exist before
        assert!(!file_path.exists());
        // Create the sink
        let config = FileConfig { file: file_path_str.clone() };
        let _sink = FileSink::new(config).await.unwrap();
        // File should now exist
        assert!(file_path.exists());
    }

    #[tokio::test]
    async fn test_provide() {
        let tmp = NamedTempFile::new().unwrap();
        let path = tmp.path().to_string_lossy().to_string();

        let config = FileConfig { file: path.clone() };
        let mut sink = FileSink::new(config).await.unwrap();

        let data = DataMsg::RawFrame {
            ts: 0.0,
            bytes: vec![0u8; 5],
            source_type: SourceType::IWL5300,
        };

        sink.provide(data.clone()).await.unwrap();

        let mut file = fs::File::open(path).await.unwrap();
        let mut contents = String::new();
        file.read_to_string(&mut contents).await.unwrap();

        assert!(contents.contains("---")); // YAML doc separator
        let deserialized: DataMsg = serde_yaml::from_str(contents.split("---").next().unwrap()).unwrap();
        assert_eq!(deserialized, data);
    }

    #[tokio::test]
    async fn test_open_and_close() {
        let tmp = NamedTempFile::new().unwrap();
        let path = tmp.path().to_string_lossy().to_string();

        let config = FileConfig { file: path };
        let mut sink = FileSink::new(config).await.unwrap();

        // Should be no-op
        sink.open().await.unwrap();
        sink.close().await.unwrap();
    }

    #[tokio::test]
    async fn test_file_sink_creation_error() {
        let config = FileConfig {
            file: "/invalid/path/file.yaml".to_string(),
        };
        let result = FileSink::new(config).await;
        assert!(result.is_err());
    }
}
