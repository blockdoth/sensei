use crate::errors::SinkError;
use crate::network::rpc_message::DataMsg;
use crate::sinks::Sink;
use async_trait::async_trait;
use tokio::fs::File;
use tokio::io::AsyncWriteExt;

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
pub struct FileConfig {
    pub file: String,
}

pub struct FileSink {
    file: File,
}

impl FileSink {
    pub async fn new(config: FileConfig) -> Result<Self, SinkError> {
        log::trace!("Creating file sink (file: {})", config.file);
        let file = File::create(config.file).await.map_err(SinkError::Io)?;
        Ok(FileSink { file })
    }
}

#[async_trait]
impl Sink for FileSink {
    async fn provide(&mut self, data: DataMsg) -> Result<(), SinkError> {
        let serialized =
            serde_json::to_string(&data).map_err(|e| SinkError::Serialize(e.to_string()))?;
        self.file
            .write_all(serialized.as_bytes())
            .await
            .map_err(SinkError::Io)?;
        self.file.write_all(b"\n").await.map_err(SinkError::Io)?;
        Ok(())
    }
}
