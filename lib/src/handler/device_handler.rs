//! DeviceHandler manages the lifecycle and data flow for a single device,
//! including source reading, data adaptation, and dispatching to sinks.
use std::fs;
use std::fs::File;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::{Mutex, mpsc, watch};
use tokio::task::JoinHandle;

use crate::adapters::*;
use crate::errors::{ConfigError, TaskError};
use crate::network::rpc_message::{DataMsg, DeviceId, SourceType};
use crate::sources::controllers::{Controller, ControllerParams};
use crate::sources::{DataSourceConfig, DataSourceT};
use crate::{FromConfig, ToConfig};

/// Configuration for a single device handler.
///
/// This struct is deserializable via Serde and contains all of the
/// parameters required to build a [`DeviceHandler`]: the device’s ID,
/// the data source type, optional controller parameters, optional
/// adapter configuration, and a list of sink configurations.
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, PartialEq)]
pub struct DeviceHandlerConfig {
    // Unique identifier for the device.
    pub device_id: u64,
    /// The kind of source producing the raw data.
    pub source_type: SourceType,
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
    /// - `Err(ConfigError::Io)` if the file cannot be read (e.g., missing, permissions).
    /// - `Err(ConfigError::Serde)` if deserialization fails (e.g., invalid YAML format).
    /// # Errors
    /// This function may return:
    /// - `ConfigError::Io` for file reading issues.
    /// - `ConfigError::Serde` for YAML deserialization errors.
    pub fn from_yaml(file: PathBuf) -> Result<Vec<DeviceHandlerConfig>, ConfigError> {
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
    /// A reference to the data source
    source: Arc<Mutex<Box<dyn DataSourceT>>>,
    /// A reference to the adapter
    adapter: Arc<Mutex<Option<Box<dyn CsiDataAdapter>>>>,
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
    pub async fn start(&mut self, data_output_tx: mpsc::Sender<(DataMsg, DeviceId)>) -> Result<(), TaskError> {
        let device_id = self.config.device_id;
        let controller_config = self.config.controller.clone();
        let (shutdown_tx, mut shutdown_rx) = watch::channel(());

        let mut source_outside = Arc::clone(&self.source);
        let mut adapter_outside = Arc::clone(&self.adapter);

        let handle = tokio::spawn(async move {
            let mut source = source_outside.lock().await;
            let mut adapter = adapter_outside.lock().await;

            // Start the source.
            source.start().await.map_err(|e| {
                log::error!("Device {device_id} source start failed: {e:?}");
                TaskError::DataSourceError(e)
            });
            log::info!("Device {device_id} source started successfully.");

            // Apply controller if configured, now that the source is started.
            if let Some(controller_cfg) = controller_config {
                log::info!("Applying controller for device {device_id}.");
                let controller: Box<dyn Controller> = <dyn Controller>::from_config(controller_cfg.clone())
                    .await
                    .map_err(|e| {
                        log::error!("Device {device_id} controller instantiation failed: {e:?}");
                        e // Assuming TaskError::Controller will be handled by `?` or caller
                    })
                    .unwrap();
                controller.apply(source.as_mut()).await.map_err(|e| {
                    log::error!("Device {device_id} controller apply failed: {e:?}");
                    TaskError::ControllerError(e)
                });
                log::info!("Device {device_id} controller applied successfully.");
            }

            log::info!("Device handler task starting for {device_id}.");
            loop {
                tokio::select! {
                    _ = shutdown_rx.changed() => {
                        log::info!("Shutdown signal received for device {device_id}.");
                        if let Err(e) = source.stop().await {
                            log::warn!("Failed to stop source for device {device_id} during shutdown: {e:?}");
                        } else {
                            log::info!("Source for device {device_id} stopped gracefully during shutdown.");
                        }
                        break;
                    }
                    read_res = source.read() => {
                        match read_res {
                            Ok(Some(raw)) => {
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
                            Ok(None) => {
                                // No data read, continue to next iteration
                                continue;
                            }
                            Err(e) => {
                                log::error!("Device {device_id} read error: {e:?}");
                                break;
                            }
                        }
                    }
                }
            }
            let _ = source.stop().await;
            log::info!("Device handler task for {device_id} has stopped.");
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
    /// and does not start a new task using the updated parameters.
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
        let source = <dyn DataSourceT>::from_config(cg.source.clone()).await?;

        let adapter = match cg.adapter {
            Some(adapter_cfg) => Some(<dyn CsiDataAdapter>::from_config(adapter_cfg).await?),
            _ => None,
        };

        // Validate controller configuration if present.
        if let Some(controller_cfg) = &cg.controller {
            match (controller_cfg, &cg.source) {
                #[cfg(feature = "esp_tool")]
                (ControllerParams::Esp32(_), DataSourceConfig::Esp32(_)) => {
                    // These combinations are allowed.
                }
                #[cfg(all(target_os = "linux", feature = "iwl5300"))]
                (ControllerParams::Netlink(_), DataSourceConfig::Netlink(_)) => {
                    // Netlink is allowed on Linux.
                }
                _ => {
                    log::error!(
                        "Incorrect controller type {:?} for source type {:?} for device_id {}",
                        controller_cfg,
                        cg.source_type,
                        cg.device_id
                    );
                    return Err(TaskError::IncorrectController);
                }
            }
        }

        let handler = Box::new(DeviceHandler {
            config: cg,
            shutdown_tx: None,
            handle: None,
            source: Arc::new(Mutex::new(source)),
            adapter: Arc::new(Mutex::new(adapter)),
        });

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

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};

    use async_trait::async_trait;
    use tempfile::NamedTempFile;
    use tokio::sync::mpsc;

    use super::*;
    use crate::ToConfig;
    use crate::adapters::{DataAdapterConfig, MockCsiDataAdapter};
    use crate::errors::TaskError;
    use crate::network::rpc_message::{DataMsg, SourceType};
    use crate::sinks::tcp::TCPConfig;
    use crate::sinks::{MockSink, SinkConfig};
    #[cfg(target_os = "linux")]
    use crate::sources::controllers::ControllerParams;
    use crate::sources::{DataSourceConfig, MockDataSourceT};

    // Provide dummy implementations for tests:
    #[async_trait]
    impl ToConfig<DataSourceConfig> for MockDataSourceT {
        async fn to_config(&self) -> Result<DataSourceConfig, TaskError> {
            Ok(DataSourceConfig::Esp32(Default::default()))
        }
    }

    #[async_trait]
    impl ToConfig<DataAdapterConfig> for MockCsiDataAdapter {
        async fn to_config(&self) -> Result<DataAdapterConfig, TaskError> {
            Ok(DataAdapterConfig::Tcp { scale_csi: true })
        }
    }

    #[async_trait]
    impl ToConfig<SinkConfig> for MockSink {
        async fn to_config(&self) -> Result<SinkConfig, TaskError> {
            Ok(SinkConfig::Tcp(TCPConfig {
                target_addr: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 23456),
                device_id: 42,
            }))
        }
    }

    fn sample_config() -> DeviceHandlerConfig {
        DeviceHandlerConfig {
            device_id: 1,
            source_type: SourceType::ESP32,
            source: DataSourceConfig::Esp32(Default::default()),
            controller: None,
            adapter: None,
            output_to: vec!["sink1".to_string()],
        }
    }

    #[tokio::test]
    async fn test_yaml_roundtrip() {
        let config = sample_config();
        let file = NamedTempFile::new().unwrap();

        DeviceHandlerConfig::to_yaml(file.path().to_path_buf(), vec![config.clone()]).unwrap();
        let loaded = DeviceHandlerConfig::from_yaml(file.path().to_path_buf()).unwrap();

        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0], config);
    }

    #[tokio::test]
    async fn test_from_config_valid() {
        let config = sample_config();
        let handler = DeviceHandler::from_config(config.clone()).await.unwrap();
        assert_eq!(handler.config(), config);
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn test_from_config_invalid_controller_combo() {
        let config = DeviceHandlerConfig {
            device_id: 1,
            source_type: SourceType::ESP32,
            source: DataSourceConfig::Esp32(Default::default()),
            controller: Some(ControllerParams::Netlink(Default::default())),
            adapter: None,
            output_to: vec![],
        };

        let result = DeviceHandler::from_config(config).await;
        assert!(matches!(result, Err(TaskError::IncorrectController)));
    }

    #[tokio::test]
    async fn test_start_and_stop_with_data() {
        let mut mock_source = MockDataSourceT::new();
        let raw_data = DataMsg::RawFrame {
            ts: 123.0,
            bytes: vec![1, 2, 3],
            source_type: SourceType::ESP32,
        };

        mock_source.expect_start().returning(|| Ok(()));

        mock_source.expect_read().returning({
            let raw_data_cloned = raw_data.clone();
            move || Ok(Some(raw_data_cloned.clone()))
        });

        mock_source.expect_stop().returning(|| Ok(()));

        let config = DeviceHandlerConfig {
            device_id: 42,
            source_type: SourceType::ESP32,
            source: DataSourceConfig::Esp32(Default::default()),
            controller: None,
            adapter: None,
            output_to: vec![],
        };

        let mut handler = DeviceHandler::from_config(config).await.unwrap();

        let (tx, mut rx) = mpsc::channel(10);
        handler.start(tx).await.unwrap();

        // let (msg, dev_id) = rx.recv().await.unwrap();
        // assert_eq!(dev_id, 42);

        // if let DataMsg::RawFrame { ref bytes, .. } = msg {
        //     assert_eq!(*bytes, vec![1, 2, 3]);
        // } else {
        //     panic!("Expected RawFrame");
        // }

        handler.stop().await.unwrap();
    }

    #[tokio::test]
    async fn test_reconfigure_replaces_state() {
        let old_config = sample_config();
        let new_config = DeviceHandlerConfig {
            device_id: 99,
            ..old_config.clone()
        };

        let mut handler = DeviceHandler::from_config(old_config).await.unwrap();
        handler.reconfigure(new_config.clone()).await.unwrap();

        assert_eq!(handler.config().device_id, 99);
    }

    #[tokio::test]
    async fn test_to_config_trait() {
        let config = sample_config();
        let handler = DeviceHandler::from_config(config.clone()).await.unwrap();

        let out = handler.to_config().await.unwrap();
        assert_eq!(out, config);
    }
}
