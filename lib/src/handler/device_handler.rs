//! DeviceHandler manages the lifecycle and data flow for a single device,
//! including source reading, data adaptation, and dispatching to sinks.
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use log::info;
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::FromConfig;
use crate::adapters::*;
use crate::errors::{ControllerError, CsiAdapterError, DataSourceError, SinkError, TaskError};
use crate::network::rpc_message::{DataMsg, SourceType};
use crate::sinks::tcp::*;
use crate::sinks::*;
use crate::sources::controllers::*;
use crate::sources::{DataSourceConfig, DataSourceT};

/// Configuration for a single device handler.
///
/// This struct is deserializable via Serde and contains all of the
/// parameters required to build a [`DeviceHandler`]: the device’s ID,
/// the data source type, optional controller parameters, optional
/// adapter configuration, and a list of sink configurations.
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
pub struct DeviceHandlerConfig {
    // Unique identifier for the device.
    pub device_id: u64,
    /// The kind of source producing the raw data.
    pub stype: SourceType,
    /// Configuration for the raw data source.
    pub source: DataSourceConfig,
    /// Optional controller parameters to configure the data source.
    #[serde(default)]
    pub controller: Option<ControllerParams>,
    /// Optional adapter configuration for transforming raw frames.
    #[serde(default)]
    pub adapter: Option<DataAdapterConfig>,
    /// One or more sinks that will consume the (adapted) data messages.
    pub sinks: Vec<SinkConfig>,
}

impl DeviceHandlerConfig {
    /// Creates a list of device handlers configs from a yaml file
    pub async fn from_yaml(file: PathBuf) -> Vec<DeviceHandlerConfig> {
        let yaml = fs::read_to_string(file).unwrap();

        serde_yaml::from_str(&yaml).unwrap()
    }
}

/// A handler for a single device: reads, adapts, and dispatches data
/// A handler responsible for consuming data from a source,
/// optionally adapting it, and dispatching it to one or more sinks.
/// It runs on it's own thread
pub struct DeviceHandler {
    /// The device_id for the device, unique in the whole network
    device_id: u64,
    /// The source type of device, ex: iwl, atheros etc.
    stype: SourceType,
    /// Used for stop mechanism
    shutdown_tx: Option<watch::Sender<()>>,
    /// Handle of the thread
    handle: Option<JoinHandle<()>>,
}

impl DeviceHandler {
    /// Spanw a thread, Begin reading from the configured data source, adapt frames if an adapter
    /// is provided, and forward each message to the configured sinks.
    ///
    /// # Parameters
    ///
    /// - `source`: A boxed dynamic data source implementing [`DataSourceT`].
    /// - `adapter`: An optional boxed dynamic adapter implementing [`CsiDataAdapter`].
    /// - `sinks`: A vector of boxed dynamic sinks implementing [`Sink`].
    ///
    /// # Errors
    ///
    /// Returns a [`TaskError`] if spawning the processing task fails.
    ///
    /// # Behavior
    ///
    /// Spawns a Tokio task that:
    /// 1. Activates the source (`source.start().await`).
    /// 2. In a loop, reads raw bytes into a buffer.
    /// 3. Wraps them in a [`DataMsg::RawFrame`], with a timestamp.
    /// 4. Passes the frame through the adapter (if present).
    /// 5. For each outgoing message, calls `.provide(msg).await` on each sink.
    /// 6. Logs and continues on adapter or sink errors, breaks the loop on source read error.
    ///    Aditionally provides functionality for shutting down the thread
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
            for sink in sinks.iter_mut() {
                if let Err(e) = sink.open().await {
                    log::error!("Device {device_id} sink open failed: {e:?}");
                    return;
                }
            }

            loop {
                tokio::select! {
                    // Shutdown the thread if it's called from stop
                    _ = shutdown_rx.changed() => {
                        log::info!("Shutting down device {device_id}");
                        break;
                    }
                    read_res = source.read() => {
                        // check that there is something read from source
                        match read_res {
                            Ok(Some(raw)) => {
                                // optional adapter
                                info!("Device handler received {raw:?} for device {device_id}");
                                let outgoing = if let Some(adapter) = adapter.as_mut() {
                                    match adapter.produce(raw).await {
                                        Ok(Some(csi_msg)) => vec![csi_msg],
                                        Ok(None) => continue,
                                        Err(err) => {
                                            //log::error!("Adapter error on device {device_id}: {err:?}"); THIS WILL LOG ERRORS IF THERE IS SIMPLY NO DATA
                                            continue;
                                        }
                                    }
                                } else {
                                    vec![raw]
                                };
                                // send to all sinks
                                for mut sink in sinks.iter_mut() {
                                    for msg in outgoing.iter().cloned() {
                                        info!("Device handler outputting {msg:?} to sink");
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
            for sink in sinks.iter_mut() {
                if let Err(e) = sink.close().await {
                    log::error!("Device {device_id} sink close failed: {e:?}");
                }
            }
        });

        self.shutdown_tx = Some(shutdown_tx);
        self.handle = Some(handle);
        Ok(())
    }

    /// Stops the device task by sending a shutdown signal and awaiting task completion.
    ///
    /// # Returns
    ///
    /// * `Ok(())` on successful shutdown
    /// * `Err(TaskError)` if the task join fails
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

    /// Reconfigures the device handler with a new configuration.
    ///
    /// This function stops the current task, applies the new configuration,
    /// and starts a new task using the updated parameters.
    ///
    /// # Arguments
    ///
    /// * `new_config` - New configuration to apply
    ///
    /// # Returns
    ///
    /// * `Ok(())` on successful reconfiguration
    /// * `Err(TaskError)` if stop or reinitialization fails
    pub async fn reconfigure(&mut self, new_config: DeviceHandlerConfig) -> Result<(), TaskError> {
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
    /// Constructs a [`DeviceHandler`] from its configuration, then immediately
    /// starts its processing loop in the background.
    ///
    /// # Parameters
    ///
    /// - `config`: The deserialized [`DeviceHandlerConfig`].
    ///
    /// # Errors
    ///
    /// Returns [`TaskError`] if any of:
    /// - Source instantiation fails
    /// - Controller application fails
    /// - Adapter instantiation fails
    /// - Sink instantiation fails
    /// - Starting the handler’s background task fails
    async fn from_config(config: DeviceHandlerConfig) -> Result<Box<Self>, TaskError> {
        // instantiate source
        let mut source = <dyn DataSourceT>::from_config(config.source.clone()).await?;

        // apply controller if configured
        if let Some(controller_cfg) = config.controller.clone() {
            // check that controller is the correct one to apply to the source
            match (&controller_cfg, &config.source) {
                (ControllerParams::Esp32(_), DataSourceConfig::Esp32(_))
                | (ControllerParams::Netlink(_), DataSourceConfig::Netlink(_))
                | (ControllerParams::Tcp(_), DataSourceConfig::Tcp(_)) => {}
                _ => return Err(TaskError::IncorrectController),
            }
            let controller: Box<dyn Controller> = <dyn Controller>::from_config(controller_cfg).await?;
            controller.apply(source.as_mut()).await?;
            // Add other checks
        }
        // instantiate adapter if configured
        let adapter = if let Some(adapt_cfg) = config.adapter {
            // Make sure the adapter is the right one for the source
            match (&adapt_cfg, &config.source) {
                (DataAdapterConfig::Esp32 { scale_csi: _ }, DataSourceConfig::Esp32(_))
                | (DataAdapterConfig::Iwl { scale_csi: _ }, DataSourceConfig::Netlink(_))
                | (_, DataSourceConfig::Tcp(_)) => {}
                _ => return Err(TaskError::IncorrectAdapter),
            }
            Some(<dyn CsiDataAdapter>::from_config(adapt_cfg).await?)
        } else {
            None
        };
        // instantiate sinks
        let mut sinks = Vec::with_capacity(config.sinks.len());
        for sc in config.sinks.into_iter() {
            // Checkin that the device id from config is the same in the casee
            // that you are using a tcpsink
            if let SinkConfig::Tcp(TCPConfig {
                target_addr: _,
                ref device_id,
            }) = sc
            {
                if device_id != &config.device_id {
                    return Err(TaskError::WrongSinkDid);
                }
            }
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
