//! Source Controllers
//! ------------------
//!
//! This module defines the [`Controller`] trait, which is use for configuring a source
//! Controllers can configure filtering, rate-limiting, injection, or any
//! other transformation at the source level.
//!
//! The [`ControllerParams`] enum and the [`FromConfig`] implementation enable
//! instantiation of controller implementations from serialized configuration.

use async_trait::async_trait;
use typetag;

use crate::FromConfig;
use crate::errors::{ControllerError, TaskError};
use crate::sources::DataSourceT;
pub mod esp32_controller;
#[cfg(target_os = "linux")]
pub mod netlink_controller;

/// Trait that must be implemented by all source controller types.
///
/// Controllers are invoked immediately after a data source is instantiated
/// (but before it is started) to apply platform- or device-specific
/// configuration.
///
/// Must be `Send + Sync` so they can be safely shared across
/// asynchronous contexts.
///
/// # Example
///
/// ```rust,ignore
/// use your_crate::{Controller, ControllerParams, FromConfig};
/// use your_crate::sources::DataSourceT;
///
/// // Deserialize params from config
/// let params: ControllerParams = toml::from_str(toml_str)?;
/// // Instantiate controller
/// let controller: Box<dyn Controller> = Controller::from_config(params).await?;
/// // Apply to source
/// controller.apply(&mut my_source).await?;
/// ```
#[typetag::serde(tag = "type")]
#[async_trait]
pub trait Controller: Send + Sync {
    /// Apply controller parameters or logic to the given source.
    ///
    /// This method allows the controller to source-level setup.
    ///
    /// # Errors
    ///
    /// Returns a [`ControllerError`] if configuration fails (e.g., invalid
    /// parameters, platform restrictions, I/O errors).
    async fn apply(&self, source: &mut dyn DataSourceT) -> Result<(), ControllerError>;
}

/// Unified configuration for all supported controller types.
///
/// Each variant carries the specific parameters needed to construct that
/// controller implementation. Tagged via Serde as `{ "type": "...", "params": { ... } }`.
#[derive(serde::Serialize, serde::Deserialize, Debug, schemars::JsonSchema, Clone)]
#[serde(tag = "type", content = "params")]
pub enum ControllerParams {
    #[cfg(target_os = "linux")]
    Netlink(netlink_controller::NetlinkControllerParams),
    Esp32(esp32_controller::Esp32Controller),
    // Extendable
}

/// Instantiates a concrete [`Controller`] from its [`ControllerParams`]
/// via the [`FromConfig`] trait.
///
///
/// # Errors
///
/// Returns a [`TaskError`] if the parameters cannot be converted into
/// the corresponding controller implementation.
#[async_trait::async_trait]
impl FromConfig<ControllerParams> for dyn Controller {
    async fn from_config(config: ControllerParams) -> Result<Box<Self>, TaskError> {
        let controller: Box<dyn Controller> = match config {
            #[cfg(target_os = "linux")]
            ControllerParams::Netlink(params) => Box::new(params),
            ControllerParams::Esp32(params) => Box::new(params),
            // Add more cases here as new controllers are added
        };
        Ok(controller)
    }
}
