// cli_test_tool.rs
//! Robust CLI tool for ESP32 CSI monitoring

use core::time;
use std::collections::VecDeque; // For log buffer
use std::env;
use std::error::Error;
use std::io;
use std::sync::{Arc, Mutex as StdMutex}; // Standard Mutex for AppState
use std::thread::{sleep};
use std::time::Duration;

use chrono::{DateTime, Local};
use crossterm::event::EventStream; // For async event reading
use futures::StreamExt; // For EventStream.next()

// Logging facade and our custom logger components
use crossbeam_channel::{Receiver as CrossbeamReceiver, Sender as CrossbeamSender};
use log::{Level, LevelFilter, Metadata, Record, SetLoggerError, error, info, warn};

use crossterm::{
    event::{Event as CEvent, KeyCode},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
// Ratatui imports
use ratatui::{
    Frame,
    Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Cell, List, ListItem, Paragraph, Row, Table},
};

use lib::sources::DataSourceT;

// Project-specific imports - Changed csi_collection_lib to crate
// Project-specific imports - Changed crate:: to lib::
use lib::csi_types::{CsiData, Complex};
use lib::sources::esp32::{Esp32Source, Esp32SourceConfig};
use lib::sources::controllers::esp32_controller::{
    Esp32Command,
    OperationMode as EspOperationMode,
    Bandwidth as EspBandwidth,
    SecondaryChannel as EspSecondaryChannel,
    CsiType as EspCsiType,
    Esp32DeviceConfigPayload,
    // CustomFrameParams, // Needed for spamming
    // MacFilterPair, // If MAC filtering is added back
};
use lib::adapters::esp32::ESP32Adapter;
use lib::adapters::CsiDataAdapter;
use lib::network::rpc_message::{DataMsg, SourceType};
use lib::errors::{DataSourceError, ControllerError};

use tokio::sync::Mutex as TokioMutex; // Tokio Mutex for Esp32Source
use tokio::task::JoinHandle;
use tokio::sync::Notify;

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
            SpamConfigField::PauseMs => SpamConfigField::SrcMacOctet(0), // Wrap around
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

// --- Custom TUI Logger ---
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
                // This eprintln should appear on the console after the alternate screen is left
                // if the log_listener_handle task has already stopped.
                eprintln!(
                    "[TUI_LOGGER_FALLBACK] {}: {} [{}] - {}", // Added timestamp and level for clarity
                    Local::now().format("%Y-%m-%d %H:%M:%S%.3f"),
                    std::thread::current().name().unwrap_or("unknown_thread"), // Log current thread
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
// --- End Custom TUI Logger ---

fn setup_terminal() -> Result<Terminal<CrosstermBackend<io::Stdout>>, Box<dyn Error>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let terminal = Terminal::new(backend)?;
    Ok(terminal)
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

// NEW: Configuration struct for AppState, mirroring Esp32DeviceConfigPayload + channel
#[derive(Clone, Debug)]
struct CliEspConfig {
    channel: u8,
    mode: EspOperationMode,
    bandwidth: EspBandwidth,
    secondary_channel: EspSecondaryChannel,
    csi_type: EspCsiType,
    manual_scale: u8, // Manual scale for CSI data (0 for auto)
}

impl Default for CliEspConfig {
    fn default() -> Self {
        Self {
            channel: 1, // Default channel
            mode: EspOperationMode::Receive,
            bandwidth: EspBandwidth::Twenty,
            secondary_channel: EspSecondaryChannel::None,
            csi_type: EspCsiType::HighThroughputLTF, // Common default
            manual_scale: 0,
        }
    }
}


struct AppState {
    ui_mode: UiMode,
    current_cli_config: CliEspConfig, // NEW
    csi_data: Vec<CsiData>,          // NEW: Storing official CsiData
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
            csi_data: Vec::with_capacity(1000),
            connection_status: "CONNECTING...".into(),
            last_error: None,
            log_messages: VecDeque::with_capacity(200),
            max_log_messages: 200,
            spam_config_src_mac: [0x12, 0x34, 0x56, 0x78, 0x9A, 0xBC],
            spam_config_dst_mac: [0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF], // Broadcast
            spam_config_n_reps: 100,
            spam_config_pause_ms: 20,
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

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let (log_tx, log_rx): (CrossbeamSender<LogEntry>, CrossbeamReceiver<LogEntry>) = crossbeam_channel::bounded(200);

    let log_level = env::var("RUST_LOG")
        .ok()
        .and_then(|s| s.parse::<LevelFilter>().ok())
        .unwrap_or(LevelFilter::Info);

    if let Err(e) = init_tui_logger(log_level, log_tx.clone()) { // Clone log_tx for init
        eprintln!("FATAL: Failed to initialize TUI logger: {e}");
        return Err(e.into());
    }

    info!("CLI tool starting up...");

    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        // Usage message as before
        eprintln!("Usage: {} <serial_port>", args[0]);
        eprintln!("Example: {} /dev/cu.usbmodemXXX", args[0]);
        eprintln!("\nAvailable ports:");
        match serialport::available_ports() {
            Ok(ports) => {
                if ports.is_empty() {
                    eprintln!("  No serial ports found.");
                } else {
                    for p in ports {
                        eprintln!("  {}", p.port_name);
                    }
                }
            }
            Err(e) => {
                eprintln!("  Error listing serial ports: {e}");
            }
        }
        return Err("Serial port argument missing".into());
    }
    let port_name = args[1].clone();
    info!("Attempting to use port: {}", port_name);

    let mut terminal = match setup_terminal() {
        Ok(term) => term,
        Err(e) => {
            error!("Failed to setup terminal: {e}");
            return Err(e);
        }
    };
    
    // Create Esp32SourceConfig
    let esp_config = Esp32SourceConfig {
        port_name: port_name.clone(),
        baud_rate: 3_000_000, // Default from old code
        csi_buffer_size: Some(100),
        ack_timeout_ms: Some(2000), // Default ACK timeout from new Esp32Source
    };

    let esp_source_instance: Esp32Source = match Esp32Source::new(esp_config) {
        Ok(src) => src,
        Err(e) => {
            let _ = restore_terminal(&mut terminal);
            error!("Failed to initialize ESP32Source: {e}");
            return Err(Box::new(e));
        }
    };
    // Wrap Esp32Source in Arc<TokioMutex> for safe async sharing
    let esp_source: Arc<TokioMutex<Esp32Source>> = Arc::new(TokioMutex::new(esp_source_instance));


    // AppState initialization
    let initial_cli_config = CliEspConfig::default();
    let app_state = Arc::new(StdMutex::new(AppState::new(initial_cli_config.clone())));

    // Attempt to connect and apply initial config
    {
        let mut esp_guard = esp_source.lock().await;
        if let Err(e) = esp_guard.start().await {
            let _ = restore_terminal(&mut terminal);
            error!("Failed to connect to ESP32 (start source): {e}");
            return Err(Box::new(e));
        }
        app_state.lock().unwrap().connection_status = "CONNECTED (Source Started)".to_string();

        info!("Giving reader thread a moment to start...");
        tokio::time::sleep(Duration::from_millis(300)).await;

        info!("ESP32 source started. Applying initial configuration...");

        // Apply the default AppState config to the device
        let initial_payload_config = Esp32DeviceConfigPayload { // Renamed to avoid conflict
            mode: initial_cli_config.mode,
            bandwidth: initial_cli_config.bandwidth,
            secondary_channel: initial_cli_config.secondary_channel,
            csi_type: initial_cli_config.csi_type,
            manual_scale: initial_cli_config.manual_scale,
        };
        let mut cmd_data_config = Vec::new(); // Renamed to avoid conflict
        cmd_data_config.push(initial_payload_config.mode as u8);
        cmd_data_config.push(initial_payload_config.bandwidth as u8);
        cmd_data_config.push(initial_payload_config.secondary_channel as u8);
        cmd_data_config.push(initial_payload_config.csi_type as u8);
        cmd_data_config.push(initial_payload_config.manual_scale);

        // 1. ApplyDeviceConfig
        if let Err(e) = esp_guard.send_esp32_command(Esp32Command::ApplyDeviceConfig, Some(cmd_data_config)).await {
            warn!("Failed to apply initial device config: {}. Continuing with defaults.", e);
            app_state.lock().unwrap().last_error = Some(format!("Initial config failed: {}",e));
        } else {
            info!("Initial device config (mode, BW, etc.) applied.");
            // 2. Set Initial Channel (only after ApplyDeviceConfig is successful)
            if let Err(e) = esp_guard.send_esp32_command(Esp32Command::SetChannel, Some(vec![initial_cli_config.channel])).await {
                warn!("Failed to set initial channel {}: {}", initial_cli_config.channel, e);
                app_state.lock().unwrap().last_error = Some(format!("Initial channel set failed: {}",e));
            } else {
                info!("Initial channel {} set.", initial_cli_config.channel);
            }
            // Note: The old apply_device_config also handled unpausing acquisition.
            // The new send_esp32_command for ApplyDeviceConfig does not inherently do this.
            // If the firmware expects an explicit UnpauseAcquisition after mode set to Receive,
            // that would need to be sent separately here.
            // For now, we address the ACK timeout. If ApplyDeviceConfig sets mode to Receive,
            // an explicit UnpauseAcquisition might be needed if the firmware doesn't auto-unpause.
            if initial_payload_config.mode == EspOperationMode::Receive {
                info!("Initial mode is Receive. Attempting to unpause CSI acquisition.");
                if let Err(e) = esp_guard.send_esp32_command(Esp32Command::UnpauseAcquisition, None).await {
                    warn!("Failed to send UnpauseAcquisition after initial config: {}", e);
                } else {
                    info!("UnpauseAcquisition command sent after initial config.");
                }
            }

        }
        
        // 3. Clear MAC filters
        info!("Attempting to clear MAC address filters...");
        if let Err(e) = esp_guard.send_esp32_command(Esp32Command::WhitelistClear, None).await {
            warn!("Failed to clear MAC filters: {e}. CSI reception might be filtered.");
            app_state.lock().unwrap().last_error = Some(format!("MAC filter clear failed: {}",e));
        } else {
            info!("MAC address filters cleared successfully.");
        }

        // 4. Synchronize time
        info!("Attempting to synchronize time...");
        if let Err(e) = esp_guard.send_esp32_command(Esp32Command::SynchronizeTimeInit, None).await {
            warn!("Time sync init failed: {}", e);
            app_state.lock().unwrap().last_error = Some(format!("Time sync init failed: {}",e));
        } else {
            let time_us = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap() // Or handle error appropriately
                .as_micros() as u64;
            if let Err(e) = esp_guard.send_esp32_command(Esp32Command::SynchronizeTimeApply, Some(time_us.to_le_bytes().to_vec())).await {
                warn!("Time sync apply failed: {}", e);
                app_state.lock().unwrap().last_error = Some(format!("Time sync apply failed: {}",e));
            } else {
                info!("Time synchronized with ESP32.");
            }
        }
    } // esp_guard (MutexGuard) is dropped here
    info!("ESP32 initial setup sequence complete.");


    // Log message receiving thread - MODIFIED for mutability
    let app_state_log_clone = Arc::clone(&app_state);
    // Make log_listener_handle mutable
    let mut log_listener_handle: JoinHandle<()> = tokio::spawn(async move { // Added mut
        eprintln!("[LOG_LISTENER_TASK] Started (abort strategy).");
        loop {
            match tokio::task::block_in_place(|| log_rx.recv_timeout(Duration::from_millis(500))) {
                Ok(log_msg) => {
                    if let Ok(mut state_guard) = app_state_log_clone.lock() {
                        state_guard.add_log_message(log_msg);
                    } else {
                        eprintln!("[LOG_LISTENER_TASK] AppState mutex poisoned, exiting.");
                        break;
                    }
                }
                Err(_) => {
                    eprintln!("[LOG_LISTENER_TASK] log_rx.recv() returned Err (channel closed or task aborting?), exiting.");
                    break;
                }
            }
        }
        eprintln!("[LOG_LISTENER_TASK] Terminated.");
    });

    
    // CSI listener thread
    // Add explicit type annotation for esp_source_csi_reader_clone
    let esp_source_csi_reader_clone: Arc<TokioMutex<Esp32Source>> = Arc::clone(&esp_source);
    let app_state_csi_clone = Arc::clone(&app_state);
    
    let csi_listener_handle: JoinHandle<()> = tokio::spawn(async move {
        info!("CSI listener thread started.");
        let mut read_buffer = vec![0u8; 4096]; // Max expected CSI payload size
        let mut esp_adapter = ESP32Adapter::new(false); // Assuming no scaling

        loop {
            let data_result = {
                let mut source_guard = esp_source_csi_reader_clone.lock().await;
                // Check if source is still running before attempting to read
                if !source_guard.is_running.load(std::sync::atomic::Ordering::Relaxed) {
                     info!("CSI listener: Source reported as not running. Exiting.");
                     if let Ok(mut state_guard) = app_state_csi_clone.lock() {
                        state_guard.connection_status = "DISCONNECTED (Source Not Running)".to_string();
                     }
                     break;
                }
                source_guard.read(&mut read_buffer).await
            };

            match data_result {
                Ok(0) => { /* timeout or no data, yield for a bit */ tokio::time::sleep(Duration::from_millis(10)).await; }
                Ok(n) => {
                    let raw_csi_payload = read_buffer[..n].to_vec();
                    let data_msg = DataMsg::RawFrame {
                        // The adapter uses the timestamp from the packet bytes.
                        // `ts` here is a frame timestamp if source provides one, ESP32Source doesn't directly.
                        ts: Local::now().timestamp_micros() as f64 / 1_000_000.0, 
                        bytes: raw_csi_payload,
                        source_type: SourceType::ESP32,
                    };

                    match esp_adapter.produce(data_msg).await {
                        Ok(Some(DataMsg::CsiFrame { csi })) => {
                            if let Ok(mut state_guard) = app_state_csi_clone.lock() {
                                state_guard.csi_data.push(csi);
                                if state_guard.csi_data.len() > 1000 { // Max buffer for UI
                                    state_guard.csi_data.remove(0);
                                }
                            }
                        }
                        Ok(None) => { /* Adapter needs more data or empty frame */ }
                        Err(e) => { warn!("ESP32Adapter failed to parse CSI: {:?}", e); }
                        Ok(Some(DataMsg::RawFrame{..})) => { /* Should not happen if adapter produces CsiFrame */ }
                    }
                }
                Err(DataSourceError::Controller(msg)) if msg.contains("Source stopped") || msg.contains("CSI data channel disconnected") => {
                    info!("CSI listener: Source stopped or channel disconnected. Error: {}", msg);
                    if let Ok(mut state_guard) = app_state_csi_clone.lock() {
                         state_guard.connection_status = "DISCONNECTED (CSI Read Error)".to_string();
                    }
                    break;
                }
                Err(DataSourceError::Io(io_err)) if io_err.kind() == std::io::ErrorKind::BrokenPipe || io_err.kind() == std::io::ErrorKind::NotConnected || io_err.kind() == std::io::ErrorKind::PermissionDenied => {
                    info!("CSI listener: Serial port disconnected or permission error ({:?}).", io_err.kind());
                     if let Ok(mut state_guard) = app_state_csi_clone.lock() {
                         state_guard.connection_status = format!("DISCONNECTED (IO: {:?})", io_err.kind()).to_string();
                     }
                    break;
                }
                Err(e) => {
                    warn!("CSI listener: error reading from ESP32 source: {:?}", e);
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
            }
            
            // Check global connection status
            if let Ok(state) = app_state_csi_clone.lock() {
                if state.connection_status.starts_with("DISCONNECTED") {
                    info!("CSI listener detected global DISCONNECTED status. Stopping.");
                    break;
                }
            } else {
                error!("CSI listener: AppState mutex poisoned. Stopping.");
                break;
            }
        }
        info!("CSI listener thread stopped.");
    });


    // Main UI loop with async event handling
    let mut event_stream = EventStream::new();

    loop {
        {
            let app_state_guard = app_state.lock().unwrap();
            terminal.draw(|f| ui(f, &app_state_guard))?;
        }

        let event_future = event_stream.next();
        tokio::select! {
            maybe_event = event_future => {
                match maybe_event {
                    Some(Ok(event)) => {
                        if let CEvent::Key(key_event) = event {
                            if handle_input(key_event.code, &esp_source, &app_state).await? {
                                break; // Quit
                            }
                        }
                    }
                    Some(Err(e)) => {
                        error!("Error reading input event: {}", e);
                        // Potentially break or handle error
                    }
                    None => break, // Event stream closed
                }
            }
            _ = tokio::time::sleep(Duration::from_millis(200)) => {
                // Timeout: This allows the loop to periodically check other conditions
                // or refresh UI even if no input events occur.
                // Useful if there are background tasks that might change app_state.
            }
        }
         // Check global connection status for loop exit outside of handle_input
        if let Ok(state) = app_state.lock() {
            if state.connection_status.starts_with("DISCONNECTED") {
                 // If it's a controlled disconnect by 'q', handle_input already signals break.
                 // This is for other disconnects detected by the listener thread.
                if state.last_error.as_deref() != Some("DISCONNECTED (by user)") {
                    info!("Main loop detected DISCONNECTED state (not by user 'q'). Shutting down.");
                    break; 
                }
            }
        } else {
            error!("Main loop: AppState mutex poisoned. Shutting down.");
            break;
        }
    }

    
    restore_terminal(&mut terminal)?;
    eprintln!("[DEBUG] Terminal restored.");
    
    // Graceful shutdown sequence
    eprintln!("[DEBUG] Main UI loop exited. Beginning shutdown sequence...");


    eprintln!("[DEBUG] Attempting to stop ESP32 source...");
    {
        let mut esp_guard = esp_source.lock().await;
        if esp_guard.is_running.load(std::sync::atomic::Ordering::Relaxed) {
            if let Err(e) = esp_guard.stop().await {
                error!("Error stopping ESP32 source: {}", e); // TuiLogger
                eprintln!("[DEBUG] Error stopping ESP32 source: {}", e);
            } else {
                info!("ESP32 source stop command issued successfully."); // TuiLogger
                eprintln!("[DEBUG] ESP32 source stop command issued successfully.");
            }
        } else {
            info!("ESP32 source was already stopped or not initially started."); // TuiLogger
            eprintln!("[DEBUG] ESP32 source was already stopped or not initially started.");
        }
    }
    eprintln!("[DEBUG] ESP32 source stop sequence finished.");


    
    eprintln!("[DEBUG] Waiting for CSI listener task to complete (max 5s)...");
    match tokio::time::timeout(Duration::from_secs(5), csi_listener_handle).await {
        Ok(Ok(_)) => {
            info!("CSI listener task finished gracefully."); // TuiLogger
            eprintln!("[DEBUG] CSI listener task finished gracefully.");
        }
        Ok(Err(e)) => {
            error!("CSI listener task panicked: {:?}", e); // TuiLogger
            eprintln!("[DEBUG] CSI listener task panicked: {:?}", e);
        }
        Err(_) => {
            warn!("CSI listener task timed out during shutdown."); // TuiLogger
            eprintln!("[DEBUG] CSI listener task timed out during shutdown.");
        }
    }


    eprintln!("[DEBUG] Signaling log listener task to stop (dropping log_tx)...");
    drop(log_tx);

    eprintln!("[DEBUG] Attempting to shutdown log listener task...");
    // The log_listener_handle is now the one spawned with the recv_timeout loop.
    log_listener_handle.abort(); // Signal the task to abort.

    // Wait for it to actually terminate. With the recv_timeout loop, it should.
    eprintln!("[DEBUG] Waiting for log listener task to complete after abort (max 2s)...");
    match tokio::time::timeout(Duration::from_secs(2), log_listener_handle).await {
        Ok(Ok(_)) => {
            eprintln!("[DEBUG] Log listener task finished gracefully after abort.");
        }
        Ok(Err(e)) => { // Task panicked
            eprintln!("[DEBUG] Log listener task panicked during shutdown: {:?}", e);
        }
        Err(_) => { // Timeout
            eprintln!("[DEBUG] Log listener task did NOT terminate even after abort and timeout. This is unexpected with recv_timeout loop.");
            // This case should ideally not happen if the recv_timeout loop is implemented correctly.
        }
    }
    
    eprintln!("[DEBUG] All tasks awaited. About to restore terminal.");
    
    eprintln!("[DEBUG] Rust main function is completing NOW.");
    std::thread::sleep(Duration::from_millis(500)); // Increased sleep
    Ok(())
}

// Returns true if the application should exit
async fn handle_input(
    key: KeyCode,
    esp_source_mutex: &Arc<TokioMutex<Esp32Source>>,
    app_state_mutex: &Arc<StdMutex<AppState>>,
) -> Result<bool, Box<dyn Error>> {
    let mut app_state_guard = app_state_mutex.lock().unwrap();
    if key != KeyCode::Char('r') {
        app_state_guard.last_error = None;
    }

    if app_state_guard.is_editing_spam_config && app_state_guard.ui_mode == UiMode::Spam {
        // Editing logic remains synchronous as it only changes AppState
        match key {
            KeyCode::Char('e') | KeyCode::Esc => {
                app_state_guard.is_editing_spam_config = false;
                info!("Exited spam config editing mode.");
            }
            KeyCode::Tab => {
                app_state_guard.current_editing_field =
                    app_state_guard.current_editing_field.next();
            }
            KeyCode::Up => match app_state_guard.current_editing_field {
                SpamConfigField::SrcMacOctet(i) => {
                    app_state_guard.spam_config_src_mac[i] =
                        app_state_guard.spam_config_src_mac[i].wrapping_add(1);
                }
                SpamConfigField::DstMacOctet(i) => {
                    app_state_guard.spam_config_dst_mac[i] =
                        app_state_guard.spam_config_dst_mac[i].wrapping_add(1);
                }
                SpamConfigField::Reps => {
                    app_state_guard.spam_config_n_reps =
                        app_state_guard.spam_config_n_reps.saturating_add(1).max(0);
                }
                SpamConfigField::PauseMs => {
                    app_state_guard.spam_config_pause_ms = app_state_guard
                        .spam_config_pause_ms
                        .saturating_add(1)
                        .max(0);
                }
            },
            KeyCode::Down => match app_state_guard.current_editing_field {
                SpamConfigField::SrcMacOctet(i) => {
                    app_state_guard.spam_config_src_mac[i] =
                        app_state_guard.spam_config_src_mac[i].wrapping_sub(1);
                }
                SpamConfigField::DstMacOctet(i) => {
                    app_state_guard.spam_config_dst_mac[i] =
                        app_state_guard.spam_config_dst_mac[i].wrapping_sub(1);
                }
                SpamConfigField::Reps => {
                    app_state_guard.spam_config_n_reps =
                        (app_state_guard.spam_config_n_reps - 1).max(0);
                }
                SpamConfigField::PauseMs => {
                    app_state_guard.spam_config_pause_ms =
                        (app_state_guard.spam_config_pause_ms - 1).max(0);
                }
            },
            _ => {}
        }
        return Ok(false);
    }
    
    // Drop guard before await points if possible, or re-acquire if state modified after await
    // For now, most ESP interactions are atomic from AppState's perspective for one key press.
    // So we hold the guard. If complex multi-await logic is needed, this might change.
    
    let mut esp_guard = esp_source_mutex.lock().await;

    match key {
        KeyCode::Char('q') => {
            info!("'q' pressed, attempting to disconnect ESP32 source.");
            if app_state_guard.ui_mode == UiMode::Spam {
                info!("Pausing WiFi transmit before disconnecting...");
                if let Err(e) = esp_guard.send_esp32_command(Esp32Command::PauseWifiTransmit, None).await {
                    warn!("Failed to pause WiFi transmit: {e}");
                }
            }
            // stop() will be called by main loop's cleanup
            // Here we just signal the app to shut down.
            app_state_guard.connection_status = "DISCONNECTED (by user)".to_string();
            app_state_guard.last_error = Some("DISCONNECTED (by user)".to_string()); // For footer display
            // The main loop will detect this status and trigger stop() and exit.
            return Ok(true); // Signal to exit application
        }
        KeyCode::Char('m') => {
            let new_ui_mode = match app_state_guard.ui_mode {
                UiMode::Csi => UiMode::Spam,
                UiMode::Spam => UiMode::Csi,
            };
            let new_esp_op_mode = match new_ui_mode {
                UiMode::Csi => EspOperationMode::Receive,
                UiMode::Spam => EspOperationMode::Transmit,
            };

            let new_config_payload = Esp32DeviceConfigPayload { // Renamed variable to avoid conflict
                mode: new_esp_op_mode,
                bandwidth: app_state_guard.current_cli_config.bandwidth,
                secondary_channel: app_state_guard.current_cli_config.secondary_channel,
                csi_type: app_state_guard.current_cli_config.csi_type,
                manual_scale: app_state_guard.current_cli_config.manual_scale,
            };
            
            let mut cmd_data = Vec::new();
            cmd_data.push(new_config_payload.mode as u8);
            cmd_data.push(new_config_payload.bandwidth as u8);
            cmd_data.push(new_config_payload.secondary_channel as u8);
            cmd_data.push(new_config_payload.csi_type as u8);
            cmd_data.push(new_config_payload.manual_scale);

            if let Err(e) = esp_guard.send_esp32_command(Esp32Command::ApplyDeviceConfig, Some(cmd_data)).await {
                let err_msg = format!("Failed to set ESP mode via ApplyDeviceConfig: {e}");
                app_state_guard.last_error = Some(err_msg.clone());
                error!("{err_msg}");
            } else {
                app_state_guard.ui_mode = new_ui_mode;
                app_state_guard.current_cli_config.mode = new_esp_op_mode;
                info!("ESP32 mode set to {:?} via ApplyDeviceConfig", new_esp_op_mode);

                // ESP32Source's ApplyDeviceConfig should implicitly handle Pause/Unpause Acquisition based on mode.
                // Additionally, for Spam mode, explicitly pause general transmit task.
                if new_esp_op_mode == EspOperationMode::Transmit {
                    info!("ESP32 in Transmit mode. WiFi transmit task will be explicitly PAUSED initially.");
                    if let Err(e_pause) = esp_guard.send_esp32_command(Esp32Command::PauseWifiTransmit, None).await {
                        let err_msg = format!("Failed to initially PAUSE transmit in Spam mode: {e_pause}");
                        app_state_guard.last_error = Some(err_msg.clone());
                        error!("{err_msg}");
                    } else {
                        info!("WiFi transmit task PAUSED. Ready for custom frames ('s').");
                    }
                } else { // CSI Mode (Receive)
                    // Ensure transmit is paused (or let ESP handle it) and acquisition is unpaused (handled by ApplyDeviceConfig)
                     info!("Switched to CSI mode. ESP32 general WiFi transmit task should be inactive. CSI acquisition should be active.");
                }
            }
        }
        KeyCode::Char('e') => {
            if app_state_guard.ui_mode == UiMode::Spam {
                app_state_guard.is_editing_spam_config = !app_state_guard.is_editing_spam_config;
                if app_state_guard.is_editing_spam_config {
                    info!("Entered spam config editing mode. Use Tab, Up/Down. 'e' or Esc to exit.");
                    app_state_guard.current_editing_field = SpamConfigField::SrcMacOctet(0);
                } else {
                    info!("Exited spam config editing mode.");
                }
            } else {
                app_state_guard.last_error = Some(
                    "Spam config editing ('e') only available in Spam mode ('m').".to_string(),
                );
            }
        }
        KeyCode::Char('s') => { // Send Spam
            if app_state_guard.ui_mode == UiMode::Spam {
                if app_state_guard.is_editing_spam_config {
                    app_state_guard.last_error = Some(
                        "Exit editing mode ('e' or Esc) before sending spam ('s').".to_string(),
                    );
                } else {
                    info!("Attempting to send custom frames with current config...");
                    let src_mac = app_state_guard.spam_config_src_mac;
                    let dst_mac = app_state_guard.spam_config_dst_mac;
                    let n_reps = app_state_guard.spam_config_n_reps;
                    let pause_ms = app_state_guard.spam_config_pause_ms;

                    // Ensure ESP32 is in Transmit mode before sending
                    if app_state_guard.current_cli_config.mode != EspOperationMode::Transmit {
                        let err_msg = "ESP32 not in Transmit mode. Switch mode first ('m').".to_string();
                        app_state_guard.last_error = Some(err_msg.clone());
                        warn!("{}", err_msg);
                    } else {
                        let mut tx_data = Vec::with_capacity(20);
                        tx_data.extend_from_slice(&dst_mac); // Note: firmware expects dst_mac first
                        tx_data.extend_from_slice(&src_mac);
                        tx_data.extend_from_slice(&n_reps.to_le_bytes());
                        tx_data.extend_from_slice(&pause_ms.to_le_bytes());
                        
                        if let Err(e) = esp_guard.send_esp32_command(Esp32Command::TransmitCustomFrame, Some(tx_data)).await {
                            let err_msg = format!("Failed to send custom frame: {e}");
                            app_state_guard.last_error = Some(err_msg.clone());
                            error!("TUI: {}",err_msg);
                        } else {
                            info!("TransmitCustomFrame command sent to ESP32.");
                        }
                    }
                }
            } else {
                app_state_guard.last_error =
                    Some("Send Spam ('s') command only active in Spam mode ('m').".to_string());
            }
        }
        KeyCode::Char('c') => { // Change Channel
            let mut new_channel = app_state_guard.current_cli_config.channel + 1;
            if new_channel > 11 { // Common 2.4GHz channels limit
                new_channel = 1;
            }
            if let Err(e) = esp_guard.send_esp32_command(Esp32Command::SetChannel, Some(vec![new_channel])).await {
                app_state_guard.last_error = Some(format!("Failed to set channel: {e}"));
            } else {
                app_state_guard.current_cli_config.channel = new_channel;
                info!("ESP32 channel changed to {}", new_channel);
            }
        }
         KeyCode::Char('b') => { // Toggle Bandwidth (20/40)
            let new_bandwidth_is_40 = app_state_guard.current_cli_config.bandwidth == EspBandwidth::Twenty;
            
            let new_esp_bw = if new_bandwidth_is_40 { EspBandwidth::Forty } else { EspBandwidth::Twenty };
            let new_secondary_chan = if new_bandwidth_is_40 {
                // Default to Above for 40MHz, could be configurable or cycle
                EspSecondaryChannel::Above 
            } else {
                EspSecondaryChannel::None
            };

            let mut current_config = app_state_guard.current_cli_config.clone();
            current_config.bandwidth = new_esp_bw;
            current_config.secondary_channel = new_secondary_chan;
            
            let payload = Esp32DeviceConfigPayload {
                mode: current_config.mode,
                bandwidth: current_config.bandwidth,
                secondary_channel: current_config.secondary_channel,
                csi_type: current_config.csi_type,
                manual_scale: current_config.manual_scale,
            };
            let mut cmd_data = Vec::new();
            cmd_data.push(payload.mode as u8);
            cmd_data.push(payload.bandwidth as u8);
            cmd_data.push(payload.secondary_channel as u8);
            cmd_data.push(payload.csi_type as u8);
            cmd_data.push(payload.manual_scale);

            if let Err(e) = esp_guard.send_esp32_command(Esp32Command::ApplyDeviceConfig, Some(cmd_data)).await {
                app_state_guard.last_error = Some(format!("Failed to set bandwidth: {e}"));
            } else {
                app_state_guard.current_cli_config = current_config;
                info!("ESP32 bandwidth set to {:?}, secondary {:?}", new_esp_bw, new_secondary_chan);
            }
        }
        KeyCode::Char('l') => { // Toggle LTF Type
            let new_csi_type_is_legacy = app_state_guard.current_cli_config.csi_type == EspCsiType::HighThroughputLTF;
            let new_esp_csi_type = if new_csi_type_is_legacy { EspCsiType::LegacyLTF } else { EspCsiType::HighThroughputLTF };

            let mut current_config = app_state_guard.current_cli_config.clone();
            current_config.csi_type = new_esp_csi_type;

            let payload = Esp32DeviceConfigPayload {
                mode: current_config.mode,
                bandwidth: current_config.bandwidth,
                secondary_channel: current_config.secondary_channel,
                csi_type: current_config.csi_type,
                manual_scale: current_config.manual_scale,
            };
             let mut cmd_data = Vec::new();
            cmd_data.push(payload.mode as u8);
            cmd_data.push(payload.bandwidth as u8);
            cmd_data.push(payload.secondary_channel as u8);
            cmd_data.push(payload.csi_type as u8);
            cmd_data.push(payload.manual_scale);

            if let Err(e) = esp_guard.send_esp32_command(Esp32Command::ApplyDeviceConfig, Some(cmd_data)).await {
                app_state_guard.last_error = Some(format!("Failed to set CSI type: {e}"));
            } else {
                app_state_guard.current_cli_config = current_config;
                info!("ESP32 CSI type set to {:?}", new_esp_csi_type);
            }
        }
        KeyCode::Char('r') => { /* No action, error cleared by other keys */ }
        KeyCode::Up => { // Clear CSI data buffer
            if !app_state_guard.is_editing_spam_config {
                app_state_guard.csi_data.clear();
                info!("CSI data buffer cleared.");
            }
        }
        KeyCode::Down => { // Synchronize Time
            if !app_state_guard.is_editing_spam_config {
                 if let Err(e) = esp_guard.send_esp32_command(Esp32Command::SynchronizeTimeInit, None).await {
                    app_state_guard.last_error = Some(format!("Time sync init failed: {e}"));
                 } else {
                    let time_us = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map_err(|se| ControllerError::Execution(format!("SystemTimeError: {}", se)))?
                        .as_micros() as u64;
                    if let Err(e) = esp_guard.send_esp32_command(Esp32Command::SynchronizeTimeApply, Some(time_us.to_le_bytes().to_vec())).await {
                        app_state_guard.last_error = Some(format!("Time sync apply failed: {e}"));
                    } else {
                        info!("Time synchronization requested with ESP32.");
                    }
                 }
            }
        }
        _ => {}
    }
    Ok(false)
}


fn ui(f: &mut Frame, app_state: &AppState) {
    let main_horizontal_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .margin(1)
        .constraints([
            Constraint::Percentage(40),
            Constraint::Percentage(60),
        ])
        .split(f.size());

    let left_panel_area = main_horizontal_chunks[0];
    let log_panel_area = main_horizontal_chunks[1];

    let status_area_height = if app_state.ui_mode == UiMode::Spam {
        if app_state.is_editing_spam_config { 8 } else { 6 }
    } else {
        3 // Reduced CSI status height
    };

    let left_vertical_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(status_area_height),
            Constraint::Min(0),    // Table area
            Constraint::Length(3), // Footer area
        ])
        .split(left_panel_area);

    let status_area = left_vertical_chunks[0];
    let table_area = left_vertical_chunks[1];
    let footer_area = left_vertical_chunks[2];

    // Status Block
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
                app_state.current_cli_config.secondary_channel, // This is EspSecondaryChannel
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
        // Spam config display (remains largely the same)
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

    // CSI Data Table - Updated for CsiData struct
    let table_header_cells = [
        "Timestamp (s)", "Seq", "RSSI (Rx0)", "Subcarriers", /* Removed MACs, AGC, FFT Gain */
    ]
    .iter()
    .map(|h| Cell::from(*h).style(Style::default().fg(Color::Yellow)));
    let table_header = Row::new(table_header_cells).height(1).bottom_margin(0);

    let rows: Vec<Row> = app_state
        .csi_data
        .iter()
        .rev() // Show newest first
        .take(table_area.height.saturating_sub(2) as usize) // -2 for header and border
        .map(|p: &CsiData| { // p is &CsiData
            let num_subcarriers = if !p.csi.is_empty() && !p.csi[0].is_empty() {
                p.csi[0][0].len()
            } else {
                0
            };
            let rssi_str = p.rssi.get(0).map_or_else(|| "N/A".to_string(), |r| r.to_string());

            Row::new(vec![
                Cell::from(format!("{:.6}", p.timestamp)), // Display f64 timestamp
                Cell::from(p.sequence_number.to_string()),
                Cell::from(rssi_str),
                Cell::from(num_subcarriers.to_string()),
            ])
        })
        .collect();

    let table_widths = [ // Adjusted widths
        Constraint::Length(18), // Timestamp
        Constraint::Length(8),  // Seq
        Constraint::Length(12), // RSSI
        Constraint::Length(12), // Subcarriers
    ];
    let table = Table::new(rows, &table_widths)
        .header(table_header)
        .block(Block::default().borders(Borders::ALL).title(" CSI Data Packets "))
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED))
        .highlight_symbol(">> ");
    f.render_widget(table, table_area);

    // Log Output List (remains largely the same)
    let log_items_list: Vec<ListItem> = app_state
        .log_messages
        .iter()
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
        })
        .collect();
    
    let num_logs_to_show = log_panel_area.height.saturating_sub(2) as usize;
    let current_log_count = log_items_list.len();
    let visible_log_items: Vec<ListItem> = if current_log_count > num_logs_to_show {
        log_items_list.into_iter().skip(current_log_count - num_logs_to_show).collect()
    } else {
        log_items_list
    };

    let logs_list_widget = List::new(visible_log_items)
        .block(Block::default().borders(Borders::ALL).title(" Log Output "));
    f.render_widget(logs_list_widget, log_panel_area);

    // Footer (Info/Errors)
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
        "Controls: [Q]uit | [M]ode | [S]end Spam | [E]dit Spam Cfg (Spam mode) | [C]hannel | [B]W | [L]TF | [↑]Clr CSI | [↓]Sync Time"
            .to_string()
    };
    let footer_style = if app_state.last_error.is_some() { Style::default().fg(Color::Red) }
    else if app_state.is_editing_spam_config { Style::default().fg(Color::Cyan) }
    else { Style::default().fg(Color::DarkGray) };

    let footer_paragraph = Paragraph::new(footer_text_str)
        .style(footer_style)
        .block(Block::default().borders(Borders::ALL).title(" Info/Errors "));
    f.render_widget(footer_paragraph, footer_area);
}