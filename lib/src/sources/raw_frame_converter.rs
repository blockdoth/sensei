use crate::errors::FileSourceError;
use async_stream::stream;
use tokio_stream::{Stream, StreamExt};

use crate::adapters::CsiDataAdapter;
use crate::errors::AdapterStreamError;
use crate::errors::CsiAdapterError;
use crate::rpc_envelope::DataMsg;

/// A wrapper that consumes a stream of RawFrames and emits CsiFrames using a CsiDataAdapter.
pub struct AdapterStream<A: CsiDataAdapter> {
    adapter: A,
}

impl<A: CsiDataAdapter + 'static> AdapterStream<A> {
    pub fn new(adapter: A) -> Self {
        Self { adapter }
    }

    pub fn stream_from_raws<S>(
        mut self,
        mut input_stream: S,
    ) -> impl Stream<Item = Result<DataMsg, AdapterStreamError>> + Send + 'static
    where
        S: Stream<Item = DataMsg> + Unpin + Send + 'static,
    {
        stream! {
            while let Some(msg) = input_stream.next().await {
                match msg {
                    f@DataMsg::RawFrame { .. } => {
                        self.adapter.consume_raw(f).await?;

                        while let Some(cooked) = self.adapter.reap_cooked().await? {
                            yield Ok(cooked);
                        }
                    },
                    _ => {
                        yield Err(AdapterStreamError::InvalidInput);
                    }
                }
            }
        }
    }
}
