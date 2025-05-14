// src/tasks/raw_source_task.rs
use crate::adapters::Adapter;
use crate::network::rpc_message::{DataMsg, RpcMessage, SourceType};
use crate::sinks::Sink;
use crate::sources::Source;
use async_trait::async_trait;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use tokio::task::JoinHandle;

pub struct RawSourceConfig {
    pub source: Box<dyn Source + Send + Sync>,
    pub adapter: Option<Box<dyn Adapter + Send + Sync>>,
    pub sinks: Vec<Box<dyn Sink + Send + Sync>>,
}

pub struct RawSourceTask {
    config: Arc<Mutex<RawSourceConfig>>,
}

impl RawSourceTask {
    pub fn new(config: RawSourceConfig) -> Self {
        RawSourceTask {
            config: Arc::new(Mutex::new(config)),
        }
    }

    pub fn update_config(&self, new_config: RawSourceConfig) {
        let mut cfg = self.config.lock().unwrap();
        *cfg = new_config;
    }


}