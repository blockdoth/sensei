use std::net::SocketAddr;

use async_trait::async_trait;
use crossterm::event::{KeyCode, KeyEvent};
use lib::csi_types::CsiData;
use lib::experiments::{ActiveExperiment, ExperimentStatus, Metadata};
use lib::network::rpc_message::{DEFAULT_ADDRESS, HostId, HostStatus as RegHostStatus, Responsiveness, SourceType};
use lib::tui::Tui;
use lib::tui::logs::{FromLog, LogEntry};
use log::info;
use ratatui::Frame;
use tokio::sync::mpsc::{Receiver, Sender};

use super::tui::ui;
use crate::orchestrator::OrgChannelMsg;

#[derive(Debug, Clone, PartialEq)]
pub struct Host {
    pub id: HostId,
    pub addr: SocketAddr,
    pub devices: Vec<Device>,
    pub status: HostStatus,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Device {
    pub id: DeviceID,
    pub dev_type: SourceType,
    pub status: DeviceStatus,
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
pub enum DeviceStatus {
    Subscribed,
    NotSubscribed,
}

#[derive(Debug, Clone, PartialEq)]
pub enum RegistryStatus {
    Connected,
    Disconnected,
    Polling,
    NotSpecified,
    WaitingForConnection,
}

#[derive(PartialEq, Debug, Clone)]
pub enum Focus {
    Main,
    Registry(FocusReg),
    Hosts(FocusHost),
    Experiments(FocusExp),
    Logs,
    Csi,
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

/// Enum representing different types of updates/events in the TUI state.
#[derive(Debug, Clone)]
pub enum OrgUpdate {
    // === Hosts ===
    Connect(SocketAddr),
    Disconnect(SocketAddr),
    Subscribe(SocketAddr, Option<SocketAddr>, DeviceID),
    Unsubscribe(SocketAddr, Option<SocketAddr>, DeviceID),
    SubscribeAll(SocketAddr, Option<SocketAddr>),
    UnsubscribeAll(SocketAddr, Option<SocketAddr>),
    SelectHost(SocketAddr),

    // === Registry ===
    ConnectRegistry,
    DisconnectRegistry,
    RegistryIsConnected(bool),
    TogglePolling,
    AddAllHosts,
    Poll,
    AddHost(Host),
    UpdateHostStatuses(Vec<RegHostStatus>),

    // === Experiments ===
    StartExperiment(Option<SocketAddr>),
    StopExperiment(Option<SocketAddr>),
    SelectExperiment(usize),
    ActiveExperiment(ActiveExperiment),
    UpdateExperimentList(Vec<Metadata>),
    ReloadExperimentConfigs,

    // Misc
    Log(LogEntry),
    CsiData(CsiData),
    FocusChange(Focus),
    ClearLogs,
    ClearCsi,
    Exit,

    // === Keys ==
    Edit(char, Focus),
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
    pub registry_hosts: Vec<Host>,
    pub registry_addr: Option<SocketAddr>,
    pub registry_status: RegistryStatus,
    pub is_polling: bool,
    pub known_hosts: Vec<Host>,
    pub logs: Vec<LogEntry>,
    pub csi: Vec<CsiData>,
    pub focussed_panel: Focus,
    pub add_host_input_socket: [char; 21],
    pub selected_host: Option<SocketAddr>,
    pub active_experiment: Option<ActiveExperiment>,
    pub experiments: Vec<Metadata>,
    pub logs_scroll_offset: usize,
    pub csi_scroll_offset: usize,
}

impl OrgTuiState {
    pub fn new() -> Self {
        OrgTuiState {
            should_quit: false,
            registry_hosts: vec![],
            registry_addr: Some(DEFAULT_ADDRESS),
            registry_status: RegistryStatus::Disconnected,
            is_polling: false,
            logs: vec![],
            csi: vec![],
            focussed_panel: Focus::Main,
            active_experiment: None,
            experiments: vec![],
            add_host_input_socket: [
                '_', '_', '_', '.', '_', '_', '_', '.', '_', '_', '_', '.', '_', '_', '_', ':', '_', '_', '_', '_', '_',
            ],
            selected_host: None,
            known_hosts: vec![],
            logs_scroll_offset: 0,
            csi_scroll_offset: 0,
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
            KeyCode::Char(',') => return Some(OrgUpdate::ClearCsi),
            KeyCode::Esc => return Some(OrgUpdate::FocusChange(Focus::Main)),
            _ => {}
        };
        let focus = self.focussed_panel.clone();
        match &focus {
            Focus::Main => match key {
                KeyCode::Char('l') | KeyCode::Char('L') => Some(OrgUpdate::FocusChange(Focus::Logs)),
                KeyCode::Char('c') | KeyCode::Char('C') => Some(OrgUpdate::FocusChange(Focus::Csi)),
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
                    KeyCode::Char('s') | KeyCode::Char('S') => Some(OrgUpdate::TogglePolling),
                    KeyCode::Char('p') | KeyCode::Char('P') => Some(OrgUpdate::Poll),
                    KeyCode::Char('a') | KeyCode::Char('A') => Some(OrgUpdate::AddAllHosts),
                    KeyCode::Char('m') | KeyCode::Char('M') => Some(OrgUpdate::FocusChange(Focus::Registry(FocusReg::AddHost(0)))),
                    _ => None,
                },
                FocusReg::AvailableHosts(host_idx) => match key {
                    KeyCode::Up => Some(OrgUpdate::Up(focus)),
                    KeyCode::Down => Some(OrgUpdate::Down(focus)),
                    KeyCode::Tab => Some(OrgUpdate::Tab(focus)),
                    KeyCode::BackTab => Some(OrgUpdate::BackTab(focus)),
                    KeyCode::Char('a') | KeyCode::Char('A') => self.registry_hosts.get(*host_idx).map(|host| OrgUpdate::AddHost((*host).clone())),
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
                    KeyCode::Char(chr) => Some(OrgUpdate::Edit(chr, focus)),
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
                        // (device_idx - 1) to account for device_idx == 0 means selecting the host
                        if let Some(host) = self.known_hosts.get(*host_idx) {
                            host.devices
                                .get(device_idx - 1)
                                .map(|device| OrgUpdate::Subscribe(host.addr, self.selected_host, device.id))
                        } else {
                            None
                        }
                    }
                    KeyCode::Char('u') | KeyCode::Char('U') => {
                        if let Some(host) = self.known_hosts.get(*host_idx) {
                            host.devices
                                .get(device_idx - 1)
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
                KeyCode::Char('r') | KeyCode::Char('R') => Some(OrgUpdate::ReloadExperimentConfigs),
                KeyCode::Char('e') | KeyCode::Char('E') => {
                    if let Some(exp) = &self.active_experiment {
                        match exp.info.status {
                            ExperimentStatus::Running => Some(OrgUpdate::StopExperiment(self.selected_host)),
                            _ => None,
                        }
                    } else {
                        None
                    }
                }
                KeyCode::Char('b') | KeyCode::Char('B') => {
                    if let Some(exp) = &self.active_experiment {
                        match exp.info.status {
                            ExperimentStatus::Done | ExperimentStatus::Ready | ExperimentStatus::Stopped => {
                                Some(OrgUpdate::StartExperiment(exp.experiment.metadata.remote_host))
                            }
                            _ => None,
                        }
                    } else {
                        None
                    }
                }
                KeyCode::Char('s') | KeyCode::Char('S') => {
                    if let Some(exp) = &self.active_experiment {
                        match exp.info.status {
                            ExperimentStatus::Done | ExperimentStatus::Ready | ExperimentStatus::Stopped => Some(OrgUpdate::SelectExperiment(*i)),
                            _ => None,
                        }
                    } else {
                        Some(OrgUpdate::SelectExperiment(*i))
                    }
                }
                _ => None,
            },
            Focus::Logs | Focus::Csi => match key {
                KeyCode::Up => Some(OrgUpdate::Up(focus)),
                KeyCode::Down => Some(OrgUpdate::Down(focus)),
                _ => None,
            },
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
            OrgUpdate::StartExperiment(addr_opt) => {
                if let Some(addr) = addr_opt {
                    command_send.send(OrgChannelMsg::StartRemoteExperiment(addr)).await;
                } else {
                    command_send.send(OrgChannelMsg::StartExperiment).await;
                }
            }
            OrgUpdate::StopExperiment(addr_opt) => {
                if let Some(addr) = addr_opt {
                    command_send.send(OrgChannelMsg::StopRemoteExperiment(addr)).await;
                } else {
                    command_send.send(OrgChannelMsg::StopExperiment).await;
                }
            }
            OrgUpdate::SelectExperiment(idx) => {
                command_send.send(OrgChannelMsg::SelectExperiment(idx)).await;
            }
            OrgUpdate::ActiveExperiment(active_experiment) => {
                // info!("Received experiment with status {:?}", active_experiment.status);
                self.active_experiment = Some(active_experiment);
            }
            OrgUpdate::UpdateExperimentList(experiments) => {
                self.experiments = experiments;
            }
            OrgUpdate::ReloadExperimentConfigs => {
                command_send.send(OrgChannelMsg::ReloadExperimentConfigs).await;
            }
            OrgUpdate::ClearLogs => self.logs.clear(),
            OrgUpdate::ClearCsi => self.csi.clear(),
            OrgUpdate::FocusChange(focused_panel) => self.focussed_panel = focused_panel,
            OrgUpdate::AddHost(status) => {
                if !self.known_hosts.iter().any(|h| h.id == status.id || h.addr == status.addr) {
                    info!("Added host from registry");
                    self.known_hosts.push(status)
                } else {
                    info!("Host already known");
                }
            }
            // Registry
            OrgUpdate::ConnectRegistry => {
                if self.registry_status == RegistryStatus::Disconnected {
                    if let Some(registry_addr) = self.registry_addr {
                        command_send.send(OrgChannelMsg::ConnectRegistry(registry_addr)).await;
                        self.registry_status = RegistryStatus::Connected;
                    }
                }
            }
            OrgUpdate::AddAllHosts => {
                self.known_hosts = self
                    .registry_hosts
                    .iter()
                    .filter(|h| h.status != HostStatus::Disconnected)
                    .cloned()
                    .collect();
            }
            OrgUpdate::RegistryIsConnected(is_connected) => {
                self.registry_status = if is_connected {
                    RegistryStatus::Connected
                } else {
                    self.registry_hosts = self
                        .registry_hosts
                        .clone()
                        .into_iter()
                        .map(|mut h| {
                            h.devices = h
                                .devices
                                .into_iter()
                                .map(|mut d| {
                                    d.status = DeviceStatus::NotSubscribed;
                                    d
                                })
                                .collect();
                            h.status = HostStatus::Unknown;
                            h
                        })
                        .collect();

                    RegistryStatus::Disconnected
                }
            }
            OrgUpdate::DisconnectRegistry => match self.registry_status {
                RegistryStatus::Polling | RegistryStatus::Connected | RegistryStatus::WaitingForConnection => {
                    command_send.send(OrgChannelMsg::DisconnectRegistry).await;
                }
                _ => {}
            },
            OrgUpdate::TogglePolling => match self.registry_status {
                RegistryStatus::Connected | RegistryStatus::Polling => {
                    self.registry_status = if self.is_polling {
                        self.is_polling = false;
                        if command_send.send(OrgChannelMsg::StopPolling).await.is_ok() {
                            RegistryStatus::Connected
                        } else {
                            RegistryStatus::Disconnected
                        }
                    } else if command_send.send(OrgChannelMsg::StartPolling).await.is_ok() {
                        self.is_polling = true;
                        RegistryStatus::Polling
                    } else {
                        RegistryStatus::Disconnected
                    }
                }
                _ => {}
            },
            OrgUpdate::Poll => {
                if self.registry_status == RegistryStatus::Connected {
                    command_send.send(OrgChannelMsg::Poll).await;
                }
            }

            OrgUpdate::UpdateHostStatuses(statuses) => {
                // Couldnt get Into() to work
                self.registry_hosts = statuses
                    .iter()
                    .map(|h| Host {
                        id: h.host_id,
                        addr: h.addr,
                        devices: h
                            .device_statuses
                            .iter()
                            .map(|d| Device {
                                id: d.id,
                                dev_type: d.dev_type,
                                status: DeviceStatus::NotSubscribed,
                            })
                            .collect(),
                        status: match h.responsiveness {
                            Responsiveness::Connected => HostStatus::Connected,
                            Responsiveness::Lossy => HostStatus::Unresponsive,
                            Responsiveness::Disconnected => HostStatus::Disconnected,
                        },
                    })
                    .collect();
            }

            OrgUpdate::Backspace(Focus::Registry(focus)) => {
                if let FocusReg::AddHost(idx) = focus {
                    self.add_host_input_socket[idx] = '_';
                    self.focussed_panel = Focus::Registry(focus.cursor_left());
                }
            }
            OrgUpdate::Enter(Focus::Registry(_)) => {
                let ip_string = self.add_host_input_socket.iter().collect::<String>().replace("_", "");
                if let Ok(socket_addr) = ip_string.parse::<SocketAddr>() {
                    if self.known_hosts.iter().any(|a| a.addr == socket_addr) {
                        info!("Address {socket_addr} already known");
                    } else {
                        info!("Adding new host with address {socket_addr}");

                        self.known_hosts.push(Host {
                            id: 0,
                            addr: socket_addr,
                            devices: vec![],
                            status: HostStatus::Available,
                        })
                    }
                } else {
                    info!("Failed to parse socket address {ip_string}");
                }
            }
            OrgUpdate::Edit(chr, Focus::Registry(focus)) => match focus {
                FocusReg::AddHost(idx) if chr.is_numeric() => {
                    self.add_host_input_socket[idx] = chr;
                    self.focussed_panel = Focus::Registry(focus.cursor_right());
                }
                _ => {}
            },

            OrgUpdate::Up(Focus::Hosts(focus)) => self.focussed_panel = Focus::Hosts(focus.cursor_up(&self.known_hosts)),
            OrgUpdate::Down(Focus::Hosts(focus)) => self.focussed_panel = Focus::Hosts(focus.cursor_down(&self.known_hosts)),
            OrgUpdate::Tab(Focus::Hosts(focus)) => self.focussed_panel = Focus::Hosts(focus.tab(&self.known_hosts)),
            OrgUpdate::BackTab(Focus::Hosts(focus)) => self.focussed_panel = Focus::Hosts(focus.back_tab(&self.known_hosts)),

            // Key logic for the experiment panel
            OrgUpdate::Up(Focus::Experiments(focus)) => self.focussed_panel = Focus::Experiments(focus.up()),
            OrgUpdate::Down(Focus::Experiments(focus)) => self.focussed_panel = Focus::Experiments(focus.down(self.experiments.len())),

            // Key logic for the registry panel
            OrgUpdate::Up(Focus::Registry(focus)) => self.focussed_panel = Focus::Registry(focus.cursor_up(self.registry_hosts.len())),
            OrgUpdate::Down(Focus::Registry(focus)) => self.focussed_panel = Focus::Registry(focus.cursor_down(self.registry_hosts.len())),
            OrgUpdate::Tab(Focus::Registry(focus)) => self.focussed_panel = Focus::Registry(focus.tab(self.registry_hosts.len())),
            OrgUpdate::BackTab(Focus::Registry(focus)) => self.focussed_panel = Focus::Registry(focus.back_tab(self.registry_hosts.len())),
            OrgUpdate::Left(Focus::Registry(focus)) => self.focussed_panel = Focus::Registry(focus.cursor_left()),
            OrgUpdate::Right(Focus::Registry(focus)) => self.focussed_panel = Focus::Registry(focus.cursor_right()),

            // Scoll logic for logs / csi
            OrgUpdate::Up(Focus::Logs) => self.logs_scroll_offset += 1,
            OrgUpdate::Up(Focus::Csi) => self.csi_scroll_offset += 1,
            OrgUpdate::Down(Focus::Logs) => self.logs_scroll_offset = self.logs_scroll_offset.saturating_sub(1),
            OrgUpdate::Down(Focus::Csi) => self.csi_scroll_offset = self.csi_scroll_offset.saturating_sub(1),

            _ => {}
        }
    }
    fn should_quit(&self) -> bool {
        self.should_quit
    }

    async fn on_tick(&mut self) {}
}

impl FocusHost {
    /// Move cursor up
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

    /// Move cursor down
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

    /// Tab key behavior
    fn tab(&self, connected_hosts: &[Host]) -> FocusHost {
        match self {
            FocusHost::HostTree(h, 0) if *h < connected_hosts.len() - 1 => FocusHost::HostTree(h + 1, 0),
            other => other.clone(),
        }
    }

    /// BackTab (shift+tab) behavior
    fn back_tab(&self, connected_hosts: &[Host]) -> FocusHost {
        match self {
            FocusHost::HostTree(0, 0) => FocusHost::HostTree(0, 0),
            FocusHost::HostTree(h, 0) if *h > 0 => FocusHost::HostTree(h - 1, 0),
            other => other.clone(),
        }
    }
}

impl FocusExp {
    /// Move up
    fn up(&self) -> Self {
        match self {
            FocusExp::Select(i) if *i > 0 => FocusExp::Select(i - 1),
            FocusExp::Select(i) => FocusExp::Select(*i),
        }
    }
    /// Move down
    fn down(&self, limit: usize) -> Self {
        match self {
            FocusExp::Select(i) if i + 1 < limit => FocusExp::Select(i + 1),
            FocusExp::Select(i) => FocusExp::Select(*i),
        }
    }
}

impl FocusReg {
    /// Move cursor up
    fn cursor_up(&self, host_count: usize) -> FocusReg {
        match self {
            FocusReg::AvailableHosts(0) => FocusReg::RegistryAddress,
            FocusReg::AvailableHosts(i) if *i > 0 => FocusReg::AvailableHosts(i - 1),
            FocusReg::AddHost(_) => FocusReg::AvailableHosts(host_count.saturating_sub(1)),
            other => other.clone(),
        }
    }
    /// Move cursor down
    fn cursor_down(&self, limit: usize) -> FocusReg {
        match self {
            FocusReg::AvailableHosts(i) if i + 1 < limit => FocusReg::AvailableHosts(i + 1),
            FocusReg::AvailableHosts(_) => FocusReg::AddHost(0),
            FocusReg::RegistryAddress => FocusReg::AvailableHosts(0),
            other => other.clone(),
        }
    }
    /// Move cursor left
    fn cursor_left(&self) -> FocusReg {
        match self {
            FocusReg::AddHost(i) if *i == 4 || *i == 8 || *i == 12 || *i == 16 => FocusReg::AddHost(i - 2),
            FocusReg::AddHost(i) if *i > 0 => FocusReg::AddHost(i - 1),
            other => other.clone(),
        }
    }
    /// Move cursor right
    fn cursor_right(&self) -> FocusReg {
        match self {
            FocusReg::AddHost(i) if *i == 2 || *i == 6 || *i == 10 || *i == 14 => FocusReg::AddHost(i + 2),
            FocusReg::AddHost(i) if *i < 20 => FocusReg::AddHost(i + 1),
            other => other.clone(),
        }
    }
    /// Tab
    fn tab(&self, limit: usize) -> FocusReg {
        match self {
            FocusReg::AvailableHosts(i) if i + 1 < limit => FocusReg::AvailableHosts(i + 1),
            FocusReg::AvailableHosts(_) => FocusReg::AddHost(0),
            FocusReg::RegistryAddress => FocusReg::AvailableHosts(0),
            FocusReg::AddHost(i) if *i < 14 => FocusReg::AddHost(4 * (i / 4) + 4),
            other => other.clone(),
        }
    }
    /// Back tab
    fn back_tab(&self, host_count: usize) -> FocusReg {
        match self {
            FocusReg::AvailableHosts(0) => FocusReg::RegistryAddress,
            FocusReg::AvailableHosts(i) if *i > 0 => FocusReg::AvailableHosts(i - 1),
            FocusReg::AddHost(i) if *i > 2 => FocusReg::AddHost(4 * (i / 4) - 4),
            FocusReg::AddHost(0) => FocusReg::AvailableHosts(host_count.saturating_sub(1)),
            other => other.clone(),
        }
    }
}
