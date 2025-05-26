use crate::FromConfig;
use crate::errors::ControllerError;
use crate::errors::TaskError;
use crate::sources::DataSourceT;
use async_trait::async_trait;
use typetag;
pub mod esp32_controller;
#[cfg(target_os = "linux")]
pub mod netlink_controller;
pub mod tcp_controller;

/// Trait that must be implemented by all source controller types
#[typetag::serde(tag = "type")]
#[async_trait]
pub trait Controller: Send + Sync {
    /// Apply controller parameters to the given source
    async fn apply(&self, source: &mut dyn DataSourceT) -> Result<(), ControllerError>;
}

/// Unified controller parameters
#[derive(serde::Serialize, serde::Deserialize, Debug, schemars::JsonSchema, Clone)]
#[serde(tag = "type", content = "params")]
pub enum ControllerParams {
    #[cfg(target_os = "linux")]
    Netlink(netlink_controller::NetlinkControllerParams),
    Esp32(esp32_controller::Esp32ControllerParams),
    Tcp(tcp_controller::TCPControllerParams),
    // Extendable
}

#[async_trait::async_trait]
impl FromConfig<ControllerParams> for dyn Controller {
    async fn from_config(config: ControllerParams) -> Result<Box<Self>, TaskError> {
        let controller: Box<dyn Controller> = match config {
            #[cfg(target_os = "linux")]
            ControllerParams::Netlink(params) => Box::new(params),
            ControllerParams::Esp32(params) => Box::new(params),
            ControllerParams::Tcp(params) => Box::new(params),
            // Add more cases here as new controllers are added
        };
        Ok(controller)
    }
}
