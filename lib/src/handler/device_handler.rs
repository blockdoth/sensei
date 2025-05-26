use crate::FromConfig;
use crate::adapters::{CsiDataAdapter, DataAdapterConfig};
use crate::errors::{ControllerError, CsiAdapterError, DataSourceError, SinkError, TaskError};
use crate::network::rpc_message::{DataMsg, SourceType};
use crate::sinks::{Sink, SinkConfig};
use crate::sources::controllers::{Controller, ControllerParams};
use crate::sources::{DataSourceConfig, DataSourceT};
use std::sync::Arc;
use tokio::task::JoinHandle;
use tokio::sync::watch;

/// Configuration for a device handler
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
pub struct DeviceHandlerConfig {
    pub device_id: u64,
    pub stype: SourceType,
    pub source: DataSourceConfig,
    #[serde(default)]
    pub controller: Option<ControllerParams>,
    #[serde(default)]
    pub adapter: Option<DataAdapterConfig>,
    pub sinks: Vec<SinkConfig>,
}

/// A handler for a single device: reads, adapts, and dispatches data
pub struct DeviceHandler {
    device_id: u64,
    stype: SourceType,
    shutdown_tx: Option<watch::Sender<()>>,
    handle: Option<JoinHandle<()>>,
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

        let (shutdown_tx, mut shutdown_rx) = watch::channel(());

        let handle = tokio::spawn(async move {
            if let Err(e) = source.start().await {
                log::error!("Device {device_id} source start failed: {e:?}");
                return;
            }

            loop {
                tokio::select! {
                    _ = shutdown_rx.changed() => {
                        log::info!("Shutting down device {device_id}");
                        break;
                    }
                    read_res = source.read() => {
                        match read_res {
                            Ok(Some(raw)) => {
                                let outgoing = if let Some(adapter) = adapter.as_mut() {
                                    match adapter.produce(raw).await {
                                        Ok(Some(csi_msg)) => vec![csi_msg],
                                        Ok(None) => continue,
                                        Err(err) => {
                                            log::error!("Adapter error on device {device_id}: {err:?}");
                                            continue;
                                        }
                                    }
                                } else {
                                    vec![raw]
                                };
                                for mut sink in sinks.iter_mut() {
                                    for msg in outgoing.iter().cloned() {
                                        if let Err(err) = sink.provide(msg).await {
                                            log::error!("Sink error on device {device_id}: {err:?}" );
                                        }
                                    }
                                }
                            }
                            Ok(None) => continue,
                            Err(e) => {
                                log::error!("Device {device_id} read error: {e:?}");
                                break;
                            }
                        }
                    }
                }
            }
            source.stop().await;
        });

        self.shutdown_tx = Some(shutdown_tx);
        self.handle = Some(handle);
        Ok(())
    }

    pub async fn stop(&mut self) -> Result<(), TaskError> {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        if let Some(handle) = self.handle.take() {
            if let Err(e) = handle.await {
                log::error!("Device task join failed: {e}");
                return Err(TaskError::JoinError(e.to_string()));
            }
        }
        Ok(())
    }


    pub async fn reconfigure(
        &mut self,
        new_config: DeviceHandlerConfig,
    ) -> Result<(), TaskError> {
        // Stop current task
        self.stop().await?;

        // Create a new handler from config
        let new_handler = DeviceHandler::from_config(new_config).await?;

        // Replace internal state
        let new = *new_handler; // unpack Box

        self.device_id = new.device_id;
        self.stype = new.stype;
        self.shutdown_tx = new.shutdown_tx;
        self.handle = new.handle;

        Ok(())
    }
}


#[async_trait::async_trait]
impl FromConfig<DeviceHandlerConfig> for DeviceHandler {
    async fn from_config(config: DeviceHandlerConfig) -> Result<Box<Self>, TaskError> {
        // instantiate source
        let mut source = <dyn DataSourceT>::from_config(config.source).await?;

        // apply controller if configured
        if let Some(controller_cfg) = config.controller {
            let controller: Box<dyn Controller> =
                <dyn Controller>::from_config(controller_cfg).await?;
            controller.apply(source.as_mut()).await?;
        }
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

        let mut handler = DeviceHandler {
            device_id: config.device_id,
            stype: config.stype.clone(),
            shutdown_tx: None,
            handle: None,
        };

        handler.start(source, adapter, sinks).await?;
        Ok(Box::new(handler))

    }
}
