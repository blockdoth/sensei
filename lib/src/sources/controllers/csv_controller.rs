use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{errors::{ControllerError, TaskError}, sources::{controllers::{Controller, ControllerParams}, DataSourceT}, ToConfig};

/// Parameters for controlling an ESP32 device.
#[derive(Serialize, Deserialize, Debug, Clone, Default, JsonSchema, PartialEq)]
#[serde(default)]
pub struct CsvControllerParams {}

#[async_trait]
impl Controller for CsvControllerParams {
    /// Applies the configuration to a CSV source.
    async fn apply(&self, source: &mut dyn DataSourceT) -> Result<(), ControllerError> {
        Ok(())
    }
}

#[async_trait::async_trait]
impl ToConfig<ControllerParams> for CsvControllerParams {
    /// Converts the current `NetlinkControllerParams` instance into its configuration representation.
    ///
    /// This method implements the `ToConfig` trait for `CsvControllerParams`, allowing a live
    /// controller instance to be converted into a `ControllerParams::CSV` variant.
    /// This is useful for persisting the controller's configuration to a file (e.g., YAML or JSON)
    /// or for reproducing its state programmatically.
    ///
    /// # Returns
    /// - `Ok(ControllerParams::CSV)` containing a cloned version of the controller's parameters.
    /// - `Err(TaskError)` if an error occurs during conversion (not applicable in this implementation).
    async fn to_config(&self) -> Result<ControllerParams, TaskError> {
        Ok(ControllerParams::CSV(self.clone()))
    }
}
