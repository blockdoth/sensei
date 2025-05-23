//! Robust CLI tool for ESP32 CSI monitoring (now as a library module)

#![allow(clippy::await_holding_lock)] // Kept as it was, though addressed by refactoring

use core::time;
use std::collections::VecDeque; // For log buffer AND CSI data
use std::env;
use std::error::Error;
use std::io;
use std::sync::{Arc, Mutex as StdMutex, PoisonError}; // Standard Mutex for AppState
use std::time::Duration;

use chrono::{DateTime, Local};
use crossterm::event::EventStream;
use futures::StreamExt;

use crossbeam_channel::{Receiver as CrossbeamReceiver, Sender as CrossbeamSender};
use log::{error, info, warn, Level, LevelFilter, Metadata, Record, SetLoggerError};
use lib::sources::controllers::Controller;

use crossterm::{
    event::{Event as CEvent, KeyCode},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Cell, List, ListItem, Paragraph, Row, Table},
    Frame, Terminal,
};

use super::EspToolSubcommandArgs;

use lib::sources::DataSourceT;

use lib::adapters::CsiDataAdapter;
use lib::adapters::esp32::ESP32Adapter;
use lib::csi_types::{Complex, CsiData};
use lib::errors::{ControllerError, DataSourceError};
use lib::network::rpc_message::{DataMsg, SourceType};
use lib::sources::controllers::esp32_controller::{
    Bandwidth as EspBandwidth, CsiType as EspCsiType, Esp32Command,
    Esp32ControllerParams, // Using this for centralized logic
    Esp32DeviceConfigPayload, OperationMode as EspOperationMode,
    SecondaryChannel as EspSecondaryChannel, CustomFrameParams,
};
use lib::sources::esp32::{Esp32Source, Esp32SourceConfig};

use tokio::sync::Mutex as TokioMutex;
use tokio::task::JoinHandle;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum SpamConfigField {
    SrcMacOctet(usize), // 0-5
    DstMacOctet(usize), // 0-5
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

pub struct LogEntry {
    pub timestamp: DateTime<Local>,
    pub level: log::Level,
    pub message: String,
}

struct TuiLogger {
    log_sender: CrossbeamSender<LogEntry>,
    level: Level,
}

impl log::Log for TuiLogger {
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
                    "[TUI_LOGGER_FALLBACK] {}: {} [{}] - {}",
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
    let logger = TuiLogger {
        log_sender: sender,
        level: log_level_filter.to_level().unwrap_or(log::Level::Error),
    };
    log::set_boxed_logger(Box::new(logger))?;
    log::set_max_level(log_level_filter);
    Ok(())
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

#[derive(Clone, Copy, PartialEq, Debug)]
enum UiMode {
    Csi,
    Spam,
}

#[derive(Clone, Debug)]
struct CliEspConfig {
    channel: u8,
    mode: EspOperationMode,
    bandwidth: EspBandwidth,
    secondary_channel: EspSecondaryChannel,
    csi_type: EspCsiType,
    manual_scale: u8,
}

impl Default for CliEspConfig {
    fn default() -> Self {
        Self {
            channel: 1,
            mode: EspOperationMode::Receive,
            bandwidth: EspBandwidth::Twenty,
            secondary_channel: EspSecondaryChannel::None,
            csi_type: EspCsiType::HighThroughputLTF,
            manual_scale: 0,
        }
    }
}

struct AppState {
    ui_mode: UiMode,
    current_cli_config: CliEspConfig,
    csi_data: VecDeque<CsiData>, // Changed to VecDeque
    connection_status: String,
    last_error: Option<String>,
    log_messages: VecDeque<LogEntry>,
    max_log_messages: usize,
    spam_config_src_mac: [u8; 6],
    spam_config_dst_mac: [u8; 6],
    spam_config_n_reps: i32,
    spam_config_pause_ms: i32,
    is_editing_spam_config: bool,
    current_editing_field: SpamConfigField,
}

impl AppState {
    fn new(initial_cli_config: CliEspConfig) -> Self {
        Self {
            ui_mode: UiMode::Csi,
            current_cli_config: initial_cli_config,
            csi_data: VecDeque::with_capacity(1000), // Changed to VecDeque
            connection_status: "CONNECTING...".into(),
            last_error: None,
            log_messages: VecDeque::with_capacity(200),
            max_log_messages: 200,
            spam_config_src_mac: [0x12, 0x34, 0x56, 0x78, 0x9A, 0xBC],
            spam_config_dst_mac: [0xB4, 0x82, 0xC5, 0x58, 0xA1, 0xC0],
            spam_config_n_reps: 2400,
            spam_config_pause_ms: 100,
            is_editing_spam_config: false,
            current_editing_field: SpamConfigField::SrcMacOctet(0),
        }
    }

    fn add_log_message(&mut self, entry: LogEntry) {
        if self.log_messages.len() >= self.max_log_messages {
            self.log_messages.pop_front();
        }
        self.log_messages.push_back(entry);
    }
}

// Helper to handle poisoned mutex locks on AppState
fn lock_app_state_mut(
    app_state_mutex: &Arc<StdMutex<AppState>>,
) -> Result<std::sync::MutexGuard<'_, AppState>, Box<dyn Error>> {
    app_state_mutex
        .lock()
        .map_err(|e: PoisonError<_>| format!("AppState lock poisoned: {e}").into())
}
fn lock_app_state(
    app_state_mutex: &Arc<StdMutex<AppState>>,
) -> Result<std::sync::MutexGuard<'_, AppState>, Box<dyn Error>> {
    app_state_mutex
        .lock()
        .map_err(|e: PoisonError<_>| format!("AppState lock poisoned: {e}").into())
}


pub async fn run_esp_test_subcommand(args: EspToolSubcommandArgs) -> Result<(), Box<dyn Error>> {
    let (log_tx, log_rx) = crossbeam_channel::bounded(200);
    let log_level = env::var("RUST_LOG")
        .ok()
        .and_then(|s| s.parse::<LevelFilter>().ok())
        .unwrap_or(LevelFilter::Info);

    init_tui_logger(log_level, log_tx.clone()).map_err(|e| {
        eprintln!("FATAL: Failed to initialize TUI logger: {e}");
        e
    })?;

    let initial_cli_config = CliEspConfig::default();
    let app_state = Arc::new(StdMutex::new(AppState::new(initial_cli_config.clone())));

    let app_state_log_clone = Arc::clone(&app_state);
    let mut log_listener_handle: JoinHandle<()> = tokio::spawn(async move {
        loop {
            tokio::task::yield_now().await;
            match tokio::task::block_in_place(|| log_rx.recv_timeout(Duration::from_secs(1))) {
                Ok(log_msg) => match app_state_log_clone.try_lock() {
                    Ok(mut state_guard) => state_guard.add_log_message(log_msg),
                    Err(std::sync::TryLockError::Poisoned(p_err)) => {
                        eprintln!("[TUI_LOGGER_FALLBACK] AppState mutex poisoned: {p_err}. Log listener terminating.");
                        break;
                    }
                    Err(std::sync::TryLockError::WouldBlock) => { /* Log dropped */ }
                },
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
            }
        }
    });

    info!("ESP Test Tool subcommand starting...");
    let port_name = args.port;
    info!("Attempting to use port: {port_name}");

    if port_name.is_empty() {
        error!("Serial port argument was empty or not provided correctly.");
        // ... (eprintln for usage and available ports - kept as is)
        return Err("Serial port argument missing or invalid".into());
    }

    let mut terminal = setup_terminal().map_err(|e| {
        error!("Failed to setup terminal: {e}");
        e
    })?;

    let esp_config = Esp32SourceConfig {
        port_name: port_name.clone(),
        baud_rate: 3_000_000,
        csi_buffer_size: Some(100),
        ack_timeout_ms: Some(2000),
    };

    let esp_source_instance = Esp32Source::new(esp_config).map_err(|e| {
        let _ = restore_terminal(&mut terminal); // Attempt to restore terminal before final error
        error!("Failed to initialize ESP32Source: {e}");
        Box::new(e) as Box<dyn Error>
    })?;
    let esp_source: Arc<TokioMutex<Esp32Source>> = Arc::new(TokioMutex::new(esp_source_instance));

    {
        let mut esp_guard = esp_source.lock().await;
        if let Err(e) = esp_guard.start().await {
            let _ = restore_terminal(&mut terminal);
            error!("Failed to connect to ESP32 (start source): {e}");
            return Err(Box::new(e));
        }
        lock_app_state_mut(&app_state)?.connection_status = "CONNECTED (Source Started)".to_string();
        info!("Giving reader thread a moment to start...");
        tokio::time::sleep(Duration::from_millis(300)).await;
        info!("ESP32 source started. Applying initial configuration...");

        // Refactored initial setup to use Esp32ControllerParams
        let initial_controller_params = Esp32ControllerParams {
            set_channel: Some(initial_cli_config.channel),
            apply_device_config: Some(Esp32DeviceConfigPayload {
                mode: initial_cli_config.mode,
                bandwidth: initial_cli_config.bandwidth,
                secondary_channel: initial_cli_config.secondary_channel,
                csi_type: initial_cli_config.csi_type,
                manual_scale: initial_cli_config.manual_scale,
            }),
            clear_all_mac_filters: Some(true),
            synchronize_time: Some(true),
            mac_filters_to_add: None,
            pause_acquisition: None, // Handled by apply_device_config logic in controller
             // Pause general Wifi transmit if starting in Transmit mode.
            pause_wifi_transmit: if initial_cli_config.mode == EspOperationMode::Transmit { Some(true) } else { None },
            transmit_custom_frame: None,
        };

        if let Err(e) = initial_controller_params.apply(&mut *esp_guard).await {
            warn!("Failed to apply initial ESP32 configuration block: {e}. Continuing with defaults or potentially unstable state.",);
            lock_app_state_mut(&app_state)?.last_error = Some(format!("Initial ESP config failed: {e}"));
        } else {
            info!("Initial ESP32 configuration block applied successfully.");
            // State like channel, mode, etc., is now managed by AppState reflecting cli_config,
            // and ESP state should match after apply() if successful.
            // The controller's apply method should handle unpausing acquisition if mode is Receive.
        }
    } // esp_guard dropped here
    info!("ESP32 initial setup sequence complete.");


    let esp_source_csi_reader_clone = Arc::clone(&esp_source);
    let app_state_csi_clone = Arc::clone(&app_state);
    let csi_listener_handle: JoinHandle<()> = tokio::spawn(async move {
        info!("CSI listener thread started.");
        let mut read_buffer = vec![0u8; 4096];
        let mut esp_adapter = ESP32Adapter::new(false);

        loop {
            let data_result = {
                let mut source_guard = esp_source_csi_reader_clone.lock().await;
                if !source_guard.is_running.load(std::sync::atomic::Ordering::Relaxed) {
                    info!("CSI listener: Source reported as not running. Exiting.");
                     match app_state_csi_clone.lock() {
                        Ok(mut state) => state.connection_status = "DISCONNECTED (Source Not Running)".to_string(),
                        Err(_) => error!("CSI Listener: AppState lock poisoned while setting status."),
                    };
                    break;
                }
                source_guard.read(&mut read_buffer).await
            }; // source_guard dropped here

            match data_result {
                Ok(0) => tokio::time::sleep(Duration::from_millis(10)).await,
                Ok(n) => {
                    let raw_csi_payload = read_buffer[..n].to_vec();
                    let data_msg = DataMsg::RawFrame {
                        ts: Local::now().timestamp_micros() as f64 / 1_000_000.0,
                        bytes: raw_csi_payload,
                        source_type: SourceType::ESP32,
                    };
                    match esp_adapter.produce(data_msg).await {
                        Ok(Some(DataMsg::CsiFrame { csi })) => {
                            if let Ok(mut state_guard) = app_state_csi_clone.lock() {
                                state_guard.csi_data.push_back(csi); // Use push_back for VecDeque
                                if state_guard.csi_data.len() > 1000 { // Max capacity
                                    state_guard.csi_data.pop_front(); // Use pop_front
                                }
                            } else {
                                error!("CSI Listener: AppState lock poisoned while adding CSI data.");
                            }
                        }
                        Ok(None) => {}
                        Err(e) => warn!("ESP32Adapter failed to parse CSI: {e:?}"),
                        Ok(Some(DataMsg::RawFrame { .. })) => {} // Should not happen from this adapter
                    }
                }
                Err(DataSourceError::Controller(msg))
                    if msg.contains("Source stopped") || msg.contains("CSI data channel disconnected") =>
                {
                    info!("CSI listener: Source stopped or channel disconnected. Error: {msg:?}");
                     match app_state_csi_clone.lock() {
                        Ok(mut state) => state.connection_status = "DISCONNECTED (CSI Read Error)".to_string(),
                        Err(_) => error!("CSI Listener: AppState lock poisoned while setting status."),
                    };
                    break;
                }
                Err(DataSourceError::Io(io_err))
                    if io_err.kind() == std::io::ErrorKind::BrokenPipe ||
                       io_err.kind() == std::io::ErrorKind::NotConnected ||
                       io_err.kind() == std::io::ErrorKind::PermissionDenied =>
                {
                    info!("CSI listener: Serial port disconnected or permission error ({:?}).", io_err.kind());
                     match app_state_csi_clone.lock() {
                        Ok(mut state) => state.connection_status = format!("DISCONNECTED (IO: {:?})", io_err.kind()),
                        Err(_) => error!("CSI Listener: AppState lock poisoned while setting status."),
                    };
                    break;
                }
                Err(e) => {
                    warn!("CSI listener: error reading from ESP32 source: {e:?}");
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
            }

            match app_state_csi_clone.lock() {
                Ok(state) => {
                    if state.connection_status.starts_with("DISCONNECTED") {
                        info!("CSI listener detected global DISCONNECTED status. Stopping.");
                        break;
                    }
                }
                Err(_) => {
                     error!("CSI listener: AppState mutex poisoned. Stopping.");
                     break;
                }
            }
        }
        info!("CSI listener thread stopped.");
    });

    let mut event_stream = EventStream::new();
    loop {
        { // Scope for app_state_guard for drawing
            let app_state_guard = lock_app_state(&app_state)?;
            terminal.draw(|f| ui(f, &app_state_guard))?;
        } // app_state_guard dropped

        tokio::select! {
            maybe_event = event_stream.next() => {
                match maybe_event {
                    Some(Ok(event)) => {
                        if let CEvent::Key(key_event) = event {
                            if handle_input(key_event.code, &esp_source, &app_state).await? {
                                break; // Exit main loop
                            }
                        }
                    }
                    Some(Err(e)) => error!("Error reading input event: {e}"),
                    None => break, // Event stream ended
                }
            }
            _ = tokio::time::sleep(Duration::from_millis(200)) => { /* UI refresh interval */ }
        }

        match lock_app_state(&app_state) {
            Ok(state) => {
                if state.connection_status.starts_with("DISCONNECTED") &&
                   state.last_error.as_deref() != Some("DISCONNECTED (by user)")
                {
                    info!("Main loop detected DISCONNECTED state (not by user 'q'). Shutting down.");
                    break;
                }
            }
            Err(_) => {
                error!("Main loop: AppState mutex poisoned. Shutting down.");
                break;
            }
        }
    }

    eprintln!("[ESP_TOOL_DEBUG] Main UI loop exited. Beginning ESP_TOOL shutdown sequence...");
    {
        let mut esp_guard = esp_source.lock().await;
        if esp_guard.is_running.load(std::sync::atomic::Ordering::Relaxed) {
            if let Err(e) = esp_guard.stop().await {
                error!("Error stopping ESP32 source: {e}");
                eprintln!("[ESP_TOOL_DEBUG] Error stopping ESP32 source: {e}");
            } else {
                info!("ESP32 source stop command issued successfully.");
                eprintln!("[ESP_TOOL_DEBUG] ESP32 source stop command issued successfully.");
            }
        } else {
            info!("ESP32 source was already stopped or not initially started.");
            eprintln!("[ESP_TOOL_DEBUG] ESP32 source was already stopped or not initially started.");
        }
    } // esp_guard dropped

    eprintln!("[ESP_TOOL_DEBUG] Waiting for CSI listener task to complete (max 5s)...");
    match tokio::time::timeout(Duration::from_secs(5), csi_listener_handle).await {
        Ok(Ok(_)) => eprintln!("[ESP_TOOL_DEBUG] CSI listener task finished gracefully."),
        Ok(Err(e)) => eprintln!("[ESP_TOOL_DEBUG] CSI listener task panicked: {e:?}"),
        Err(_) => eprintln!("[ESP_TOOL_DEBUG] CSI listener task timed out during shutdown."),
    }

    eprintln!("[ESP_TOOL_DEBUG] Signaling log listener task to stop (dropping this function's log_tx)...");
    drop(log_tx);
    eprintln!("[ESP_TOOL_DEBUG] Attempting to shutdown log listener task via abort()...");
    log_listener_handle.abort(); // Signal abort

    eprintln!("[ESP_TOOL_DEBUG] Waiting for log listener task to complete after abort (max 3s)...");
     match tokio::time::timeout(Duration::from_secs(3), log_listener_handle).await {
        // The handle might have been awaited above, ensure it's not awaited twice if it was moved.
        // Re-check if log_listener_handle can be awaited again or if it consumes the handle.
        // Tokio JoinHandle is consumed by await. So, we should not await it again.
        // Abort is sufficient to signal. We cannot join again.
        // For robust shutdown, we might need a separate flag for the log listener or make its loop selectable.
        // Given the current structure, the timeout on recv_timeout and disconnect signal from drop(log_tx)
        // are the primary mechanisms for its termination. Abort is an additional signal.
        // Let's assume the abort signal + drop(log_tx) is enough, and the task will exit.
        // We won't await the handle again here after aborting.
         _ => eprintln!("[ESP_TOOL_DEBUG] Log listener abort signal sent. Previous await for recv was the primary stop for its loop."),
    }


    if let Err(e) = restore_terminal(&mut terminal) {
        eprintln!("[ESP_TOOL_ERROR] Failed to restore terminal: {e}");
    } else {
        eprintln!("[ESP_TOOL_DEBUG] Terminal restored by esp_tool subcommand.");
    }

    eprintln!("[ESP_TOOL_DEBUG] ESP Test Tool subcommand finishing.");
    Ok(())
}


async fn handle_input(
    key: KeyCode,
    esp_source_mutex: &Arc<TokioMutex<Esp32Source>>,
    app_state_mutex: &Arc<StdMutex<AppState>>,
) -> Result<bool, Box<dyn Error>> {
    // Temporary state storage to avoid holding app_state_lock across .await
    let mut temp_is_editing_spam_config = false;
    let mut temp_ui_mode = UiMode::Csi;
    let mut temp_current_editing_field = SpamConfigField::SrcMacOctet(0);
    let mut temp_spam_config_src_mac = [0u8; 6];
    let mut temp_spam_config_dst_mac = [0u8; 6];
    let mut temp_spam_config_n_reps = 0;
    let mut temp_spam_config_pause_ms = 0;
    let mut temp_current_cli_config = CliEspConfig::default(); // Placeholder

    { // Scope for reading from AppState
        let mut app_state_guard = lock_app_state_mut(app_state_mutex)?;
        if key != KeyCode::Char('r') {
            app_state_guard.last_error = None;
        }
        temp_is_editing_spam_config = app_state_guard.is_editing_spam_config;
        temp_ui_mode = app_state_guard.ui_mode;
        temp_current_editing_field = app_state_guard.current_editing_field;
        if temp_is_editing_spam_config && temp_ui_mode == UiMode::Spam {
            // Handle spam config editing directly as it doesn't involve async calls
            match key {
                KeyCode::Char('e') | KeyCode::Esc => {
                    app_state_guard.is_editing_spam_config = false;
                    info!("Exited spam config editing mode.");
                }
                KeyCode::Tab => {
                    app_state_guard.current_editing_field = app_state_guard.current_editing_field.next();
                }
                KeyCode::Up => match app_state_guard.current_editing_field {
                    SpamConfigField::SrcMacOctet(i) => app_state_guard.spam_config_src_mac[i] = app_state_guard.spam_config_src_mac[i].wrapping_add(1),
                    SpamConfigField::DstMacOctet(i) => app_state_guard.spam_config_dst_mac[i] = app_state_guard.spam_config_dst_mac[i].wrapping_add(1),
                    SpamConfigField::Reps => app_state_guard.spam_config_n_reps = app_state_guard.spam_config_n_reps.saturating_add(1).max(0),
                    SpamConfigField::PauseMs => app_state_guard.spam_config_pause_ms = app_state_guard.spam_config_pause_ms.saturating_add(1).max(0),
                },
                KeyCode::Down => match app_state_guard.current_editing_field {
                    SpamConfigField::SrcMacOctet(i) => app_state_guard.spam_config_src_mac[i] = app_state_guard.spam_config_src_mac[i].wrapping_sub(1),
                    SpamConfigField::DstMacOctet(i) => app_state_guard.spam_config_dst_mac[i] = app_state_guard.spam_config_dst_mac[i].wrapping_sub(1),
                    SpamConfigField::Reps => app_state_guard.spam_config_n_reps = (app_state_guard.spam_config_n_reps - 1).max(0),
                    SpamConfigField::PauseMs => app_state_guard.spam_config_pause_ms = (app_state_guard.spam_config_pause_ms - 1).max(0),
                },
                _ => {}
            }
            return Ok(false); // Handled, no exit
        }
        // Clone necessary data for ESP commands
        temp_spam_config_src_mac = app_state_guard.spam_config_src_mac;
        temp_spam_config_dst_mac = app_state_guard.spam_config_dst_mac;
        temp_spam_config_n_reps = app_state_guard.spam_config_n_reps;
        temp_spam_config_pause_ms = app_state_guard.spam_config_pause_ms;
        temp_current_cli_config = app_state_guard.current_cli_config.clone();
    } // app_state_guard dropped

    let mut params = Esp32ControllerParams::default();
    let mut new_cli_config_opt: Option<CliEspConfig> = None;
    let mut new_ui_mode_opt: Option<UiMode> = None;
    let mut should_exit = false;
    let mut action_performed = false; // To track if params were set

    match key {
        KeyCode::Char('q') => {
            info!("'q' pressed, attempting to disconnect ESP32 source.");
            if temp_ui_mode == UiMode::Spam {
                info!("Pausing WiFi transmit before disconnecting...");
                // This is a direct command before full stop.
                // If ESP is already stopped, this might error, but stop() handles that.
                let mut esp_guard = esp_source_mutex.lock().await;
                 if let Err(e) = esp_guard.send_esp32_command(Esp32Command::PauseWifiTransmit, None).await {
                     warn!("Failed to pause WiFi transmit on quit: {e}");
                 }
            }
            should_exit = true;
            // Update AppState after potential async call
            {
                let mut app_state_guard = lock_app_state_mut(app_state_mutex)?;
                app_state_guard.connection_status = "DISCONNECTED (by user)".to_string();
                app_state_guard.last_error = Some("DISCONNECTED (by user)".to_string());
            }
        }
        KeyCode::Char('m') => {
            action_performed = true;
            let new_ui_mode = match temp_ui_mode {
                UiMode::Csi => UiMode::Spam,
                UiMode::Spam => UiMode::Csi,
            };
            let new_esp_op_mode = match new_ui_mode {
                UiMode::Csi => EspOperationMode::Receive,
                UiMode::Spam => EspOperationMode::Transmit,
            };
            new_ui_mode_opt = Some(new_ui_mode);

            let mut new_config = temp_current_cli_config.clone();
            new_config.mode = new_esp_op_mode;
            new_cli_config_opt = Some(new_config.clone());

            params.apply_device_config = Some(Esp32DeviceConfigPayload {
                mode: new_esp_op_mode,
                bandwidth: new_config.bandwidth,
                secondary_channel: new_config.secondary_channel,
                csi_type: new_config.csi_type,
                manual_scale: new_config.manual_scale,
            });
            // Controller's `apply` will handle Pause/UnpauseAcquisition based on mode.
            // If switching to Transmit mode, also explicitly pause general WiFi transmit.
            if new_esp_op_mode == EspOperationMode::Transmit {
                params.pause_wifi_transmit = Some(true);
            }
            // If switching to Receive, controller ensures UnpauseAcquisition.
            // pause_wifi_transmit = Some(false) could be sent to ensure it is on, if desired.
            // For now, only explicitly pause it when going to Transmit.
        }
        KeyCode::Char('e') => { // Edit spam config
            if temp_ui_mode == UiMode::Spam {
                let mut app_state_guard = lock_app_state_mut(app_state_mutex)?;
                app_state_guard.is_editing_spam_config = !app_state_guard.is_editing_spam_config;
                if app_state_guard.is_editing_spam_config {
                    info!("Entered spam config editing mode. Use Tab, Up/Down. 'e' or Esc to exit.");
                    app_state_guard.current_editing_field = SpamConfigField::SrcMacOctet(0);
                } else {
                    info!("Exited spam config editing mode.");
                }
            } else {
                lock_app_state_mut(app_state_mutex)?.last_error = Some(
                    "Spam config editing ('e') only available in Spam mode ('m').".to_string(),
                );
            }
        }
        KeyCode::Char('s') => { // Send spam
            action_performed = true;
            if temp_ui_mode == UiMode::Spam {
                if temp_current_cli_config.mode != EspOperationMode::Transmit {
                     lock_app_state_mut(app_state_mutex)?.last_error = Some("ESP32 not in Transmit mode. Switch mode first ('m').".to_string());
                } else {
                    params.transmit_custom_frame = Some(CustomFrameParams {
                        src_mac: temp_spam_config_src_mac,
                        dst_mac: temp_spam_config_dst_mac,
                        n_reps: temp_spam_config_n_reps,
                        pause_ms: temp_spam_config_pause_ms,
                    });
                }
            } else {
                lock_app_state_mut(app_state_mutex)?.last_error = Some("Send Spam ('s') command only active in Spam mode ('m').".to_string());
            }
        }
        KeyCode::Char('c') => { // Change channel
            action_performed = true;
            let mut new_channel = temp_current_cli_config.channel + 1;
            if new_channel > 11 { new_channel = 1; } // Assuming 1-11 common valid channels
            
            let mut new_config = temp_current_cli_config.clone();
            new_config.channel = new_channel;
            new_cli_config_opt = Some(new_config);
            params.set_channel = Some(new_channel);
        }
        KeyCode::Char('b') => { // Change bandwidth
            action_performed = true;
            let new_bandwidth_is_40 = temp_current_cli_config.bandwidth == EspBandwidth::Twenty;
            let new_esp_bw = if new_bandwidth_is_40 { EspBandwidth::Forty } else { EspBandwidth::Twenty };
            let new_secondary_chan = if new_bandwidth_is_40 { EspSecondaryChannel::Above } else { EspSecondaryChannel::None };

            let mut new_config = temp_current_cli_config.clone();
            new_config.bandwidth = new_esp_bw;
            new_config.secondary_channel = new_secondary_chan;
            new_cli_config_opt = Some(new_config.clone());

            params.apply_device_config = Some(Esp32DeviceConfigPayload {
                mode: new_config.mode, // Keep current mode
                bandwidth: new_config.bandwidth,
                secondary_channel: new_config.secondary_channel,
                csi_type: new_config.csi_type,
                manual_scale: new_config.manual_scale,
            });
        }
        KeyCode::Char('l') => { // Change LTF Type
            action_performed = true;
            let new_csi_type_is_legacy = temp_current_cli_config.csi_type == EspCsiType::HighThroughputLTF;
            let new_esp_csi_type = if new_csi_type_is_legacy { EspCsiType::LegacyLTF } else { EspCsiType::HighThroughputLTF };

            let mut new_config = temp_current_cli_config.clone();
            new_config.csi_type = new_esp_csi_type;
            new_cli_config_opt = Some(new_config.clone());

            params.apply_device_config = Some(Esp32DeviceConfigPayload {
                mode: new_config.mode, // Keep current mode
                bandwidth: new_config.bandwidth,
                secondary_channel: new_config.secondary_channel,
                csi_type: new_config.csi_type,
                manual_scale: new_config.manual_scale,
            });
        }
        KeyCode::Char('r') => { /* Clears last_error (handled at the start) */ }
        KeyCode::Up => { // Clear CSI Buffer (if not editing spam)
            if !temp_is_editing_spam_config {
                 lock_app_state_mut(app_state_mutex)?.csi_data.clear();
                 info!("CSI data buffer cleared.");
            }
        }
        KeyCode::Down => { // Synchronize Time (if not editing spam)
            if !temp_is_editing_spam_config {
                action_performed = true;
                params.synchronize_time = Some(true);
            }
        }
        _ => {}
    }

    if action_performed {
        let mut esp_guard = esp_source_mutex.lock().await;
        let command_name = format!("{:?}", key); // Simple name for logging

        match params.apply(&mut *esp_guard).await {
            Ok(_) => {
                info!("ESP32 command ({command_name}) successful via ControllerParams.");
                // Update AppState with new configurations if they were set
                let mut app_state_guard = lock_app_state_mut(app_state_mutex)?;
                if let Some(new_conf) = new_cli_config_opt {
                    app_state_guard.current_cli_config = new_conf;
                }
                if let Some(new_mode) = new_ui_mode_opt {
                    app_state_guard.ui_mode = new_mode;
                }
                 // Clear previous error on success
                app_state_guard.last_error = None;
            }
            Err(e) => {
                let err_msg = format!("ESP32 command ({command_name}) failed: {e}");
                error!("TUI: {err_msg}");
                lock_app_state_mut(app_state_mutex)?.last_error = Some(err_msg);
            }
        }
    }
    Ok(should_exit)
}


// ui function remains largely the same, ensure it handles VecDeque for csi_data correctly.
// Iteration `app_state.csi_data.iter().rev().take(...)` works for VecDeque.
fn ui(f: &mut Frame, app_state: &AppState) {
    let main_horizontal_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .margin(1)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)].as_ref())
        .split(f.area());

    let left_panel_area = main_horizontal_chunks[0];
    let log_panel_area = main_horizontal_chunks[1];

    let status_area_height = if app_state.ui_mode == UiMode::Spam {
        if app_state.is_editing_spam_config { 8 } else { 6 }
    } else {
        3
    };

    let left_vertical_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(status_area_height),
            Constraint::Min(0),
            Constraint::Length(3),
        ])
        .split(left_panel_area);

    let status_area = left_vertical_chunks[0];
    let table_area = left_vertical_chunks[1];
    let footer_area = left_vertical_chunks[2];

    let mode_str = match app_state.ui_mode {
        UiMode::Csi => "CSI RX",
        UiMode::Spam => "WiFi SPAM",
    };
    let bw_str = match app_state.current_cli_config.bandwidth {
        EspBandwidth::Twenty => "20MHz",
        EspBandwidth::Forty => "40MHz",
    };
    let ltf_str = match app_state.current_cli_config.csi_type {
        EspCsiType::HighThroughputLTF => "HT-LTF",
        EspCsiType::LegacyLTF => "L-LTF",
    };

    let mut status_lines = vec![
        Line::from(vec![
            Span::raw("ESP32 | "),
            Span::styled(mode_str, Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(format!(
                " | Chan: {} | BW: {} ({:?}) | LTF: {}",
                app_state.current_cli_config.channel,
                bw_str,
                app_state.current_cli_config.secondary_channel,
                ltf_str
            )),
        ]),
        Line::from(vec![
            Span::raw(format!("CSI Buf: {} | Status: ", app_state.csi_data.len())),
            Span::styled(
                app_state.connection_status.clone(),
                match app_state.connection_status.as_str() {
                    s if s.starts_with("CONNECTED") => Style::default().fg(Color::Green),
                    _ => Style::default().fg(Color::Red),
                },
            ),
        ]),
    ];

    if app_state.ui_mode == UiMode::Spam {
        let mut src_mac_spans = vec![Span::raw("  Src MAC: ")];
        for i in 0..6 {
            let val_str = format!("{:02X}", app_state.spam_config_src_mac[i]);
            let style = if app_state.is_editing_spam_config && app_state.current_editing_field == SpamConfigField::SrcMacOctet(i) {
                Style::default().fg(Color::Yellow).add_modifier(Modifier::REVERSED)
            } else { Style::default() };
            src_mac_spans.push(Span::styled(val_str, style));
            if i < 5 { src_mac_spans.push(Span::raw(":")); }
        }

        let mut dst_mac_spans = vec![Span::raw("  Dst MAC: ")];
        for i in 0..6 {
            let val_str = format!("{:02X}", app_state.spam_config_dst_mac[i]);
            let style = if app_state.is_editing_spam_config && app_state.current_editing_field == SpamConfigField::DstMacOctet(i) {
                Style::default().fg(Color::Yellow).add_modifier(Modifier::REVERSED)
            } else { Style::default() };
            dst_mac_spans.push(Span::styled(val_str, style));
            if i < 5 { dst_mac_spans.push(Span::raw(":")); }
        }
        
        let reps_style = if app_state.is_editing_spam_config && app_state.current_editing_field == SpamConfigField::Reps {
            Style::default().fg(Color::Yellow).add_modifier(Modifier::REVERSED)
        } else { Style::default() };
        let pause_style = if app_state.is_editing_spam_config && app_state.current_editing_field == SpamConfigField::PauseMs {
            Style::default().fg(Color::Yellow).add_modifier(Modifier::REVERSED)
        } else { Style::default() };

        status_lines.push(Line::from("Spam Configuration:"));
        status_lines.push(Line::from(src_mac_spans));
        status_lines.push(Line::from(dst_mac_spans));
        status_lines.push(Line::from(vec![
            Span::raw("  Reps: "), Span::styled(app_state.spam_config_n_reps.to_string(), reps_style),
            Span::raw(" | Pause: "), Span::styled(app_state.spam_config_pause_ms.to_string(), pause_style), Span::raw("ms"),
        ]));
    }
    
    let header_paragraph = Paragraph::new(Text::from(status_lines))
        .block(Block::default().borders(Borders::ALL).title(" Status "));
    f.render_widget(header_paragraph, status_area);

    let table_header_cells = ["Timestamp (s)", "Seq", "RSSI (Rx0)", "Subcarriers"]
        .iter().map(|h| Cell::from(*h).style(Style::default().fg(Color::Yellow)));
    let table_header = Row::new(table_header_cells).height(1).bottom_margin(0);

    let rows: Vec<Row> = app_state.csi_data.iter().rev() // Iterating VecDeque
        .take(table_area.height.saturating_sub(2) as usize)
        .map(|p: &CsiData| {
            let num_subcarriers = if !p.csi.is_empty() && !p.csi[0].is_empty() { p.csi[0][0].len() } else { 0 };
            let rssi_str = p.rssi.first().map_or_else(|| "N/A".to_string(), |r| r.to_string());
            Row::new(vec![
                Cell::from(format!("{:.6}", p.timestamp)),
                Cell::from(p.sequence_number.to_string()),
                Cell::from(rssi_str),
                Cell::from(num_subcarriers.to_string()),
            ])
        }).collect();

    let table_widths = [Constraint::Length(18), Constraint::Length(8), Constraint::Length(12), Constraint::Length(12)];
    let table = Table::new(rows, &table_widths)
        .header(table_header)
        .block(Block::default().borders(Borders::ALL).title(" CSI Data Packets "))
        .row_highlight_style(Style::default().add_modifier(Modifier::REVERSED))
        .highlight_symbol(">> ");
    f.render_widget(table, table_area);

    let log_items_list: Vec<ListItem> = app_state.log_messages.iter()
        .map(|entry| {
            let timestamp_str = entry.timestamp.format("%H:%M:%S").to_string();
            let display_msg_with_timestamp = format!("{timestamp_str} [{}] {}", entry.level, entry.message);
            let style = match entry.level {
                log::Level::Error => Style::default().fg(Color::Red),
                log::Level::Warn => Style::default().fg(Color::Yellow),
                log::Level::Info => Style::default().fg(Color::Cyan),
                log::Level::Debug => Style::default().fg(Color::Blue),
                log::Level::Trace => Style::default().fg(Color::Magenta),
            };
            ListItem::new(display_msg_with_timestamp).style(style)
        }).collect();
    
    let num_logs_to_show = log_panel_area.height.saturating_sub(2) as usize;
    let current_log_count = log_items_list.len();
    let visible_log_items: Vec<ListItem> = if current_log_count > num_logs_to_show {
        log_items_list.into_iter().skip(current_log_count - num_logs_to_show).collect()
    } else { log_items_list };

    let logs_list_widget = List::new(visible_log_items)
        .block(Block::default().borders(Borders::ALL).title(" Log Output "));
    f.render_widget(logs_list_widget, log_panel_area);

    let footer_text_str = if let Some(err_msg) = &app_state.last_error {
        format!("ERROR: {err_msg} (Press 'R' to keep, other keys clear error)")
    } else if app_state.is_editing_spam_config && app_state.ui_mode == UiMode::Spam {
        let field_name = match app_state.current_editing_field {
            SpamConfigField::SrcMacOctet(i) => format!("Src MAC Octet {}", i + 1),
            SpamConfigField::DstMacOctet(i) => format!("Dst MAC Octet {}", i + 1),
            SpamConfigField::Reps => "Repetitions".to_string(),
            SpamConfigField::PauseMs => "Pause (ms)".to_string(),
        };
        format!("[E]xit Edit | [Tab]Next Field | [↑↓]Modify | Editing: {field_name}")
    } else {
        "Controls: [Q]uit | [M]ode | [S]end Spam | [E]dit Spam Cfg (Spam mode) | [C]hannel | [B]W | [L]TF | [↑]Clr CSI | [↓]Sync Time".to_string()
    };
    let footer_style = if app_state.last_error.is_some() { Style::default().fg(Color::Red) }
                       else if app_state.is_editing_spam_config { Style::default().fg(Color::Cyan) }
                       else { Style::default().fg(Color::DarkGray) };

    let footer_paragraph = Paragraph::new(footer_text_str).style(footer_style)
        .block(Block::default().borders(Borders::ALL).title(" Info/Errors "));
    f.render_widget(footer_paragraph, footer_area);
}