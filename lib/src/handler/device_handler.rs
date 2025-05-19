use crate::FromConfig;
use crate::adapters::{CsiDataAdapter, DataAdapterConfig};
use crate::errors::{CsiAdapterError, DataSourceError, SinkError, TaskError};
use crate::network::rpc_message::{DataMsg, SourceType};
use crate::sinks::{Sink, SinkConfig};
use crate::sources::{DataSourceConfig, DataSourceT};
use std::sync::Arc;
use tokio::task::JoinHandle;

/// Configuration for a device handler
#[derive(serde::Deserialize, Debug, Clone)]
pub struct DeviceHandlerConfig {
    pub device_id: u64,
    pub stype: SourceType,
    pub source: DataSourceConfig,
    #[serde(default)]
    pub adapter: Option<DataAdapterConfig>,
    pub sinks: Vec<SinkConfig>,
}

/// A handler for a single device: reads, adapts, and dispatches data
pub struct DeviceHandler {
    device_id: u64,
    stype: SourceType,
}

impl DeviceHandler {
    /// Start consuming from source, adapting and forwarding to sinks
    pub async fn start(
        &mut self,
        mut source: Box<dyn DataSourceT>,
        mut adapter: Option<Box<dyn CsiDataAdapter>>,
        mut sinks: Vec<Box<dyn Sink>>,
    ) -> Result<(), TaskError> {
        let device_id = self.device_id;
        let stype = self.stype.clone();

        // spawn the main loop
        tokio::spawn(async move {
            // activate source
            if let Err(e) = source.start().await {
                log::error!("Device {device_id} source start failed: {e:?}");
                return;
            }

            let mut buf = vec![0u8; 8192];
            loop {
                tokio::select! {
                    read_res = source.read(&mut buf) => {
                        match read_res {
                            Ok(size) => {
                                let raw = DataMsg::RawFrame {
                                    ts: chrono::Utc::now().timestamp_millis() as f64 / 1e3,
                                    bytes: buf[..size].to_vec(),
                                    source_type: stype.clone(),
                                };
                                // feed through adapter
                                let outgoing = if let Some(adapter) = adapter.as_mut() {
                                    match adapter.produce(raw).await {
                                        Ok(Some(csi_msg)) => vec![csi_msg],
                                        Ok(None) => continue, // need more data
                                        Err(err) => {
                                            log::error!("Adapter error on device {device_id}: {err:?}");
                                            continue;
                                        }
                                    }
                                } else {
                                    vec![raw]
                                };
                                // send to sinks
                                for mut sink in sinks.iter_mut() {
                                    for msg in outgoing.iter().cloned() {
                                        if let Err(err) = sink.provide(msg).await {
                                            log::error!("Sink error on device {device_id}: {err:?}" );
                                        }
                                    }
                                }
                            }
                            Err(e) => {
                                log::error!("Device {device_id} read error: {e:?}");
                                break;
                            }
                        }
                    }
                }
            }
        });
        Ok(())
    }
}

#[async_trait::async_trait]
impl FromConfig<DeviceHandlerConfig> for DeviceHandler {
    async fn from_config(config: DeviceHandlerConfig) -> Result<Box<Self>, TaskError> {
        // instantiate source
        let source = <dyn DataSourceT>::from_config(config.source).await?;
        // instantiate adapter if configured
        let adapter = if let Some(adapt_cfg) = config.adapter {
            Some(<dyn CsiDataAdapter>::from_config(adapt_cfg).await?)
        } else {
            None
        };
        // instantiate sinks
        let mut sinks = Vec::with_capacity(config.sinks.len());
        for sc in config.sinks.into_iter() {
            sinks.push(<dyn Sink>::from_config(sc).await?);
        }

        // build handler
        let mut handler = DeviceHandler {
            device_id: config.device_id,
            stype: config.stype.clone(),
        };

        // start processing
        handler.start(source, adapter, sinks).await?;
        Ok(Box::new(handler))
    }
}
