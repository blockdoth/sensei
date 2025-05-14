use crate::errors::ControllerError;

/// Trait that must be implemented by all controller types
// Tag is for desrialization
#[typetag::serde(tag = "type")]
#[async_trait::async_trait]
pub trait Controller: Send + Sync {
    async fn configure(&self) -> Result<(), ControllerError>;
}
