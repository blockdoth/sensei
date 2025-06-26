use core::fmt;
use std::collections::HashMap;
use std::ffi::os_str::Display;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use crossterm::event::{KeyCode, KeyEvent};
use lib::csi_types::CsiData;
use lib::network::rpc_message::DeviceId;
use lib::tui::Tui;
use lib::tui::logs::{FromLog, LogEntry};
use log::{error, info};
use ratatui::Frame;
use tokio::sync::Mutex;
use tokio::sync::mpsc::{Receiver, Sender};
use tokio::time::Sleep;

use crate::visualiser::Visualiser;
use crate::visualiser::tui::ui;

pub const DECAY_RATE: f64 = 0.9;
pub const STALE_THRESHOLD: Duration = Duration::from_millis(200);
pub const MIN_POWER_THRESHOLD: f64 = 0.015;

#[derive(Debug, Clone)]
pub struct Graph {
    pub gtype: GraphConfig,
    pub target_addr: SocketAddr,
    pub device_id: DeviceId,
    pub data: Vec<(f64, f64)>,
}

#[derive(Clone, Debug)]
pub struct GraphDisplayState {
    pub data_points: Vec<(f64, f64)>,
    pub csi_timestamp: f64,             // Timestamp of the CsiData this state is based on
    pub last_loop_update_time: Instant, // When this state was last updated or checked in the tui_loop
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GraphConfig {
    Amplitude(AmplitudeConfig),
    #[allow(clippy::upper_case_acronyms)]
    PDP(PDPConfig),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GraphType {
    Amplitude,
    #[allow(clippy::upper_case_acronyms)]
    PDP,
}

impl fmt::Display for GraphType {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            GraphType::Amplitude => write!(f, "AMP"),
            GraphType::PDP => write!(f, "PDP"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AmplitudeConfig {
    pub core: usize,
    pub stream: usize,
    pub subcarrier: usize,
    pub time_range: usize,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PDPConfig {
    pub core: usize,
    pub stream: usize,
    pub y_axis_bounds: Option<[f64; 2]>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum VisUpdate {
    Log(LogEntry),
    Quit,
    ClearGraphs,
    AddGraph,
    SetInterval(usize, u64),
    RemoveGraph,
    Up,
    Down,
    Left,
    Right,
}

impl FromLog for VisUpdate {
    /// Converts a `VisUpdate` into a `VisUpdate::Log`.
    fn from_log(log: LogEntry) -> Self {
        VisUpdate::Log(log)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Focus {
    Graph(usize),
    AddGraph(AddGraphFocus),
    GraphType,
    Address,
    DeviceID,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AddGraphFocus {
    PDPFocus(PDPFocusField),
    AmpFocus(AmpFocusField),
}

#[derive(Debug, Clone, PartialEq)]
pub enum PDPFocusField {
    Core,
    Stream,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AmpFocusField {
    Core,
    Stream,
    Subcarriers,
}

pub struct VisState {
    pub should_quit: bool,
    pub logs: Vec<LogEntry>,
    pub last_tick: Instant,
    pub graph_update_interval: Duration,
    pub logs_scroll_offset: usize,
    pub focus: Focus,

    #[allow(clippy::type_complexity)]
    pub csi_data: Arc<Mutex<HashMap<SocketAddr, HashMap<u64, Vec<CsiData>>>>>,
    pub graphs: Vec<Graph>,

    // Fields
    pub graph_type_input: GraphType,
    pub target_addr_input: String,

    pub device_id_input: u64,
    pub core_input: usize,
    pub subcarrier_input: usize,
    pub stream_input: usize,
}

impl VisState {
    /// Constructs a new `TuiState` with default configurations.
    pub fn new() -> Self {
        Self {
            should_quit: false,
            logs: vec![],
            logs_scroll_offset: 0,
            csi_data: Arc::new(Default::default()),
            graphs: vec![],
            last_tick: Instant::now(),
            graph_update_interval: Duration::from_millis(200),

            graph_type_input: GraphType::Amplitude,
            target_addr_input: "127.0.0.1:6969".to_owned(),
            device_id_input: 0,
            core_input: 0,
            subcarrier_input: 0,
            stream_input: 0,
            focus: Focus::GraphType,
        }
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
            KeyCode::Char('q') | KeyCode::Char('Q') => Some(VisUpdate::Quit),
            KeyCode::Char('c') | KeyCode::Char('C') => Some(VisUpdate::ClearGraphs),
            KeyCode::Up => Some(VisUpdate::Up),
            KeyCode::Down => Some(VisUpdate::Down),
            KeyCode::Left => Some(VisUpdate::Left),
            KeyCode::Right => Some(VisUpdate::Right),
            KeyCode::Enter => Some(VisUpdate::AddGraph),
            KeyCode::Char('d') | KeyCode::Char('C') => Some(VisUpdate::RemoveGraph),
            _ => None,
        }
    }

    fn should_quit(&self) -> bool {
        self.should_quit
    }

    async fn on_tick(&mut self) {
        let now = Instant::now();
        if now.duration_since(self.last_tick) > self.graph_update_interval {
            let csi_data_snapshot = self.csi_data.lock().await;

            for graph in self.graphs.iter_mut() {
                if let Some(host_data) = csi_data_snapshot.get(&graph.target_addr) {
                    if let Some(device_data) = host_data.get(&graph.device_id) {
                        let (data, timestamp) = Visualiser::process_data(device_data, graph.gtype);
                        graph.data = data.into();
                    }
                }
            }
        }

        self.last_tick = now;
    }

    /// Applies updates and potentially sends commands to background tasks.
    async fn handle_update(&mut self, update: VisUpdate, command_send: &Sender<VisCommand>, update_recv: &mut Receiver<VisUpdate>) {
        match update {
            VisUpdate::Log(entry) => {
                self.logs.push(entry);
            }
            VisUpdate::Quit => {
                self.should_quit = true;
            }
            VisUpdate::ClearGraphs => {}
            VisUpdate::AddGraph=> {
                let graph_type = match self.graph_type_input {
                    GraphType::Amplitude => GraphConfig::Amplitude(
                        (AmplitudeConfig {
                            core: self.core_input,
                            stream: self.stream_input,
                            subcarrier: self.subcarrier_input,
                            time_range: 100,
                        }),
                    ),
                    GraphType::PDP => GraphConfig::PDP(
                        (PDPConfig {
                            core: self.core_input,
                            stream: self.stream_input,
                            y_axis_bounds: None,
                        }),
                    ),
                };
                if let Ok(target_addr) = self.target_addr_input.parse() {
                    let graph = Graph {
                        gtype: graph_type,
                        target_addr: target_addr,
                        device_id: self.device_id_input,
                        data: vec![],
                    };

                    self.graphs.push(graph);
                }else {
                  error!("Failed to parse target address {}", self.target_addr_input);
                }
            }
            VisUpdate::RemoveGraph => {
                if let Focus::Graph(idx) = self.focus {
                  self.graphs.remove(idx);
                }
            }
            VisUpdate::SetInterval(idx, interval) => {
                if let Some(mut graph_to_modify) = self.graphs.get_mut(idx) {
                    match graph_to_modify.gtype {
                        GraphConfig::PDP(mut config) => {
                            config.y_axis_bounds = Some([0.0, interval as f64]);
                        }
                        GraphConfig::Amplitude(mut config) => {
                            config.time_range = interval as usize;
                        }
                    }
                }
            }
            VisUpdate::Up => match &self.focus {
                Focus::AddGraph(focus_field) => match focus_field {
                    AddGraphFocus::PDPFocus(pdpfocus_field) => match pdpfocus_field {
                        PDPFocusField::Core => self.core_input += 1,
                        PDPFocusField::Stream => self.stream_input += 1,
                    },
                    AddGraphFocus::AmpFocus(amp_focus_field) => match amp_focus_field {
                        AmpFocusField::Core => self.core_input += 1,
                        AmpFocusField::Stream => self.stream_input += 1,
                        AmpFocusField::Subcarriers => self.subcarrier_input += 1,
                    },
                },
                Focus::GraphType => self.graph_type_input = self.graph_type_input.next(),
                _ => {}
            },
            VisUpdate::Down => match &self.focus {
                Focus::AddGraph(focus_field) => match focus_field {
                    AddGraphFocus::PDPFocus(pdpfocus_field) => match pdpfocus_field {
                        PDPFocusField::Core => self.core_input = self.core_input.saturating_sub(1),
                        PDPFocusField::Stream => self.stream_input = self.stream_input.saturating_sub(1),
                    },
                    AddGraphFocus::AmpFocus(amp_focus_field) => match amp_focus_field {
                        AmpFocusField::Core => self.core_input = self.core_input.saturating_sub(1),
                        AmpFocusField::Stream => self.stream_input = self.stream_input.saturating_sub(1),
                        AmpFocusField::Subcarriers => self.subcarrier_input = self.subcarrier_input.saturating_sub(1),
                    },
                },
                Focus::GraphType => self.graph_type_input = self.graph_type_input.prev(),
                _ => {}
            },
            VisUpdate::Right => self.focus = self.focus.right(self.graph_type_input, self.graphs.len()),
            VisUpdate::Left => self.focus = self.focus.left(),
        }
    }
}

impl Focus {
    fn right(&self, graph_type: GraphType, graph_count: usize) -> Self {
        match self {
            Focus::AddGraph(focus_field) => match focus_field {
                        AddGraphFocus::PDPFocus(pdpfocus_field) => match pdpfocus_field {
                            PDPFocusField::Core => Focus::AddGraph(AddGraphFocus::PDPFocus(PDPFocusField::Stream)),
                            PDPFocusField::Stream => Focus::AddGraph(AddGraphFocus::PDPFocus(PDPFocusField::Stream)),
                        },
                        AddGraphFocus::AmpFocus(amp_focus_field) => match amp_focus_field {
                            AmpFocusField::Core => Focus::AddGraph(AddGraphFocus::AmpFocus(AmpFocusField::Stream)),
                            AmpFocusField::Stream => Focus::AddGraph(AddGraphFocus::AmpFocus(AmpFocusField::Subcarriers)),
                            AmpFocusField::Subcarriers => Focus::AddGraph(AddGraphFocus::AmpFocus(AmpFocusField::Subcarriers)),
                        },
                    },
            Focus::GraphType => match graph_type {
                        GraphType::PDP => Focus::AddGraph(AddGraphFocus::PDPFocus(PDPFocusField::Core)),
                        GraphType::Amplitude => Focus::AddGraph(AddGraphFocus::AmpFocus(AmpFocusField::Core)),
                    },
            Focus::Address => Focus::DeviceID,
            Focus::DeviceID => Focus::GraphType,
            Focus::Graph(i) if *i < graph_count => Focus::Graph(i + 1),
            Focus::Graph(i) => Focus::Graph(*i),
        }
    }

    fn left(&self) -> Self {
        match self {
            Focus::AddGraph(focus_field) => match focus_field {
                AddGraphFocus::PDPFocus(pdpfocus_field) => match pdpfocus_field {
                    PDPFocusField::Core => Focus::GraphType,
                    PDPFocusField::Stream => Focus::AddGraph(AddGraphFocus::PDPFocus(PDPFocusField::Core)),
                },
                AddGraphFocus::AmpFocus(amp_focus_field) => match amp_focus_field {
                    AmpFocusField::Core => Focus::GraphType,
                    AmpFocusField::Stream => Focus::AddGraph(AddGraphFocus::AmpFocus(AmpFocusField::Core)),
                    AmpFocusField::Subcarriers => Focus::AddGraph(AddGraphFocus::AmpFocus(AmpFocusField::Stream)),
                },
            },
            Focus::GraphType => Focus::DeviceID,
            Focus::DeviceID => Focus::Address,
            Focus::Address => Focus::Address,
            Focus::Graph(i) if *i > 0 => Focus::Graph(i - 1),
            Focus::Graph(i) => Focus::Graph(*i),            
        }
    }
}

impl GraphType {
    fn next(&self) -> Self {
        match self {
            GraphType::Amplitude => GraphType::PDP,
            GraphType::PDP => GraphType::Amplitude,
        }
    }
    fn prev(&self) -> Self {
        match self {
            GraphType::Amplitude => GraphType::PDP,
            GraphType::PDP => GraphType::Amplitude,
        }
    }
}
