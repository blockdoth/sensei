use async_stream::stream;
use std::path::PathBuf;
use tokio::fs::File;
use tokio::io::{AsyncReadExt, BufReader};
use tokio_stream::{Stream, StreamExt};

use crate::adapters::CsiDataAdapter;
use crate::csi_types::CsiData;
use crate::errors::CsiAdapterError;
use crate::errors::FileSourceError;
use crate::rpc_envelope::DataMsg;
use crate::rpc_envelope::SourceType;

/// A reader for continuously updated files that streams CSI data using an adapter.
pub struct FileReader<A: CsiDataAdapter> {
    path: PathBuf,
    adapter: A,
    sourcetype: SourceType,
}

impl<A: CsiDataAdapter + 'static> FileReader<A> {
    pub fn new(path: PathBuf, adapter: A, sourcetype: SourceType) -> Self {
        Self {
            path,
            adapter,
            sourcetype,
        }
    }

    /// Stream CSI frames from the file continuously.
    pub fn stream(
        mut self,
    ) -> impl Stream<Item = Result<DataMsg, FileSourceError>> + Send + 'static {
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

                let mut v = Vec::new();
                v.extend_from_slice(&buffer[..n]);
                self.adapter.consume_raw(DataMsg::RawFrame {
                    ts: chrono::Utc::now().timestamp_millis() as u128,
                    bytes: v,
                    source_type: self.sourcetype.clone(),
                }).await?;
                //self.adapter.consume(&buffer[..n]).await?;

                while let Some(cooked) = self.adapter.reap_cooked().await? {
                    yield Ok(cooked);
                }
            }
        }
    }
}
