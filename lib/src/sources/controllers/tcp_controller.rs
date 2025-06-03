use crate::ToConfig;
use crate::errors::{ControllerError, TaskError};
use crate::sources::DataSourceT;
use crate::sources::controllers::{Controller, ControllerParams};

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
    async fn apply(&self, _source: &mut dyn DataSourceT) -> Result<(), ControllerError> {
        Ok(())
    }
}

#[async_trait::async_trait]
impl ToConfig<ControllerParams> for TCPControllerParams {
    /// Converts the current `TCPControllerParams` instance into its configuration representation.
    ///
    /// This method implements the `ToConfig` trait for `TCPControllerParams`, enabling the runtime
    /// parameters of a TCP controller to be converted into a `ControllerParams::Tcp` variant.
    /// This is particularly useful for serializing controller state to configuration files such as
    /// YAML or JSON, or for logging and diagnostics purposes.
    ///
    /// # Returns
    /// - `Ok(ControllerParams::Tcp)` containing a reference to the current `TCPControllerParams`.
    /// - `Err(TaskError)` if an error occurs during conversion (not applicable in this implementation).
    async fn to_config(&self) -> Result<ControllerParams, TaskError> {
        Ok(ControllerParams::Tcp(self.clone()))
    }
}
