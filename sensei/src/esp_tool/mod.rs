//! Robust CLI tool for ESP32 CSI monitoring.

mod spam_settings;
mod state;
mod tui;

use std::collections::VecDeque;
use std::env;
use std::error::Error;
use std::io::{self, stdout};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering, Ordering as AtomicOrdering};
use std::sync::mpsc::RecvTimeoutError;
use std::time::Duration;

use chrono::{DateTime, Local};
// Logging facade and our custom logger components
use crossbeam_channel::{Receiver as CrossbeamReceiver, Sender as CrossbeamSender};
use crossterm::event::EventStream; // For async event reading
use crossterm::event::{Event as CEvent, KeyCode, KeyEvent, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode};
use futures::StreamExt;
// Project-specific imports - Changed csi_collection_lib to crate
// Project-specific imports - Changed crate:: to lib::
use lib::adapters::CsiDataAdapter;
use lib::adapters::esp32::ESP32Adapter;
use lib::csi_types::CsiData;
use lib::errors::{ControllerError, DataSourceError};
use lib::network::rpc_message::{DataMsg, SourceType};
use lib::sources::DataSourceT;
// If cli.rs is in src/main.rs or src/lib.rs and esp_tool.rs is src/esp_tool.rs,
// and EspToolSubcommandArgs is in a cli module (e.g. src/cli/mod.rs)
// you might need: use crate::cli::EspToolSubcommandArgs;
use lib::sources::controllers::Controller;
use lib::sources::controllers::esp32_controller::{
    Bandwidth as EspBandwidth, CsiType as EspCsiType, CustomFrameParams, Esp32ControllerParams, Esp32DeviceConfig, OperationMode as EspOperationMode,
    SecondaryChannel as EspSecondaryChannel,
};
use lib::sources::esp32::{Esp32Source, Esp32SourceConfig};
use lib::tui::TuiRunner;
use lib::tui::logs::{FromLog, LogEntry, TuiLogger, init_logger};
use log::{Level, LevelFilter, Metadata, Record, SetLoggerError, debug, error, info, warn};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Cell, List, ListItem, Paragraph, Row, Table, Wrap};
use ratatui::{Frame, Terminal};
use state::{EspUpdate, TuiState};
use tokio::sync::mpsc::{Receiver, Sender};
use tokio::sync::{Mutex, mpsc};
use tokio::task::JoinHandle;
use tokio::time::{sleep, timeout};
use tui::ui;

use crate::cli::EspToolSubcommandArgs;
use crate::services::{EspToolConfig, GlobalConfig, Run};

const LOG_BUFFER_CAPACITY: usize = 200;
const CSI_DATA_BUFFER_CAPACITY: usize = 1000;
const UI_REFRESH_INTERVAL_MS: u64 = 20;
const ACTOR_CHANNEL_CAPACITY: usize = 10;
const ESP_READ_BUFFER_SIZE: usize = 4096;

impl FromLog for EspUpdate {
    fn from_log(log: LogEntry) -> Self {
        EspUpdate::Log(log)
    }
}

pub struct EspTool {}

impl Run<EspToolConfig> for EspTool {
    fn new() -> Self {
        EspTool {}
    }

    async fn run(&mut self, global_config: GlobalConfig, esp_config: EspToolConfig) -> Result<(), Box<dyn std::error::Error>> {
        let (command_send, mut command_recv) = mpsc::channel::<Esp32ControllerParams>(10);
        let (update_send, mut update_recv) = mpsc::channel::<EspUpdate>(10);

        let update_send_clone = update_send.clone();

        let mut esp_src_config = Esp32SourceConfig {
            port_name: esp_config.serial_port,
            ..Default::default()
        };

        let esp_source: Esp32Source = match Esp32Source::new(esp_src_config) {
            Ok(src) => src,
            Err(e) => {
                info!("Failed to initialize ESP32Source: {e}");
                return Err(Box::new(e));
            }
        };
        let esp_device_config = Esp32DeviceConfig::default();

        let esp_task = Self::esp_source_task(esp_source, esp_device_config, update_send_clone, command_recv);
        let tasks = vec![esp_task];

        let tui_runner = TuiRunner::new(TuiState::new(), command_send, update_recv, update_send, global_config.log_level);
        tui_runner.run(tasks).await;
        Ok(())
    }
}

impl EspTool {
    // Handles updates to the state of the TUI based the esp source
    pub async fn esp_source_task(
        mut esp: Esp32Source,
        device_config: Esp32DeviceConfig,
        update_send_channel: Sender<EspUpdate>,
        mut command_recv_channel: Receiver<Esp32ControllerParams>,
    ) {
        let controller = Esp32ControllerParams {
            device_config,
            mac_filters_to_add: vec![],
            clear_all_mac_filters: true,
            control_acquisition: true,
            control_wifi_transmit: true,
            synchronize_time: true,
            transmit_custom_frame: None,
        };

        if let Err(e) = controller.apply(&mut esp).await {
            warn!("ESP Actor: Failed to apply initial ESP32 configuration: {e}");
            update_send_channel.send(EspUpdate::Status("Failed to initialize".to_string())).await;
            return;
        } else {
            info!("ESP Actor: Initial ESP32 configuration applied successfully.");
            update_send_channel.send(EspUpdate::ControllerUpdateSuccess).await;
        }

        info!("ESP actor task started for port {:?}.", esp.port);

        if let Err(e) = esp.start().await {
            error!("ESP Actor: Failed to start Esp32Source: {e}");
            update_send_channel.send(EspUpdate::Error(format!("ESP Start Fail: {e}"))).await;
            update_send_channel.send(EspUpdate::Status("DISCONNECTED (Start Fail)".to_string())).await;
        }

        update_send_channel
            .send(EspUpdate::Status("CONNECTED (Source Started)".to_string()))
            .await;

        let mut read_buffer = vec![0u8; ESP_READ_BUFFER_SIZE];
        let mut esp_adapter = ESP32Adapter::new(false);

        loop {
            tokio::select! {
                biased; // Prioritizes based on order

                Some(updated_controller) = command_recv_channel.recv() => {
                    info!("ESP Actor: Received command: {updated_controller:?}");
                    match updated_controller.apply(&mut esp).await {
                        Ok(_) => {
                            debug!("ESP Actor: Command applied successfully.");
                            if update_send_channel.send(EspUpdate::ControllerUpdateSuccess).await.is_err() {
                                break;
                            }
                        }
                        Err(e) => {
                            error!("ESP Actor: Failed to apply command: {e}");
                            if update_send_channel.send(EspUpdate::Error(e.to_string())).await.is_err() {
                                break;
                            }
                        }
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
                            tokio::time::sleep(Duration::from_millis(100)).await;
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
                                    update_send_channel.try_send(EspUpdate::Error(format!("Adapter Parse Fail: {e}")));
                                }
                            }
                        }
                        Err(e @ DataSourceError::NotConnected(_)) | Err(e @ DataSourceError::Io(_)) if !esp.is_running.load(AtomicOrdering::Relaxed) => {
                            info!("ESP Actor: Source disconnected (Error: {e}), signaling UI.");
                            update_send_channel.send(EspUpdate::EspDisconnected).await;
                        }
                        Err(e) => {
                            warn!("ESP Actor: Error reading from ESP32 source: {e:?}");
                            if update_send_channel.send(EspUpdate::Error(format!("ESP Read Fail: {e}"))).await.is_err() {
                                break;
                            }
                            tokio::time::sleep(Duration::from_secs(1)).await;
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
