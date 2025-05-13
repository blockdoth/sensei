// cli_test_tool.rs
//! Robust CLI tool for ESP32 CSI monitoring

use std::error::Error;
use std::time::{Duration, Instant};
use log::{error};
use crossterm::{
    event::{Event as CEvent, KeyCode},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use tui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Style},
    widgets::{Block, Borders, Paragraph, Row, Table},
    Frame, Terminal,
};

mod esp32;
use esp32::{Command, CsiPacket, CsiType, Esp32, EspError, EspStatus, OperationMode, SecondaryChannel};

fn setup_logging() {
    env_logger::Builder::from_default_env()
        .format_timestamp_millis()
        .target(env_logger::Target::Stderr)
        .filter_level(log::LevelFilter::Debug)
        .init();
}

#[derive(Clone)]
struct AppConfig {
    port: String,
    channel: u8,
    bandwidth_40: bool,
    legacy_ltf: bool,
    mode: Mode,
    secondary_channel: SecondaryChannel,
    keep_lines: usize,
}

#[derive(Clone, Copy, PartialEq)]
enum Mode { Spam, Csi }

struct AppState {
    config: AppConfig,
    csi_data: Vec<CsiPacket>,
    esp: Option<Esp32>,
    last_error: Option<String>,
    connection_status: String,
}

fn main() -> Result<(), Box<dyn Error>> {
    setup_logging();
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        error!("Usage: {} <SERIAL_PORT>", args[0]);
        return Err("Missing serial port argument".into());
    }
    let port = &args[1];

    let config = AppConfig {
        port: port.clone(),
        channel: 11,
        bandwidth_40: false,
        legacy_ltf: false,
        mode: Mode::Spam,
        secondary_channel: SecondaryChannel::Above,
        keep_lines: 1000,
    };

    let mut state = AppState {
        config,
        csi_data: Vec::with_capacity(1000),
        esp: None,
        last_error: None,
        connection_status: "DISCONNECTED".into(),
    };

    // initial connect
    match Esp32::open(&state.config.port) {
        Ok(mut esp) => {
            state.esp = Some(esp);
            state.connection_status = "CONNECTED".into();
        }
        Err(e) => {
            state.last_error = Some(format!("Connection failed: {}", e));
            state.connection_status = format!("DISCONNECTED: {}", e);
        }
    }

    

    let mut terminal = setup_terminal()?;
    let res = run_app(&mut terminal, &mut state);
    cleanup_terminal()?;

    if let Err(e) = res {
        error!("Application error: {}", e);
        Err(e)
    } else {
        Ok(())
    }
}

fn setup_terminal() -> Result<Terminal<CrosstermBackend<std::io::Stdout>>, Box<dyn Error>> {
    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    Ok(Terminal::new(CrosstermBackend::new(stdout))?)
}

fn cleanup_terminal() -> Result<(), Box<dyn Error>> {
    disable_raw_mode()?;
    execute!(std::io::stdout(), LeaveAlternateScreen)?;
    Ok(())
}

fn run_app(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    state: &mut AppState,
) -> Result<(), Box<dyn Error>> {
    let tick_rate = Duration::from_millis(200);
    let mut last_tick = Instant::now();

    loop {
        // status‐check
        loop {
            if let Some(status) = state.esp.as_ref().and_then(|esp| esp.check_status()) {
                match status {
                    EspStatus::Connected => {
                        state.last_error = None;
                        state.connection_status = "CONNECTED".into();
                    }
                    EspStatus::Disconnected(reason) => {
                        state.last_error = Some(format!("Disconnected: {}", reason));
                        state.connection_status = format!("DISCONNECTED: {}", reason);
                        state.esp = None;
                    }
                    EspStatus::Error(msg) => {
                        state.last_error = Some(msg.clone());
                        state.connection_status = format!("ERROR: {}", msg);
                    }
                }
            } else {
                break;
            }
        }

        terminal.draw(|f| ui(f, state))?;

        let timeout = tick_rate
            .checked_sub(last_tick.elapsed())
            .unwrap_or(Duration::ZERO);

        if crossterm::event::poll(timeout)? {
            if let CEvent::Key(key) = crossterm::event::read()? {
                if handle_input(key.code, state)? {
                    return Ok(());
                }
            }
        }

        if last_tick.elapsed() >= tick_rate {
            process_csi_data(state);
            last_tick = Instant::now();
        }

        if state.esp.is_none() && state.last_error.is_some() {
            match Esp32::open(&state.config.port) {
                Ok(esp) => {
                    state.esp = Some(esp);
                    state.last_error = None;
                    state.connection_status = "CONNECTED".into();
                }
                Err(e) => {
                    state.last_error = Some(format!("Reconnect failed: {}", e));
                    state.connection_status = format!("DISCONNECTED: {}", e);
                }
            }
        }
    }
}

fn handle_input(key: KeyCode, state: &mut AppState) -> Result<bool, Box<dyn Error>> {
    match key {
        KeyCode::Char('q') => return Ok(true),
        KeyCode::Char('m') => toggle_mode(state),
        KeyCode::Char('c') => change_channel(state),
        KeyCode::Char('b') => toggle_bandwidth(state),
        KeyCode::Char('l') => toggle_ltf(state),
        KeyCode::Char('r') => attempt_recovery(state),
        KeyCode::Up => adjust_keep_lines(state, 100),
        KeyCode::Down => adjust_keep_lines(state, -100),
        _ => {}
    }
    Ok(false)
}

fn toggle_mode(state: &mut AppState) {
    state.config.mode = match state.config.mode {
        Mode::Spam => Mode::Csi,
        Mode::Csi => Mode::Spam,
    };
    if let Err(e) = apply_config(state) {
        state.last_error = Some(format!("Mode change failed: {}", e));
    }
}

fn change_channel(state: &mut AppState) {
    state.config.channel = (state.config.channel % 12) + 1;
    if let Err(e) = apply_config(state) {
        state.last_error = Some(format!("Channel change failed: {}", e));
    }
}

fn toggle_bandwidth(state: &mut AppState) {
    state.config.bandwidth_40 = !state.config.bandwidth_40;
    state.config.secondary_channel = if state.config.bandwidth_40 {
        SecondaryChannel::Below
    } else {
        SecondaryChannel::No
    };
    if let Err(e) = apply_config(state) {
        state.last_error = Some(format!("Bandwidth change failed: {}", e));
    }
}

fn toggle_ltf(state: &mut AppState) {
    state.config.legacy_ltf = !state.config.legacy_ltf;
    if let Err(e) = apply_config(state) {
        state.last_error = Some(format!("LTF change failed: {}", e));
    }
}

fn adjust_keep_lines(state: &mut AppState, delta: i32) {
    let new_value = state.config.keep_lines as i32 + delta;
    state.config.keep_lines = new_value.clamp(100, 5000) as usize;
}

fn attempt_recovery(state: &mut AppState) {
    state.last_error = None;
    if let Err(e) = reconnect_device(state) {
        state.last_error = Some(format!("Recovery failed: {}", e));
    }
}

fn reconnect_device(state: &mut AppState) -> Result<(), EspError> {
    if let Some(esp) = &mut state.esp {
        esp.close();
    }
    state.esp = None;
    
    let new_esp = Esp32::open(&state.config.port)?;
    apply_config_impl(&new_esp, &state.config)?;
    state.esp = Some(new_esp);
    Ok(())
}

fn apply_config(state: &mut AppState) -> Result<(), EspError> {
    if let Some(esp) = &state.esp {
        apply_config_impl(esp, &state.config)
    } else {
        reconnect_device(state)
    }
}

fn apply_config_impl(esp: &Esp32, config: &AppConfig) -> Result<(), EspError> {
    esp.send_command(Command::PauseAcquisition, &[])?;
    esp.send_command(Command::PauseWifiTx, &[])?;

    let (mode, bw_flag, secondary, csi_type) = match config.mode {
        Mode::Spam => (
            OperationMode::Tx as u8,
            0,
            SecondaryChannel::No as u8,
            CsiType::LegacyLtf as u8,
        ),
        Mode::Csi => (
            OperationMode::Rx as u8,
            config.bandwidth_40 as u8,
            config.secondary_channel as u8,
            config.legacy_ltf as u8,
        ),
    };

    let config_data = [mode, bw_flag, secondary, csi_type, 0];
    esp.send_command(Command::ApplyDeviceConfig, &config_data)?;
    esp.send_command(Command::SetChannel, &[config.channel])?;

    match config.mode {
        Mode::Spam => esp.send_command(Command::ResumeWifiTx, &[])?,
        Mode::Csi => esp.send_command(Command::UnpauseAcquisition, &[])?,
    }

    Ok(())
}

fn process_csi_data(state: &mut AppState) {
    if let Some(esp) = &state.esp {
        while let Ok(packet) = esp.csi_rx().try_recv() {
            state.csi_data.push(packet);
            if state.csi_data.len() > state.config.keep_lines {
                state.csi_data.remove(0);
            }
        }
    }
}

fn ui<B: tui::backend::Backend>(f: &mut Frame<B>, state: &AppState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(4),
            Constraint::Length(3),
        ])
        .split(f.size());

    // Header
    let mode_str = match state.config.mode {
        Mode::Spam => "TRANSMIT",
        Mode::Csi => "RECEIVE",
    };
    let header = Paragraph::new(
        format!("ESP32 CSI Tool | Mode: {} | Channel: {} | BW: {}MHz | {} | Buffer: {} samples",
            mode_str,
            state.config.channel,
            if state.config.bandwidth_40 { 40 } else { 20 },
            if state.config.legacy_ltf { "Legacy LTF" } else { "HT LTF" },
            state.csi_data.len()
        )
    )
    .block(Block::default().borders(Borders::ALL).title("Status"));
    f.render_widget(header, chunks[0]);

    // Connection status
    let status_style = if state.connection_status.starts_with("CONNECTED") {
        Style::default().fg(Color::Green)
    } else {
        Style::default().fg(Color::Red)
    };
    let status_block = Paragraph::new(format!("Connection: {}", state.connection_status))
        .style(status_style)
        .block(Block::default().borders(Borders::ALL).title("Link"));
    f.render_widget(status_block, chunks[0]);

    // CSI Data table
    let rows = state.csi_data.iter().rev().map(|p| Row::new(vec![
        format!("{}", p.timestamp_us),
        format!("{:02X?}", p.src_mac),
        format!("{:02X?}", p.dst_mac),
        format!("{}", p.seq),
        format!("{:.1} dB", p.rssi),
        format!("{}", p.csi_data.len()),
    ]));
    let table = Table::new(rows)
        .header(Row::new(vec!["Timestamp", "Source", "Dest", "Seq", "RSSI", "Len"]))
        .block(Block::default().borders(Borders::ALL).title("CSI Data"))
        .widths(&[
            Constraint::Length(12),
            Constraint::Length(17),
            Constraint::Length(17),
            Constraint::Length(6),
            Constraint::Length(8),
            Constraint::Length(6),
        ]);
    f.render_widget(table, chunks[1]);

    // Footer
    let footer_text = if let Some(err) = &state.last_error {
        format!(" ERROR: {} (Press R) ", err)
    } else {
        " Controls: [Q]uit [M]ode [C]hannel [B]W [L]TF [↑↓]Buf ".to_string()
    };
    let footer = Paragraph::new(footer_text)
        .style(Style::default().fg(
            if state.last_error.is_some() { Color::Red } else { Color::Gray }
        ))
        .block(Block::default().borders(Borders::ALL));
    f.render_widget(footer, chunks[2]);
}