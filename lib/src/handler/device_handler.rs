//! DeviceHandler manages the lifecycle and data flow for a single device,
//! including source reading, data adaptation, and dispatching to sinks.
use std::fs;
use std::fs::File;
use std::io::Write;
use std::path::PathBuf;

use log::info;
use tokio::sync::watch;
use tokio::sync::mpsc; // Added for sending data out
use tokio::task::JoinHandle;

use crate::adapters::*;
use crate::errors::TaskError;
use crate::network::rpc_message::{DataMsg, DeviceId, SourceType}; // Added DataMsg, DeviceId
use crate::sources::controllers::{Controller, ControllerParams};
use crate::sources::{DataSourceConfig, DataSourceT};
use crate::{FromConfig, ToConfig};

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
    /// List of sink IDs this handler should output to.
    #[serde(default)]
    pub output_to: Vec<String>,
}

impl DeviceHandlerConfig {
    /// Asynchronously loads a list of `DeviceHandlerConfig` instances from a YAML file.
    ///
    /// This function reads the contents of the provided YAML file and deserializes it into
    /// a `Vec<DeviceHandlerConfig>`. It is intended for use during system initialization
    /// or configuration loading from a persisted file (e.g., `device_configs.yaml`).
    ///
    /// # Parameters
    /// - `file`: A `PathBuf` specifying the location of the YAML file to load.
    ///
    /// # Returns
    /// - `Ok(Vec<DeviceHandlerConfig>)` if the file is read and deserialized successfully.
    /// - `Err(TaskError::Io)` if the file cannot be read (e.g., missing, permissions).
    /// - `Err(TaskError::Parse)` if deserialization fails (e.g., invalid YAML format).
    /// # Errors
    /// This function may return:
    /// - `TaskError::Io` for file reading issues.
    /// - `TaskError::Parse` for YAML deserialization errors.
    pub fn from_yaml(file: PathBuf) -> Result<Vec<DeviceHandlerConfig>, TaskError> {
        let yaml = fs::read_to_string(file)?;
        let configs = serde_yaml::from_str(&yaml)?;
        Ok(configs)
    }

    /// Serializes a list of `DeviceHandlerConfig` instances to a YAML file.
    ///
    /// # Parameters
    /// - `file`: The output file path.
    /// - `configs`: A vector of `DeviceHandlerConfig` instances to serialize.
    ///
    /// # Returns
    /// - `Ok(())` if the serialization and writing succeed.
    /// - `Err(TaskError)` if writing or serialization fails.
    pub fn to_yaml(file: PathBuf, configs: Vec<DeviceHandlerConfig>) -> Result<(), TaskError> {
        // Serialize the vector to a YAML string
        let yaml = serde_yaml::to_string(&configs)?;

        // Write the string to the file
        let mut f = File::create(file)?;

        f.write_all(yaml.as_bytes())?;

        Ok(())
    }
}

/// A handler for a single device: reads, adapts, and dispatches data
/// A handler responsible for consuming data from a source,
/// optionally adapting it, and dispatching it to one or more sinks.
/// It runs on it's own thread
pub struct DeviceHandler {
    /// Configuration of the device Handler
    config: DeviceHandlerConfig,
    /// Used for stop mechanism
    shutdown_tx: Option<watch::Sender<()>>,
    /// Handle of the thread
    handle: Option<JoinHandle<()>>,
    // data_output_tx: Option<mpsc::Sender<(DataMsg, DeviceId)>>, // To send data to SystemNode - This will be passed to start()
}

impl DeviceHandler {
    /// Returns a clone of the device handler's configuration.
    pub fn config(&self) -> DeviceHandlerConfig {
        self.config.clone()
    }

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
    /// 5. Logs and continues on adapter errors, breaks the loop on source read error.
    ///    Aditionally provides functionality for shutting down the thread
    pub async fn start(
        &mut self,
        mut source: Box<dyn DataSourceT>,
        mut adapter: Option<Box<dyn CsiDataAdapter>>,
        data_output_tx: mpsc::Sender<(DataMsg, DeviceId)>, // Added channel sender
    ) -> Result<(), TaskError> {
        let device_id = self.config.device_id;

        let (shutdown_tx, mut shutdown_rx) = watch::channel(());

        let handle = tokio::spawn(async move {
            if let Err(e) = source.start().await {
                log::error!("Device {device_id} source start failed: {e:?}");
                return;
            }
            // Sink opening is now handled by SystemNode

            loop {
                tokio::select! {
                    _ = shutdown_rx.changed() => {
                        log::info!("Shutting down device {device_id}");
                        break;
                    }
                    read_res = source.read() => {
                        match read_res {
                            Ok(Some(raw)) => {
                                info!("Device handler received {raw:?} for device {device_id}");
                                let outgoing_msgs = if let Some(adapter) = adapter.as_mut() {
                                    match adapter.produce(raw).await {
                                        Ok(Some(csi_msg)) => vec![csi_msg], // Assuming adapter produces one DataMsg
                                        Ok(None) => continue,
                                        Err(_) => {
                                            continue;
                                        }
                                    }
                                } else {
                                    // If no adapter, assume raw is a DataMsg. This might need adjustment
                                    // depending on what `source.read()` returns if it's not a `DataMsg` directly.
                                    // For now, assuming `raw` itself is a `DataMsg` if no adapter.
                                    // This part might need clarification based on `DataSourceT::read`'s typical output.
                                    // If `raw` is bytes, it needs to be wrapped in `DataMsg::RawFrame` first.
                                    // Let's assume `raw` is already a `DataMsg` for simplicity here.
                                    // If it's Vec<u8>, it should be: vec![DataMsg::RawFrame { device_id, timestamp: SystemTime::now(), data: raw }]
                                    // For now, we'll stick to the previous logic where `raw` was directly usable or adapted.
                                    vec![raw]
                                };

                                for msg in outgoing_msgs {
                                    if let Err(e) = data_output_tx.send((msg, device_id)).await {
                                        log::error!("Device {device_id} failed to send data to SystemNode: {e:?}");
                                        // Decide if we should break or continue if the channel is closed
                                        // If SystemNode is down, the channel will be closed.
                                        break; // Breaking if channel is likely closed
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
            let _ = source.stop().await;
            // Sink closing is now handled by SystemNode
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

        self.config = new.config;
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
    /// - Starting the handler’s background task fails
    async fn from_config(cg: DeviceHandlerConfig) -> Result<Box<Self>, TaskError> {
        // instantiate source
        let mut source = <dyn DataSourceT>::from_config(cg.source.clone()).await?;

        // apply controller if configured
        if let Some(controller_cfg) = cg.controller.clone() {
            // check that controller is the correct one to apply to the source
            match (&controller_cfg, &cg.source) {
                (ControllerParams::Esp32(_), DataSourceConfig::Esp32(_)) | (ControllerParams::Tcp(_), DataSourceConfig::Tcp(_)) => {
                    // These combinations are allowed on any OS
                }
                // --- Conditional arm for Netlink on Linux ---
                #[cfg(target_os = "linux")] // This arm is only compiled for Linux
                (ControllerParams::Netlink(_), DataSourceConfig::Netlink(_)) => {
                    // Netlink is allowed on Linux
                }
                _ => {
                    // This will be hit if:
                    // 1. It's a general mismatch (e.g., Esp32 controller with Tcp source).
                    // 2. It's a Netlink controller/source on a non-Linux OS (because the above arm is compiled out).
                    return Err(TaskError::IncorrectController);
                }
            }
            let controller: Box<dyn Controller> = <dyn Controller>::from_config(controller_cfg).await?;
            controller.apply(source.as_mut()).await?;
            // Add other checks
        }
        // instantiate adapter if configured
        let adapter_opt = if let Some(adapter_cfg) = cg.adapter.clone() {
            Some(<dyn CsiDataAdapter>::from_config(adapter_cfg).await?)
        } else {
            None
        };

        let mut handler = Box::new(DeviceHandler {
            config: cg,
            shutdown_tx: None,
            handle: None,
            // data_output_tx will be set by SystemNode when calling start
        });

        // Pass source and adapter to start, but no sinks
        // The data_output_tx needs to be passed here by the caller (SystemNode)
        // This means from_config cannot call start directly if it needs the sender.
        // Or, from_config needs to take the sender as an argument.
        // For now, SystemNode will call start() separately.
        // handler.start(source, adapter_opt).await?; // SystemNode will call this

        Ok(handler)
    }
}

#[async_trait::async_trait]
impl ToConfig<DeviceHandlerConfig> for DeviceHandler {
    /// Converts the current `DeviceHandler` instance into its configuration representation.
    ///
    /// This method implements the `ToConfig` trait for `DeviceHandler`, returning a cloned
    /// copy of its internal `DeviceHandlerConfig`. This allows the handler’s configuration
    /// to be extracted for serialization, inspection, or export.
    ///
    /// # Returns
    /// - `Ok(DeviceHandlerConfig)` containing a clone of the current configuration.
    /// - `Err(TaskError)` if an error occurs (not applicable in this implementation).
    async fn to_config(&self) -> Result<DeviceHandlerConfig, TaskError> {
        // Create a new DeviceHandlerConfig with the current configuration
        // Note: Sinks are not part of DeviceHandler anymore, so they are not included here.
        Ok(self.config.clone())
    }
}
