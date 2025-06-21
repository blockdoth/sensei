use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};

use async_trait::async_trait;
use crossterm::event::{KeyCode, KeyEvent};
use lib::csi_types::CsiData;
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

#[derive(Debug, Clone, PartialEq)]
pub struct Host {
    pub id: Option<HostId>,
    pub addr: SocketAddr,
    pub devices: Vec<Device>,
    pub status: HostStatus,
}

#[derive(Debug, Clone, PartialEq)]
pub enum HostStatus {
    Available,
    Connected,
    Disconnected,
    Sending,
    Unresponsive,
    Unknown,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Device {
    pub id: DeviceID,
    pub dev_type: SourceType,
    pub status: DeviceStatus,
}
#[derive(Debug, Clone, PartialEq)]
pub enum DeviceStatus {
    Subscribed,
    NotSubscribed,
}
pub enum RegistryStatus {
    Connected,
    Disconnected,
    NotSpecified,
}

#[derive(PartialEq, Debug, Clone)]
pub enum Focus {
    Main,
    Registry(FocusReg),
    Hosts(FocusHost),
    Experiments(FocusExp),
    Logs,
}

#[derive(PartialEq, Debug, Clone)]
pub enum FocusExp {
    Select(usize),
}

#[derive(PartialEq, Debug, Clone)]
pub enum FocusReg {
    RegistryAddress,
    AvailableHosts(usize),
    AddHost(usize),
}

#[derive(PartialEq, Debug, Clone)]
pub enum FocusHost {
    None,
    HostTree(usize, usize),
}

type DeviceID = u64;

#[derive(Debug)]
pub enum OrgUpdate {
    Log(LogEntry),
    CsiData(CsiData),
    Connect(SocketAddr),
    Disconnect(SocketAddr),
    Subscribe(SocketAddr, Option<SocketAddr>, DeviceID),
    Unsubscribe(SocketAddr, Option<SocketAddr>, DeviceID),
    SubscribeAll(SocketAddr, Option<SocketAddr>),
    UnsubscribeAll(SocketAddr, Option<SocketAddr>),
    SelectHost(SocketAddr),
    FocusChange(Focus),
    AddHost(Host),
    SelectExperiment(usize),
    ActiveExperiment(ActiveExperiment),
    UpdateExperimentList(Vec<ExperimentMetadata>),
    Edit(char),
    ConnectRegistry,
    DisconnectRegistry,
    StatusUpdate,
    StartExperiment,
    StopExperiment,
    ClearLogs,
    ClearCsi,
    Exit,
    // Keys
    Up(Focus),
    Down(Focus),
    Left(Focus),
    Right(Focus),
    Tab(Focus),
    BackTab(Focus),
    Backspace(Focus),
    Enter(Focus),
}

impl FromLog for OrgUpdate {
    fn from_log(log: LogEntry) -> Self {
        OrgUpdate::Log(log)
    }
}

/// Holds the entire state of the TUI, including configurations, logs, and mode information.
pub struct OrgTuiState {
    pub should_quit: bool,
    pub hosts_from_reg: Vec<Host>,
    pub registry_addr: Option<SocketAddr>,
    pub registry_status: RegistryStatus,
    pub known_hosts: Vec<Host>,
    pub logs: Vec<LogEntry>,
    pub csi: Vec<CsiData>,
    pub focussed_panel: Focus,
    pub add_host_input_socket: [char; 21],
    pub selected_host: Option<SocketAddr>,
    pub active_experiment: Option<ActiveExperiment>,
    pub experiments: Vec<ExperimentMetadata>,
}

impl OrgTuiState {
    pub fn new() -> Self {
        OrgTuiState {
            should_quit: false,
            hosts_from_reg: vec![
                Host {
                    id: Some(100),
                    addr: SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 4242)),
                    devices: vec![],
                    status: HostStatus::Available,
                },
                Host {
                    id: Some(101),
                    addr: SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 2121)),
                    devices: vec![],
                    status: HostStatus::Unresponsive,
                },
            ],
            registry_addr: Some(DEFAULT_ADDRESS),
            registry_status: RegistryStatus::Disconnected,
            logs: vec![],
            csi: vec![],
            focussed_panel: Focus::Main,
            active_experiment: None,
            experiments: vec![],
            add_host_input_socket: [
                '_', '_', '_', '.', '_', '_', '_', '.', '_', '_', '_', '.', '_', '_', '_', ':', '_', '_', '_', '_', '_',
            ],
            selected_host: None,
            known_hosts: vec![
                Host {
                    id: Some(0),
                    status: HostStatus::Available,
                    devices: vec![],
                    addr: SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 8080)),
                },
                Host {
                    id: Some(2),
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
            KeyCode::Esc => return Some(OrgUpdate::FocusChange(Focus::Main)),
            _ => {}
        };
        let focus = self.focussed_panel.clone();
        match &focus {
            Focus::Main => match key {
                KeyCode::Char('r') | KeyCode::Char('R') => Some(OrgUpdate::FocusChange(Focus::Registry(FocusReg::RegistryAddress))),
                KeyCode::Char('e') | KeyCode::Char('E') => Some(OrgUpdate::FocusChange(Focus::Experiments(FocusExp::Select(0)))),
                KeyCode::Char('h') | KeyCode::Char('H') => {
                    if !self.known_hosts.is_empty() {
                        Some(OrgUpdate::FocusChange(Focus::Hosts(FocusHost::HostTree(0, 0))))
                    } else {
                        Some(OrgUpdate::FocusChange(Focus::Hosts(FocusHost::None)))
                    }
                }
                _ => None,
            },

            Focus::Registry(focused_registry_panel) => match focused_registry_panel {
                FocusReg::RegistryAddress => match key {
                    KeyCode::Tab => Some(OrgUpdate::Tab(focus)),
                    KeyCode::Down => Some(OrgUpdate::Down(focus)),
                    KeyCode::Char('c') | KeyCode::Char('C') => Some(OrgUpdate::ConnectRegistry),
                    KeyCode::Char('d') | KeyCode::Char('D') => Some(OrgUpdate::DisconnectRegistry),
                    KeyCode::Char('s') | KeyCode::Char('S') => Some(OrgUpdate::StatusUpdate),
                    KeyCode::Char('m') | KeyCode::Char('M') => Some(OrgUpdate::FocusChange(Focus::Registry(FocusReg::AddHost(0)))),
                    _ => None,
                },
                FocusReg::AvailableHosts(host_idx) => match key {
                    KeyCode::Up => Some(OrgUpdate::Up(focus)),
                    KeyCode::Down => Some(OrgUpdate::Down(focus)),
                    KeyCode::Tab => Some(OrgUpdate::Tab(focus)),
                    KeyCode::BackTab => Some(OrgUpdate::BackTab(focus)),
                    KeyCode::Char('a') | KeyCode::Char('A') => self.hosts_from_reg.get(*host_idx).map(|host| OrgUpdate::AddHost((*host).clone())),
                    KeyCode::Char('m') | KeyCode::Char('M') => Some(OrgUpdate::FocusChange(Focus::Registry(FocusReg::AddHost(0)))),
                    _ => None,
                },
                FocusReg::AddHost(idx) => match key {
                    KeyCode::Up => Some(OrgUpdate::Up(focus)),
                    KeyCode::Down => Some(OrgUpdate::Down(focus)),
                    KeyCode::Right => Some(OrgUpdate::Right(focus)),
                    KeyCode::Left => Some(OrgUpdate::Left(focus)),
                    KeyCode::Tab => Some(OrgUpdate::Tab(focus)),
                    KeyCode::BackTab => Some(OrgUpdate::BackTab(focus)),
                    KeyCode::Backspace => Some(OrgUpdate::Backspace(focus)),
                    KeyCode::Enter => Some(OrgUpdate::Enter(focus)),
                    KeyCode::Char(chr) => Some(OrgUpdate::Edit(chr)),
                    _ => None,
                },
            },
            Focus::Hosts(focused_hosts_panel) => match focused_hosts_panel {
                FocusHost::None => None,
                FocusHost::HostTree(host_idx, 0) => match key {
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
                    KeyCode::Up => Some(OrgUpdate::Up(focus)),
                    KeyCode::Down => Some(OrgUpdate::Down(focus)),
                    KeyCode::Tab => Some(OrgUpdate::Tab(focus)),
                    KeyCode::BackTab => Some(OrgUpdate::BackTab(focus)),
                    _ => None,
                },
                FocusHost::HostTree(host_idx, device_idx) => match key {
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
                    KeyCode::Up => Some(OrgUpdate::Up(focus)),
                    KeyCode::Down => Some(OrgUpdate::Down(focus)),
                    KeyCode::Tab => Some(OrgUpdate::Tab(focus)),
                    KeyCode::BackTab => Some(OrgUpdate::BackTab(focus)),
                    _ => None,
                },
            },
            Focus::Experiments(FocusExp::Select(i)) => match key {
                KeyCode::Up => Some(OrgUpdate::Up(focus)),
                KeyCode::Down => Some(OrgUpdate::Down(focus)),
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
            Focus::Logs => None,
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
            OrgUpdate::CsiData(data) => {
                self.csi.push(data);
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
            OrgUpdate::ClearCsi => self.csi.clear(),
            OrgUpdate::FocusChange(focused_panel) => self.focussed_panel = focused_panel,
            OrgUpdate::AddHost(host) => {
                if !self.known_hosts.iter().any(|h| h.id == host.id || h.addr == host.addr) {
                    info!("Adding new host from registry");
                    self.known_hosts.push(host)
                } else {
                    info!("Host already known");
                }
            }

            OrgUpdate::ConnectRegistry => {
                // todo!()
                self.registry_status = RegistryStatus::Connected
            }
            OrgUpdate::DisconnectRegistry => {
                // todo!()
                self.registry_status = RegistryStatus::Disconnected
            }
            OrgUpdate::StatusUpdate => {
                todo!()
            }

            OrgUpdate::Backspace(Focus::Registry(focus)) => {
                if let FocusReg::AddHost(idx) = focus {
                    self.add_host_input_socket[idx] = '_';
                    self.focussed_panel = Focus::Registry(focus.cursor_left());
                }
            }
            OrgUpdate::Enter(Focus::Hosts(focus)) => {
                let ip_string = self.add_host_input_socket.iter().collect::<String>().replace("_", "");
                if let Ok(socket_addr) = ip_string.parse::<SocketAddr>() {

                    if self.known_hosts.iter().any(|a|  a.addr == socket_addr) {
                        info!("Address {socket_addr} already known");
                    } else {
                        info!("Adding new host with address {socket_addr}");

                        self.known_hosts.push(Host {
                            id: None,
                            addr: socket_addr,
                            devices: vec![],
                            status: HostStatus::Available,
                        })
                    }
                } else {
                    info!("Failed to parse socket address {ip_string}");
                }
            }
            OrgUpdate::Up(Focus::Hosts(focus)) => self.focussed_panel = Focus::Hosts(focus.cursor_up(&self.known_hosts)),
            OrgUpdate::Down(Focus::Hosts(focus)) => self.focussed_panel = Focus::Hosts(focus.cursor_down(&self.known_hosts)),
            OrgUpdate::Tab(Focus::Hosts(focus)) => self.focussed_panel = Focus::Hosts(focus.tab(&self.known_hosts)),
            OrgUpdate::BackTab(Focus::Hosts(focus)) => self.focussed_panel = Focus::Hosts(focus.back_tab(&self.known_hosts)),

            // Key logic for the experiment panel
            OrgUpdate::Up(Focus::Experiments(focus)) => self.focussed_panel = Focus::Experiments(focus.up()),
            OrgUpdate::Down(Focus::Experiments(focus)) => self.focussed_panel = Focus::Experiments(focus.down(self.experiments.len())),

            // Key logic for the registry panel
            OrgUpdate::Up(Focus::Registry(focus)) => self.focussed_panel = Focus::Registry(focus.cursor_up(self.hosts_from_reg.len())),
            OrgUpdate::Down(Focus::Registry(focus)) => self.focussed_panel = Focus::Registry(focus.cursor_down(self.hosts_from_reg.len())),
            OrgUpdate::Tab(Focus::Registry(focus)) => self.focussed_panel = Focus::Registry(focus.tab(self.hosts_from_reg.len())),
            OrgUpdate::BackTab(Focus::Registry(focus)) => self.focussed_panel = Focus::Registry(focus.back_tab()),
            OrgUpdate::Left(Focus::Registry(focus)) => self.focussed_panel = Focus::Registry(focus.cursor_left()),
            OrgUpdate::Right(Focus::Registry(focus)) => self.focussed_panel = Focus::Registry(focus.cursor_right()),

            // Handles key updates, which are highly dependant on which panel is focussed
            key_update => {},
        }
    }
    fn should_quit(&self) -> bool {
        self.should_quit
    }

    async fn on_tick(&mut self) {}
}
impl FocusHost {
    // Move cursor up
    fn cursor_up(&self, connected_hosts: &[Host]) -> FocusHost {
        match self {
            FocusHost::HostTree(0, 0) => FocusHost::HostTree(0, 0),
            FocusHost::HostTree(h, 0) if *h > 0 => {
                if let Some(host) = connected_hosts.get(h - 1) {
                    let d = host.devices.len();
                    FocusHost::HostTree(h - 1, d)
                } else {
                    FocusHost::HostTree(*h, 0)
                }
            }
            FocusHost::HostTree(h, d) if *d > 0 => FocusHost::HostTree(*h, d - 1),
            other => other.clone(),
        }
    }

    // Move cursor down
    fn cursor_down(&self, connected_hosts: &[Host]) -> FocusHost {
        match self {
            FocusHost::HostTree(h, d) if *h < connected_hosts.len() => {
                if let Some(host) = connected_hosts.get(*h) {
                    if *d < host.devices.len() {
                        FocusHost::HostTree(*h, d + 1)
                    } else if *h < connected_hosts.len() - 1 {
                        FocusHost::HostTree(h + 1, 0)
                    } else {
                        FocusHost::HostTree(*h, *d)
                    }
                } else {
                    self.clone()
                }
            }
            other => other.clone(),
        }
    }

    // Tab key behavior
    fn tab(&self, connected_hosts: &[Host]) -> FocusHost {
        match self {
            FocusHost::HostTree(h, 0) if *h < connected_hosts.len() - 1 => FocusHost::HostTree(h + 1, 0),
            other => other.clone(),
        }
    }

    // BackTab (shift+tab) behavior
    fn back_tab(&self, connected_hosts: &[Host]) -> FocusHost {
        match self {
            FocusHost::HostTree(0, 0) => FocusHost::HostTree(0, 0),
            FocusHost::HostTree(h, 0) if *h > 0 => FocusHost::HostTree(h - 1, 0),
            other => other.clone(),
        }
    }
}

impl FocusExp {
    fn up(&self) -> Self {
        match self {
            FocusExp::Select(i) if *i > 0 => FocusExp::Select(i - 1),
            FocusExp::Select(i) => FocusExp::Select(*i),
        }
    }

    fn down(&self, limit: usize) -> Self {
        match self {
            FocusExp::Select(i) if i + 1 < limit => FocusExp::Select(i + 1),
            FocusExp::Select(i) => FocusExp::Select(*i),
        }
    }
}

impl FocusReg {
    fn cursor_up(&self, host_count:usize) -> FocusReg {
        match self {
            FocusReg::AvailableHosts(0) => FocusReg::RegistryAddress,
            FocusReg::AvailableHosts(i) if *i > 0 => FocusReg::AvailableHosts(i - 1),
            FocusReg::AddHost(0) => FocusReg::AvailableHosts(host_count),
            FocusReg::AddHost(i) if *i == 3 || *i == 7 || *i == 11 || *i == 15 => FocusReg::AddHost(*i + 1),
            FocusReg::AddHost(i) if *i >= 4  => FocusReg::AddHost(i - 4),
            other => other.clone(),
        }
    }

    fn cursor_down(&self, limit: usize) -> FocusReg {
        match self {
            FocusReg::AvailableHosts(i) if i + 1 < limit => FocusReg::AvailableHosts(i + 1),
            FocusReg::RegistryAddress => FocusReg::AvailableHosts(0),
            FocusReg::AddHost(i) if *i < 16 => FocusReg::AddHost(*i + 4),
            other => other.clone(),
        }
    }

    fn cursor_left(&self) -> FocusReg {
        match self {
            FocusReg::AddHost(i) if *i == 4 || *i == 8 || *i == 12 || *i == 16 => FocusReg::AddHost(i - 2),
            FocusReg::AddHost(i) if *i > 0 => FocusReg::AddHost(i - 1),
            other => other.clone(),
        }
    }

    fn cursor_right(&self) -> FocusReg {
        match self {
            FocusReg::AddHost(i) if *i == 2 || *i == 6 || *i == 10 || *i == 14 => FocusReg::AddHost(i + 2),
            FocusReg::AddHost(i) if *i < 20 => FocusReg::AddHost(i + 1),
            other => other.clone(),
        }
    }

    fn tab(&self, limit: usize) -> FocusReg {
        match self {
            FocusReg::AvailableHosts(i) if i + 1 < limit => FocusReg::AvailableHosts(i + 1),
            FocusReg::RegistryAddress => FocusReg::AvailableHosts(0),
            FocusReg::AddHost(i) if *i < 14 => FocusReg::AddHost(4 * (i / 4) + 4),
            FocusReg::AddHost(_) => FocusReg::AddHost(0),
            other => other.clone(),
        }
    }

    fn back_tab(&self) -> FocusReg {
        match self {
            FocusReg::AvailableHosts(0) => FocusReg::RegistryAddress,
            FocusReg::AvailableHosts(i) if *i > 0 => FocusReg::AvailableHosts(i - 1),
            FocusReg::AddHost(i) if *i > 2 => FocusReg::AddHost(4 * (i / 4) - 4),
            FocusReg::AddHost(_) => FocusReg::AddHost(0),
            other => other.clone(),
        }
    }
}
