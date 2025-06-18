use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};

use async_trait::async_trait;
use crossterm::event::{KeyCode, KeyEvent};
use lib::network::rpc_message::{HostId, SourceType};
use lib::tui::Tui;
use lib::tui::logs::{FromLog, LogEntry};
use log::info;
use ratatui::Frame;
use tokio::sync::mpsc::{Receiver, Sender};

use super::tui::ui;
use crate::orchestrator::OrgChannelMsg;
use crate::orchestrator::experiment::{ActiveExperiment, ExperimentMetadata, ExperimentStatus};
use crate::services::DEFAULT_ADDRESS;

pub struct Host {
    pub id: HostId,
    pub addr: SocketAddr,
    pub devices: Vec<Device>,
    pub status: HostStatus,
}

#[derive(Debug, PartialEq)]
pub enum HostStatus {
    Available,
    Connected,
    Disconnected,
    Sending,
    Unresponsive,
}

pub struct Device {
    pub id: DeviceID,
    pub dev_type: SourceType,
    pub status: DeviceStatus,
}
#[derive(Debug, PartialEq)]
pub enum DeviceStatus {
    Subscribed,
    NotSubscribed,
}
pub enum RegistryStatus {
    Connected,
    Disconnected,
    NotSpecified,
}

#[derive(PartialEq, Debug)]
pub enum Focused {
    Main,
    Registry(FocusedRegistry),
    Hosts(FocusedHosts),
    Experiments(FocusedExperiments),
    Logs,
}

#[derive(PartialEq, Debug, Clone)]
pub enum FocusedExperiments {
    Select(usize),
}

#[derive(PartialEq, Debug)]
pub enum FocusedRegistry {
    RegistryAddress(usize),
    AvailableHosts(usize),
}

#[derive(PartialEq, Debug, Clone)]
pub enum FocusedHosts {
    None,
    HostTree(usize, usize),
    AddHost(usize, FocusedAddHostField),
}

#[derive(PartialEq, Debug, Clone, Copy)]
pub enum FocusedAddHostField {
    Address,
    ID,
}

pub enum OrgCommand {}

type DeviceID = u64;

#[derive(Debug)]
pub enum OrgUpdate {
    Log(LogEntry),
    Connect(SocketAddr),
    Disconnect(SocketAddr),
    Subscribe(SocketAddr, Option<SocketAddr>, DeviceID),
    Unsubscribe(SocketAddr, Option<SocketAddr>, DeviceID),
    SubscribeAll(SocketAddr, Option<SocketAddr>),
    UnsubscribeAll(SocketAddr, Option<SocketAddr>),
    SelectHost(SocketAddr),
    FocusChange(Focused),
    StartExperiment,
    StopExperiment,
    SelectExperiment(usize),
    ActiveExperiment(ActiveExperiment),
    UpdateExperimentList(Vec<ExperimentMetadata>),
    Edit(char),
    Exit,
    Up,
    Down,
    Left,
    Right,
    Tab,
    BackTab,
    Backspace,
    Enter,
    ClearLogs,
}

impl FromLog for OrgUpdate {
    fn from_log(log: LogEntry) -> Self {
        OrgUpdate::Log(log)
    }
}

/// Holds the entire state of the TUI, including configurations, logs, and mode information.
pub struct OrgTuiState {
    pub should_quit: bool,
    pub host_available_from_reg: Vec<SocketAddr>,
    pub registry_addr: Option<SocketAddr>,
    pub registry_status: RegistryStatus,
    pub known_hosts: Vec<Host>,
    pub logs: Vec<LogEntry>,
    pub focussed_panel: Focused,
    pub add_host_input_socket: [char; 21],
    pub add_host_input_id: [char; 21], // Made to match with the add_host_input_socket, one larger than the max size of an u64
    pub selected_host: Option<SocketAddr>,
    pub active_experiment: Option<ActiveExperiment>,
    pub experiments: Vec<ExperimentMetadata>,
}

impl OrgTuiState {
    pub fn new() -> Self {
        OrgTuiState {
            should_quit: false,
            host_available_from_reg: vec![DEFAULT_ADDRESS, DEFAULT_ADDRESS, DEFAULT_ADDRESS],
            registry_addr: Some(DEFAULT_ADDRESS),
            registry_status: RegistryStatus::Disconnected,
            logs: vec![],
            focussed_panel: Focused::Main,
            active_experiment: None,
            experiments: vec![],
            add_host_input_socket: [
                '_', '_', '_', '.', '_', '_', '_', '.', '_', '_', '_', '.', '_', '_', '_', ':', '_', '_', '_', '_', '_',
            ],
            add_host_input_id: ['_'; 21],
            selected_host: None,
            known_hosts: vec![
                Host {
                    id: 0,
                    status: HostStatus::Available,
                    devices: vec![],
                    addr: SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 8080)),
                },
                Host {
                    id: 2,
                    status: HostStatus::Sending,
                    devices: vec![
                        Device {
                            id: 5,
                            dev_type: SourceType::AX210,
                            status: DeviceStatus::Subscribed,
                        },
                        Device {
                            id: 9,
                            dev_type: SourceType::AtherosQCA,
                            status: DeviceStatus::Subscribed,
                        },
                    ],
                    addr: DEFAULT_ADDRESS,
                },
            ],
        }
    }
}

#[async_trait]
impl Tui<OrgUpdate, OrgChannelMsg> for OrgTuiState {
    /// Draws the UI layout and content.
    fn draw_ui(&self, frame: &mut Frame) {
        ui(frame, self);
    }

    /// Handles keyboard input events and maps them to updates.
    fn handle_keyboard_event(&self, key_event: KeyEvent) -> Option<OrgUpdate> {
        let key = key_event.code;
        match key {
            KeyCode::Char('q') | KeyCode::Char('Q') => return Some(OrgUpdate::Exit),
            KeyCode::Char('.') => return Some(OrgUpdate::ClearLogs),
            KeyCode::Esc => return Some(OrgUpdate::FocusChange(Focused::Main)),
            _ => {}
        };

        match &self.focussed_panel {
            Focused::Main => match key {
                KeyCode::Char('r') | KeyCode::Char('R') => Some(OrgUpdate::FocusChange(Focused::Registry(FocusedRegistry::RegistryAddress(0)))),
                KeyCode::Char('e') | KeyCode::Char('E') => Some(OrgUpdate::FocusChange(Focused::Experiments(FocusedExperiments::Select(0)))),
                KeyCode::Char('h') | KeyCode::Char('H') => {
                    if !self.known_hosts.is_empty() {
                        Some(OrgUpdate::FocusChange(Focused::Hosts(FocusedHosts::HostTree(0, 0))))
                    } else {
                        Some(OrgUpdate::FocusChange(Focused::Hosts(FocusedHosts::None)))
                    }
                }
                _ => None,
            },

            Focused::Registry(focused_registry_panel) => match focused_registry_panel {
                FocusedRegistry::RegistryAddress(_) => None,
                FocusedRegistry::AvailableHosts(_) => None,
            },
            Focused::Hosts(focused_hosts_panel) => match focused_hosts_panel {
                FocusedHosts::None => match key {
                    KeyCode::Char('a') | KeyCode::Char('A') => Some(OrgUpdate::FocusChange(Focused::Hosts(FocusedHosts::AddHost(
                        0,
                        FocusedAddHostField::Address,
                    )))),
                    _ => None,
                },
                FocusedHosts::HostTree(host_idx, 0) => match key {
                    KeyCode::Char('a') | KeyCode::Char('A') => Some(OrgUpdate::FocusChange(Focused::Hosts(FocusedHosts::AddHost(
                        0,
                        FocusedAddHostField::Address,
                    )))),
                    KeyCode::Char('c') | KeyCode::Char('C') => self.known_hosts.get(*host_idx).map(|host| OrgUpdate::Connect(host.addr)),
                    KeyCode::Char('d') | KeyCode::Char('D') => self.known_hosts.get(*host_idx).map(|host| OrgUpdate::Disconnect(host.addr)),
                    KeyCode::Char('e') | KeyCode::Char('E') => self.known_hosts.get(*host_idx).map(|host| OrgUpdate::SelectHost(host.addr)),

                    KeyCode::Char('s') | KeyCode::Char('S') => self
                        .known_hosts
                        .get(*host_idx)
                        .map(|host| OrgUpdate::SubscribeAll(host.addr, self.selected_host)),
                    KeyCode::Char('u') | KeyCode::Char('U') => self
                        .known_hosts
                        .get(*host_idx)
                        .map(|host| OrgUpdate::UnsubscribeAll(host.addr, self.selected_host)),
                    KeyCode::Up => Some(OrgUpdate::Up),
                    KeyCode::Down => Some(OrgUpdate::Down),
                    KeyCode::Tab => Some(OrgUpdate::Tab),
                    KeyCode::BackTab => Some(OrgUpdate::BackTab),
                    _ => None,
                },
                FocusedHosts::HostTree(host_idx, device_idx) => match key {
                    KeyCode::Char('a') | KeyCode::Char('A') => Some(OrgUpdate::FocusChange(Focused::Hosts(FocusedHosts::AddHost(
                        0,
                        FocusedAddHostField::Address,
                    )))),
                    KeyCode::Char('s') | KeyCode::Char('S') => {
                        if let Some(host) = self.known_hosts.get(*host_idx) {
                            host.devices
                                .get(*device_idx)
                                .map(|device| OrgUpdate::Subscribe(host.addr, self.selected_host, device.id))
                        } else {
                            None
                        }
                    }
                    KeyCode::Char('u') | KeyCode::Char('U') => {
                        if let Some(host) = self.known_hosts.get(*host_idx) {
                            host.devices
                                .get(*device_idx)
                                .map(|device| OrgUpdate::Unsubscribe(host.addr, self.selected_host, device.id))
                        } else {
                            None
                        }
                    }
                    KeyCode::Up => Some(OrgUpdate::Up),
                    KeyCode::Down => Some(OrgUpdate::Down),
                    KeyCode::Tab => Some(OrgUpdate::Tab),
                    KeyCode::BackTab => Some(OrgUpdate::BackTab),
                    _ => None,
                },
                FocusedHosts::AddHost(host_idx, _) => match key {
                    KeyCode::Up => Some(OrgUpdate::Up),
                    KeyCode::Down => Some(OrgUpdate::Down),
                    KeyCode::Right => Some(OrgUpdate::Right),
                    KeyCode::Left => Some(OrgUpdate::Left),
                    KeyCode::Tab => Some(OrgUpdate::Tab),
                    KeyCode::BackTab => Some(OrgUpdate::BackTab),
                    KeyCode::Backspace => Some(OrgUpdate::Backspace),
                    KeyCode::Enter => Some(OrgUpdate::Enter),
                    KeyCode::Char(chr) => Some(OrgUpdate::Edit(chr)),
                    _ => None,
                },
            },
            Focused::Experiments(FocusedExperiments::Select(i)) => match key {
                KeyCode::Up => Some(OrgUpdate::Up),
                KeyCode::Down => Some(OrgUpdate::Down),
                KeyCode::Char('e') | KeyCode::Char('E') => {
                    if let Some(exp) = &self.active_experiment {
                        match exp.status {
                            ExperimentStatus::Running => Some(OrgUpdate::StopExperiment),
                            _ => None,
                        }
                    } else {
                        None
                    }
                }
                KeyCode::Char('b') | KeyCode::Char('B') => {
                    if let Some(exp) = &self.active_experiment {
                        match exp.status {
                            ExperimentStatus::Done | ExperimentStatus::Ready | ExperimentStatus::Stopped => Some(OrgUpdate::StartExperiment),
                            _ => None,
                        }
                    } else {
                        None
                    }
                }
                KeyCode::Char('s') | KeyCode::Char('S') => {
                    if let Some(exp) = &self.active_experiment {
                        match exp.status {
                            ExperimentStatus::Done | ExperimentStatus::Ready | ExperimentStatus::Stopped => Some(OrgUpdate::SelectExperiment(*i)),
                            _ => None,
                        }
                    } else {
                        Some(OrgUpdate::SelectExperiment(*i))
                    }
                }
                _ => None,
            },
            Focused::Logs => None,
        }
    }

    /// Applies updates and potentially sends commands to background tasks.
    async fn handle_update(&mut self, update: OrgUpdate, command_send: &Sender<OrgChannelMsg>, update_recv: &mut Receiver<OrgUpdate>) {
        match update {
            OrgUpdate::Exit => {
                self.should_quit = true;
                command_send.send(OrgChannelMsg::Shutdown).await; // Graceful shutdown
            }
            OrgUpdate::Log(entry) => {
                self.logs.push(entry);
            }
            OrgUpdate::Connect(to_addr) => {
                command_send.send(OrgChannelMsg::Connect(to_addr)).await;
                self.known_hosts.iter_mut().find(|a| a.addr == to_addr).unwrap().status = HostStatus::Connected;
            }
            OrgUpdate::Disconnect(from_addr) => {
                command_send.send(OrgChannelMsg::Disconnect(from_addr)).await;
                self.known_hosts.iter_mut().find(|a| a.addr == from_addr).unwrap().status = HostStatus::Disconnected;
            }
            OrgUpdate::Subscribe(to_addr, msg_origin_addr, device_id) => {
                command_send.send(OrgChannelMsg::Subscribe(to_addr, msg_origin_addr, device_id)).await;
            }
            OrgUpdate::Unsubscribe(to_addr, msg_origin_addr, device_id) => {
                command_send.send(OrgChannelMsg::Unsubscribe(to_addr, msg_origin_addr, device_id)).await;
            }
            OrgUpdate::SubscribeAll(to_addr, msg_origin_addr) => {
                command_send.send(OrgChannelMsg::SubscribeAll(to_addr, msg_origin_addr)).await;
            }
            OrgUpdate::UnsubscribeAll(to_addr, msg_origin_addr) => {
                command_send.send(OrgChannelMsg::UnsubscribeAll(to_addr, msg_origin_addr)).await;
            }
            OrgUpdate::SelectHost(selected_addr) => {
                self.selected_host = match self.selected_host {
                    None => Some(selected_addr),
                    Some(prev) if prev != selected_addr => Some(selected_addr),
                    _ => None,
                }
            }
            OrgUpdate::StartExperiment => {
                command_send.send(OrgChannelMsg::StartExperiment).await;
            }
            OrgUpdate::StopExperiment => {
                command_send.send(OrgChannelMsg::StopExperiment).await;
            }
            OrgUpdate::SelectExperiment(idx) => {
                command_send.send(OrgChannelMsg::SelectExperiment(idx)).await;
            }
            OrgUpdate::ActiveExperiment(active_experiment) => {
                // info!("Received experiment with status {:?}", active_experiment.status);
                self.active_experiment = Some(active_experiment);
            }
            OrgUpdate::UpdateExperimentList(experiments) => {
                self.experiments.extend(experiments);
            }
            OrgUpdate::ClearLogs => self.logs.clear(),
            OrgUpdate::FocusChange(focused_panel) => self.focussed_panel = focused_panel,

            // Handles key updates, which are highly dependant on which panel is focussed
            key_update => match (key_update, &self.focussed_panel) {
                // Key logic for the host panel
                (OrgUpdate::Edit(chr), Focused::Hosts(focus)) if chr.is_numeric() => match focus {
                    FocusedHosts::AddHost(idx, FocusedAddHostField::Address) => {
                        self.add_host_input_socket[*idx] = chr;
                        self.focussed_panel = Focused::Hosts(focus.cursor_right());
                    }
                    FocusedHosts::AddHost(idx, FocusedAddHostField::ID) => {
                        self.add_host_input_id[*idx] = chr;
                        self.focussed_panel = Focused::Hosts(focus.cursor_right());
                    }
                    _ => {}
                },

                (OrgUpdate::Backspace, Focused::Hosts(focus)) => match focus {
                    FocusedHosts::AddHost(idx, FocusedAddHostField::Address) => {
                        self.add_host_input_socket[*idx] = '_';
                        self.focussed_panel = Focused::Hosts(focus.cursor_left());
                    }
                    FocusedHosts::AddHost(idx, FocusedAddHostField::ID) => {
                        self.add_host_input_id[*idx] = '_';
                        self.focussed_panel = Focused::Hosts(focus.cursor_left());
                    }
                    _ => {}
                },

                (OrgUpdate::Enter, Focused::Hosts(focus)) => {
                    let ip_string = self.add_host_input_socket.iter().collect::<String>().replace("_", "");
                    if let Ok(socket_addr) = ip_string.parse::<SocketAddr>() {
                        let id = self
                            .add_host_input_id
                            .iter()
                            .collect::<String>()
                            .replace("_", "")
                            .parse::<u64>()
                            .unwrap_or(69);

                        if self.known_hosts.iter().any(|a| a.id == id || a.addr == socket_addr) {
                            info!("Id {id} or address {socket_addr} already known");
                        } else {
                            info!("Adding new host with id {id} and address {socket_addr}");

                            self.known_hosts.push(Host {
                                id,
                                addr: socket_addr,
                                devices: vec![],
                                status: HostStatus::Available,
                            })
                        }
                    } else {
                        info!("Failed to parse socket address {ip_string}");
                    }
                }
                (OrgUpdate::Up, Focused::Hosts(focus)) => self.focussed_panel = Focused::Hosts(focus.cursor_up(&self.known_hosts)),
                (OrgUpdate::Down, Focused::Hosts(focus)) => self.focussed_panel = Focused::Hosts(focus.cursor_down(&self.known_hosts)),
                (OrgUpdate::Tab, Focused::Hosts(focus)) => self.focussed_panel = Focused::Hosts(focus.tab(&self.known_hosts)),
                (OrgUpdate::BackTab, Focused::Hosts(focus)) => self.focussed_panel = Focused::Hosts(focus.back_tab(&self.known_hosts)),
                (OrgUpdate::Left, Focused::Hosts(focus)) => self.focussed_panel = Focused::Hosts(focus.cursor_left()),
                (OrgUpdate::Right, Focused::Hosts(focus)) => self.focussed_panel = Focused::Hosts(focus.cursor_right()),

                // Key logic for the experiment panel
                (OrgUpdate::Up, Focused::Experiments(focus)) => self.focussed_panel = Focused::Experiments(focus.up()),
                (OrgUpdate::Down, Focused::Experiments(focus)) => self.focussed_panel = Focused::Experiments(focus.down(self.experiments.len())),
                _ => {}
            },
        }
    }
    fn should_quit(&self) -> bool {
        self.should_quit
    }

    async fn on_tick(&mut self) {}
}

impl FocusedHosts {
    // Move cursor up
    fn cursor_up(&self, connected_hosts: &[Host]) -> FocusedHosts {
        match self {
            FocusedHosts::HostTree(0, 0) => FocusedHosts::HostTree(0, 0),
            FocusedHosts::HostTree(h, 0) if *h > 0 => {
                if let Some(host) = connected_hosts.get(h - 1) {
                    let d = host.devices.len();
                    FocusedHosts::HostTree(h - 1, d)
                } else {
                    FocusedHosts::HostTree(*h, 0)
                }
            }
            FocusedHosts::HostTree(h, d) if *d > 0 => FocusedHosts::HostTree(*h, d - 1),
            FocusedHosts::AddHost(_, FocusedAddHostField::Address) => {
                if let Some(host) = connected_hosts.last() {
                    let h = connected_hosts.len();
                    let d = host.devices.len();
                    FocusedHosts::HostTree(h - 1, d)
                } else {
                    // No hosts, fallback
                    FocusedHosts::AddHost(0, FocusedAddHostField::Address)
                }
            }
            FocusedHosts::AddHost(i, FocusedAddHostField::ID) if *i == 3 || *i == 7 || *i == 11 || *i == 15 => {
                FocusedHosts::AddHost(*i + 1, FocusedAddHostField::Address)
            }
            FocusedHosts::AddHost(i, FocusedAddHostField::ID) => FocusedHosts::AddHost(*i, FocusedAddHostField::Address),
            other => other.clone(),
        }
    }

    // Move cursor down
    fn cursor_down(&self, connected_hosts: &[Host]) -> FocusedHosts {
        match self {
            FocusedHosts::HostTree(h, d) if *h < connected_hosts.len() => {
                if let Some(host) = connected_hosts.get(*h) {
                    if *d < host.devices.len() {
                        FocusedHosts::HostTree(*h, d + 1)
                    } else if *h < connected_hosts.len() - 1 {
                        FocusedHosts::HostTree(h + 1, 0)
                    } else {
                        FocusedHosts::AddHost(0, FocusedAddHostField::Address)
                    }
                } else {
                    self.clone()
                }
            }
            FocusedHosts::AddHost(i, FocusedAddHostField::Address) => FocusedHosts::AddHost(*i, FocusedAddHostField::ID),
            other => other.clone(),
        }
    }

    // Move cursor left
    fn cursor_left(&self) -> FocusedHosts {
        match self {
            FocusedHosts::AddHost(i, FocusedAddHostField::Address) if *i == 4 || *i == 8 || *i == 12 || *i == 16 => {
                FocusedHosts::AddHost(i - 2, FocusedAddHostField::Address)
            }
            FocusedHosts::AddHost(i, f) if *i > 0 => FocusedHosts::AddHost(i - 1, *f),
            other => other.clone(),
        }
    }

    // Move cursor right
    fn cursor_right(&self) -> FocusedHosts {
        match self {
            FocusedHosts::AddHost(i, FocusedAddHostField::Address) if *i == 2 || *i == 6 || *i == 10 || *i == 14 => {
                FocusedHosts::AddHost(i + 2, FocusedAddHostField::Address)
            }
            FocusedHosts::AddHost(i, f) if *i < 20 => FocusedHosts::AddHost(i + 1, *f),
            other => other.clone(),
        }
    }

    // Tab key behavior
    fn tab(&self, connected_hosts: &[Host]) -> FocusedHosts {
        match self {
            FocusedHosts::HostTree(h, 0) if *h < connected_hosts.len() - 1 => FocusedHosts::HostTree(h + 1, 0),
            FocusedHosts::HostTree(h, 0) if *h == connected_hosts.len() - 1 => FocusedHosts::AddHost(0, FocusedAddHostField::Address),
            FocusedHosts::AddHost(i, FocusedAddHostField::Address) if *i < 14 => FocusedHosts::AddHost(4 * (i / 4) + 4, FocusedAddHostField::Address),
            FocusedHosts::AddHost(_, FocusedAddHostField::Address) => FocusedHosts::AddHost(0, FocusedAddHostField::ID),
            other => other.clone(),
        }
    }

    // BackTab (shift+tab) behavior
    fn back_tab(&self, connected_hosts: &[Host]) -> FocusedHosts {
        match self {
            FocusedHosts::HostTree(0, 0) => FocusedHosts::HostTree(0, 0),
            FocusedHosts::HostTree(h, 0) if *h > 0 => FocusedHosts::HostTree(h - 1, 0),
            FocusedHosts::AddHost(i, FocusedAddHostField::Address) if *i > 2 => FocusedHosts::AddHost(4 * (i / 4) - 4, FocusedAddHostField::Address),
            FocusedHosts::AddHost(_, FocusedAddHostField::ID) => FocusedHosts::AddHost(0, FocusedAddHostField::Address),
            other => other.clone(),
        }
    }
}

impl FocusedExperiments {
    fn up(&self) -> Self {
        match self {
            FocusedExperiments::Select(i) if *i > 0 => FocusedExperiments::Select(i - 1),
            FocusedExperiments::Select(i) => FocusedExperiments::Select(*i),
        }
    }

    fn down(&self, limit: usize) -> Self {
        match self {
            FocusedExperiments::Select(i) if i + 1 < limit => FocusedExperiments::Select(i + 1),
            FocusedExperiments::Select(i) => FocusedExperiments::Select(*i),
        }
    }
}
