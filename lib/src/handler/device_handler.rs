use crate::FromConfig;
use crate::adapters::{CsiDataAdapter, DataAdapterConfig};
use crate::errors::{ControllerError, CsiAdapterError, DataSourceError, SinkError, TaskError};
use crate::network::rpc_message::{DataMsg, SourceType};
use crate::sinks::{Sink, SinkConfig};
use crate::sources::controllers::{Controller, ControllerParams};
use crate::sources::{DataSourceConfig, DataSourceT};
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::task::JoinHandle;

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
    device_id: u64,
    stype: SourceType,
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
                                            //log::error!("Adapter error on device {device_id}: {err:?}"); THIS WILL LOG ERRORS IF THERE IS SIMPLY NO DATA
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
        let mut source = <dyn DataSourceT>::from_config(config.source).await?;

        source.start().await?;

        // apply controller if configured
        if let Some(controller_cfg) = config.controller {
            let controller: Box<dyn Controller> = <dyn Controller>::from_config(controller_cfg).await?;
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
