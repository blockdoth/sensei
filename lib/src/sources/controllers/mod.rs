use crate::errors::ControllerError;
use crate::sources::DataSourceT;
use async_trait::async_trait;
use typetag;
pub mod netlink_controller;

/// Trait that must be implemented by all source controller types
#[typetag::serde(tag = "type")]
#[async_trait]
pub trait Controller: Send + Sync {
    /// Apply controller parameters to the given source
    async fn apply(&self, source: &mut dyn DataSourceT) -> Result<(), ControllerError>;
}
