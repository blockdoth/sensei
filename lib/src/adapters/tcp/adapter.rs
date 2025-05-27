use crate::adapters::CsiDataAdapter;
pub use crate::adapters::csv::adapter::CSVAdapter;
pub use crate::adapters::esp32::adapter::ESP32Adapter;
pub use crate::adapters::iwl::adapter::IwlAdapter;
use crate::csi_types::{Complex, CsiData};
use crate::errors::CsiAdapterError;
use crate::network::rpc_message::{DataMsg, SourceType};

/// Adapter that delegates CSI parsing to appropriate source-specific adapters
/// based on the runtime `SourceType` in each `DataMsg`.
///
/// This adapter supports scaling of CSI data if `scale_csi` is enabled.
pub struct TCPAdapter {
    /// Whether to apply scaling to CSI data based on RSSI, noise, and AGC.
    scale_csi: bool,
}

impl TCPAdapter {
    /// Creates a new TCP adapter
    ///
    /// # Arguments
    ///
    /// * `scale_csi` - Whether to apply signal scaling after parsing CSI.
    /// # Returns
    ///
    /// A new instance of `TCPAdapter`.
    pub fn new(scale_csi: bool) -> Self {
        Self { scale_csi }
    }
}

#[async_trait::async_trait]
impl CsiDataAdapter for TCPAdapter {
    /// Parses raw CSI data buffer and produces structured `CsiData`.
    ///
    /// This function:
    /// - Checks the source type of the message and passes to the right adapter
    ///
    /// # Arguments
    ///
    /// * `msg` - DataMsg containing frames either raw or cooked
    ///
    /// # Returns
    ///
    /// * `Ok(Some(DataMsg))` if parsing is successful.
    /// * `Err(CsiAdapterError) or Some(None)` if parsing fails.
    async fn produce(&mut self, msg: DataMsg) -> Result<Option<DataMsg>, CsiAdapterError> {
        match msg {
            ref f @ DataMsg::RawFrame { ref source_type, .. } => match source_type {
                SourceType::IWL5300 => IwlAdapter::new(self.scale_csi).produce(f.clone()).await,
                SourceType::ESP32 => ESP32Adapter::new(self.scale_csi).produce(f.clone()).await,
                SourceType::CSV => CSVAdapter::default().produce(f.clone()).await,
                _ => Err(CsiAdapterError::InvalidInput),
            },
            // Already parsed just forward
            DataMsg::CsiFrame { csi } => Ok(Some(DataMsg::CsiFrame { csi })),
        }
    }
}
