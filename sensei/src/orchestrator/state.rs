use std::net::SocketAddr;

use async_trait::async_trait;
use crossterm::event::{KeyCode, KeyEvent};
use lib::csi_types::CsiData;
use lib::experiments::{ActiveExperiment, ExperimentStatus, Metadata};
use lib::network::rpc_message::{DEFAULT_ADDRESS, HostId, HostStatus as RegHostStatus, Responsiveness, SourceType};
use lib::tui::Tui;
use lib::tui::logs::{FromLog, LogEntry};
use log::{debug, info};
use ratatui::Frame;
use tokio::sync::mpsc::{Receiver, Sender};

use super::tui::ui;
use crate::orchestrator::OrchChannelMsg;

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
#[allow(unused)] // Expecting more host statuses to be implemented in the future
pub enum HostStatus {
    Available,
    Connected,
    Disconnected,
    Sending,
    Unresponsive,
    Unknown,
}

#[derive(Debug, Clone, PartialEq)]
#[allow(unused)] // Expecting more device statuses to be implemented in the future
pub enum DeviceStatus {
    Subscribed,
    NotSubscribed,
}

#[derive(Debug, Clone, PartialEq)]
#[allow(unused)] // Expecting more registry statuses to be implemented in the future
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
#[allow(unused)] // Updates might get used in the future
pub enum OrchUpdate {
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

impl FromLog for OrchUpdate {
    fn from_log(log: LogEntry) -> Self {
        OrchUpdate::Log(log)
    }
}

/// Holds the entire state of the TUI, including configurations, logs, and mode information.
pub struct OrchTuiState {
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

impl OrchTuiState {
    pub fn new() -> Self {
        OrchTuiState {
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
impl Tui<OrchUpdate, OrchChannelMsg> for OrchTuiState {
    /// Draws the UI layout and content.
    fn draw_ui(&self, frame: &mut Frame) {
        ui(frame, self);
    }

    /// Handles keyboard input events and maps them to updates.
    fn handle_keyboard_event(&self, key_event: KeyEvent) -> Option<OrchUpdate> {
        let key = key_event.code;
        match key {
            KeyCode::Char('q') | KeyCode::Char('Q') => return Some(OrchUpdate::Exit),
            KeyCode::Char('.') => return Some(OrchUpdate::ClearLogs),
            KeyCode::Char(',') => return Some(OrchUpdate::ClearCsi),
            KeyCode::Esc => return Some(OrchUpdate::FocusChange(Focus::Main)),
            _ => {}
        };
        let focus = self.focussed_panel.clone();
        match &focus {
            Focus::Main => match key {
                KeyCode::Char('l') | KeyCode::Char('L') => Some(OrchUpdate::FocusChange(Focus::Logs)),
                KeyCode::Char('c') | KeyCode::Char('C') => Some(OrchUpdate::FocusChange(Focus::Csi)),
                KeyCode::Char('r') | KeyCode::Char('R') => Some(OrchUpdate::FocusChange(Focus::Registry(FocusReg::RegistryAddress))),
                KeyCode::Char('e') | KeyCode::Char('E') => Some(OrchUpdate::FocusChange(Focus::Experiments(FocusExp::Select(0)))),
                KeyCode::Char('h') | KeyCode::Char('H') => {
                    if !self.known_hosts.is_empty() {
                        Some(OrchUpdate::FocusChange(Focus::Hosts(FocusHost::HostTree(0, 0))))
                    } else {
                        Some(OrchUpdate::FocusChange(Focus::Hosts(FocusHost::None)))
                    }
                }
                _ => None,
            },

            Focus::Registry(focused_registry_panel) => match focused_registry_panel {
                FocusReg::RegistryAddress => match key {
                    KeyCode::Tab => Some(OrchUpdate::Tab(focus)),
                    KeyCode::Down => Some(OrchUpdate::Down(focus)),
                    KeyCode::Char('c') | KeyCode::Char('C') => Some(OrchUpdate::ConnectRegistry),
                    KeyCode::Char('d') | KeyCode::Char('D') => Some(OrchUpdate::DisconnectRegistry),
                    KeyCode::Char('s') | KeyCode::Char('S') => Some(OrchUpdate::TogglePolling),
                    KeyCode::Char('p') | KeyCode::Char('P') => Some(OrchUpdate::Poll),
                    KeyCode::Char('a') | KeyCode::Char('A') => Some(OrchUpdate::AddAllHosts),
                    KeyCode::Char('m') | KeyCode::Char('M') => Some(OrchUpdate::FocusChange(Focus::Registry(FocusReg::AddHost(0)))),
                    _ => None,
                },
                FocusReg::AvailableHosts(host_idx) => match key {
                    KeyCode::Up => Some(OrchUpdate::Up(focus)),
                    KeyCode::Down => Some(OrchUpdate::Down(focus)),
                    KeyCode::Tab => Some(OrchUpdate::Tab(focus)),
                    KeyCode::BackTab => Some(OrchUpdate::BackTab(focus)),
                    KeyCode::Char('a') | KeyCode::Char('A') => self.registry_hosts.get(*host_idx).map(|host| OrchUpdate::AddHost((*host).clone())),
                    KeyCode::Char('m') | KeyCode::Char('M') => Some(OrchUpdate::FocusChange(Focus::Registry(FocusReg::AddHost(0)))),
                    _ => None,
                },
                FocusReg::AddHost(_) => match key {
                    KeyCode::Up => Some(OrchUpdate::Up(focus)),
                    KeyCode::Down => Some(OrchUpdate::Down(focus)),
                    KeyCode::Right => Some(OrchUpdate::Right(focus)),
                    KeyCode::Left => Some(OrchUpdate::Left(focus)),
                    KeyCode::Tab => Some(OrchUpdate::Tab(focus)),
                    KeyCode::BackTab => Some(OrchUpdate::BackTab(focus)),
                    KeyCode::Backspace => Some(OrchUpdate::Backspace(focus)),
                    KeyCode::Enter => Some(OrchUpdate::Enter(focus)),
                    KeyCode::Char(chr) => Some(OrchUpdate::Edit(chr, focus)),
                    _ => None,
                },
            },
            Focus::Hosts(focused_hosts_panel) => match focused_hosts_panel {
                FocusHost::None => None,
                FocusHost::HostTree(host_idx, 0) => match key {
                    KeyCode::Char('c') | KeyCode::Char('C') => self.known_hosts.get(*host_idx).map(|host| OrchUpdate::Connect(host.addr)),
                    KeyCode::Char('d') | KeyCode::Char('D') => self.known_hosts.get(*host_idx).map(|host| OrchUpdate::Disconnect(host.addr)),
                    KeyCode::Char('e') | KeyCode::Char('E') => self.known_hosts.get(*host_idx).map(|host| OrchUpdate::SelectHost(host.addr)),

                    KeyCode::Char('s') | KeyCode::Char('S') => self
                        .known_hosts
                        .get(*host_idx)
                        .map(|host| OrchUpdate::SubscribeAll(host.addr, self.selected_host)),
                    KeyCode::Char('u') | KeyCode::Char('U') => self
                        .known_hosts
                        .get(*host_idx)
                        .map(|host| OrchUpdate::UnsubscribeAll(host.addr, self.selected_host)),
                    KeyCode::Up => Some(OrchUpdate::Up(focus)),
                    KeyCode::Down => Some(OrchUpdate::Down(focus)),
                    KeyCode::Tab => Some(OrchUpdate::Tab(focus)),
                    KeyCode::BackTab => Some(OrchUpdate::BackTab(focus)),
                    _ => None,
                },
                FocusHost::HostTree(host_idx, device_idx) => match key {
                    KeyCode::Char('s') | KeyCode::Char('S') => {
                        // (device_idx - 1) to account for device_idx == 0 means selecting the host
                        if let Some(host) = self.known_hosts.get(*host_idx) {
                            host.devices
                                .get(device_idx - 1)
                                .map(|device| OrchUpdate::Subscribe(host.addr, self.selected_host, device.id))
                        } else {
                            None
                        }
                    }
                    KeyCode::Char('u') | KeyCode::Char('U') => {
                        if let Some(host) = self.known_hosts.get(*host_idx) {
                            host.devices
                                .get(device_idx - 1)
                                .map(|device| OrchUpdate::Unsubscribe(host.addr, self.selected_host, device.id))
                        } else {
                            None
                        }
                    }
                    KeyCode::Up => Some(OrchUpdate::Up(focus)),
                    KeyCode::Down => Some(OrchUpdate::Down(focus)),
                    KeyCode::Tab => Some(OrchUpdate::Tab(focus)),
                    KeyCode::BackTab => Some(OrchUpdate::BackTab(focus)),
                    _ => None,
                },
            },
            Focus::Experiments(FocusExp::Select(i)) => match key {
                KeyCode::Up => Some(OrchUpdate::Up(focus)),
                KeyCode::Down => Some(OrchUpdate::Down(focus)),
                KeyCode::Char('r') | KeyCode::Char('R') => Some(OrchUpdate::ReloadExperimentConfigs),
                KeyCode::Char('e') | KeyCode::Char('E') => {
                    if let Some(exp) = &self.active_experiment {
                        match exp.info.status {
                            ExperimentStatus::Running => Some(OrchUpdate::StopExperiment(self.selected_host)),
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
                                Some(OrchUpdate::StartExperiment(exp.experiment.metadata.remote_host))
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
                            ExperimentStatus::Done | ExperimentStatus::Ready | ExperimentStatus::Stopped => Some(OrchUpdate::SelectExperiment(*i)),
                            _ => None,
                        }
                    } else {
                        Some(OrchUpdate::SelectExperiment(*i))
                    }
                }
                _ => None,
            },
            Focus::Logs | Focus::Csi => match key {
                KeyCode::Up => Some(OrchUpdate::Up(focus)),
                KeyCode::Down => Some(OrchUpdate::Down(focus)),
                _ => None,
            },
        }
    }

    /// Applies updates and potentially sends commands to background tasks.
    async fn handle_update(&mut self, update: OrchUpdate, command_send: &Sender<OrchChannelMsg>, _update_recv: &mut Receiver<OrchUpdate>) {
        #[allow(unused_variables, unused_must_use)] // Not touching this monstrous match. TODO
        match update {
            OrchUpdate::Exit => {
                self.should_quit = true;
                command_send.send(OrchChannelMsg::Shutdown).await; // Graceful shutdown
            }
            OrchUpdate::Log(entry) => {
                self.logs.push(entry);
            }
            OrchUpdate::CsiData(data) => {
                self.csi.push(data);
            }
            OrchUpdate::Connect(to_addr) => {
                command_send.send(OrchChannelMsg::Connect(to_addr)).await;
                self.known_hosts.iter_mut().find(|a| a.addr == to_addr).unwrap().status = HostStatus::Connected;
            }
            OrchUpdate::Disconnect(from_addr) => {
                command_send.send(OrchChannelMsg::Disconnect(from_addr)).await;
                self.known_hosts.iter_mut().find(|a| a.addr == from_addr).unwrap().status = HostStatus::Disconnected;
            }
            OrchUpdate::Subscribe(to_addr, msg_origin_addr, device_id) => {
                command_send.send(OrchChannelMsg::Subscribe(to_addr, msg_origin_addr, device_id)).await;
            }
            OrchUpdate::Unsubscribe(to_addr, msg_origin_addr, device_id) => {
                command_send.send(OrchChannelMsg::Unsubscribe(to_addr, msg_origin_addr, device_id)).await;
            }
            OrchUpdate::SubscribeAll(to_addr, msg_origin_addr) => {
                command_send.send(OrchChannelMsg::SubscribeAll(to_addr, msg_origin_addr)).await;
            }
            OrchUpdate::UnsubscribeAll(to_addr, msg_origin_addr) => {
                command_send.send(OrchChannelMsg::UnsubscribeAll(to_addr, msg_origin_addr)).await;
            }
            OrchUpdate::SelectHost(selected_addr) => {
                self.selected_host = match self.selected_host {
                    None => Some(selected_addr),
                    Some(prev) if prev != selected_addr => Some(selected_addr),
                    _ => None,
                }
            }
            OrchUpdate::StartExperiment(addr_opt) => {
                if let Some(addr) = addr_opt {
                    command_send.send(OrchChannelMsg::StartRemoteExperiment(addr)).await;
                } else {
                    command_send.send(OrchChannelMsg::StartExperiment).await;
                }
            }
            OrchUpdate::StopExperiment(addr_opt) => {
                if let Some(addr) = addr_opt {
                    command_send.send(OrchChannelMsg::StopRemoteExperiment(addr)).await;
                } else {
                    command_send.send(OrchChannelMsg::StopExperiment).await;
                }
            }
            OrchUpdate::SelectExperiment(idx) => {
                command_send.send(OrchChannelMsg::SelectExperiment(idx)).await;
            }
            OrchUpdate::ActiveExperiment(active_experiment) => {
                // info!("Received experiment with status {:?}", active_experiment.status);
                self.active_experiment = Some(active_experiment);
            }
            OrchUpdate::UpdateExperimentList(experiments) => {
                self.experiments = experiments;
            }
            OrchUpdate::ReloadExperimentConfigs => {
                command_send.send(OrchChannelMsg::ReloadExperimentConfigs).await;
            }
            OrchUpdate::ClearLogs => self.logs.clear(),
            OrchUpdate::ClearCsi => self.csi.clear(),
            OrchUpdate::FocusChange(focused_panel) => self.focussed_panel = focused_panel,
            OrchUpdate::AddHost(status) => {
                if !self.known_hosts.iter().any(|h| h.id == status.id || h.addr == status.addr) {
                    debug!("Added host from registry");
                    let _ = command_send.send(OrchChannelMsg::Connect(status.addr)).await;
                    self.known_hosts.push(status);
                } else {
                    info!("Host already known");
                }
            }
            // Registry
            OrchUpdate::ConnectRegistry => {
                if self.registry_status == RegistryStatus::Disconnected {
                    if let Some(registry_addr) = self.registry_addr {
                        command_send.send(OrchChannelMsg::ConnectRegistry(registry_addr)).await;
                        self.registry_status = RegistryStatus::Connected;
                    }
                }
            }
            OrchUpdate::AddAllHosts => {
                self.known_hosts = self
                    .registry_hosts
                    .iter()
                    .filter(|h| h.status != HostStatus::Disconnected)
                    .cloned()
                    .collect();
            }
            OrchUpdate::RegistryIsConnected(is_connected) => {
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
            OrchUpdate::DisconnectRegistry => match self.registry_status {
                RegistryStatus::Polling | RegistryStatus::Connected | RegistryStatus::WaitingForConnection => {
                    command_send.send(OrchChannelMsg::DisconnectRegistry).await;
                }
                _ => {}
            },
            OrchUpdate::TogglePolling => match self.registry_status {
                RegistryStatus::Connected | RegistryStatus::Polling => {
                    self.registry_status = if self.is_polling {
                        self.is_polling = false;
                        if command_send.send(OrchChannelMsg::StopPolling).await.is_ok() {
                            RegistryStatus::Connected
                        } else {
                            RegistryStatus::Disconnected
                        }
                    } else if command_send.send(OrchChannelMsg::StartPolling).await.is_ok() {
                        self.is_polling = true;
                        RegistryStatus::Polling
                    } else {
                        RegistryStatus::Disconnected
                    }
                }
                _ => {}
            },
            OrchUpdate::Poll => {
                if self.registry_status == RegistryStatus::Connected {
                    command_send.send(OrchChannelMsg::Poll).await;
                }
            }

            OrchUpdate::UpdateHostStatuses(statuses) => {
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

            OrchUpdate::Backspace(Focus::Registry(focus)) => {
                if let FocusReg::AddHost(idx) = focus {
                    self.add_host_input_socket[idx] = '_';
                    self.focussed_panel = Focus::Registry(focus.cursor_left());
                }
            }
            OrchUpdate::Enter(Focus::Registry(_)) => {
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
            OrchUpdate::Edit(chr, Focus::Registry(focus)) => match focus {
                FocusReg::AddHost(idx) if chr.is_numeric() => {
                    self.add_host_input_socket[idx] = chr;
                    self.focussed_panel = Focus::Registry(focus.cursor_right());
                }
                _ => {}
            },

            OrchUpdate::Up(Focus::Hosts(focus)) => self.focussed_panel = Focus::Hosts(focus.cursor_up(&self.known_hosts)),
            OrchUpdate::Down(Focus::Hosts(focus)) => self.focussed_panel = Focus::Hosts(focus.cursor_down(&self.known_hosts)),
            OrchUpdate::Tab(Focus::Hosts(focus)) => self.focussed_panel = Focus::Hosts(focus.tab(&self.known_hosts)),
            OrchUpdate::BackTab(Focus::Hosts(focus)) => self.focussed_panel = Focus::Hosts(focus.back_tab(&self.known_hosts)),

            // Key logic for the experiment panel
            OrchUpdate::Up(Focus::Experiments(focus)) => self.focussed_panel = Focus::Experiments(focus.up()),
            OrchUpdate::Down(Focus::Experiments(focus)) => self.focussed_panel = Focus::Experiments(focus.down(self.experiments.len())),

            // Key logic for the registry panel
            OrchUpdate::Up(Focus::Registry(focus)) => self.focussed_panel = Focus::Registry(focus.cursor_up(self.registry_hosts.len())),
            OrchUpdate::Down(Focus::Registry(focus)) => self.focussed_panel = Focus::Registry(focus.cursor_down(self.registry_hosts.len())),
            OrchUpdate::Tab(Focus::Registry(focus)) => self.focussed_panel = Focus::Registry(focus.tab(self.registry_hosts.len())),
            OrchUpdate::BackTab(Focus::Registry(focus)) => self.focussed_panel = Focus::Registry(focus.back_tab(self.registry_hosts.len())),
            OrchUpdate::Left(Focus::Registry(focus)) => self.focussed_panel = Focus::Registry(focus.cursor_left()),
            OrchUpdate::Right(Focus::Registry(focus)) => self.focussed_panel = Focus::Registry(focus.cursor_right()),

            // Scoll logic for logs / csi
            OrchUpdate::Up(Focus::Logs) => self.logs_scroll_offset += 1,
            OrchUpdate::Up(Focus::Csi) => self.csi_scroll_offset += 1,
            OrchUpdate::Down(Focus::Logs) => self.logs_scroll_offset = self.logs_scroll_offset.saturating_sub(1),
            OrchUpdate::Down(Focus::Csi) => self.csi_scroll_offset = self.csi_scroll_offset.saturating_sub(1),

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
    fn back_tab(&self, _connected_hosts: &[Host]) -> FocusHost {
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
