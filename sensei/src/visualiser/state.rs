use core::fmt;
use std::cmp::max;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use crossterm::event::{KeyCode, KeyEvent};
use lib::csi_types::CsiData;
use lib::network::rpc_message::{DeviceId, HostId};
use lib::tui::Tui;
use lib::tui::logs::{FromLog, LogEntry};
use log::error;
use ratatui::Frame;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tokio::sync::mpsc::{Receiver, Sender};

use crate::visualiser::Visualiser;
use crate::visualiser::tui::ui;

const INTERVAL_CHANGE_STEP: i64 = 5;

#[derive(Debug, Clone)]
pub struct Graph {
    pub gtype: GraphConfig,
    pub device_id: DeviceId,
    pub data: Vec<(f64, f64)>,
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
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq)]
pub enum GraphConfig {
    Amplitude(AmplitudeConfig),
    #[allow(clippy::upper_case_acronyms)]
    PDP(PDPConfig),
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq)]
pub struct AmplitudeConfig {
    pub core: usize,
    pub stream: usize,
    pub subcarrier: usize,
    pub time_range: usize,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq)]
pub struct PDPConfig {
    pub core: usize,
    pub stream: usize,
    pub y_axis_bounds: [f64; 2],
}

#[derive(Debug, Clone, PartialEq)]
pub enum VisUpdate {
    Log(LogEntry),
    Edit(char),
    Delete,
    Connect,
    Quit,
    ClearGraphs,
    AddGraph,
    ChangeInterval(usize, i64),
    UpdateConnectionStatus(ConnectionStatus),
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

#[derive(Debug, Clone, PartialEq)]
pub enum ConnectionStatus {
    Connected,
    NotConnected,
    Unknown,
}

pub struct VisState {
    pub should_quit: bool,
    pub logs: Vec<LogEntry>,
    pub last_tick: Instant,
    pub graph_update_interval: Duration,
    pub logs_scroll_offset: usize,
    pub focus: Focus,
    pub connection_status: ConnectionStatus,

    pub csi_data: Arc<Mutex<HashMap<u64, Vec<CsiData>>>>,
    pub graphs: Vec<Graph>,

    // Fields
    pub graph_type_input: GraphType,

    pub addr_input: String,
    pub device_id_input: String,
    pub core_input: usize,
    pub subcarrier_input: usize,
    pub stream_input: usize,
}

impl VisState {
    /// Constructs a new `TuiState` with default configurations.
    pub fn new(update_interval: usize) -> Self {
        Self {
            should_quit: false,
            logs: vec![],
            logs_scroll_offset: 0,
            csi_data: Arc::new(Default::default()),
            graphs: vec![],
            last_tick: Instant::now(),
            graph_update_interval: Duration::from_millis(update_interval.try_into().unwrap()),

            graph_type_input: GraphType::Amplitude,
            device_id_input: "0".to_owned(),
            addr_input: "127.0.0.1:6969".to_owned(),
            core_input: 0,
            subcarrier_input: 0,
            stream_input: 0,
            focus: Focus::DeviceID,
            connection_status: ConnectionStatus::Unknown,
        }
    }
}
#[derive(Debug)]
pub enum VisCommand {
    Connect(SocketAddr),
    Subscribe(HostId),
    Unsubscribe(HostId),
    UnsubscribeAll,
}

#[async_trait]
impl Tui<VisUpdate, VisCommand> for VisState {
    /// Draws the UI layout and content.
    fn draw_ui(&self, frame: &mut Frame) {
        ui(frame, self);
    }

    /// Handles keyboard input events and maps them to updates.
    fn handle_keyboard_event(&self, key_event: KeyEvent) -> Option<VisUpdate> {
        match key_event.code {
            KeyCode::Char(chr) if self.focus == Focus::Address => Some(VisUpdate::Edit(chr)),
            KeyCode::Char(chr) if self.focus == Focus::DeviceID && chr.is_numeric() => Some(VisUpdate::Edit(chr)),
            KeyCode::Enter if self.focus == Focus::Address => Some(VisUpdate::Connect),
            KeyCode::Backspace if self.focus == Focus::Address || self.focus == Focus::DeviceID => Some(VisUpdate::Delete),
            KeyCode::Char('q') | KeyCode::Char('Q') => Some(VisUpdate::Quit),
            KeyCode::Char('.') => Some(VisUpdate::ClearGraphs),
            KeyCode::Char('+') | KeyCode::Char('=') => match self.focus {
                Focus::Graph(i) => Some(VisUpdate::ChangeInterval(i, INTERVAL_CHANGE_STEP)),
                _ => None,
            },
            KeyCode::Char('-') => match self.focus {
                Focus::Graph(i) => Some(VisUpdate::ChangeInterval(i, -INTERVAL_CHANGE_STEP)),
                _ => None,
            },
            KeyCode::Up => Some(VisUpdate::Up),
            KeyCode::Down => Some(VisUpdate::Down),
            KeyCode::Left => Some(VisUpdate::Left),
            KeyCode::BackTab => Some(VisUpdate::Left),
            KeyCode::Right => Some(VisUpdate::Right),
            KeyCode::Tab => Some(VisUpdate::Right),
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
                if let Some(device_data) = csi_data_snapshot.get(&graph.device_id) {
                    let (data, _) = Visualiser::process_data(device_data, graph.gtype);
                    graph.data = data;
                }
            }
            self.last_tick = now;
        }
    }

    /// Applies updates and potentially sends commands to background tasks.
    async fn handle_update(&mut self, update: VisUpdate, command_send: &Sender<VisCommand>, _update_recv: &mut Receiver<VisUpdate>) {
        match update {
            VisUpdate::Log(entry) => {
                self.logs.push(entry);
            }
            VisUpdate::Quit => {
                self.should_quit = true;
            }
            VisUpdate::ClearGraphs => {
                let _ = command_send.send(VisCommand::UnsubscribeAll).await;
                self.graphs.clear();
            }
            VisUpdate::Connect => {
                if let Ok(addr) = self.addr_input.parse() {
                    let _ = command_send.send(VisCommand::Connect(addr)).await;
                } else {
                    error!("Unable to parse address {:?}", self.addr_input);
                }
            }
            VisUpdate::UpdateConnectionStatus(status) => self.connection_status = status,
            VisUpdate::AddGraph => {
                let graph_type = match self.graph_type_input {
                    GraphType::Amplitude => GraphConfig::Amplitude(AmplitudeConfig {
                        core: self.core_input,
                        stream: self.stream_input,
                        subcarrier: self.subcarrier_input,
                        time_range: 100,
                    }),
                    GraphType::PDP => GraphConfig::PDP(PDPConfig {
                        core: self.core_input,
                        stream: self.stream_input,
                        y_axis_bounds: [0.0, 10.0],
                    }),
                };
                if let Ok(device_id) = self.device_id_input.parse() {
                    let graph = Graph {
                        gtype: graph_type,
                        device_id,
                        data: vec![],
                    };
                    if command_send.send(VisCommand::Subscribe(device_id)).await.is_ok() {
                        self.graphs.push(graph);
                    };
                }
            }
            VisUpdate::RemoveGraph => {
                if let Focus::Graph(idx) = self.focus
                    && let Some(graph) = self.graphs.get(idx)
                    && command_send.send(VisCommand::Unsubscribe(graph.device_id)).await.is_ok()
                {
                    self.graphs.remove(idx);
                };
            }
            VisUpdate::Delete => match self.focus {
                Focus::DeviceID => {
                    self.device_id_input.pop();
                }
                Focus::Address => {
                    self.addr_input.pop();
                }
                _ => {}
            },
            VisUpdate::Edit(chr) => match self.focus {
                Focus::DeviceID if chr.is_numeric() => {
                    self.device_id_input.push(chr);
                }
                Focus::Address if chr.is_numeric() => {
                    self.addr_input.push(chr);
                }
                _ => {}
            },
            VisUpdate::ChangeInterval(idx, step) => {
                if let Some(graph_to_modify) = self.graphs.get_mut(idx) {
                    match &mut graph_to_modify.gtype {
                        GraphConfig::PDP(config) => {
                            let new_bound = config.y_axis_bounds[1] + (step as f64);

                            config.y_axis_bounds = [0.0, new_bound.clamp(0.0, 10000.0)];
                        }
                        GraphConfig::Amplitude(config) => {
                            config.time_range = max(0, (config.time_range as i64) + step) as usize;
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
                Focus::Address => {
                    if !self.graphs.is_empty() {
                        self.focus = Focus::Graph(0)
                    }
                }
                Focus::DeviceID => self.focus = Focus::Address,
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
                Focus::Address => self.focus = Focus::DeviceID,
                Focus::DeviceID => self.focus = Focus::GraphType,
                Focus::Graph(0) => self.focus = Focus::Address,
                _ => {}
            },
            VisUpdate::Right => match self.focus {
                Focus::Graph(i) => self.focus = Focus::Graph(i + 1),
                _ => self.focus = self.focus.right(self.graph_type_input, self.graphs.len()),
            },
            VisUpdate::Left => match self.focus {
                Focus::Graph(i) if i > 0 => self.focus = Focus::Graph(i - 1),
                _ => self.focus = self.focus.left(),
            },
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
