//! Robust CLI tool for ESP32 CSI monitoring.

use chrono::{DateTime, Local};
use crossbeam_channel::{Receiver as CrossbeamReceiver, Sender as CrossbeamSender};
use crossterm::event::{Event as CEvent, EventStream, KeyCode, KeyEvent, KeyModifiers};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::execute;
use futures::StreamExt;
use log::{debug, error, info, warn, Level, LevelFilter, Metadata, Record, SetLoggerError};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span, Text};
// MODIFICATION: Added Wrap to imports
use ratatui::widgets::{Block, Borders, Cell, List, ListItem, Paragraph, Row, Table, Wrap};
use ratatui::{Frame, Terminal};
use std::collections::VecDeque;
use std::env;
use std::error::Error;
use std::io;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering as AtomicOrdering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

// Corrected and added imports
use lib::adapters::CsiDataAdapter; // For ESP32Adapter's trait methods
use lib::adapters::esp32::ESP32Adapter;
use lib::csi_types::CsiData;
use lib::errors::{ControllerError, DataSourceError};
use lib::network::rpc_message::{DataMsg, SourceType};
use lib::sources::controllers::esp32_controller::{
    Bandwidth as EspBandwidth, CsiType as EspCsiType, Esp32ControllerParams, // Esp32Command not directly used here
    Esp32DeviceConfigPayload, OperationMode as EspOperationMode,
    SecondaryChannel as EspSecondaryChannel, CustomFrameParams, // MacFilterPair not directly used here
};
use lib::sources::controllers::Controller; // Import the Controller trait
use lib::sources::esp32::{Esp32Source, Esp32SourceConfig};
use lib::sources::DataSourceT;
use crate::cli::EspToolSubcommandArgs;

const LOG_BUFFER_CAPACITY: usize = 200;
const CSI_DATA_BUFFER_CAPACITY: usize = 1000;
const UI_REFRESH_INTERVAL_MS: u64 = 20;
const ACTOR_CHANNEL_CAPACITY: usize = 10;

#[derive(Clone, Copy, PartialEq, Debug)]
enum UiMode {
    Csi,
    Spam,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum SpamConfigField {
    SrcMacOctet(usize),
    DstMacOctet(usize),
    Reps,
    PauseMs,
}

impl SpamConfigField {
    fn next(self) -> Self {
        match self {
            SpamConfigField::SrcMacOctet(5) => SpamConfigField::DstMacOctet(0),
            SpamConfigField::SrcMacOctet(i) => SpamConfigField::SrcMacOctet(i + 1),
            SpamConfigField::DstMacOctet(5) => SpamConfigField::Reps,
            SpamConfigField::DstMacOctet(i) => SpamConfigField::DstMacOctet(i + 1),
            SpamConfigField::Reps => SpamConfigField::PauseMs,
            SpamConfigField::PauseMs => SpamConfigField::SrcMacOctet(0),
        }
    }

    fn prev(self) -> Self {
        match self {
            SpamConfigField::SrcMacOctet(0) => SpamConfigField::PauseMs,
            SpamConfigField::SrcMacOctet(i) => SpamConfigField::SrcMacOctet(i - 1),
            SpamConfigField::DstMacOctet(0) => SpamConfigField::SrcMacOctet(5),
            SpamConfigField::DstMacOctet(i) => SpamConfigField::DstMacOctet(i - 1),
            SpamConfigField::Reps => SpamConfigField::DstMacOctet(5),
            SpamConfigField::PauseMs => SpamConfigField::Reps,
        }
    }
}

#[derive(Clone, Debug)]
struct EspCurrentConfig {
    channel: u8,
    device_config: Esp32DeviceConfigPayload,
}

impl Default for EspCurrentConfig {
    fn default() -> Self {
        Self {
            channel: 1,
            device_config: Esp32DeviceConfigPayload {
                mode: EspOperationMode::Receive,
                bandwidth: EspBandwidth::Twenty,
                secondary_channel: EspSecondaryChannel::None,
                csi_type: EspCsiType::HighThroughputLTF,
                manual_scale: 0,
            },
        }
    }
}

#[derive(Clone, Debug)]
struct SpamSettings {
    src_mac: [u8; 6],
    dst_mac: [u8; 6],
    n_reps: i32,
    pause_ms: i32,
}

impl Default for SpamSettings {
    fn default() -> Self {
        Self {
            src_mac: [0x12, 0x34, 0x56, 0x78, 0x9A, 0xBC],
            dst_mac: [0xB4, 0x82, 0xC5, 0x58, 0xA1, 0xC0],
            n_reps: 1000,
            pause_ms: 100,
        }
    }
}

struct AppState {
    ui_mode: UiMode,
    esp_config: EspCurrentConfig,
    csi_data: VecDeque<CsiData>,
    connection_status: String,
    last_error: Option<String>,
    log_messages: VecDeque<LogEntry>,
    spam_settings: SpamSettings,
    is_editing_spam_config: bool,
    current_editing_field: SpamConfigField,
    is_continuous_spam_active: bool,
    spam_input_buffer: String,
    current_field_has_error: bool,
    should_quit: bool,
    // Fields to store the state *before* an optimistic update due to a command
    previous_esp_config_for_revert: Option<EspCurrentConfig>,
    previous_is_continuous_spam_active_for_revert: Option<bool>,
}

impl AppState {
    fn new() -> Self {
        Self {
            ui_mode: UiMode::Csi,
            esp_config: EspCurrentConfig::default(),
            csi_data: VecDeque::with_capacity(CSI_DATA_BUFFER_CAPACITY),
            connection_status: "INITIALIZING...".into(),
            last_error: None,
            log_messages: VecDeque::with_capacity(LOG_BUFFER_CAPACITY),
            spam_settings: SpamSettings::default(),
            is_editing_spam_config: false,
            current_editing_field: SpamConfigField::SrcMacOctet(0),
            spam_input_buffer: String::new(),
            current_field_has_error: false,
            is_continuous_spam_active: false,
            should_quit: false,
            previous_esp_config_for_revert: None,
            previous_is_continuous_spam_active_for_revert: None,
        }
    }

    fn add_log_message(&mut self, entry: LogEntry) {
        if self.log_messages.len() >= LOG_BUFFER_CAPACITY {
            self.log_messages.pop_front();
        }
        self.log_messages.push_back(entry);
    }

    fn add_csi_data(&mut self, data: CsiData) {
        if self.csi_data.len() >= CSI_DATA_BUFFER_CAPACITY {
            self.csi_data.pop_front();
        }
        self.csi_data.push_back(data);
    }

    fn apply_spam_value_change(&mut self, increment: bool) {
        if !self.is_editing_spam_config { return; }
        let val_change_u8: u8 = if increment { 1 } else { 255 }; // u8::MAX for wrapping_sub(1)
        let val_change_i32: i32 = if increment { 1 } else { -1 };

        match self.current_editing_field {
            SpamConfigField::SrcMacOctet(i) => {
                self.spam_settings.src_mac[i] = self.spam_settings.src_mac[i].wrapping_add(val_change_u8);
            }
            SpamConfigField::DstMacOctet(i) => {
                self.spam_settings.dst_mac[i] = self.spam_settings.dst_mac[i].wrapping_add(val_change_u8);
            }
            SpamConfigField::Reps => {
                self.spam_settings.n_reps = (self.spam_settings.n_reps + val_change_i32).max(0);
            }
            SpamConfigField::PauseMs => {
                self.spam_settings.pause_ms = (self.spam_settings.pause_ms + val_change_i32).max(0);
            }
        }
    }
}

#[derive(Debug)] // Added derive Debug
pub struct LogEntry {
    pub timestamp: DateTime<Local>,
    pub level: log::Level,
    pub message: String,
}

struct TuiCrossbeamLogger {
    log_sender: CrossbeamSender<LogEntry>,
    level: Level,
}

impl log::Log for TuiCrossbeamLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= self.level
    }

    fn log(&self, record: &Record) {
        if self.enabled(record.metadata()) {
            let log_entry = LogEntry {
                timestamp: Local::now(),
                level: record.level(),
                message: format!("{}", record.args()),
            };
            if self.log_sender.try_send(log_entry).is_err() {
                eprintln!(
                    "[TUI_LOG_FALLBACK] {}: {} [{}] - {}",
                    Local::now().format("%Y-%m-%d %H:%M:%S%.3f"),
                    std::thread::current().name().unwrap_or("unknown_thread"),
                    record.level(),
                    record.args()
                );
            }
        }
    }
    fn flush(&self) {}
}

fn init_tui_logger(
    log_level_filter: LevelFilter,
    sender: CrossbeamSender<LogEntry>,
) -> Result<(), SetLoggerError> {
    let logger = TuiCrossbeamLogger {
        log_sender: sender,
        level: log_level_filter.to_level().unwrap_or(log::Level::Error),
    };
    log::set_boxed_logger(Box::new(logger))?;
    log::set_max_level(log_level_filter);
    Ok(())
}

#[derive(Debug)]
enum AppUpdate {
    Log(LogEntry),
    Csi(CsiData),
    Status(String),
    Error(String),
    EspDisconnected,
    ControllerUpdateSuccess,
    ControllerUpdateError(String),
}

// Renamed function to match call site in main.rs
pub async fn run_esp_test_subcommand(args: EspToolSubcommandArgs) -> Result<(), Box<dyn Error>> {
    let (log_entry_tx, log_entry_rx_cb) = crossbeam_channel::bounded(LOG_BUFFER_CAPACITY);
    let log_level = env::var("RUST_LOG")
        .ok()
        .and_then(|s| s.parse::<LevelFilter>().ok())
        .unwrap_or(LevelFilter::Info);

    init_tui_logger(log_level, log_entry_tx.clone()).map_err(|e| {
        eprintln!("FATAL: Failed to initialize TUI logger: {e}");
        e
    })?;

    info!("ESP CLI starting...");
    if args.port.is_empty() {
        error!("Serial port argument was empty.");
        return Err("Serial port argument missing.".into());
    }

    let mut terminal = setup_terminal().map_err(|e| {
        error!("Failed to setup terminal: {e}");
        e
    })?;

    let shutdown_signal = Arc::new(AtomicBool::new(false));
    let (command_tx, command_rx) = mpsc::channel::<Esp32ControllerParams>(ACTOR_CHANNEL_CAPACITY);
    let (update_tx, mut update_rx) = mpsc::channel::<AppUpdate>(ACTOR_CHANNEL_CAPACITY * 2);

    let esp_source_config = Esp32SourceConfig {
        port_name: args.port.clone(),
        baud_rate: 3_000_000,
        csi_buffer_size: 200, 
        ack_timeout_ms: 2500, 
    };
    let initial_esp_config_for_actor = AppState::new().esp_config;
   
    let esp_actor_handle = tokio::spawn(esp_actor_task(
        esp_source_config,
        initial_esp_config_for_actor,
        command_rx,
        update_tx.clone(),
        shutdown_signal.clone(),
    ));

    let update_tx_log = update_tx.clone();
    let shutdown_signal_log = shutdown_signal.clone();
    let log_processor_handle: JoinHandle<()> = tokio::spawn(async move {
        info!("Log processor task started.");
        loop {
            if shutdown_signal_log.load(AtomicOrdering::Relaxed) {
                break;
            }
            match log_entry_rx_cb.recv_timeout(Duration::from_millis(100)) {
                Ok(log_msg) => {
                    if update_tx_log.send(AppUpdate::Log(log_msg)).await.is_err() {
                        debug!("Log processor: AppUpdate channel closed.");
                        break;
                    }
                }
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                    info!("Log processor: Log entry channel disconnected.");
                    break;
                }
            }
        }
        info!("Log processor task stopped.");
    });

    let mut app_state = AppState::new();
    let mut event_stream = EventStream::new();

    'main_loop: loop {
        terminal.draw(|f| ui(f, &app_state))?;

        tokio::select! {
            biased;
            _ = tokio::signal::ctrl_c() => {
                info!("Ctrl+C received. Initiating shutdown...");
                shutdown_signal.store(true, AtomicOrdering::SeqCst);
                app_state.should_quit = true;
            }
            maybe_event = event_stream.next() => {
                match maybe_event {
                    Some(Ok(CEvent::Key(key_event))) => {
                        if handle_keyboard_event(key_event, &mut app_state, &command_tx, &update_tx).await {
                            shutdown_signal.store(true, AtomicOrdering::SeqCst);
                        }
                    }
                    Some(Err(e)) => {
                        if update_tx.send(AppUpdate::Error(format!("Input event error: {e}"))).await.is_err() {
                            error!("Failed to send input error to UI: channel closed.");
                            app_state.should_quit = true;
                            shutdown_signal.store(true, AtomicOrdering::SeqCst);
                        }
                    }
                    None => {
                        info!("Crossterm event stream ended. Shutting down.");
                        app_state.should_quit = true;
                        shutdown_signal.store(true, AtomicOrdering::SeqCst);
                    }
                    _ => {}
                }
            }
            Some(update) = update_rx.recv() => {
                match update {
                    AppUpdate::Log(log_entry) => app_state.add_log_message(log_entry),
                    AppUpdate::Csi(csi_data) => app_state.add_csi_data(csi_data),
                    AppUpdate::Status(status) => app_state.connection_status = status,
                    AppUpdate::Error(err_msg) => app_state.last_error = Some(err_msg),
                    AppUpdate::EspDisconnected => {
                        app_state.connection_status = "DISCONNECTED (ESP)".to_string();
                    }
                    AppUpdate::ControllerUpdateSuccess => {
                        app_state.last_error = None;
                        app_state.previous_esp_config_for_revert = None;
                        app_state.previous_is_continuous_spam_active_for_revert = None;
                        // info!("Controller command successful, committed optimistic UI updates.");
                    }
                    AppUpdate::ControllerUpdateError(err_msg) => {
                        let error_message_for_log = err_msg.clone();
                        app_state.last_error = Some(err_msg);
                        if let Some(old_config) = app_state.previous_esp_config_for_revert.take() {
                            app_state.esp_config = old_config;
                            info!("ESP config reverted due to actor error: {}", error_message_for_log);
                        }
                        if let Some(old_spam_active) = app_state.previous_is_continuous_spam_active_for_revert.take() {
                            app_state.is_continuous_spam_active = old_spam_active;
                            info!("Continuous spam state reverted due to actor error: {}", error_message_for_log);
                        }
                    }
                }
            }
            _ = tokio::time::sleep(Duration::from_millis(UI_REFRESH_INTERVAL_MS)) => {}
        }

        if app_state.should_quit {
            info!("Shutdown flag set. Exiting main UI loop.");
            break 'main_loop;
        }
    }

    info!("Main UI loop exited. Beginning CLI shutdown sequence...");
    shutdown_signal.store(true, AtomicOrdering::SeqCst);

    info!("Waiting for ESP actor task to complete (max 5s)...");
    match tokio::time::timeout(Duration::from_secs(5), esp_actor_handle).await {
        Ok(Ok(_)) => info!("ESP actor task finished gracefully."),
        Ok(Err(e)) => error!("ESP actor task panicked or returned an error: {e:?}"),
        Err(_) => warn!("ESP actor task timed out during shutdown."),
    }

    drop(log_entry_tx); // Close the sender to allow the log_processor_handle to join
    info!("Waiting for log processor task to complete (max 3s)...");
      match tokio::time::timeout(Duration::from_secs(3), log_processor_handle).await {
        Ok(Ok(_)) => info!("Log processor task finished gracefully."),
        Ok(Err(e)) => error!("Log processor task panicked: {e:?}"),
        Err(_) => warn!("Log processor task timed out during shutdown."),
    }

    if let Err(e) = restore_terminal(&mut terminal) {
        eprintln!("[CLI_ERROR] Failed to restore terminal: {e}");
    } else {
        info!("Terminal restored by ESP CLI.");
    }

    info!("ESP CLI finished.");
    Ok(())
}

async fn esp_actor_task(
    source_config: Esp32SourceConfig,
    initial_esp_config: EspCurrentConfig,
    mut command_rx: mpsc::Receiver<Esp32ControllerParams>,
    update_tx: mpsc::Sender<AppUpdate>,
    shutdown_signal: Arc<AtomicBool>,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    info!("ESP actor task started for port {}.", source_config.port_name);
    let mut esp_source = match Esp32Source::new(source_config.clone()) { 
        Ok(s) => s,
        Err(e) => {
            error!("ESP Actor: Failed to initialize Esp32Source: {e}");
            let _ = update_tx.send(AppUpdate::Error(format!("ESP Init Fail: {e}"))).await;
            let _ = update_tx.send(AppUpdate::Status("DISCONNECTED (Init Fail)".to_string())).await;
            return Err(Box::new(e));
        }
    };

    if let Err(e) = esp_source.start().await {
        error!("ESP Actor: Failed to start Esp32Source: {e}");
        let _ = update_tx.send(AppUpdate::Error(format!("ESP Start Fail: {e}"))).await;
        let _ = update_tx.send(AppUpdate::Status("DISCONNECTED (Start Fail)".to_string())).await;
        return Err(Box::new(e));
    }
    let _ = update_tx.send(AppUpdate::Status("CONNECTED (Source Started)".to_string())).await;

    let mut initial_params = Esp32ControllerParams::default();
    initial_params.set_channel = Some(initial_esp_config.channel);
    initial_params.apply_device_config = Some(initial_esp_config.device_config.clone());
    initial_params.clear_all_mac_filters = Some(true);
    initial_params.synchronize_time = Some(true);
    if initial_esp_config.device_config.mode == EspOperationMode::Receive {
        initial_params.control_acquisition = Some(true);
    } else {
        initial_params.control_acquisition = Some(false);
        initial_params.control_wifi_transmit = Some(false);
    }

    info!("ESP Actor: Applying initial ESP32 configuration...");
    if let Err(e) = initial_params.apply(&mut esp_source).await { 
        warn!("ESP Actor: Failed to apply initial ESP32 configuration: {e}. Continuing with defaults or potentially unstable state.");
        let _ = update_tx.send(AppUpdate::ControllerUpdateError(format!("Initial ESP config failed: {e}"))).await;
    } else {
        info!("ESP Actor: Initial ESP32 configuration applied successfully.");
          let _ = update_tx.send(AppUpdate::ControllerUpdateSuccess).await;
    }

    let mut csi_read_buffer = vec![0u8; 4096];
    let mut esp_adapter = ESP32Adapter::new(false);

    loop {
        tokio::select! {
            biased;
            _ = tokio::time::sleep(Duration::from_millis(1)), if shutdown_signal.load(AtomicOrdering::Relaxed) => {
                info!("ESP Actor: Shutdown signal received.");
                break;
            }
            Some(params) = command_rx.recv() => {
                info!("ESP Actor: Received command: {:?}", params);
                match params.apply(&mut esp_source).await { 
                    Ok(_) => {
                        debug!("ESP Actor: Command applied successfully.");
                        if update_tx.send(AppUpdate::ControllerUpdateSuccess).await.is_err() {
                            break;
                        }
                    }
                    Err(e) => {
                        error!("ESP Actor: Failed to apply command: {}", e);
                        if update_tx.send(AppUpdate::ControllerUpdateError(e.to_string())).await.is_err() {
                            break;
                        }
                    }
                }
            }
            read_result = esp_source.read(&mut csi_read_buffer) => {
                match read_result {
                    Ok(0) => {
                        if !esp_source.is_source_running() {
                            info!("ESP Actor: Source reported not running and read Ok(0). Signaling disconnect.");
                            let _ = update_tx.send(AppUpdate::EspDisconnected).await;
                        }
                        tokio::time::sleep(Duration::from_millis(5)).await;
                    }
                    Ok(n) => {
                        let raw_csi_payload = csi_read_buffer[..n].to_vec();
                        let data_msg = DataMsg::RawFrame {
                            ts: Local::now().timestamp_micros() as f64 / 1_000_000.0,
                            bytes: raw_csi_payload,
                            source_type: SourceType::ESP32,
                        };
                        match esp_adapter.produce(data_msg).await {
                            Ok(Some(DataMsg::CsiFrame { csi })) => {
                                match update_tx.try_send(AppUpdate::Csi(csi)) {
                                    Ok(_) => { /* Successfully enqueued for UI */ }
                                    Err(mpsc::error::TrySendError::Full(_csi_update_dropped)) => {
                                        static LOG_DROP_COUNTER: AtomicUsize = AtomicUsize::new(0);
                                        let current_drop_count = LOG_DROP_COUNTER.fetch_add(1, AtomicOrdering::Relaxed);
                                        if current_drop_count % 10000 == 0 { 
                                            warn!("ESP Actor: UI update channel full, dropping CSI data ({} drops so far). UI might be lagging.", current_drop_count + 1);
                                        }
                                        debug!("ESP Actor: UI update channel full, dropping CSI data.");
                                    }
                                    Err(mpsc::error::TrySendError::Closed(_)) => {
                                        info!("ESP Actor: UI update channel closed. Terminating actor.");
                                        break; 
                                    }
                                }
                            }
                            Ok(None) => { /* Adapter needs more data or ignored packet */ }
                            Err(e) => {
                                warn!("ESP Actor: ESP32Adapter failed to parse CSI: {:?}", e);
                                 let _ = update_tx.try_send(AppUpdate::Error(format!("Adapter Parse Fail: {e}")));
                            }
                              Ok(Some(DataMsg::RawFrame { .. })) => { /* Should not happen from this adapter */ }
                        }
                    }
                    Err(e @ DataSourceError::NotConnected(_)) | Err(e @ DataSourceError::Io(_)) if !esp_source.is_source_running() => {
                        info!("ESP Actor: Source disconnected (Error: {}), signaling UI.", e);
                        let _ = update_tx.send(AppUpdate::EspDisconnected).await;
                    }
                    Err(e) => {
                        warn!("ESP Actor: Error reading from ESP32 source: {:?}", e);
                          if update_tx.send(AppUpdate::Error(format!("ESP Read Fail: {e}"))).await.is_err() {
                              break;
                          }
                        tokio::time::sleep(Duration::from_secs(1)).await; // Prevent rapid error spamming
                    }
                }
            }
        }
    }

    info!("ESP actor task: initiating source stop...");
    if let Err(e) = esp_source.stop().await {
        error!("ESP actor task: Error stopping ESP32 source: {e}");
    } else {
        info!("ESP actor task: ESP32 source stopped successfully.");
    }
    let _ = update_tx.send(AppUpdate::Status("DISCONNECTED (Actor Stopped)".to_string())).await;
    info!("ESP actor task for port {} stopped.", esp_source.port_name());
    Ok(())
}

fn apply_spam_input_buffer(app_state: &mut AppState) {
    let buffer_content = app_state.spam_input_buffer.trim();
    app_state.current_field_has_error = false; 

    if buffer_content.is_empty() {
        match app_state.current_editing_field {
            SpamConfigField::SrcMacOctet(_) | SpamConfigField::DstMacOctet(_) => {
                app_state.last_error = Some("MAC octet cannot be empty. Reverted.".to_string());
                app_state.current_field_has_error = true;
                return; 
            }
            SpamConfigField::Reps => app_state.spam_settings.n_reps = 0,
            SpamConfigField::PauseMs => app_state.spam_settings.pause_ms = 0,
        }
        app_state.last_error = None; 
        return;
    }

    let parse_result: Result<(), String> = match app_state.current_editing_field {
        SpamConfigField::SrcMacOctet(i) => {
            u8::from_str_radix(buffer_content, 16)
                .map_err(|e| format!("Invalid SrcMAC Octet value: '{}' ({})", buffer_content, e))
                .map(|val| app_state.spam_settings.src_mac[i] = val)
        }
        SpamConfigField::DstMacOctet(i) => {
            u8::from_str_radix(buffer_content, 16)
                .map_err(|e| format!("Invalid DstMAC Octet value: '{}' ({})", buffer_content, e))
                .map(|val| app_state.spam_settings.dst_mac[i] = val)
        }
        SpamConfigField::Reps => {
            buffer_content.parse::<i32>()
                .map_err(|e| format!("Invalid Reps value: '{}' ({})", buffer_content, e))
                .and_then(|val| {
                    if val >= 0 {
                        app_state.spam_settings.n_reps = val;
                        Ok(())
                    } else {
                        Err("Reps must be non-negative".to_string())
                    }
                })
        }
        SpamConfigField::PauseMs => {
            buffer_content.parse::<i32>()
                .map_err(|e| format!("Invalid PauseMs value: '{}' ({})", buffer_content, e))
                .and_then(|val| {
                    if val >= 0 {
                        app_state.spam_settings.pause_ms = val;
                        Ok(())
                    } else {
                        Err("PauseMs must be non-negative".to_string())
                    }
                })
        }
    };

    if let Err(e) = parse_result {
        app_state.last_error = Some(e);
        app_state.current_field_has_error = true;
    } else {
        app_state.last_error = None; 
    }
}

fn prepare_spam_input_buffer_for_current_field(app_state: &mut AppState) {
    app_state.spam_input_buffer.clear();
    app_state.current_field_has_error = false; 
    match app_state.current_editing_field {
        SpamConfigField::SrcMacOctet(i) => {
            app_state.spam_input_buffer = format!("{:02X}", app_state.spam_settings.src_mac[i]);
        }
        SpamConfigField::DstMacOctet(i) => {
            app_state.spam_input_buffer = format!("{:02X}", app_state.spam_settings.dst_mac[i]);
        }
        SpamConfigField::Reps => {
            app_state.spam_input_buffer = app_state.spam_settings.n_reps.to_string();
        }
        SpamConfigField::PauseMs => {
            app_state.spam_input_buffer = app_state.spam_settings.pause_ms.to_string();
        }
    }
}

async fn handle_keyboard_event(
    key_event: KeyEvent,
    app_state: &mut AppState,
    command_tx: &mpsc::Sender<Esp32ControllerParams>,
    update_tx: &mpsc::Sender<AppUpdate>,
) -> bool {
    if key_event.code != KeyCode::Char('r') && key_event.code != KeyCode::Char('R') {
        if app_state.last_error.as_deref().unwrap_or("").contains("Press 'R' to dismiss") {
            // Do nothing, keep error until R is pressed
        } else if !app_state.last_error.as_deref().unwrap_or("").starts_with("Cmd Send Fail:") && // Don't clear cmd send fail errors this way
                  !app_state.last_error.as_deref().unwrap_or("").contains("actor error:") // Don't clear actor errors this way
        {
            app_state.last_error = None;
        }
    }


    if app_state.is_editing_spam_config {
        match key_event.code {
            KeyCode::Char(ch) => {
                match app_state.current_editing_field {
                    SpamConfigField::SrcMacOctet(_) | SpamConfigField::DstMacOctet(_) => {
                        if app_state.spam_input_buffer.len() < 2 && ch.is_ascii_hexdigit() {
                            app_state.spam_input_buffer.push(ch.to_ascii_uppercase());
                            app_state.current_field_has_error = false; 
                        }
                    }
                    SpamConfigField::Reps | SpamConfigField::PauseMs => {
                        if app_state.spam_input_buffer.len() < 7 && ch.is_ascii_digit() {
                            app_state.spam_input_buffer.push(ch);
                            app_state.current_field_has_error = false; 
                        }
                    }
                }
            }
            KeyCode::Backspace => {
                app_state.spam_input_buffer.pop();
                app_state.current_field_has_error = false; 
            }
            KeyCode::Enter => {
                apply_spam_input_buffer(app_state);
                if !app_state.current_field_has_error { 
                    app_state.current_editing_field = app_state.current_editing_field.next();
                    prepare_spam_input_buffer_for_current_field(app_state);
                }
            }
            KeyCode::Tab => {
                apply_spam_input_buffer(app_state);
                app_state.current_editing_field = app_state.current_editing_field.next();
                prepare_spam_input_buffer_for_current_field(app_state);
            }
            KeyCode::BackTab => { 
                apply_spam_input_buffer(app_state);
                app_state.current_editing_field = app_state.current_editing_field.prev();
                prepare_spam_input_buffer_for_current_field(app_state);
            }
            KeyCode::Esc => {
                app_state.spam_input_buffer.clear();
                app_state.is_editing_spam_config = false;
                app_state.last_error = None; 
                app_state.current_field_has_error = false;
                info!("Exited spam config editing mode.");
            }
            KeyCode::Up => app_state.apply_spam_value_change(true), // Modify value directly
            KeyCode::Down => app_state.apply_spam_value_change(false), // Modify value directly
            KeyCode::Left | KeyCode::Right => {} 
            _ => {} 
        }
        return false; 
    }

    let mut params = Esp32ControllerParams::default();
    let mut action_taken = false;

    match key_event.code {
        KeyCode::Char('q') | KeyCode::Char('Q') => {
            info!("'q' pressed, initiating shutdown.");
            let mut final_params = Esp32ControllerParams::default();

            final_params.control_acquisition = Some(false);
            info!("Shutdown: Requesting to PAUSE CSI acquisition.");

            if app_state.esp_config.device_config.mode == EspOperationMode::Transmit {
                final_params.control_wifi_transmit = Some(false);
                info!("Shutdown: Requesting to PAUSE WiFi transmit task (was in Transmit mode).");
            } else {
                info!("Shutdown: Skipping PauseWifiTransmit command (was in Receive mode, task likely not active).");
            }
           
            // Simplified check: if any relevant field is Some
            let needs_final_command = final_params.control_acquisition.is_some() ||
                                      final_params.control_wifi_transmit.is_some();


            if needs_final_command { 
                 if command_tx.try_send(final_params).is_err() {
                    warn!("Failed to send final 'idle' command(s) to ESP actor on quit.");
                }
            } else {
                info!("Shutdown: No specific ESP32 cleanup commands deemed necessary for current state beyond what actor stop does.");
            }

            app_state.should_quit = true;
            return true;
        }
        KeyCode::Char('m') | KeyCode::Char('M') => {
            app_state.previous_esp_config_for_revert = Some(app_state.esp_config.clone());
            action_taken = true;
            let new_ui_mode = match app_state.ui_mode {
                UiMode::Csi => UiMode::Spam,
                UiMode::Spam => UiMode::Csi,
            };
            app_state.ui_mode = new_ui_mode;
            app_state.esp_config.device_config.mode = match new_ui_mode {
                UiMode::Csi => EspOperationMode::Receive,
                UiMode::Spam => EspOperationMode::Transmit,
            };
            params.apply_device_config = Some(app_state.esp_config.device_config.clone());
            // When switching modes, ensure continuous spam is off and acquisition is handled by controller logic.
            if new_ui_mode == UiMode::Spam { // Switching to Spam
                params.control_acquisition = Some(false); // Explicitly pause acquisition
                // If continuous spam was on, turn it off visually and command-wise
                if app_state.is_continuous_spam_active {
                     app_state.previous_is_continuous_spam_active_for_revert = Some(app_state.is_continuous_spam_active);
                     app_state.is_continuous_spam_active = false;
                     params.control_wifi_transmit = Some(false); // Command to turn off continuous spam
                }
            } else { // Switching to CSI
                params.control_acquisition = Some(true); // Explicitly resume acquisition
                params.control_wifi_transmit = Some(false); // Ensure general transmit is off
                app_state.is_continuous_spam_active = false; // Visually turn off
            }
        }
        KeyCode::Char('e') | KeyCode::Char('E') => {
            if app_state.ui_mode == UiMode::Spam {
                app_state.is_editing_spam_config = true;
                app_state.current_editing_field = SpamConfigField::SrcMacOctet(0); 
                prepare_spam_input_buffer_for_current_field(app_state); 
                app_state.last_error = None; 
                info!("Entered spam config editing. Tab/Enter/Arrows to navigate/modify. Esc to exit.");
            } else {
                 let _ = update_tx.send(AppUpdate::Error("Edit ('e') for Spam mode only. Switch mode with 'm'.".to_string())).await;
            }
        }
        KeyCode::Char('s') | KeyCode::Char('S') => { 
            if app_state.ui_mode == UiMode::Spam {
                if app_state.esp_config.device_config.mode != EspOperationMode::Transmit {
                    let _ = update_tx.send(AppUpdate::Error("Burst spam ('s') requires ESP Transmit mode. Use 'm'.".to_string())).await;
                } else {
                    action_taken = true;
                    params.transmit_custom_frame = Some(CustomFrameParams {
                        src_mac: app_state.spam_settings.src_mac,
                        dst_mac: app_state.spam_settings.dst_mac,
                        n_reps: app_state.spam_settings.n_reps,
                        pause_ms: app_state.spam_settings.pause_ms,
                    });
                    info!("Requesting to send custom frame burst.");
                }
            } else {
                let _ = update_tx.send(AppUpdate::Error("Send burst spam ('s') for Spam mode only. Use 'm'.".to_string())).await;
            }
        }
        KeyCode::Char('t') | KeyCode::Char('T') => { 
            if app_state.ui_mode == UiMode::Spam {
                if app_state.esp_config.device_config.mode != EspOperationMode::Transmit {
                     let _ = update_tx.send(AppUpdate::Error("Continuous spam ('t') requires ESP Transmit mode. Use 'm'.".to_string())).await;
                } else {
                    app_state.previous_is_continuous_spam_active_for_revert = Some(app_state.is_continuous_spam_active);
                    action_taken = true;
                    if app_state.is_continuous_spam_active {
                        params.control_wifi_transmit = Some(false);
                        app_state.is_continuous_spam_active = false; 
                        info!("Requesting to PAUSE continuous WiFi transmit.");
                    } else {
                        params.control_wifi_transmit = Some(true);
                        app_state.is_continuous_spam_active = true; 
                        info!("Requesting to RESUME continuous WiFi transmit.");
                    }
                }
            } else {
                let _ = update_tx.send(AppUpdate::Error("Toggle continuous spam ('t') for Spam mode only. Use 'm'.".to_string())).await;
            }
        }
        KeyCode::Char('c') | KeyCode::Char('C') => {
            app_state.previous_esp_config_for_revert = Some(app_state.esp_config.clone());
            action_taken = true;
            app_state.esp_config.channel = (app_state.esp_config.channel % 11) + 1; // Cycle 1-11
            params.set_channel = Some(app_state.esp_config.channel);
        }
        KeyCode::Char('b') | KeyCode::Char('B') => {
            app_state.previous_esp_config_for_revert = Some(app_state.esp_config.clone());
            action_taken = true;
            match app_state.esp_config.device_config.bandwidth {
                EspBandwidth::Twenty => {
                    app_state.esp_config.device_config.bandwidth = EspBandwidth::Forty;
                    app_state.esp_config.device_config.secondary_channel = EspSecondaryChannel::Above; 
                }
                EspBandwidth::Forty => {
                    app_state.esp_config.device_config.bandwidth = EspBandwidth::Twenty;
                    app_state.esp_config.device_config.secondary_channel = EspSecondaryChannel::None;
                }
            }
            params.apply_device_config = Some(app_state.esp_config.device_config.clone());
        }
        KeyCode::Char('l') | KeyCode::Char('L') => {
            app_state.previous_esp_config_for_revert = Some(app_state.esp_config.clone());
            action_taken = true;
            app_state.esp_config.device_config.csi_type = match app_state.esp_config.device_config.csi_type {
                EspCsiType::HighThroughputLTF => EspCsiType::LegacyLTF,
                EspCsiType::LegacyLTF => EspCsiType::HighThroughputLTF,
            };
            
            params.apply_device_config = Some(app_state.esp_config.device_config.clone());
        }
        KeyCode::Char('r') | KeyCode::Char('R') => {
            app_state.last_error = None;
        }
        KeyCode::Up => {
            app_state.csi_data.clear();
            info!("CSI data buffer cleared by user.");
        }
        KeyCode::Down => {
            action_taken = true;
            params.synchronize_time = Some(true);
        }
        _ => {}
    }

    if action_taken {
        let params_to_send = params.clone(); // Clone params for analysis if send fails & for sending
        if command_tx.try_send(params_to_send).is_err() {
            let _ = update_tx.send(AppUpdate::Error("Cmd Send Fail: Chan full. UI reverted.".to_string())).await;

            // Revert optimistic changes based on the params we *tried* to send
            if params.apply_device_config.is_some() || params.set_channel.is_some() {
                if let Some(old_config) = app_state.previous_esp_config_for_revert.take() {
                    app_state.esp_config = old_config;
                    info!("ESP config reverted due to command send failure (channel full).");
                }
            }
            // This specifically handles the 't' key's optimistic update of is_continuous_spam_active
            if params.control_wifi_transmit.is_some() { 
                 if let Some(old_spam_active) = app_state.previous_is_continuous_spam_active_for_revert.take() {
                    app_state.is_continuous_spam_active = old_spam_active;
                    info!("Continuous spam state reverted due to command send failure (channel full).");
                }
            }
        }
    }
    false 
}

fn setup_terminal() -> Result<Terminal<CrosstermBackend<io::Stdout>>, Box<dyn Error>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    Terminal::new(backend).map_err(Into::into)
}

fn restore_terminal(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
) -> Result<(), Box<dyn Error>> {
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
}

fn ui(f: &mut Frame, app_state: &AppState) {
    // --- Main Layout: Content Area and Global Footer ---
    let screen_chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1) // Margin around the whole screen
        .constraints([
            Constraint::Min(0),     // Content area (everything else)
            Constraint::Length(4), // Global Footer for Info/Errors
        ])
        .split(f.area());

    let content_area = screen_chunks[0];
    let global_footer_area = screen_chunks[1];

    // --- Split Content Area: Left Panel and Right Log Panel ---
    let content_horizontal_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(65), // Left panel width
            Constraint::Percentage(35), // Right panel (logs) width
        ])
        .split(content_area);

    let left_panel_area = content_horizontal_chunks[0];
    let log_panel_area = content_horizontal_chunks[1];

    // --- Calculate height for the Status Block ---
    const BASE_ESP_CONFIG_LINES: u16 = 6; // General ESP32 config lines
    const SPAM_DETAILS_LINES: u16 = 5;    // Lines for spam-specific configuration details

    let status_area_height = if app_state.ui_mode == UiMode::Spam {
        BASE_ESP_CONFIG_LINES + SPAM_DETAILS_LINES
    } else {
        BASE_ESP_CONFIG_LINES
    };

    // --- Vertical layout for the Left Panel (Status, CSI Table) ---
    let left_vertical_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(status_area_height), // Status block
            Constraint::Min(0),                      // CSI Data Table
        ])
        .split(left_panel_area);

    let status_area = left_vertical_chunks[0];
    let table_area = left_vertical_chunks[1];

    // --- Build Status Block Content ---
    let mode_str = match app_state.esp_config.device_config.mode { // Read from esp_config
        EspOperationMode::Receive => "CSI RX",
        EspOperationMode::Transmit => "WiFi SPAM TX",
    };
    let dev_conf = &app_state.esp_config.device_config;
    let bw_str = match dev_conf.bandwidth {
        EspBandwidth::Twenty => "20MHz",
        EspBandwidth::Forty => "40MHz",
    };
    let ltf_str = match dev_conf.csi_type { // This should correctly provide HT-LTF or L-LTF
        EspCsiType::HighThroughputLTF => "HT-LTF",
        EspCsiType::LegacyLTF => "L-LTF",
    };
    let sec_chan_str = match dev_conf.secondary_channel {
        EspSecondaryChannel::None => "None", // Should only be None if BW is 20MHz
        EspSecondaryChannel::Above => "HT40+",
        EspSecondaryChannel::Below => "HT40-",
    };
    let connection_style = match app_state.connection_status.as_str() {
        s if s.starts_with("CONNECTED") => Style::default().fg(Color::Green),
        s if s.starts_with("INITIALIZING") => Style::default().fg(Color::Yellow),
        _ => Style::default().fg(Color::Red),
    };

    let mut status_lines = vec![
        Line::from(vec![
            Span::raw("ESP32 Status: "),
            Span::styled(app_state.connection_status.clone(), connection_style),
            Span::raw(" | Mode: "),
            Span::styled(mode_str, Style::default().add_modifier(Modifier::BOLD)),
        ]),
    ];

    // WiFi Channel, Bandwidth, and Secondary Channel line
    let mut wifi_line_spans = vec![
        Span::raw("WiFi Channel: "), Span::styled(app_state.esp_config.channel.to_string(), Style::default().fg(Color::Yellow)),
        Span::raw(" | Bandwidth: "), Span::styled(bw_str, Style::default().fg(Color::Yellow)),
    ];
    if dev_conf.bandwidth == EspBandwidth::Forty { // Only show secondary channel info if BW is 40MHz
        wifi_line_spans.push(Span::raw(format!(" ({})", sec_chan_str)));
    }
    status_lines.push(Line::from(wifi_line_spans));

    // CSI Type and RSSI Scale line
    status_lines.push(Line::from(vec![
        Span::raw("CSI Type: "), Span::styled(ltf_str, Style::default().fg(Color::Yellow)), // ltf_str used here
        Span::raw(" | RSSI Scale: "), Span::styled(dev_conf.manual_scale.to_string(), Style::default().fg(Color::Yellow)),
    ]));

    // CSI Buffer line
    status_lines.push(Line::from(vec![
        Span::raw("CSI Data Rx Buffer: "), Span::styled(
            format!("{}/{}", app_state.csi_data.len(), CSI_DATA_BUFFER_CAPACITY),
            Style::default().fg(Color::Cyan)
        ),
    ]));


    if app_state.ui_mode == UiMode::Spam { // Check app_state.ui_mode for spam section
        let spam = &app_state.spam_settings;
        status_lines.push(Line::from(Span::styled("Spam Configuration:", Style::default().add_modifier(Modifier::UNDERLINED))));

        let get_field_style = |app_state: &AppState, _field_type: SpamConfigField, is_active: bool| {
            if is_active {
                if app_state.current_field_has_error {
                    Style::default().fg(Color::Black).bg(Color::LightRed)
                } else {
                    Style::default().fg(Color::Black).bg(Color::Cyan)
                }
            } else {
                Style::default()
            }
        };
       
        let mut src_mac_spans = vec![Span::raw("  Src MAC: ")];
        for i in 0..6 {
            let current_field_type = SpamConfigField::SrcMacOctet(i);
            let is_active_field = app_state.is_editing_spam_config && app_state.current_editing_field == current_field_type;
            let val_str = if is_active_field {
                app_state.spam_input_buffer.clone() + "_"
            } else {
                format!("{:02X}", spam.src_mac[i])
            };
            let style = get_field_style(app_state, current_field_type, is_active_field);
            src_mac_spans.push(Span::styled(val_str, style));
            if i < 5 { src_mac_spans.push(Span::raw(if is_active_field && app_state.spam_input_buffer.len() == 2 { "" } else { ":" }).style(Style::default())); }
        }
        status_lines.push(Line::from(src_mac_spans));

        let mut dst_mac_spans = vec![Span::raw("  Dst MAC: ")];
        for i in 0..6 {
            let current_field_type = SpamConfigField::DstMacOctet(i);
            let is_active_field = app_state.is_editing_spam_config && app_state.current_editing_field == current_field_type;
            let val_str = if is_active_field {
                app_state.spam_input_buffer.clone() + "_"
            } else {
                format!("{:02X}", spam.dst_mac[i])
            };
            let style = get_field_style(app_state, current_field_type, is_active_field);
            dst_mac_spans.push(Span::styled(val_str, style));
            if i < 5 { dst_mac_spans.push(Span::raw(if is_active_field && app_state.spam_input_buffer.len() == 2 { "" } else { ":" }).style(Style::default())); }
        }
        status_lines.push(Line::from(dst_mac_spans));
       
        let reps_field_type = SpamConfigField::Reps;
        let is_reps_active = app_state.is_editing_spam_config && app_state.current_editing_field == reps_field_type;
        let reps_str = if is_reps_active { app_state.spam_input_buffer.clone() + "_" } else { spam.n_reps.to_string() };
        let reps_style = get_field_style(app_state, reps_field_type, is_reps_active);

        let pause_field_type = SpamConfigField::PauseMs;
        let is_pause_active = app_state.is_editing_spam_config && app_state.current_editing_field == pause_field_type;
        let pause_str = if is_pause_active { app_state.spam_input_buffer.clone() + "_" } else { spam.pause_ms.to_string() };
        let pause_style = get_field_style(app_state, pause_field_type, is_pause_active);

        status_lines.push(Line::from(vec![
            Span::raw("  Reps: "), Span::styled(reps_str, reps_style),
            Span::raw(" | Pause: "), Span::styled(pause_str, pause_style), Span::raw("ms"),
        ]));

        status_lines.push(Line::from(Span::styled(
            format!("  Continuous Spam ('t'): {}", if app_state.is_continuous_spam_active { "ON" } else { "OFF" }),
            if app_state.is_continuous_spam_active { Style::default().fg(Color::LightRed).add_modifier(Modifier::BOLD) } else { Style::default() }
        )));
    }
   
    let status_paragraph = Paragraph::new(Text::from(status_lines))
        .block(Block::default().borders(Borders::ALL).title(" ESP32 Real-Time Status "))
        .wrap(Wrap { trim: true }); // Added wrap for status paragraph
    f.render_widget(status_paragraph, status_area);

    // --- CSI Data Table ---
    let table_header_cells = ["Timestamp (s)", "Seq", "RSSI", "Subcarriers"]
        .iter().map(|h| Cell::from(*h).style(Style::default().fg(Color::Yellow)));
    let table_header = Row::new(table_header_cells).height(1).bottom_margin(0);

    let rows: Vec<Row> = app_state.csi_data.iter().rev()
        .take(table_area.height.saturating_sub(2).max(0) as usize) 
        .map(|p: &CsiData| {
            let num_subcarriers = p.csi.get(0).and_then(|rx| rx.get(0)).map_or(0, |sc_row| sc_row.len());
            let rssi_str = p.rssi.first().map_or_else(|| "N/A".to_string(), |r| r.to_string());
            Row::new(vec![
                Cell::from(format!("{:.6}", p.timestamp)),
                Cell::from(p.sequence_number.to_string()),
                Cell::from(rssi_str),
                Cell::from(num_subcarriers.to_string()),
            ])
        }).collect();

    let table_widths = [
        Constraint::Length(18), Constraint::Length(7), Constraint::Length(6), Constraint::Min(10)
    ];
    let csi_table = Table::new(rows, &table_widths)
        .header(table_header)
        .block(Block::default().borders(Borders::ALL).title(" CSI Data Packets "))
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED))
        .highlight_symbol(">> ");
    f.render_widget(csi_table, table_area);

    // --- Log Output (Right Panel) ---
    let log_panel_content_height = log_panel_area.height.saturating_sub(2).max(0) as usize;

    let log_lines_to_display: Vec<Line> = {
        let current_log_count = app_state.log_messages.len();
        let start_index = if current_log_count > log_panel_content_height {
            current_log_count - log_panel_content_height 
        } else {
            0
        };

        app_state.log_messages.iter().skip(start_index).map(|entry| {
            let timestamp_str = entry.timestamp.format("%H:%M:%S").to_string();
            let level_str = format!("[{}]", entry.level);
            let message_str = &entry.message;

            let style = match entry.level {
                log::Level::Error => Style::default().fg(Color::Red),
                log::Level::Warn => Style::default().fg(Color::Yellow),
                log::Level::Info => Style::default().fg(Color::Cyan),
                log::Level::Debug => Style::default().fg(Color::Blue),
                log::Level::Trace => Style::default().fg(Color::Magenta),
            };
            Line::from(vec![
                Span::raw(format!("{timestamp_str} ")),
                Span::styled(level_str, style.clone()),
                Span::styled(format!(" {}", message_str), style),
            ])
        }).collect()
    };

    let log_text = Text::from(log_lines_to_display);

    let logs_widget = Paragraph::new(log_text)
        .wrap(Wrap { trim: true })
        .block(Block::default().borders(Borders::ALL).title(format!(" Log ({}/{}) ", app_state.log_messages.len(), LOG_BUFFER_CAPACITY)));
    f.render_widget(logs_widget, log_panel_area);

    // --- Global Footer Info/Errors ---
    let footer_text_str = if let Some(err_msg) = &app_state.last_error {
        format!("ERROR: {err_msg} (Press 'R' to dismiss)")
    } else if app_state.is_editing_spam_config && app_state.ui_mode == UiMode::Spam {
        let field_name = match app_state.current_editing_field {
            SpamConfigField::SrcMacOctet(i) => format!("Src MAC[{}]", i + 1),
            SpamConfigField::DstMacOctet(i) => format!("Dst MAC[{}]", i + 1),
            SpamConfigField::Reps => "Repetitions".to_string(),
            SpamConfigField::PauseMs => "Pause (ms)".to_string(),
        };
        format!("[Esc] Exit Edit | [Tab]/[Ent] Next | [Shft+Tab] Prev | [↑↓] Modify Val | Editing: {}", field_name)
    } else {
        "Controls: [Q]uit | [M]ode | [C]h | [B]W | [L]TF | [↑]ClrCSI | [↓]SyncTime | [R]ClrErr | SpamMode: [E]dit | [S]Burst | [T]Cont."
            .to_string()
    };
    let footer_style = if app_state.last_error.is_some() { Style::default().fg(Color::Red) }
        else if app_state.is_editing_spam_config { Style::default().fg(Color::Cyan) }
        else { Style::default().fg(Color::DarkGray) };

    let footer_paragraph = Paragraph::new(footer_text_str)
        .wrap(Wrap { trim: true })
        .block(Block::default().borders(Borders::ALL).title(" Info/Errors "));
    f.render_widget(footer_paragraph, global_footer_area);
}