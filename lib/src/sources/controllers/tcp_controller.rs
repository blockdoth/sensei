use crate::errors::ControllerError;
use crate::sources::DataSourceT;
use crate::sources::controllers::Controller;
use std::net::SocketAddr;

/// Parameters for the TCP controller.
///
/// This struct currently carries no parameters, as TCP sources do not require
/// control configuration. It is included for interface completeness and potential
/// future extension.
#[derive(serde::Deserialize, serde::Serialize, Debug, Clone, schemars::JsonSchema, Default)]
#[serde(default)]
pub struct TCPControllerParams {}

/// Implements the [`Controller`] trait for TCP sources.
///
/// This is a no-op controller implementation intended for use with TCP-based
/// [`DataSourceT`] instances. It satisfies the trait requirements but does not
/// alter the source configuration.
///
/// # Returns
/// Always returns `Ok(())` with no side effects.
#[typetag::serde(name = "TCP")]
#[async_trait::async_trait]
impl Controller for TCPControllerParams {
    async fn apply(&self, source: &mut dyn DataSourceT) -> Result<(), ControllerError> {
        Ok(())
    }
}
