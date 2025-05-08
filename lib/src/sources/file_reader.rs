use std::path::PathBuf;
use tokio::fs::File;
use tokio::io::{AsyncReadExt, BufReader};
use tokio_stream::{Stream, StreamExt};
use async_stream::stream;
use thiserror::Error;

use crate::{DataMsg, CsiDataAdapter, CsiAdapterError, CsiData};

#[derive(Error, Debug)]
pub enum FileSourceError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("CSI adapter error: {0}")]
    Adapter(#[from] CsiAdapterError),
}

/// A reader for continuously updated files that streams CSI data using an adapter.
pub struct FileReader<A: CsiDataAdapter> {
    path: PathBuf,
    adapter: A,
}

impl<A: CsiDataAdapter> FileReader<A> {
    pub fn new(path: PathBuf, adapter: A) -> Self {
        Self { path, adapter }
    }

    /// Stream CSI frames from the file continuously.
    pub fn stream(
        mut self,
    ) -> impl Stream<Item = Result<DataMsg, FileSourceError>> + Unpin + Send + 'static {
        stream! {
            let file = File::open(&self.path).await?;
            let mut reader = BufReader::new(file);
            let mut buffer = vec![0u8; 4096];

            loop {
                let n = reader.read(&mut buffer).await?;

                if n == 0 {
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    continue;
                }

                self.adapter.consume(&buffer[..n]).await?;

                while let Some(cooked) = self.adapter.reap_cooked().await? {
                    yield Ok(cooked);
                }
            }
        }
    }
}