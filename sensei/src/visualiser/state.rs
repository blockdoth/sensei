use core::fmt;
use std::net::SocketAddr;
use std::str::FromStr;
use std::time::Instant;

use async_trait::async_trait;
use crossterm::event::{KeyCode, KeyEvent};
use lib::tui::Tui;
use lib::tui::example::Update;
use lib::tui::logs::{FromLog, LogEntry};
use log::info;
use ratatui::Frame;
use tokio::sync::mpsc::{Receiver, Sender};

use crate::visualiser::tui::ui;

#[derive(Debug, Clone, Copy)]
pub struct Graph {
    pub graph_type: GraphType,
    pub target_addr: SocketAddr,
    pub device: u64,
    pub core: usize,
    pub stream: usize,
    pub subcarrier: usize,
    pub time_interval: usize,            // For Amplitude plots: x-axis time window in ms
    pub y_axis_bounds: Option<[f64; 2]>, // For PDP plots: fixed y-axis bounds [min, max]
}

impl PartialEq for Graph {
    fn eq(&self, other: &Self) -> bool {
        self.graph_type == other.graph_type
            && self.target_addr == other.target_addr
            && self.device == other.device
            && self.core == other.core
            && self.stream == other.stream
            && self.subcarrier == other.subcarrier
        // time_interval is intentionally omitted for robust state tracking against graph spec changes
        // y_axis_bounds is also omitted for the same reason
    }
}

#[derive(Clone, Debug)]
pub struct GraphDisplayState {
    data_points: Vec<(f64, f64)>,
    csi_timestamp: f64,             // Timestamp of the CsiData this state is based on
    last_loop_update_time: Instant, // When this state was last updated or checked in the tui_loop
}

#[derive(Debug, Clone, Copy)]
pub enum GraphType {
    Amplitude,
    #[allow(clippy::upper_case_acronyms)]
    PDP,
}

impl FromStr for GraphType {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "amp" => Ok(GraphType::Amplitude),
            "amplitude" => Ok(GraphType::Amplitude),
            "pdp" => Ok(GraphType::PDP),
            _ => Err(()),
        }
    }
}

impl fmt::Display for GraphType {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

impl PartialEq for GraphType {
    fn eq(&self, other: &Self) -> bool {
        self.to_string() == other.to_string()
    }
}

pub struct VisState {
    pub should_quit: bool,
    pub logs: Vec<LogEntry>,
    pub graph_data: Vec<Vec<(f64, f64)>>,
}

pub enum VisUpdate {
    Log(LogEntry),
    Quit,
}

impl FromLog for VisUpdate {
    /// Converts a `VisUpdate` into a `VisUpdate::Log`.
    fn from_log(log: LogEntry) -> Self {
        VisUpdate::Log(log)
    }
}

pub enum VisCommand {}

#[async_trait]
impl Tui<VisUpdate, VisCommand> for VisState {
    /// Draws the UI layout and content.
    fn draw_ui(&self, frame: &mut Frame) {
        ui(frame, self);
    }

    /// Handles keyboard input events and maps them to updates.
    fn handle_keyboard_event(&self, key_event: KeyEvent) -> Option<VisUpdate> {
        match key_event.code {
            KeyCode::Char('q') => Some(VisUpdate::Quit),
            _ => None,
        }
    }

    fn should_quit(&self) -> bool {
        self.should_quit
    }

    async fn on_tick(&mut self) {}

    /// Applies updates and potentially sends commands to background tasks.
    async fn handle_update(&mut self, update: VisUpdate, command_send: &Sender<VisCommand>, update_recv: &mut Receiver<VisUpdate>) {
        match update {
            VisUpdate::Log(entry) => {
                self.logs.push(entry);
            }
            VisUpdate::Quit => todo!(),
        }
    }
}

impl VisState {
    /// Constructs a new `TuiState` with default configurations.
    pub fn new() -> Self {
        Self {
            should_quit: false,
            logs: vec![],
            graph_data: vec![],
        }
    }
}
