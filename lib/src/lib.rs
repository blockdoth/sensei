use crate::errors::TaskError;
use async_trait::async_trait;

pub mod adapters;
pub mod csi_types;
pub mod devices;
pub mod errors;
pub mod handler;
pub mod network;
pub mod sinks;
pub mod sources;

// Trait to create an instance from a configuration, needs to be implemented for all configurable things
// Like sources, controllers, adapters and sinks
#[async_trait]
pub trait FromConfig<C> {
    async fn from_config(config: C) -> Result<Box<Self>, TaskError>;
}
