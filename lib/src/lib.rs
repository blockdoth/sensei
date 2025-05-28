//!
//!Sensei library
//!
//! Contains utility functions, structs and enums.
//! The network implements the tcp logic
//! Device handling is implemented through adapters, sources and sinks,
//! All of them are commbined in the handler pipeline
//! Usefull errors are also made available
//!

use crate::errors::TaskError;
use async_trait::async_trait;

pub mod adapters;
pub mod csi_types;
pub mod errors;
pub mod handler;
pub mod network;
pub mod sinks;
pub mod sources;

/// Trait to create an instance from a configuration, needs to be implemented for all configurable things
/// Like sources, controllers, adapters and sinks
/// Very useful for initialization from Yaml files
#[async_trait]
pub trait FromConfig<C> {
    async fn from_config(config: C) -> Result<Box<Self>, TaskError>;
}

/// Trait to convert an instance into its configuration representation.
/// Useful for serialization or saving the current state to a configuration file (e.g., YAML).
/// This trait should be implemented for all configurable components such as
/// sources, controllers, adapters, and sinks.
#[async_trait]
pub trait ToConfig<C> {
    async fn to_config(&self) -> Result<C, TaskError>;
}
