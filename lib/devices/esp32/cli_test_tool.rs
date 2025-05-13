// cli_test_tool.rs
//! Robust CLI tool for ESP32 CSI monitoring

use std::collections::VecDeque; // For log buffer
use std::error::Error;
use std::io;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use std::env;

use chrono::{DateTime, Local};

// Logging facade and our custom logger components
use log::{debug, error, info, warn, Level, LevelFilter, Metadata, Record, SetLoggerError}; // Added more log items
use crossbeam_channel::{Sender, Receiver}; // For log channel

use crossterm::{
    event::{Event as CEvent, KeyCode},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use tui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, Cell, List, ListItem, Paragraph, Row, Table}, // Added List, ListItem
    Frame, Terminal,
    text::Spans, // For multi-line list items or paragraphs
};

mod esp32;
use esp32::{
    Bandwidth, CsiPacket, CsiType, Esp32, OperationMode, SecondaryChannel,
    DeviceConfig as EspDeviceConfig,
    // Command as EspCommand, // Not directly used by CLI, remove if truly unused
};


pub struct LogEntry {
    pub timestamp: DateTime<Local>,
    pub level: log::Level,
    pub message: String,
}

// --- Custom TUI Logger ---
struct TuiLogger {
    log_sender: Sender<LogEntry>, // MODIFIED: Was Sender<String>
    level: Level, // This field is used by the enabled() method
}

impl log::Log for TuiLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= self.level
    }

    fn log(&self, record: &Record) {
        if self.enabled(record.metadata()) {
            // MODIFIED: Create and send LogEntry
            let log_entry = LogEntry {
                timestamp: Local::now(), // Capture timestamp at event time
                level: record.level(),
                message: format!("{}", record.args()),
            };
            
            if self.log_sender.try_send(log_entry).is_err() {
                eprintln!("TUI_LOGGER_ERROR: Failed to send log to TUI: {} - {}", record.level(), record.args());
            }
        }
    }

    fn flush(&self) {}
}

fn init_tui_logger(log_level_filter: LevelFilter, sender: Sender<LogEntry>) -> Result<(), SetLoggerError> { // MODIFIED: Sender type
    let logger = TuiLogger { 
        log_sender: sender, 
        level: log_level_filter.to_level().unwrap_or(log::Level::Error) // Ensure TuiLogger.level is initialized
    };
    log::set_boxed_logger(Box::new(logger))?;
    log::set_max_level(log_level_filter);
    Ok(())
}
// --- End Custom TUI Logger ---


// Remove the old setup_logging that uses env_logger
// fn setup_logging() {
//     env_logger::Builder::from_default_env()
//         .format_timestamp_millis()
//         .target(env_logger::Target::Stderr)
//         .filter_level(log::LevelFilter::Info)
//         .init();
// }

fn setup_terminal() -> Result<Terminal<CrosstermBackend<io::Stdout>>, Box<dyn Error>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let terminal = Terminal::new(backend)?;
    Ok(terminal)
}

fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<(), Box<dyn Error>> {
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
    log_messages: VecDeque<LogEntry>, // MODIFIED: Was VecDeque<String>
    max_log_messages: usize,
}

impl AppState {
    fn new(initial_esp_config: EspDeviceConfig) -> Self {
        Self {
            ui_mode: UiMode::Csi,
            current_esp_config: initial_esp_config,
            csi_data: Vec::with_capacity(1000),
            connection_status: "CONNECTING...".into(),
            last_error: None,
            log_messages: VecDeque::with_capacity(200), // Stores LogEntry now
            max_log_messages: 200,
        }
    }

    fn add_log_message(&mut self, entry: LogEntry) { // MODIFIED: Accepts LogEntry
        if self.log_messages.len() >= self.max_log_messages {
            self.log_messages.pop_front();
        }
        self.log_messages.push_back(entry);
    }
}


fn main() -> Result<(), Box<dyn Error>> {
    // Create a channel for log messages
    let (log_tx, log_rx): (Sender<LogEntry>, Receiver<LogEntry>) = crossbeam_channel::bounded(200); // MODIFIED: Channel type

    // Initialize our TUI logger INSTEAD of env_logger
    // Default to Info, can be changed via RUST_LOG or a command-line arg later
    let log_level = env::var("RUST_LOG")
        .ok()
        .and_then(|s| s.parse::<LevelFilter>().ok())
        .unwrap_or(LevelFilter::Info);

    if let Err(e) = init_tui_logger(log_level, log_tx) {
        eprintln!("FATAL: Failed to initialize TUI logger: {}", e);
        // No TUI at this point, so direct eprintln is fine.
        return Err(e.into());
    }

    info!("CLI tool starting up..."); // This will now go to our TUI logger

    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        // These eprintsln! are fine as TUI is not active yet.
        eprintln!("Usage: {} <serial_port>", args[0]);
        eprintln!("Example: {} /dev/ttyUSB0", args[0]);
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
            Err(e) => { eprintln!("  Error listing serial ports: {}",e); }
        }
        return Err("Serial port argument missing".into());
    }
    let port_name = &args[1];
    info!("Attempting to use port: {}", port_name);

    let mut terminal = match setup_terminal() {
        Ok(term) => term,
        Err(e) => {
            error!("Failed to setup terminal: {}", e); // This log will go to TUI if logger init worked
            return Err(e);
        }
    };

    let esp_mutex = match Esp32::new(port_name, 3_000_000) {
        Ok(esp_instance) => Arc::new(Mutex::new(esp_instance)),
        Err(e) => {
            let _ = restore_terminal(&mut terminal);
            error!("Failed to initialize ESP32: {}", e);
            return Err(e.into());
        }
    };

    let initial_esp_config_for_appstate: EspDeviceConfig;
    match esp_mutex.lock() {
        Ok(mut esp_guard) => {
            if let Err(e) = esp_guard.connect() {
                let _ = restore_terminal(&mut terminal);
                error!("Failed to connect to ESP32: {}", e);
                return Err(e.into());
            }
            info!("ESP32 connected, basic configuration applied.");
            info!("Attempting to clear MAC address whitelist...");
            if let Err(e) = esp_guard.clear_mac_filters() {
                warn!("Failed to clear MAC whitelist: {}. CSI reception might be filtered.", e);
            } else {
                info!("MAC address whitelist cleared successfully.");
            }
            initial_esp_config_for_appstate = esp_guard.get_current_config().clone();
        }
        Err(poisoned) => {
            let _ = restore_terminal(&mut terminal);
            error!("ESP32 Mutex poisoned during initial connect: {}", poisoned);
            return Err("Mutex poisoned".into());
        }
    }
    info!("ESP32 setup complete.");

    let app_state = Arc::new(Mutex::new(AppState::new(initial_esp_config_for_appstate)));
    app_state.lock().unwrap().connection_status = "CONNECTED".to_string();


    // --- Log processing thread ---
    let app_state_log_clone = Arc::clone(&app_state);
    thread::spawn(move || {
        // info!("Log listener thread started."); // This info log is fine
        while let Ok(log_msg) = log_rx.recv() {
            if let Ok(mut state_guard) = app_state_log_clone.lock() {
                state_guard.add_log_message(log_msg);
            } else {
                // Fallback if app_state mutex is poisoned
                // eprintln!("TUI_LOG_THREAD_ERR (Mutex): {}", log_msg);
                break; // Exit thread if AppState is inaccessible
            }
        }
        // eprintln!("Log listener thread stopped."); // Log to stderr when TUI might be gone
    });
    // --- End Log processing thread ---

    let state_clone_csi = app_state.clone();
    let csi_rx_channel = esp_mutex.lock().unwrap().csi_receiver(); // Ensure lock is released

    thread::spawn(move || {
        info!("CSI listener thread started."); // Log through TUI logger
        loop {
            match csi_rx_channel.recv_timeout(Duration::from_secs(1)) {
                Ok(packet) => {
                    info!("CSI LISTENER THREAD RECEIVED PACKET: ts={}", packet.timestamp_us);
                    if let Ok(mut state_guard) = state_clone_csi.lock() {
                        state_guard.csi_data.push(packet);
                        if state_guard.csi_data.len() > 1000 {
                            state_guard.csi_data.remove(0);
                        }
                    } else { break; } // Mutex poisoned
                }
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => { /* Continue */ }
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                    info!("CSI channel disconnected.");
                    if let Ok(mut state_guard) = state_clone_csi.lock() {
                        state_guard.connection_status = "DISCONNECTED (CSI Channel Closed)".to_string();
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
    info!("CLI tool shutting down."); // This log might not be seen if terminal is restored too quickly
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

    match key {
        KeyCode::Char('q') => {
            // ... (q handling as before)
            info!("'q' pressed, attempting to disconnect ESP32.");
            match esp_mutex.lock() {
                Ok(mut esp_guard) => {
                    // If current mode is Spam, pause transmission before disconnecting
                    if app_state_guard.ui_mode == UiMode::Spam {
                        info!("Pausing WiFi transmit before disconnecting...");
                        if let Err(e) = esp_guard.pause_wifi_transmit() {
                            warn!("Failed to pause WiFi transmit: {}", e);
                            // Log but continue with disconnect
                        }
                    }
                    if let Err(e) = esp_guard.disconnect() {
                        app_state_guard.last_error = Some(format!("Error during disconnect: {}", e));
                        error!("Error disconnecting ESP32: {}", e);
                    } else {
                        info!("ESP32 disconnected successfully by user.");
                    }
                }
                Err(poisoned) => {
                    let err_msg = format!("ESP32 Mutex poisoned on disconnect: {}", poisoned);
                    app_state_guard.last_error = Some(err_msg.clone());
                    error!("{}", err_msg);
                }
            }
            app_state_guard.connection_status = "DISCONNECTED (by user)".to_string();
            return Ok(true);
        }
        KeyCode::Char('m') => { // Toggle UI Mode (CSI/Spam) and apply to ESP32
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
                    esp_guard.update_config_mode(new_esp_op_mode);
                    if let Err(e) = esp_guard.apply_device_config() {
                        app_state_guard.last_error = Some(format!("Failed to set ESP mode via ApplyDeviceConfig: {}", e));
                    } else {
                        app_state_guard.ui_mode = new_ui_mode;
                        app_state_guard.current_esp_config.mode = new_esp_op_mode;
                        info!("ESP32 mode set to {:?} via ApplyDeviceConfig", new_esp_op_mode);

                        // ***** CRITICAL CHANGE: Resume/Pause Transmit Task *****
                        if new_esp_op_mode == OperationMode::Transmit {
                            info!("Attempting to RESUME WiFi transmit task on ESP32...");
                            if let Err(e_resume) = esp_guard.resume_wifi_transmit() {
                                app_state_guard.last_error = Some(format!("Failed to RESUME transmit: {}", e_resume));
                                error!("Failed to RESUME WiFi transmit: {}", e_resume);
                            } else {
                                info!("WiFi transmit task RESUMED.");
                            }
                        } else { // Switched to Receive or other non-Transmit mode
                            info!("WiFi transmit task PAUSED.");
                        }
                        // ******************************************************
                    }
                }
                Err(p) => app_state_guard.last_error = Some(format!("Mutex error: {}", p)),
            }
        }
        KeyCode::Char('c') => {
            let mut new_channel = app_state_guard.current_esp_config.channel + 1;
            if new_channel > 11 { new_channel = 1; }

            match esp_mutex.lock() {
                Ok(mut esp_guard) => {
                    if let Err(e) = esp_guard.set_channel(new_channel) {
                        app_state_guard.last_error = Some(format!("Failed to set channel: {}", e));
                    } else {
                        app_state_guard.current_esp_config.channel = new_channel; // esp32.set_channel updates its internal config
                        info!("ESP32 channel changed to {}", new_channel);
                    }
                }
                Err(p) => app_state_guard.last_error = Some(format!("Mutex error: {}", p)),
            }
        }
        KeyCode::Char('b') => {
            let new_bandwidth_is_40 = app_state_guard.current_esp_config.bandwidth == Bandwidth::Twenty;
            match esp_mutex.lock() {
                Ok(mut esp_guard) => {
                    let new_esp_bw = if new_bandwidth_is_40 { Bandwidth::Forty } else { Bandwidth::Twenty };
                    esp_guard.update_config_bandwidth(new_esp_bw);
                    let new_secondary_chan = if new_bandwidth_is_40 { SecondaryChannel::Above } else { SecondaryChannel::None };
                    esp_guard.update_config_secondary_channel(new_secondary_chan);

                    if let Err(e) = esp_guard.apply_device_config() {
                        app_state_guard.last_error = Some(format!("Failed to set bandwidth: {}", e));
                    } else {
                        app_state_guard.current_esp_config.bandwidth = new_esp_bw;
                        app_state_guard.current_esp_config.secondary_channel = new_secondary_chan;
                        info!("ESP32 bandwidth set to {:?}, secondary {:?}", new_esp_bw, new_secondary_chan);
                    }
                }
                Err(p) => app_state_guard.last_error = Some(format!("Mutex error: {}", p)),
            }
        }
        KeyCode::Char('l') => {
            let new_csi_type_is_legacy = app_state_guard.current_esp_config.csi_type == CsiType::HTLTF;
            match esp_mutex.lock() {
                Ok(mut esp_guard) => {
                    let new_esp_csi_type = if new_csi_type_is_legacy { CsiType::LegacyLTF } else { CsiType::HTLTF };
                    esp_guard.update_config_csi_type(new_esp_csi_type);
                    if let Err(e) = esp_guard.apply_device_config() {
                        app_state_guard.last_error = Some(format!("Failed to set CSI type: {}", e));
                    } else {
                        app_state_guard.current_esp_config.csi_type = new_esp_csi_type;
                        info!("ESP32 CSI type set to {:?}", new_esp_csi_type);
                    }
                }
                Err(p) => app_state_guard.last_error = Some(format!("Mutex error: {}", p)),
            }
        }
        KeyCode::Char('r') => { /* Pressing 'r' does nothing to the error; it's cleared by other keys */ }
        KeyCode::Up => {
            app_state_guard.csi_data.clear();
            info!("CSI data buffer cleared.");
        }
        KeyCode::Down => {
             match esp_mutex.lock() {
                Ok(mut esp_guard) => {
                    if let Err(e) = esp_guard.synchronize_time() {
                        app_state_guard.last_error = Some(format!("Failed to sync time: {}", e));
                    } else {
                        info!("Time synchronization requested.");
                    }
                }
                Err(p) => app_state_guard.last_error = Some(format!("Mutex error: {}", p)),
            }
        }
        _ => {}
    }
    Ok(false)
}

fn ui<B: tui::backend::Backend>(f: &mut Frame<B>, app_state: &AppState) {
    // No need for `use chrono::Local;` here anymore for timestamp generation,
    // as timestamps are already part of LogEntry. chrono::Local is used by LogEntry's DateTime<Local>.

    // Main layout: Horizontally split screen
    let main_horizontal_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .margin(1) // Apply a margin around the entire UI
        .constraints([
            Constraint::Percentage(60), // Left panel
            Constraint::Percentage(40), // Right panel (Logs)
        ].as_ref())
        .split(f.size());

    let left_panel_area = main_horizontal_chunks[0];
    let log_panel_area = main_horizontal_chunks[1];

    // Left panel layout: Vertically split for Status, Table, Footer
    let left_vertical_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),    // Status area
            Constraint::Min(0),       // Table area
            Constraint::Length(3),    // Footer area
        ].as_ref())
        .split(left_panel_area);

    let status_area = left_vertical_chunks[0];
    let table_area = left_vertical_chunks[1];
    let footer_area = left_vertical_chunks[2];

    // --- Status Header (top-left) ---
    let mode_str = match app_state.ui_mode {
        UiMode::Csi => "CSI RX",
        UiMode::Spam => "WiFi SPAM",
    };
    let bw_str = match app_state.current_esp_config.bandwidth {
        Bandwidth::Twenty => "20MHz",
        Bandwidth::Forty => "40MHz",
    };
    let ltf_str = match app_state.current_esp_config.csi_type {
        CsiType::HTLTF => "HT-LTF",
        CsiType::LegacyLTF => "L-LTF",
    };
    let status_text = format!(
        "ESP32 | {} | Chan: {} | BW: {} ({:?}) | LTF: {} | Buf: {} | Status: {}",
        mode_str,
        app_state.current_esp_config.channel,
        bw_str,
        app_state.current_esp_config.secondary_channel,
        ltf_str,
        app_state.csi_data.len(),
        app_state.connection_status
    );
    let header_paragraph = Paragraph::new(status_text)
        .style(match app_state.connection_status.as_str() {
            s if s.starts_with("CONNECTED") => Style::default().fg(Color::Green),
            _ => Style::default().fg(Color::Red),
        })
        .block(Block::default().borders(Borders::ALL).title(" Status "));
    f.render_widget(header_paragraph, status_area);


    // --- CSI Data Table (middle-left) ---
    let table_header_cells = ["Timestamp (us)", "Src MAC", "Dst MAC", "Seq", "RSSI", "AGC Gain", "FFT Gain", "CSI Len"]
        .iter().map(|h| Cell::from(*h).style(Style::default().fg(Color::Yellow)));
    let table_header = Row::new(table_header_cells).height(1).bottom_margin(0);

    let rows = app_state.csi_data.iter().rev().take(table_area.height.saturating_sub(2).max(0) as usize).map(|p| {
        let src_mac_str = format!("{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
                                  p.src_mac[0], p.src_mac[1], p.src_mac[2],
                                  p.src_mac[3], p.src_mac[4], p.src_mac[5]);
        let dst_mac_str = format!("{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
                                  p.dst_mac[0], p.dst_mac[1], p.dst_mac[2],
                                  p.dst_mac[3], p.dst_mac[4], p.dst_mac[5]);
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
    });

    let table_widths = [
        Constraint::Length(18), Constraint::Length(18), Constraint::Length(18), 
        Constraint::Length(6), Constraint::Length(6), Constraint::Length(9),    
        Constraint::Length(9), Constraint::Length(8),    
    ];
    let table = Table::new(rows)
        .header(table_header)
        .block(Block::default().borders(Borders::ALL).title(" CSI Data Packets "))
        .widths(&table_widths)
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED))
        .highlight_symbol(">> ");
    f.render_widget(table, table_area);

    // --- Log Messages (right panel) ---
    // app_state.log_messages now contains VecDeque<LogEntry>
    let log_items_list: Vec<ListItem> = app_state
        .log_messages
        .iter()
        .map(|entry| { // entry is &LogEntry
            // Format the LogEntry for display
            let timestamp_str = entry.timestamp.format("%H:%M:%S").to_string();
            // The message string now includes level from the LogEntry struct.
            let display_msg_with_timestamp = format!("{} [{}] {}", timestamp_str, entry.level, entry.message);
            
            // Determine style based on entry.level
            let style = match entry.level {
                log::Level::Error => Style::default().fg(Color::Red),
                log::Level::Warn  => Style::default().fg(Color::Yellow),
                log::Level::Info  => Style::default().fg(Color::White), // Consider Cyan or Green for Info
                log::Level::Debug => Style::default().fg(Color::Blue),
                log::Level::Trace => Style::default().fg(Color::Magenta),
            };
            
            //ListItem::new(String) converts to Text, which enables wrapping.
            ListItem::new(display_msg_with_timestamp).style(style)
        })
        .collect();

    let num_logs_to_show = log_panel_area.height.saturating_sub(2).max(0) as usize; // -2 for block borders
    
    let current_log_count = log_items_list.len();
    let visible_log_items = if current_log_count > num_logs_to_show {
        let items_to_skip = current_log_count - num_logs_to_show;
        log_items_list.into_iter().skip(items_to_skip).collect()
    } else {
        log_items_list
    };

    let logs_list_widget = List::new(visible_log_items)
        .block(Block::default().borders(Borders::ALL).title(" Log Output "));
    f.render_widget(logs_list_widget, log_panel_area);

    // --- Footer / Controls (bottom-left) ---
    let footer_text = if let Some(err_msg) = &app_state.last_error {
        format!("ERROR: {} (Press 'R' to keep, other keys clear error)", err_msg)
    } else {
        "Controls: [Q]uit | [M]ode | [C]hannel | [B]W | [L]TF | [↑]Clr CSI | [↓]Sync Time".to_string()
    };
    let footer_style = if app_state.last_error.is_some() { Style::default().fg(Color::Red) }
                        else { Style::default().fg(Color::DarkGray) };
    let footer_paragraph = Paragraph::new(footer_text)
        .style(footer_style)
        .block(Block::default().borders(Borders::ALL).title(" Info/Errors "));
    f.render_widget(footer_paragraph, footer_area);
}