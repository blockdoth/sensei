// cli_test_tool.rs
//! Robust CLI tool for ESP32 CSI monitoring

use std::error::Error;
use std::io;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use std::env; // For command line arguments

use bincode::de;
use log::{info, error, warn};
use crossterm::{
    event::{Event as CEvent, KeyCode}, // Removed event::self
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use tui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, Cell, Paragraph, Row, Table},
    Frame, Terminal,
};

mod esp32;
use esp32::{
    Bandwidth, CsiPacket, CsiType, Esp32, OperationMode, SecondaryChannel,
    DeviceConfig as EspDeviceConfig, Command as EspCommand, // Ensure EspCommand is imported if not already
};


fn setup_logging() {
    env_logger::Builder::from_default_env()
        .format_timestamp_millis()
        .target(env_logger::Target::Stderr)
        .filter_level(log::LevelFilter::Info)
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
    Csi,
    Spam,
}

struct AppState {
    ui_mode: UiMode,
    current_esp_config: EspDeviceConfig,
    csi_data: Vec<CsiPacket>,
    connection_status: String,
    last_error: Option<String>,
}

impl AppState {
    fn new(initial_esp_config: EspDeviceConfig) -> Self {
        Self {
            ui_mode: UiMode::Csi,
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

    // Attempt to set up the terminal FIRST.
    // If this fails, we don't have a terminal to restore.
    let mut terminal = match setup_terminal() {
        Ok(term) => term,
        Err(e) => {
            eprintln!("Failed to setup terminal: {}", e); // eprintln as logging might not be fully up if terminal fails
            return Err(e);
        }
    };

    // Now that terminal is successfully set up, proceed.
    // All subsequent errors before the main loop should try to restore it.
    let esp_mutex = match Esp32::new(port_name, 3_000_000) {
        Ok(esp_instance) => Arc::new(Mutex::new(esp_instance)),
        Err(e) => {
            let _ = restore_terminal(&mut terminal); // terminal IS in scope here
            error!("Failed to initialize ESP32: {}", e);
            return Err(e.into());
        }
    };

    let initial_esp_config_for_appstate: EspDeviceConfig;

    match esp_mutex.lock() {
        Ok(mut esp_guard) => {
            if let Err(e) = esp_guard.connect() {
                let _ = restore_terminal(&mut terminal); // terminal IS in scope
                error!("Failed to connect to ESP32: {}", e);
                return Err(e.into());
            }
            info!("ESP32 connected, basic configuration applied by connect().");

            info!("Attempting to clear MAC address whitelist on ESP32...");
            if let Err(e) = esp_guard.clear_mac_filters() {
                warn!("Failed to clear MAC whitelist on ESP32: {}. CSI reception might be filtered.", e);
                // Decide if this should be shown in AppState.last_error
            } else {
                info!("MAC address whitelist cleared successfully.");
                info!("ASHKDAKFHHDUHUOHFDUHDHFAKJHDFKJHAJKDHFJKAHKJDFHAJKH");
            }
            initial_esp_config_for_appstate = esp_guard.get_current_config().clone();
        }
        Err(poisoned) => {
            let _ = restore_terminal(&mut terminal); // terminal IS in scope
            error!("ESP32 Mutex was poisoned during initial connect: {}", poisoned);
            return Err("Mutex poisoned".into());
        }
    }
    info!("ESP32 setup complete via CLI.");

    let app_state = Arc::new(Mutex::new(AppState::new(initial_esp_config_for_appstate)));
    app_state.lock().unwrap().connection_status = "CONNECTED".to_string();

    let state_clone_csi = app_state.clone();
    let csi_rx = esp_mutex.lock().unwrap().csi_receiver();

    thread::spawn(move || {
        info!("CSI listener thread started.");
        loop {
            match csi_rx.recv_timeout(Duration::from_secs(1)) {
                Ok(packet) => {
                    let mut state_guard = state_clone_csi.lock().unwrap();
                    state_guard.csi_data.push(packet);
                    if state_guard.csi_data.len() > 1000 {
                        state_guard.csi_data.remove(0);
                    }
                }
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                    // Continue
                }
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                    info!("CSI channel disconnected. ESP32 likely disconnected.");
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
                            info!("Attempting to PAUSE WiFi transmit task on ESP32...");
                            if let Err(e_pause) = esp_guard.pause_wifi_transmit() {
                                app_state_guard.last_error = Some(format!("Failed to PAUSE transmit: {}", e_pause));
                                warn!("Failed to PAUSE WiFi transmit: {}. (This is okay if it wasn't running)", e_pause);
                            } else {
                                info!("WiFi transmit task PAUSED.");
                            }
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
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(4),
            Constraint::Length(3),
        ])
        .split(f.size());

    let mode_str = match app_state.ui_mode {
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
        .block(Block::default().borders(Borders::ALL).title("ESP32 CSI Tool Status"));
    f.render_widget(header_paragraph, chunks[0]);

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
        Constraint::Length(18), Constraint::Length(18), Constraint::Length(18),
        Constraint::Length(6), Constraint::Length(6), Constraint::Length(5),
        Constraint::Length(5), Constraint::Length(8),
    ];
    let table = Table::new(rows)
        .header(table_header)
        .block(Block::default().borders(Borders::ALL).title("CSI Data Packets"))
        .widths(&table_widths)
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED))
        .highlight_symbol(">> ");
    f.render_widget(table, chunks[1]);

    let footer_text = if let Some(err_msg) = &app_state.last_error {
        format!("ERROR: {} (Press 'R' to keep, other keys clear error)", err_msg)
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