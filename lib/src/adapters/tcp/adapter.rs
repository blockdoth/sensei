use crate::ToConfig;
pub use crate::adapters::csv::adapter::CSVAdapter;
pub use crate::adapters::esp32::adapter::ESP32Adapter;
pub use crate::adapters::iwl::adapter::IwlAdapter;
use crate::adapters::{CsiDataAdapter, DataAdapterConfig};
use crate::errors::{CsiAdapterError, TaskError};
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

#[async_trait::async_trait]
impl ToConfig<DataAdapterConfig> for TCPAdapter {
    /// Converts the current `TCPAdapter` instance into its configuration representation.
    ///
    /// This method implements the `ToConfig` trait for `TCPAdapter`, enabling the instance
    /// to be converted into a `DataAdapterConfig::Tcp` variant. This is particularly useful
    /// for persisting the adapter's configuration or exporting it as part of a system-wide
    /// configuration file (e.g., JSON or YAML).
    ///
    /// # Returns
    /// - `Ok(DataAdapterConfig::Tcp)` containing the current value of `scale_csi`.
    /// - `Err(TaskError)` if an error occurs during the conversion (not applicable in this implementation).
    async fn to_config(&self) -> Result<DataAdapterConfig, TaskError> {
        Ok(DataAdapterConfig::Tcp { scale_csi: self.scale_csi })
    }
}
