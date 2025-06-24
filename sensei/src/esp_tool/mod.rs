#![allow(unused)]

//! Robust CLI tool for ESP32 CSI monitoring.

mod spam_settings;
mod state;
mod tui;

use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
use std::time::Duration;

use lib::adapters::CsiDataAdapter;
use lib::adapters::esp32::ESP32Adapter;
use lib::errors::DataSourceError;
use lib::network::rpc_message::DataMsg;
use lib::sources::DataSourceT;
use lib::sources::controllers::Controller;
use lib::sources::controllers::esp32_controller::{Esp32ControllerParams, Esp32DeviceConfig, EspMode};
use lib::sources::esp32::{Esp32Source, Esp32SourceConfig};
use lib::tui::TuiRunner;
use lib::tui::logs::{FromLog, LogEntry};
use log::{LevelFilter, debug, error, info, warn};
use state::{EspTuiState, EspUpdate};
use tokio::sync::mpsc::{self, Receiver, Sender};

use crate::services::{EspToolConfig, GlobalConfig, Run};

/// Capacity of the TUI log buffer.
const LOG_BUFFER_CAPACITY: usize = 200;
/// Capacity of the TUI CSI data buffer.
const CSI_DATA_BUFFER_CAPACITY: usize = 50000;
/// Refresh interval for the TUI in milliseconds.
const UI_REFRESH_INTERVAL_MS: u64 = 20;
/// Capacity of the actor channel for ESP commands.
/// This channel is used to send commands from the TUI to the ESP32 actor task.
const ACTOR_CHANNEL_CAPACITY: usize = 10;
/// Size of the read buffer for ESP32 serial communication.
const ESP_READ_BUFFER_SIZE: usize = 4096;

/// Commands that can be sent to the ESP32 actor task.
///
/// Used to update the ESP32 configuration or terminate the task.
#[derive(Debug)]
pub enum EspChannelCommand {
    /// Updates the ESP32 configuration with new parameters.
    UpdatedConfig(Esp32ControllerParams),
    /// Signals the ESP task to terminate.
    Exit,
}

impl FromLog for EspUpdate {
    /// Converts a `LogEntry` into a `EspUpdate::Log`.
    fn from_log(log: LogEntry) -> Self {
        EspUpdate::Log(log)
    }
}

/// Command-line tool for monitoring CSI data from ESP32 devices.
///
/// This tool sets up a TUI to visualize CSI data, manages the ESP32 connection,
/// and handles configuration commands for the ESP device.
pub struct EspTool {
    /// The serial port to connect to the ESP32 device.
    serial_port: String,
    /// The logging level for the tool.
    log_level: LevelFilter,
}

impl Run<EspToolConfig> for EspTool {
    /// Creates a new `EspTool` instance.
    ///
    /// # Arguments
    /// * `global_config` - Global application configuration.
    /// * `esp_config` - ESP32 tool specific configuration.
    fn new(global_config: GlobalConfig, esp_config: EspToolConfig) -> Self {
        EspTool {
            serial_port: esp_config.serial_port,
            log_level: global_config.log_level,
        }
    }
    /// Starts the ESP32 monitoring tool with the provided configuration.
    ///
    /// Spawns the ESP32 actor task and starts the TUI.
    ///
    /// # Arguments
    /// * `self` - The `EspTool` instance.
    ///
    /// # Errors
    /// Returns a boxed `Error` if initialization or runtime encounters a failure.
    async fn run(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let (command_send, command_recv) = mpsc::channel::<EspChannelCommand>(ACTOR_CHANNEL_CAPACITY);
        let (update_send, update_recv) = mpsc::channel::<EspUpdate>(ACTOR_CHANNEL_CAPACITY);

        let update_send_clone = update_send.clone();

        let esp_src_config = Esp32SourceConfig {
            port_name: self.serial_port.clone(),
            ..Default::default()
        };

        let esp_device_config = Esp32DeviceConfig::default();

        let esp_task = Self::esp_source_task(esp_src_config, esp_device_config, update_send_clone, command_recv);
        let tasks = vec![esp_task];

        let tui_runner = TuiRunner::new(EspTuiState::new(), command_send, update_recv, update_send, self.log_level);
        tui_runner.run(tasks).await;
        Ok(())
    }
}

impl EspTool {
    /// ESP32 actor task responsible for data acquisition and command handling.
    ///
    /// This task:
    /// - Starts and configures the ESP32 data source.
    /// - Listens for CSI frames from the device.
    /// - Forwards CSI data or status updates to the TUI.
    /// - Applies runtime configuration updates when received via channel.
    ///
    /// # Arguments
    /// * `esp_src_config` - Configuration for initializing the ESP32 source.
    /// * `device_config` - Initial device configuration for the ESP32.
    /// * `update_send_channel` - Channel to send updates to the TUI.
    /// * `command_recv_channel` - Channel to receive commands like configuration changes or exit.
    pub async fn esp_source_task(
        esp_src_config: Esp32SourceConfig,
        device_config: Esp32DeviceConfig,
        update_send_channel: Sender<EspUpdate>,
        mut command_recv_channel: Receiver<EspChannelCommand>,
    ) {
        let port_name = esp_src_config.port_name.clone();
        let mut esp: Esp32Source = match Esp32Source::new(esp_src_config) {
            Ok(src) => src,
            Err(e) => {
                error!("Failed to initialize ESP32Source: {e}, Exiting");
                return;
            }
        };
        if let Err(e) = esp.start().await {
            update_send_channel.send(EspUpdate::Status("DISCONNECTED (Start Fail)".to_string())).await;
            warn!("Shutting down ESP task");
            return;
        }

        let default_controller = Esp32ControllerParams {
            device_config,
            mac_filters: vec![],
            mode: EspMode::Listening,
            synchronize_time: true,
            transmit_custom_frame: None,
        };

        if let Err(e) = default_controller.apply(&mut esp).await {
            warn!("ESP Actor: Failed to apply initial ESP32 configuration: {e}");
            update_send_channel.send(EspUpdate::Status("Failed to initialize".to_string())).await;
            warn!("Shuting down ESP task");
            return;
        } else {
            info!("ESP Actor: Initial ESP32 configuration applied successfully.");
            update_send_channel.send(EspUpdate::ControllerUpdateSuccess).await;
        }

        // Do not look at this
        info!("ESP actor task started for port {port_name}.");

        update_send_channel
            .send(EspUpdate::Status("CONNECTED (Source Started)".to_string()))
            .await;

        let read_buffer = vec![0u8; ESP_READ_BUFFER_SIZE];
        let mut esp_adapter = ESP32Adapter::new(false);

        loop {
            tokio::select! {
                biased; // Prioritizes based on order

                Some(command) = command_recv_channel.recv() => {
                    debug!("ESP Actor: Received command: {command:?}");
                    match command {
                        EspChannelCommand::UpdatedConfig(new_controller) => {
                          match new_controller.apply(&mut esp).await {
                            Ok(_) => {
                                debug!("ESP Actor: Command applied successfully.");
                                if update_send_channel.send(EspUpdate::ControllerUpdateSuccess).await.is_err() {
                                    break;
                                }
                            }
                            Err(e) => {
                                error!("ESP Actor: Failed to apply command: {e}");
                                break;
                            }
                          }

                        }
                        EspChannelCommand::Exit => {
                          debug!("Starting graceful exit");
                          match default_controller.apply(&mut esp).await {
                            Ok(_) => {
                                debug!("ESP Actor: Reverted ESP back to default config ");
                                if update_send_channel.send(EspUpdate::ControllerUpdateSuccess).await.is_err() {
                                    break;
                                }
                            }
                            Err(e) => {
                                error!("ESP Actor: Failed to reset ESP: {e}");
                                break;
                            }
                          }
                          break
                        },
                    }

                }

                read_result = esp.read() => {
                    match read_result {
                        Ok(None) => {
                            if !esp.is_running.load(AtomicOrdering::Relaxed) {
                                info!("ESP Actor: Source reported not running and read Ok(0). Signaling disconnect.");
                                let _ = update_send_channel.send(EspUpdate::EspDisconnected).await;
                                break;
                            }
                            tokio::time::sleep(Duration::from_millis(UI_REFRESH_INTERVAL_MS)).await;
                        }
                        Ok(Some(data_msg)) => {
                            match esp_adapter.produce(data_msg).await {
                                Ok(Some(DataMsg::CsiFrame { csi })) => {
                                    match update_send_channel.try_send(EspUpdate::CsiData(csi)) {
                                        Ok(_) => {}
                                        Err(mpsc::error::TrySendError::Full(_)) => {
                                            static LOG_DROP_COUNTER: AtomicUsize = AtomicUsize::new(0);
                                            let current_drop_count = LOG_DROP_COUNTER.fetch_add(1, AtomicOrdering::Relaxed);
                                            if current_drop_count % 10000 == 0 {
                                                warn!("ESP Actor: UI update channel full, dropping CSI data ({} drops so far). UI might be lagging.", current_drop_count + 1);
                                            }
                                        }
                                        Err(mpsc::error::TrySendError::Closed(_)) => {
                                            info!("ESP Actor: UI update channel closed. Terminating actor.");
                                            break;
                                        }
                                    }
                                }
                                Ok(Some(DataMsg::RawFrame { .. })) => {}
                                Ok(None) => {}
                                Err(e) => {
                                    warn!("ESP Actor: ESP32Adapter failed to parse CSI: {e:?}");
                                }
                            }
                        }
                        Err(e @ DataSourceError::NotConnected(_)) | Err(e @ DataSourceError::Io(_)) if !esp.is_running.load(AtomicOrdering::Relaxed) => {
                            info!("ESP Actor: Source disconnected (Error: {e}), signaling UI.");
                            update_send_channel.send(EspUpdate::EspDisconnected).await;
                        }
                        Err(e) => {
                            warn!("ESP Actor: Error reading from ESP32 source: {e:?}");
                            break;
                        }
                    }
                }
            }
        }
        info!("ESP actor task: initiating source stop...");
        if let Err(e) = esp.stop().await {
            error!("ESP actor task: Error stopping ESP32 source: {e}");
        } else {
            info!("ESP actor task: ESP32 source stopped successfully.");
        }

        update_send_channel
            .send(EspUpdate::Status("DISCONNECTED (Actor Stopped)".to_string()))
            .await;
        info!("ESP actor task for port {:?} stopped.", esp.port);
    }
}

#[cfg(test)]
mod tests {
    use chrono::Local;
    use lib::tui::logs::LogEntry;
    use log::Level;

    use super::*;
    use crate::services::GlobalConfig; // Import Local for timestamp

    #[test]
    fn test_esp_tool_new() {
        let global_config = GlobalConfig {
            log_level: LevelFilter::Debug,
            num_workers: 4,
        };
        let esp_config = EspToolConfig {
            serial_port: "/dev/ttyUSB0".to_string(),
        };
        let esp_tool = EspTool::new(global_config, esp_config);
        assert_eq!(esp_tool.serial_port, "/dev/ttyUSB0");
        assert_eq!(esp_tool.log_level, LevelFilter::Debug);
    }

    #[test]
    fn test_from_log_for_esp_update() {
        let log_entry = LogEntry {
            level: Level::Info,
            message: "Test log message".to_string(),
            timestamp: Local::now(),
        };
        let esp_update = EspUpdate::from_log(log_entry.clone());
        match esp_update {
            EspUpdate::Log(received_log_entry) => {
                assert_eq!(received_log_entry.level, log_entry.level);
                assert_eq!(received_log_entry.message, log_entry.message);
                assert_eq!(received_log_entry.timestamp, log_entry.timestamp);
            }
            _ => panic!("EspUpdate should be of type Log"),
        }
    }
}
