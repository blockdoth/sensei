use crate::errors::{SenseiError, SinkError};
use crate::sinks::SubscriberData;

pub use tokio::fs::File;
use tokio::io::AsyncWriteExt;

#[derive(serde::Deserialize, Debug, Clone)]
pub struct FileConfig {
    file: String,
}

/// File sink creation.
pub async fn create(config: FileConfig) -> Result<File, SinkError> {
    log::trace!("Creating file sink (file: {})", config.file);
    File::create(config.file).await.map_err(SinkError::Io)
}

pub async fn file_write<'a>(file: &mut File, data: SubscriberData<'a>) -> Result<(), SinkError> {
    match data {
        SubscriberData::Raw(buf) => {
            let len = buf.len() as u16;
            file.write_all(&len.to_be_bytes()).await?;
            file.write_all(buf).await?;
        }
        SubscriberData::Csi(_) => {
            return Err(SenseiError::NotImplemented(
                "No serialization implemented for CSI yet!".into(),
            ))?;
        }
        SubscriberData::Probe => {}
    }

    Ok(())
}
