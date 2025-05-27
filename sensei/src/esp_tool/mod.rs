//! Robust CLI tool for ESP32 CSI monitoring.

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
use crossterm::event::{Event as CEvent, EventStream, KeyCode, KeyEvent, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode};
use futures::StreamExt;
use lib::adapters::CsiDataAdapter;
use lib::adapters::esp32::ESP32Adapter;
use lib::csi_types::CsiData;
use lib::errors::{ControllerError, DataSourceError};
use lib::network::rpc_message::{DataMsg, SourceType};
use lib::sources::DataSourceT;
use lib::sources::controllers::Controller;
use lib::sources::controllers::esp32_controller::{
    Bandwidth as EspBandwidth, CsiType as EspCsiType, CustomFrameParams, Esp32Controller, Esp32DeviceConfig, OperationMode as EspOperationMode,
    SecondaryChannel as EspSecondaryChannel,
};
use lib::sources::esp32::{Esp32Source, Esp32SourceConfig};
use lib::tui::logs::{LogEntry, TuiLogger, init_logger};
use log::{Level, LevelFilter, Metadata, Record, SetLoggerError, debug, error, info, warn};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Cell, List, ListItem, Paragraph, Row, Table, Wrap};
use ratatui::{Frame, Terminal};
use state::TuiState;
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

#[derive(Debug)]
pub enum AppUpdate {
    Log(LogEntry),
    Csi(CsiData),
    Status(String),
    Error(String),
    EspDisconnected,
    ControllerUpdateSuccess,
    ControllerUpdateError(String),
}

pub struct EspTool {
    tui_state: TuiState,
    event_stream: EventStream,
}

impl Run<EspToolConfig> for EspTool {
    fn new() -> Self {
        EspTool {
            tui_state: TuiState::new(),
            event_stream: EventStream::new(),
        }
    }

    async fn run(&mut self, global_config: GlobalConfig, config: EspToolConfig) -> Result<(), Box<dyn std::error::Error>> {
        let (command_send, command_recv) = mpsc::channel::<Esp32Controller>(ACTOR_CHANNEL_CAPACITY);
        let (update_send, mut update_recv) = mpsc::channel::<AppUpdate>(ACTOR_CHANNEL_CAPACITY * 2);
        let (log_send, log_recv) = mpsc::channel::<LogEntry>(LOG_BUFFER_CAPACITY);
        let shutdown_signal = Arc::new(AtomicBool::new(false));

        let mut terminal = setup_terminal()?;
        {
            // Start of TUI
            let log_processor_handle = Self::log_handler_task(log_recv, update_send.clone(), shutdown_signal.clone()).await;
            init_logger(global_config.log_level, log_send.clone());

            let esp = Esp32Source::new(Esp32SourceConfig::default()).unwrap();
            let device_config = Esp32DeviceConfig::default();
            let esp_source_handle = Self::esp_source_task(esp, device_config, update_send.clone(), command_recv, shutdown_signal.clone()).await;

            while !self.tui_state.should_quit {
                terminal.draw(|frame| ui(frame, &self.tui_state))?;
                self.update(&command_send, &update_send, &mut update_recv, shutdown_signal.clone()).await;
            }

            info!("Main UI loop exited. Beginning CLI shutdown sequence...");
            shutdown_signal.store(true, Ordering::SeqCst);
            drop(log_send);

            info!("Waiting for ESP actor task to complete (max 5s)...");
            match tokio::time::timeout(Duration::from_secs(5), esp_source_handle).await {
                Ok(_) => info!("ESP actor task finished gracefully."),
                Err(e) => error!("ESP actor task panicked or returned an error: {e:?}"),
            }

            info!("Waiting for log processor task to complete (max 3s)...");
            match tokio::time::timeout(Duration::from_secs(3), log_processor_handle).await {
                Ok(Ok(_)) => info!("Log processor task finished gracefully."),
                Ok(Err(e)) => error!("Log processor task panicked: {e:?}"),
                Err(_) => warn!("Log processor task timed out during shutdown."),
            }
        } // End of TUI
        restore_terminal(&mut terminal)?;

        Ok(())
    }
}

impl EspTool {
    // Handles all updates to the state of the TUI based on user interactions
    pub async fn update(
        &mut self,
        command_send_channel: &Sender<Esp32Controller>,
        update_send_channel: &Sender<AppUpdate>,
        update_recv_channel: &mut Receiver<AppUpdate>,
        shutdown_signal: Arc<AtomicBool>,
    ) {
        tokio::select! {
          biased;
          _ = tokio::signal::ctrl_c() => {
              info!("Ctrl+C received. Initiating shutdown...");
              shutdown_signal.store(true, Ordering::SeqCst);
              self.tui_state.should_quit = true;
          }
          maybe_event = self.event_stream.next() => {
              match maybe_event {
                  Some(Ok(CEvent::Key(key_event))) => {
                      if self.tui_state.handle_keyboard_event(key_event , command_send_channel, update_send_channel).await {
                        shutdown_signal.store(true, Ordering::SeqCst);
                      }
                  }
                  Some(Err(e)) => {
                      if update_send_channel.send(AppUpdate::Error(format!("Input event error: {e}"))).await.is_err() {
                          error!("Failed to send input error to UI: channel closed.");
                          self.tui_state.should_quit = true;
                          shutdown_signal.store(true, Ordering::SeqCst);
                      }
                  }
                  None => {
                      info!("Crossterm event stream ended. Shutting down.");
                      self.tui_state.should_quit = true;
                      shutdown_signal.store(true, Ordering::SeqCst);
                  }
                  _ => {}
              }
          }
          Some(update) = update_recv_channel.recv() => {
              match update {
                  AppUpdate::Log(log_entry) => self.tui_state.add_log_message(log_entry),
                  AppUpdate::Csi(csi_data) => self.tui_state.add_csi_data(csi_data),
                  AppUpdate::Status(status) => self.tui_state.connection_status = status,
                  AppUpdate::Error(err_msg) => self.tui_state.last_error = Some(err_msg),
                  AppUpdate::EspDisconnected => {
                    self.tui_state.connection_status = "DISCONNECTED (ESP)".to_string();
                  }
                  AppUpdate::ControllerUpdateSuccess => {
                    self.tui_state.last_error = None;
                    self.tui_state.previous_esp_config_for_revert = None;
                    self.tui_state.previous_is_continuous_spam_active_for_revert = None;
                  }
                  AppUpdate::ControllerUpdateError(err_msg) => {
                      self.tui_state.last_error = Some(err_msg.clone());
                      if let Some(old_config) = self.tui_state.previous_esp_config_for_revert.take() {
                          self.tui_state.esp_config = old_config;
                          info!("ESP config reverted due to actor error: {err_msg}");
                      }
                      if let Some(old_spam_active) = self.tui_state.previous_is_continuous_spam_active_for_revert.take() {
                          self.tui_state.is_continuous_spam_active = old_spam_active;
                          info!("Continuous spam state reverted due to actor error: {err_msg}");
                      }
                  }
              }
          }
          _ = tokio::time::sleep(Duration::from_millis(UI_REFRESH_INTERVAL_MS)) => {}
        }
    }

    // Handles updates to the state of the TUI based the esp source
    pub async fn esp_source_task(
        mut esp: Esp32Source,
        device_config: Esp32DeviceConfig,
        update_send_channel: Sender<AppUpdate>,
        mut command_recv_channel: Receiver<Esp32Controller>,
        shutdown_signal: Arc<AtomicBool>,
    ) -> JoinHandle<()> {
        let controller = Esp32Controller {
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
            update_send_channel
                .send(AppUpdate::ControllerUpdateError(format!("Initial ESP config failed: {e}")))
                .await;
        } else {
            info!("ESP Actor: Initial ESP32 configuration applied successfully.");
            update_send_channel.send(AppUpdate::ControllerUpdateSuccess).await;
        }

        info!("ESP actor task started for port {:?}.", esp.port);

        if let Err(e) = esp.start().await {
            error!("ESP Actor: Failed to start Esp32Source: {e}");
            update_send_channel.send(AppUpdate::Error(format!("ESP Start Fail: {e}"))).await;
            update_send_channel.send(AppUpdate::Status("DISCONNECTED (Start Fail)".to_string())).await;
        }

        update_send_channel
            .send(AppUpdate::Status("CONNECTED (Source Started)".to_string()))
            .await;

        let mut read_buffer = vec![0u8; ESP_READ_BUFFER_SIZE];
        let mut esp_adapter = ESP32Adapter::new(false);

        tokio::spawn(async move {
            loop {
                tokio::select! {
                    biased; // Prioritizes based on order

                    _ = tokio::time::sleep(Duration::from_millis(1)), if shutdown_signal.load(AtomicOrdering::Relaxed) => {
                        info!("ESP Actor: Shutdown signal received.");
                        break;
                    }

                    Some(updated_controller) = command_recv_channel.recv() => {
                        info!("ESP Actor: Received command: {updated_controller:?}");
                        match updated_controller.apply(&mut esp).await {
                            Ok(_) => {
                                debug!("ESP Actor: Command applied successfully.");
                                if update_send_channel.send(AppUpdate::ControllerUpdateSuccess).await.is_err() {
                                    break;
                                }
                            }
                            Err(e) => {
                                error!("ESP Actor: Failed to apply command: {e}");
                                if update_send_channel.send(AppUpdate::ControllerUpdateError(e.to_string())).await.is_err() {
                                    break;
                                }
                            }
                        }
                    }

                    read_result = esp.read(&mut read_buffer) => {
                        match read_result {
                            Ok(0) => {
                                if !esp.is_running.load(AtomicOrdering::Relaxed) {
                                    info!("ESP Actor: Source reported not running and read Ok(0). Signaling disconnect.");
                                    let _ = update_send_channel.send(AppUpdate::EspDisconnected).await;
                                    break;
                                }
                                tokio::time::sleep(Duration::from_millis(100)).await;
                            }
                            Ok(n) => {
                                let raw_csi_payload = read_buffer[..n].to_vec();
                                let data_msg = DataMsg::RawFrame {
                                    ts: Local::now().timestamp_micros() as f64 / 1_000_000.0,
                                    bytes: raw_csi_payload,
                                    source_type: SourceType::ESP32,
                                };
                                match esp_adapter.produce(data_msg).await {
                                    Ok(Some(DataMsg::CsiFrame { csi })) => {
                                        match update_send_channel.try_send(AppUpdate::Csi(csi)) {
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
                                        update_send_channel.try_send(AppUpdate::Error(format!("Adapter Parse Fail: {e}")));
                                    }
                                }
                            }
                            Err(e @ DataSourceError::NotConnected(_)) | Err(e @ DataSourceError::Io(_)) if !esp.is_running.load(AtomicOrdering::Relaxed) => {
                                info!("ESP Actor: Source disconnected (Error: {e}), signaling UI.");
                                update_send_channel.send(AppUpdate::EspDisconnected).await;
                            }
                            Err(e) => {
                                warn!("ESP Actor: Error reading from ESP32 source: {e:?}");
                                if update_send_channel.send(AppUpdate::Error(format!("ESP Read Fail: {e}"))).await.is_err() {
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
                .send(AppUpdate::Status("DISCONNECTED (Actor Stopped)".to_string()))
                .await;
            info!("ESP actor task for port {:?} stopped.", esp.port);
        })
    }

    // Configures the global logger such that all logs get routed to our custom TuiLogger struct
    // which sends them over a channel to a log handler task
    pub fn init_logger(log_level_filter: LevelFilter, sender: Sender<LogEntry>) -> Result<(), SetLoggerError> {
        let logger = TuiLogger {
            log_sender: sender,
            level: log_level_filter.to_level().unwrap_or(log::Level::Error),
        };

        log::set_boxed_logger(Box::new(logger))?;
        log::set_max_level(log_level_filter);
        info!("Initiated logger");
        Ok(())
    }

    // Processes all incoming logs by routing them through a channel connected to the TUI, this allows for complete flexibility in where to display logs
    pub async fn log_handler_task(
        mut log_recv_channel: Receiver<LogEntry>,
        update_send_channel: Sender<AppUpdate>,
        shutdown_signal: Arc<AtomicBool>,
    ) -> JoinHandle<()> {
        tokio::spawn(async move {
            info!("Log processor task started.");
            loop {
                if shutdown_signal.load(AtomicOrdering::Relaxed) {
                    break;
                }
                tokio::select! {
                  log = log_recv_channel.recv() => {
                    match log {
                      Some(log_msg) => {
                        if update_send_channel.send(AppUpdate::Log(log_msg)).await.is_err() {
                            debug!("Log processor: AppUpdate channel closed.");
                            break;
                        }
                      }
                      None => {
                          info!("Log processor: Log entry channel disconnected.");
                          break;
                      }
                    }
                  }
                  _ = sleep(Duration::from_millis(10)) => {
                  }
                }
            }
            info!("Log processor task stopped.");
        })
    }
}

pub fn setup_terminal() -> Result<Terminal<CrosstermBackend<io::Stdout>>, Box<dyn Error>> {
    enable_raw_mode()?;
    let mut stdout = stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    Terminal::new(backend).map_err(Into::into)
}

pub fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<(), Box<dyn Error>> {
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
}
