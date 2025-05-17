// cli_test_tool.rs
//! Robust CLI tool for ESP32 CSI monitoring

use std::collections::VecDeque; // For log buffer
use std::env;
use std::error::Error;
use std::io;
use std::sync::{Arc, Mutex};
use std::thread::{self, sleep};
use std::time::Duration;

use chrono::{DateTime, Local};

// Logging facade and our custom logger components
use crossbeam_channel::{Receiver, Sender};
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
    style::{Color, Modifier, Style, Stylize}, // Stylize can be useful for .fg(Color), .bold(), etc.
    text::{Line, Span, Text}, // Replaced Spans with Line, Text is a collection of Lines
    widgets::{Block, Borders, Cell, List, ListItem, Paragraph, Row, Table},
};

mod esp32;
use esp32::{
    Bandwidth, CsiPacket, CsiType, DeviceConfig as EspDeviceConfig, Esp32, OperationMode,
    SecondaryChannel,
};

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
    log_sender: Sender<LogEntry>,
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
                    "TUI_LOGGER_ERROR: Failed to send log to TUI: {} - {}",
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
    sender: Sender<LogEntry>,
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

struct AppState {
    ui_mode: UiMode,
    current_esp_config: EspDeviceConfig,
    csi_data: Vec<CsiPacket>,
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
    fn new(initial_esp_config: EspDeviceConfig) -> Self {
        Self {
            ui_mode: UiMode::Csi,
            current_esp_config: initial_esp_config,
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

fn main() -> Result<(), Box<dyn Error>> {
    let (log_tx, log_rx): (Sender<LogEntry>, Receiver<LogEntry>) = crossbeam_channel::bounded(200);

    let log_level = env::var("RUST_LOG")
        .ok()
        .and_then(|s| s.parse::<LevelFilter>().ok())
        .unwrap_or(LevelFilter::Info);

    if let Err(e) = init_tui_logger(log_level, log_tx) {
        eprintln!("FATAL: Failed to initialize TUI logger: {e}");
        return Err(e.into());
    }

    info!("CLI tool starting up...");

    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
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
    let port_name = &args[1];
    info!("Attempting to use port: {port_name}");

    let mut terminal = match setup_terminal() {
        Ok(term) => term,
        Err(e) => {
            error!("Failed to setup terminal: {e}");
            return Err(e);
        }
    };

    let esp_mutex = match Esp32::new(port_name, 3_000_000) {
        Ok(esp_instance) => Arc::new(Mutex::new(esp_instance)),
        Err(e) => {
            let _ = restore_terminal(&mut terminal);
            error!("Failed to initialize ESP32: {e}");
            return Err(e.into());
        }
    };

    let initial_esp_config_for_appstate: EspDeviceConfig = match esp_mutex.lock() {
        Ok(mut esp_guard) => {
            if let Err(e) = esp_guard.connect() {
                let _ = restore_terminal(&mut terminal);
                error!("Failed to connect to ESP32: {e}");
                return Err(e.into());
            }
            info!("ESP32 connected, basic configuration applied.");
            info!("Attempting to clear MAC address whitelist...");
            if let Err(e) = esp_guard.clear_mac_filters() {
                warn!("Failed to clear MAC whitelist: {e}. CSI reception might be filtered.");
            } else {
                info!("MAC address whitelist cleared successfully.");
            }
            esp_guard.config.clone()
        }
        Err(poisoned) => {
            let _ = restore_terminal(&mut terminal);
            error!("ESP32 Mutex poisoned during initial connect: {poisoned}");
            return Err("Mutex poisoned".into());
        }
    };
    info!("ESP32 setup complete.");

    let app_state = Arc::new(Mutex::new(AppState::new(initial_esp_config_for_appstate)));
    app_state.lock().unwrap().connection_status = "CONNECTED".to_string();

    let app_state_log_clone = Arc::clone(&app_state);
    thread::spawn(move || {
        while let Ok(log_msg) = log_rx.recv() {
            if let Ok(mut state_guard) = app_state_log_clone.lock() {
                state_guard.add_log_message(log_msg);
            } else {
                break;
            }
        }
    });

    let state_clone_csi = app_state.clone();
    let csi_rx_channel = esp_mutex.lock().unwrap().csi_receiver();

    thread::spawn(move || {
        info!("CSI listener thread started.");
        loop {
            match csi_rx_channel.recv_timeout(Duration::from_secs(1)) {
                Ok(packet) => {
                    info!(
                        "CSI LISTENER THREAD RECEIVED PACKET: ts={}",
                        packet.timestamp_us
                    );
                    if let Ok(mut state_guard) = state_clone_csi.lock() {
                        state_guard.csi_data.push(packet);
                        if state_guard.csi_data.len() > 1000 {
                            state_guard.csi_data.remove(0);
                        }
                    } else {
                        break;
                    }
                }
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => { /* Continue */ }
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                    info!("CSI channel disconnected.");
                    if let Ok(mut state_guard) = state_clone_csi.lock() {
                        state_guard.connection_status =
                            "DISCONNECTED (CSI Channel Closed)".to_string();
                    }
                    break;
                }
            }
        }
        info!("CSI listener thread stopped.");
    });

    // Main UI loop
    loop {
        {
            let app_state_guard = app_state.lock().unwrap();
            terminal.draw(|f| ui(f, &app_state_guard))?;
        }
        if crossterm::event::poll(Duration::from_millis(100))? {
            if let CEvent::Key(key_event) = crossterm::event::read()? {
                if handle_input(key_event.code, &esp_mutex, &app_state)? {
                    break;
                }
            }
        }
    }

    restore_terminal(&mut terminal)?;
    sleep(Duration::from_millis(100));
    info!("CLI tool shutting down.");
    Ok(())
}

// Returns true if the application should exit
fn handle_input(
    key: KeyCode,
    esp_mutex: &Arc<Mutex<Esp32>>,
    app_state_mutex: &Arc<Mutex<AppState>>,
) -> Result<bool, Box<dyn Error>> {
    let mut app_state_guard = app_state_mutex.lock().unwrap();
    if key != KeyCode::Char('r') {
        app_state_guard.last_error = None;
    }

    if app_state_guard.is_editing_spam_config && app_state_guard.ui_mode == UiMode::Spam {
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

    match key {
        KeyCode::Char('q') => {
            info!("'q' pressed, attempting to disconnect ESP32.");
            match esp_mutex.lock() {
                Ok(mut esp_guard) => {
                    if app_state_guard.ui_mode == UiMode::Spam {
                        info!("Pausing WiFi transmit before disconnecting...");
                        if let Err(e) = esp_guard.pause_wifi_transmit() {
                            warn!("Failed to pause WiFi transmit: {e}");
                        }
                    }
                    if let Err(e) = esp_guard.disconnect() {
                        app_state_guard.last_error = Some(format!("Error during disconnect: {e}"));
                        error!("Error disconnecting ESP32: {e}");
                    } else {
                        info!("ESP32 disconnected successfully by user.");
                    }
                }
                Err(poisoned) => {
                    let err_msg = format!("ESP32 Mutex poisoned on disconnect: {poisoned}");
                    app_state_guard.last_error = Some(err_msg.clone());
                    error!("{err_msg}");
                }
            }
            app_state_guard.connection_status = "DISCONNECTED (by user)".to_string();
            return Ok(true);
        }
        KeyCode::Char('m') => {
            let new_ui_mode = match app_state_guard.ui_mode {
                UiMode::Csi => UiMode::Spam,
                UiMode::Spam => UiMode::Csi,
            };
            let new_esp_op_mode = match new_ui_mode {
                UiMode::Csi => OperationMode::Receive,
                UiMode::Spam => OperationMode::Transmit,
            };

            match esp_mutex.lock() {
                Ok(mut esp_guard) => {
                    esp_guard.config.mode = new_esp_op_mode;
                    if let Err(e) = esp_guard.apply_device_config() {
                        app_state_guard.last_error =
                            Some(format!("Failed to set ESP mode via ApplyDeviceConfig: {e}"));
                        error!("Failed to apply device config for mode change: {e}");
                    } else {
                        app_state_guard.ui_mode = new_ui_mode;
                        app_state_guard.current_esp_config.mode = new_esp_op_mode;
                        info!("ESP32 mode set to {new_esp_op_mode:?} via ApplyDeviceConfig");

                        if new_esp_op_mode == OperationMode::Transmit {
                            info!(
                                "ESP32 in Transmit mode. WiFi transmit task will be explicitly PAUSED initially."
                            );
                            info!("Use 's' to send configured custom frames.");
                            if let Err(e_pause) = esp_guard.pause_wifi_transmit() {
                                app_state_guard.last_error = Some(format!(
                                    "Failed to initially PAUSE transmit in Spam mode: {e_pause}"
                                ));
                                error!(
                                    "Failed to PAUSE WiFi transmit after switching to Spam mode: {e_pause}"
                                );
                            } else {
                                info!("WiFi transmit task PAUSED. Ready for custom frames.");
                            }
                        } else {
                            info!(
                                "Switched to CSI mode. ESP32 general WiFi transmit task should be inactive."
                            );
                        }
                    }
                }
                Err(p) => {
                    let err_msg = format!("Mutex error during mode toggle: {p}");
                    app_state_guard.last_error = Some(err_msg.clone());
                    error!("{err_msg}");
                }
            }
        }
        KeyCode::Char('e') => {
            if app_state_guard.ui_mode == UiMode::Spam {
                app_state_guard.is_editing_spam_config = !app_state_guard.is_editing_spam_config;
                if app_state_guard.is_editing_spam_config {
                    info!(
                        "Entered spam config editing mode. Use Tab, Up/Down. 'e' or Esc to exit."
                    );
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
        KeyCode::Char('s') => {
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

                    match esp_mutex.lock() {
                        Ok(mut esp_guard) => {
                            if app_state_guard.current_esp_config.mode != OperationMode::Transmit {
                                let err_msg = "ESP32 not in Transmit mode. This shouldn't happen if UI mode is Spam.".to_string();
                                app_state_guard.last_error = Some(err_msg.clone());
                                warn!("{err_msg}");
                            } else if let Err(e) = esp_guard
                                .transmit_custom_frame(&src_mac, &dst_mac, n_reps, pause_ms)
                            {
                                let err_msg = format!("Failed to send custom frame: {e}");
                                app_state_guard.last_error = Some(err_msg.clone());
                                error!("TUI: {err_msg}");
                            } else {
                                info!(
                                    "TransmitCustomFrame command sent to ESP32 with configured parameters."
                                );
                            }
                        }
                        Err(p_err) => {
                            let err_msg = format!("ESP32 Mutex error for custom frame: {p_err}");
                            app_state_guard.last_error = Some(err_msg.clone());
                            error!("TUI: {err_msg}");
                        }
                    }
                }
            } else {
                app_state_guard.last_error =
                    Some("Send Spam ('s') command only active in Spam mode ('m').".to_string());
            }
        }
        KeyCode::Char('c') => {
            let mut new_channel = app_state_guard.current_esp_config.channel + 1;
            if new_channel > 11 {
                new_channel = 1;
            }

            match esp_mutex.lock() {
                Ok(mut esp_guard) => {
                    if let Err(e) = esp_guard.set_channel(new_channel) {
                        app_state_guard.last_error = Some(format!("Failed to set channel: {e}"));
                    } else {
                        app_state_guard.current_esp_config.channel = new_channel;
                        info!("ESP32 channel changed to {new_channel}");
                    }
                }
                Err(p) => app_state_guard.last_error = Some(format!("Mutex error: {p}")),
            }
        }
        KeyCode::Char('b') => {
            let new_bandwidth_is_40 =
                app_state_guard.current_esp_config.bandwidth == Bandwidth::Twenty;
            match esp_mutex.lock() {
                Ok(mut esp_guard) => {
                    let new_esp_bw = if new_bandwidth_is_40 {
                        Bandwidth::Forty
                    } else {
                        Bandwidth::Twenty
                    };
                    esp_guard.config.bandwidth = new_esp_bw;
                    let new_secondary_chan = if new_bandwidth_is_40 {
                        SecondaryChannel::Above
                    } else {
                        SecondaryChannel::None
                    };
                    esp_guard.config.secondary_channel = new_secondary_chan;

                    if let Err(e) = esp_guard.apply_device_config() {
                        app_state_guard.last_error = Some(format!("Failed to set bandwidth: {e}"));
                    } else {
                        app_state_guard.current_esp_config.bandwidth = new_esp_bw;
                        app_state_guard.current_esp_config.secondary_channel = new_secondary_chan;
                        info!(
                            "ESP32 bandwidth set to {new_esp_bw:?}, secondary {new_secondary_chan:?}"
                        );
                    }
                }
                Err(p) => app_state_guard.last_error = Some(format!("Mutex error: {p}")),
            }
        }
        KeyCode::Char('l') => {
            let new_csi_type_is_legacy =
                app_state_guard.current_esp_config.csi_type == CsiType::HighThroughputLTF;
            match esp_mutex.lock() {
                Ok(mut esp_guard) => {
                    let new_esp_csi_type = if new_csi_type_is_legacy {
                        CsiType::LegacyLTF
                    } else {
                        CsiType::HighThroughputLTF
                    };
                    esp_guard.config.csi_type = new_esp_csi_type;
                    if let Err(e) = esp_guard.apply_device_config() {
                        app_state_guard.last_error = Some(format!("Failed to set CSI type: {e}"));
                    } else {
                        app_state_guard.current_esp_config.csi_type = new_esp_csi_type;
                        info!("ESP32 CSI type set to {new_esp_csi_type:?}");
                    }
                }
                Err(p) => app_state_guard.last_error = Some(format!("Mutex error: {p}")),
            }
        }
        KeyCode::Char('r') => { /* No action, error cleared by other keys */ }
        KeyCode::Up => {
            if !app_state_guard.is_editing_spam_config {
                app_state_guard.csi_data.clear();
                info!("CSI data buffer cleared.");
            }
        }
        KeyCode::Down => {
            if !app_state_guard.is_editing_spam_config {
                match esp_mutex.lock() {
                    Ok(mut esp_guard) => {
                        if let Err(e) = esp_guard.synchronize_time() {
                            app_state_guard.last_error = Some(format!("Failed to sync time: {e}"));
                        } else {
                            info!("Time synchronization requested.");
                        }
                    }
                    Err(p_err) => {
                        app_state_guard.last_error = Some(format!("Mutex error: {p_err}"))
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
            Constraint::Percentage(60), // Left panel
            Constraint::Percentage(40), // Right panel (Logs)
        ])
        .split(f.size());

    let left_panel_area = main_horizontal_chunks[0];
    let log_panel_area = main_horizontal_chunks[1];

    let status_area_height = if app_state.ui_mode == UiMode::Spam {
        if app_state.is_editing_spam_config {
            8
        } else {
            6
        }
    } else {
        3
    };

    let left_vertical_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(status_area_height),
            Constraint::Min(0),
            Constraint::Length(3), // Footer area
        ])
        .split(left_panel_area);

    let status_area = left_vertical_chunks[0];
    let table_area = left_vertical_chunks[1];
    let footer_area = left_vertical_chunks[2];

    let mode_str = match app_state.ui_mode {
        UiMode::Csi => "CSI RX",
        UiMode::Spam => "WiFi SPAM",
    };
    let bw_str = match app_state.current_esp_config.bandwidth {
        Bandwidth::Twenty => "20MHz",
        Bandwidth::Forty => "40MHz",
    };
    let ltf_str = match app_state.current_esp_config.csi_type {
        CsiType::HighThroughputLTF => "HT-LTF",
        CsiType::LegacyLTF => "L-LTF",
    };

    let mut status_lines = vec![
        Line::from(vec![
            Span::raw("ESP32 | "),
            Span::styled(mode_str, Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(format!(
                " | Chan: {} | BW: {} ({:?}) | LTF: {}",
                app_state.current_esp_config.channel,
                bw_str,
                app_state.current_esp_config.secondary_channel,
                ltf_str
            )),
        ]),
        Line::from(vec![
            Span::raw(format!("Buf: {} | Status: ", app_state.csi_data.len())),
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
            let style = if app_state.is_editing_spam_config
                && app_state.current_editing_field == SpamConfigField::SrcMacOctet(i)
            {
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::REVERSED)
            } else {
                Style::default()
            };
            src_mac_spans.push(Span::styled(val_str, style));
            if i < 5 {
                src_mac_spans.push(Span::raw(":"));
            }
        }

        let mut dst_mac_spans = vec![Span::raw("  Dst MAC: ")];
        for i in 0..6 {
            let val_str = format!("{:02X}", app_state.spam_config_dst_mac[i]);
            let style = if app_state.is_editing_spam_config
                && app_state.current_editing_field == SpamConfigField::DstMacOctet(i)
            {
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::REVERSED)
            } else {
                Style::default()
            };
            dst_mac_spans.push(Span::styled(val_str, style));
            if i < 5 {
                dst_mac_spans.push(Span::raw(":"));
            }
        }

        let reps_style = if app_state.is_editing_spam_config
            && app_state.current_editing_field == SpamConfigField::Reps
        {
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::REVERSED)
        } else {
            Style::default()
        };

        let pause_style = if app_state.is_editing_spam_config
            && app_state.current_editing_field == SpamConfigField::PauseMs
        {
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::REVERSED)
        } else {
            Style::default()
        };

        status_lines.push(Line::from("Spam Configuration:")); // Line::from can take &str
        status_lines.push(Line::from(src_mac_spans));
        status_lines.push(Line::from(dst_mac_spans));
        status_lines.push(Line::from(vec![
            Span::raw("  Reps: "),
            Span::styled(app_state.spam_config_n_reps.to_string(), reps_style),
            Span::raw(" | Pause: "),
            Span::styled(app_state.spam_config_pause_ms.to_string(), pause_style),
            Span::raw("ms"),
        ]));
    }

    let header_paragraph = Paragraph::new(Text::from(status_lines)) // Text::from(Vec<Line>)
        .block(Block::default().borders(Borders::ALL).title(" Status "));
    f.render_widget(header_paragraph, status_area);

    let table_header_cells = [
        "Timestamp (us)",
        "Src MAC",
        "Dst MAC",
        "Seq",
        "RSSI",
        "AGC Gain",
        "FFT Gain",
        "CSI Len",
    ]
    .iter()
    .map(|h| Cell::from(*h).style(Style::default().fg(Color::Yellow))); // Can use .yellow() with Stylize trait
    let table_header = Row::new(table_header_cells).height(1).bottom_margin(0);

    let rows: Vec<Row> = app_state
        .csi_data
        .iter()
        .rev()
        .take(table_area.height.saturating_sub(2) as usize)
        .map(|p| {
            let src_mac_str = format!(
                "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
                p.src_mac[0], p.src_mac[1], p.src_mac[2], p.src_mac[3], p.src_mac[4], p.src_mac[5]
            );
            let dst_mac_str = format!(
                "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
                p.dst_mac[0], p.dst_mac[1], p.dst_mac[2], p.dst_mac[3], p.dst_mac[4], p.dst_mac[5]
            );
            Row::new(vec![
                Cell::from(p.timestamp_us.to_string()),
                Cell::from(src_mac_str),
                Cell::from(dst_mac_str),
                Cell::from(p.seq.to_string()),
                Cell::from(p.rssi.to_string()),
                Cell::from(p.agc_gain.to_string()),
                Cell::from(p.fft_gain.to_string()),
                Cell::from(p.csi_data.len().to_string()),
            ])
        })
        .collect();

    let table_widths = [
        Constraint::Length(18),
        Constraint::Length(18),
        Constraint::Length(18),
        Constraint::Length(6),
        Constraint::Length(6),
        Constraint::Length(9),
        Constraint::Length(9),
        Constraint::Length(8),
    ];
    let table = Table::new(rows, &table_widths) // Pass widths to constructor (Ratatui specific) or use .widths()
        .header(table_header)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" CSI Data Packets "),
        )
        // .widths(&table_widths) // Alternative: use builder method if not in constructor
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED))
        .highlight_symbol(">> ");
    f.render_widget(table, table_area);

    let log_items_list: Vec<ListItem> = app_state
        .log_messages
        .iter()
        .map(|entry| {
            let timestamp_str = entry.timestamp.format("%H:%M:%S").to_string();
            let display_msg_with_timestamp =
                format!("{timestamp_str} [{}] {}", entry.level, entry.message);

            let style = match entry.level {
                log::Level::Error => Style::default().fg(Color::Red),
                log::Level::Warn => Style::default().fg(Color::Yellow),
                log::Level::Info => Style::default().fg(Color::Cyan),
                log::Level::Debug => Style::default().fg(Color::Blue),
                log::Level::Trace => Style::default().fg(Color::Magenta),
            };
            ListItem::new(display_msg_with_timestamp).style(style) // String auto-converts to Text
        })
        .collect();

    let num_logs_to_show = log_panel_area.height.saturating_sub(2) as usize;
    let current_log_count = log_items_list.len();
    let visible_log_items: Vec<ListItem> = if current_log_count > num_logs_to_show {
        log_items_list
            .into_iter()
            .skip(current_log_count - num_logs_to_show)
            .collect()
    } else {
        log_items_list
    };

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
        "Controls: [Q]uit | [M]ode | [S]end Spam | [E]dit Spam Cfg (in Spam mode) | [C]hannel | [B]W | [L]TF | [↑]Clr CSI | [↓]Sync Time"
            .to_string()
    };
    let footer_style = if app_state.last_error.is_some() {
        Style::default().fg(Color::Red)
    } else if app_state.is_editing_spam_config {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let footer_paragraph = Paragraph::new(footer_text_str) // String auto-converts to Text
        .style(footer_style)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Info/Errors "),
        );
    f.render_widget(footer_paragraph, footer_area);
}
