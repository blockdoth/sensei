// cli_test_tool.rs
//! Robust CLI tool for ESP32 CSI monitoring

use std::error::Error;
use std::io;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration; // Removed Instant
use std::env; // For command line arguments

use log::{info, error, warn}; // Removed debug for now, can be re-added if specific debug logs are needed here
use crossterm::{
    event::{self, Event as CEvent, KeyCode}, // Removed KeyEvent as key_event.code is used directly
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use tui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    // symbols, // Removed unused
    // text::Span, // Removed unused
    widgets::{Block, Borders, Cell, Paragraph, Row, Table}, // Removed Tabs
    Frame, Terminal,
};

// Assuming esp32.rs is in the same directory or accessible via `mod esp32;`
// If esp32.rs is in a parent directory, you might need to adjust your module structure
// e.g., in lib.rs: pub mod esp32; and then use crate::esp32::{...}
mod esp32;
use esp32::{
    Bandwidth, CsiPacket, CsiType, Esp32, OperationMode, SecondaryChannel,
    DeviceConfig as EspDeviceConfig // Alias to avoid confusion if a local one existed
};


fn setup_logging() {
    env_logger::Builder::from_default_env()
        .format_timestamp_millis()
        .target(env_logger::Target::Stderr)
        .filter_level(log::LevelFilter::Info) // Default to Info, can be overridden by RUST_LOG
        .init();
}

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
    Csi, // Corresponds to esp32::OperationMode::Receive
    Spam, // Corresponds to esp32::OperationMode::Transmit
}

struct AppState {
    // This config represents the desired state/UI state.
    // It's used to update the ESP32's actual config.
    ui_mode: UiMode,
    current_esp_config: EspDeviceConfig, // Stores the latest config known to be on ESP or desired for it
    csi_data: Vec<CsiPacket>,
    connection_status: String,
    last_error: Option<String>,
}

impl AppState {
    fn new(initial_esp_config: EspDeviceConfig) -> Self {
        Self {
            ui_mode: UiMode::Csi, // Default UI mode
            current_esp_config: initial_esp_config,
            csi_data: Vec::with_capacity(1000),
            connection_status: "CONNECTING...".into(),
            last_error: None,
        }
    }
}


fn main() -> Result<(), Box<dyn Error>> {
    setup_logging();

    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
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
            Err(e) => {
                eprintln!("  Error listing serial ports: {}",e);
            }
        }
        return Err("Serial port argument missing".into());
    }
    let port_name = &args[1];
    info!("Attempting to use port: {}", port_name);


    let mut terminal = setup_terminal()?;

    // Initialize ESP32
    let esp_mutex = match Esp32::new(port_name, 3_000_000) {
        Ok(esp_instance) => Arc::new(Mutex::new(esp_instance)),
        Err(e) => {
            restore_terminal(&mut terminal)?;
            error!("Failed to initialize ESP32: {}", e);
            return Err(e.into());
        }
    };

    // Attempt to connect
    match esp_mutex.lock() {
        Ok(mut esp_guard) => {
            if let Err(e) = esp_guard.connect() {
                restore_terminal(&mut terminal)?;
                error!("Failed to connect to ESP32: {}", e);
                return Err(e.into());
            }
        }
        Err(poisoned) => {
            restore_terminal(&mut terminal)?;
            error!("ESP32 Mutex was poisoned during initial connect: {}", poisoned);
            return Err("Mutex poisoned".into());
        }
    }
    info!("ESP32 connected successfully via CLI.");


    // AppState now uses the default config from the ESP32 module
    let initial_esp_config = esp_mutex.lock().unwrap().get_current_config().clone();
    let app_state = Arc::new(Mutex::new(AppState::new(initial_esp_config)));
    app_state.lock().unwrap().connection_status = "CONNECTED".to_string();


    // CSI processing thread
    let state_clone_csi = app_state.clone();
    let csi_rx = esp_mutex.lock().unwrap().csi_receiver(); // Get receiver before moving esp_mutex

    thread::spawn(move || {
        info!("CSI listener thread started.");
        loop {
            match csi_rx.recv_timeout(Duration::from_secs(1)) { // Add timeout to allow checking connection
                Ok(packet) => {
                    let mut state_guard = state_clone_csi.lock().unwrap();
                    state_guard.csi_data.push(packet);
                    if state_guard.csi_data.len() > 1000 { // Keep buffer size limited
                        state_guard.csi_data.remove(0);
                    }
                }
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                    // Timeout is fine, check if we should still be running by trying to lock.
                    // If esp32 is disconnected, csi_rx will eventually be dropped by sender,
                    // then recv() will return an error.
                }
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                    info!("CSI channel disconnected. ESP32 likely disconnected.");
                    let mut state_guard = state_clone_csi.lock().unwrap();
                    state_guard.connection_status = "DISCONNECTED (CSI Channel Closed)".to_string();
                    break; // Exit thread
                }
            }
        }
        info!("CSI listener thread stopped.");
    });

    // Main UI loop
    loop {
        { // Keep lock scope minimal
            let app_state_guard = app_state.lock().unwrap();
            terminal.draw(|f| ui(f, &app_state_guard))?;
        }

        if crossterm::event::poll(Duration::from_millis(100))? {
            if let CEvent::Key(key_event) = crossterm::event::read()? {
                if handle_input(key_event.code, &esp_mutex, &app_state)? {
                    // disconnect() is called within handle_input for 'q'
                    break; // Exit loop
                }
            }
        }
    }

    restore_terminal(&mut terminal)?;
    Ok(())
}

// Returns true if the application should exit
fn handle_input(
    key: KeyCode,
    esp_mutex: &Arc<Mutex<Esp32>>,
    app_state_mutex: &Arc<Mutex<AppState>>,
) -> Result<bool, Box<dyn Error>> {
    let mut app_state_guard = app_state_mutex.lock().unwrap();
    // Clear previous error on new input attempt, unless it's 'r' to explicitly keep/view
    if key != KeyCode::Char('r') {
        app_state_guard.last_error = None;
    }

    match key {
        KeyCode::Char('q') => {
            info!("'q' pressed, attempting to disconnect ESP32.");
            match esp_mutex.lock() {
                Ok(mut esp_guard) => {
                    if let Err(e) = esp_guard.disconnect() {
                        app_state_guard.last_error = Some(format!("Error during disconnect: {}", e));
                        error!("Error disconnecting ESP32: {}", e);
                        // Still try to exit
                    } else {
                        info!("ESP32 disconnected successfully by user.");
                    }
                }
                Err(poisoned) => {
                    app_state_guard.last_error = Some(format!("ESP32 Mutex poisoned on disconnect: {}", poisoned));
                    error!("ESP32 Mutex was poisoned on disconnect: {}", poisoned);
                    // Still try to exit
                }
            }
            app_state_guard.connection_status = "DISCONNECTED (by user)".to_string();
            return Ok(true); // Signal to exit
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
                        app_state_guard.last_error = Some(format!("Failed to set ESP mode: {}", e));
                    } else {
                        app_state_guard.ui_mode = new_ui_mode;
                        app_state_guard.current_esp_config.mode = new_esp_op_mode; // Update our reflection
                        info!("ESP32 mode changed to {:?}", new_esp_op_mode);
                    }
                }
                Err(p) => app_state_guard.last_error = Some(format!("Mutex error: {}", p)),
            }
        }
        KeyCode::Char('c') => { // Change channel
            let mut new_channel = app_state_guard.current_esp_config.channel + 1;
            if new_channel > 11 { // Cycle 1-11
                new_channel = 1;
            }
            match esp_mutex.lock() {
                Ok(mut esp_guard) => {
                    // No need to call update_config_channel_local if set_channel updates it
                    if let Err(e) = esp_guard.set_channel(new_channel) {
                        app_state_guard.last_error = Some(format!("Failed to set channel: {}", e));
                    } else {
                        // set_channel in esp32.rs now updates its internal config
                        app_state_guard.current_esp_config.channel = new_channel;
                        info!("ESP32 channel changed to {}", new_channel);
                    }
                }
                Err(p) => app_state_guard.last_error = Some(format!("Mutex error: {}", p)),
            }
        }
        KeyCode::Char('b') => { // Toggle bandwidth
            let new_bandwidth_is_40 = app_state_guard.current_esp_config.bandwidth == Bandwidth::Twenty;

            match esp_mutex.lock() {
                Ok(mut esp_guard) => {
                    let new_esp_bw = if new_bandwidth_is_40 { Bandwidth::Forty } else { Bandwidth::Twenty };
                    esp_guard.update_config_bandwidth(new_esp_bw);

                    let new_secondary_chan = if new_bandwidth_is_40 {
                        // Default to Above when switching to 40MHz. User can cycle if needed.
                        SecondaryChannel::Above
                    } else {
                        SecondaryChannel::None
                    };
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
        KeyCode::Char('l') => { // Toggle LTF type
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
        KeyCode::Char('r') => { // Clear error message (or rather, pressing 'r' does nothing to the error)
            // app_state_guard.last_error = None; // now cleared by default at start of function
        }
        KeyCode::Up => { // Clear CSI data buffer
            app_state_guard.csi_data.clear();
            info!("CSI data buffer cleared.");
        }
        KeyCode::Down => { // Resynchronize time
             match esp_mutex.lock() {
                Ok(mut esp_guard) => {
                    if let Err(e) = esp_guard.synchronize_time() {
                        app_state_guard.last_error = Some(format!("Failed to sync time: {}", e));
                    } else {
                        info!("Time synchronization requested.");
                        // No explicit confirmation, last_error remains None if successful
                    }
                }
                Err(p) => app_state_guard.last_error = Some(format!("Mutex error: {}", p)),
            }
        }
        _ => {} // Other keys do nothing
    }
    Ok(false) // By default, don't exit
}


fn ui<B: tui::backend::Backend>(f: &mut Frame<B>, app_state: &AppState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([
            Constraint::Length(3), // Status header
            Constraint::Min(4),    // CSI Data table
            Constraint::Length(3), // Footer / Error
        ])
        .split(f.size());

    // Header / Status
    let mode_str = match app_state.ui_mode { // Display based on UI mode
        UiMode::Spam => "TRANSMIT (Spam)",
        UiMode::Csi => "RECEIVE (CSI)",
    };
    let bw_str = match app_state.current_esp_config.bandwidth {
        Bandwidth::Twenty => "20MHz",
        Bandwidth::Forty => "40MHz",
    };
    let ltf_str = match app_state.current_esp_config.csi_type {
        CsiType::LegacyLTF => "L-LTF",
        CsiType::HTLTF => "HT-LTF",
    };
    let status_text = format!(
        "ESP32 | {} | Chan: {} | BW: {} ({:?}) | LTF: {} | Buf: {} | Status: {}",
        mode_str,
        app_state.current_esp_config.channel,
        bw_str,
        app_state.current_esp_config.secondary_channel, // Show secondary channel
        ltf_str,
        app_state.csi_data.len(),
        app_state.connection_status
    );

    let header_paragraph = Paragraph::new(status_text)
        .style(match app_state.connection_status.as_str() {
            s if s.starts_with("CONNECTED") => Style::default().fg(Color::Green),
            _ => Style::default().fg(Color::Red),
        })
        .block(Block::default().borders(Borders::ALL).title("ESP32 CSI Tool Status"));
    f.render_widget(header_paragraph, chunks[0]);


    // CSI Data table
    let table_header_cells = ["Timestamp (us)", "Src MAC", "Dst MAC", "Seq", "RSSI", "AGC", "FFT", "CSI Len"]
        .iter()
        .map(|h| Cell::from(*h).style(Style::default().fg(Color::Yellow)));
    let table_header = Row::new(table_header_cells).height(1).bottom_margin(1);

    let rows = app_state.csi_data.iter().rev().take(chunks[1].height.saturating_sub(3) as usize).map(|p| {
        Row::new(vec![
            Cell::from(format!("{}", p.timestamp_us)),
            Cell::from(format!("{:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}", p.src_mac[0],p.src_mac[1],p.src_mac[2],p.src_mac[3],p.src_mac[4],p.src_mac[5])),
            Cell::from(format!("{:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}", p.dst_mac[0],p.dst_mac[1],p.dst_mac[2],p.dst_mac[3],p.dst_mac[4],p.dst_mac[5])),
            Cell::from(format!("{}", p.seq)),
            Cell::from(format!("{}", p.rssi)),
            Cell::from(format!("{}", p.agc_gain)),
            Cell::from(format!("{}", p.fft_gain)),
            Cell::from(format!("{}", p.csi_data.len())),
        ])
    });

    let table_widths = [
        Constraint::Length(18), // Timestamp
        Constraint::Length(18), // Src MAC
        Constraint::Length(18), // Dst MAC
        Constraint::Length(6),  // Seq
        Constraint::Length(6),  // RSSI
        Constraint::Length(5),  // AGC
        Constraint::Length(5),  // FFT
        Constraint::Length(8),  // CSI Len
    ];
    let table = Table::new(rows)
        .header(table_header)
        .block(Block::default().borders(Borders::ALL).title("CSI Data Packets"))
        .widths(&table_widths)
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED))
        .highlight_symbol(">> ");
    f.render_widget(table, chunks[1]);


    // Footer / Error Display
    let footer_text = if let Some(err_msg) = &app_state.last_error {
        format!("ERROR: {} (Press 'R' to clear error, other keys clear it too)", err_msg)
    } else {
        "Controls: [Q]uit | [M]ode | [C]hannel | [B]andwidth | [L]TF Type | [↑]Clear Buf | [↓]Sync Time".to_string()
    };
    let footer_style = if app_state.last_error.is_some() {
        Style::default().fg(Color::Red)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let footer_paragraph = Paragraph::new(footer_text)
        .style(footer_style)
        .block(Block::default().borders(Borders::ALL).title("Controls / Log"));
    f.render_widget(footer_paragraph, chunks[2]);
}