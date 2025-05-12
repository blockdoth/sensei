use crate::adapters::CsiDataAdapter;
use crate::csi_types::{Complex, CsiData};
use crate::errors::CsiAdapterError;
use crate::network::rpc_message::{DataMsg, SourceType};

pub struct EmptyAdapter {
    frame: Option<DataMsg>,
}

impl EmptyAdapter {
    pub fn new() -> Self {
        Self { frame: None }
    }
}

impl Default for EmptyAdapter {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl CsiDataAdapter for EmptyAdapter {
    // DO NOT USE THIS method as there is no way to know sourceType
    async fn consume(&mut self, buf: &[u8]) -> Result<(), CsiAdapterError> {
        match &mut self.frame {
            None => {
                let mut v = Vec::new();
                v.extend_from_slice(buf);
                self.frame = Some(DataMsg::RawFrame {
                    ts: chrono::Utc::now().timestamp_millis() as f64,
                    bytes: v,
                    source_type: SourceType::Unknown,
                });
                Ok(())
            }
            Some(DataMsg::RawFrame { bytes, .. }) => {
                bytes.extend_from_slice(buf);
                Ok(())
            }
            Some(_) => Err(CsiAdapterError::InvalidInput),
        }
    }

    // Returns None as there is no CsiData parsed yet, DO NOT USE
    async fn reap(&mut self) -> Result<Option<CsiData>, CsiAdapterError> {
        Ok(None)
    }

    // Consumes raw frames
    async fn consume_raw(&mut self, raw_frame: DataMsg) -> Result<(), CsiAdapterError> {
        match raw_frame {
            DataMsg::RawFrame { .. } => {
                self.frame = Some(raw_frame);
                Ok(())
            }
            _ => Err(CsiAdapterError::InvalidInput),
        }
    }

    // Returns that same raw frame
    async fn reap_cooked(&mut self) -> Result<Option<DataMsg>, CsiAdapterError> {
        let result = self.frame.take();
        Ok(result)
    }
}
