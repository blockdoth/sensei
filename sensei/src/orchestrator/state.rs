use std::net::SocketAddr;
use std::sync::Arc;

use async_trait::async_trait;
use crossterm::event::{KeyCode, KeyEvent};
use lib::network::rpc_message::RpcMessageKind::Ctrl;
use lib::network::rpc_message::{CtrlMsg, HostId, SourceType};
use lib::network::tcp::ChannelMsg;
use lib::network::tcp::client::TcpClient;
use lib::tui::Tui;
use lib::tui::logs::{FromLog, LogEntry};
use log::info;
use ratatui::Frame;
use tokio::sync::Mutex;
use tokio::sync::mpsc::{Receiver, Sender};

use super::tui::ui;
use crate::services::DEFAULT_ADDRESS;

pub struct OrgTuiState {
    pub should_quit: bool,
    pub client: Arc<Mutex<TcpClient>>,
    pub known_hosts: Vec<SocketAddr>,
    pub registry_addr: Option<SocketAddr>,
    pub registry_status: RegistryStatus,
    pub connected_hosts: Vec<Host>,
    pub logs: Vec<LogEntry>,
    pub focussed_panel: Focused,
}

pub struct Host {
    pub id: HostId,
    pub addr: SocketAddr,
    pub devices: Vec<Device>,
    pub status: HostStatus,
}

#[derive(Debug, PartialEq)]
pub enum HostStatus {
    Connected,
    Dead,
    Sending,
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

#[derive(PartialEq)]
pub enum Focused {
    Main,
    Registry(FocusedRegistry),
    Hosts(FocusedHosts),
    Experiments,
    Status,
}

#[derive(PartialEq)]
pub enum FocusedRegistry {
    RegistryAddress(usize),
    AvailableHosts(usize),
}

#[derive(PartialEq)]
pub enum FocusedHosts {
    None,
    HostTree(usize, usize),
    AddHost(usize),
}

pub enum OrgCommand {}

type DeviceID = u64;
pub enum OrgUpdate {
    Log(LogEntry),
    Connect(SocketAddr),
    Disconnect(SocketAddr),
    Subscribe(SocketAddr, DeviceID),
    Unsubscribe(SocketAddr, DeviceID),
    FocusChange(Focused),
    SubscribeAll(SocketAddr),
    UnsubscribeAll(SocketAddr),
    Exit,
    Up,
    Down,
    Left,
    Right,
    Tab,
    BackTab,
}

impl FromLog for OrgUpdate {
    fn from_log(log: LogEntry) -> Self {
        OrgUpdate::Log(log)
    }
}

impl OrgTuiState {
    pub fn new(client: Arc<Mutex<TcpClient>>) -> Self {
        OrgTuiState {
            client,
            should_quit: false,
            known_hosts: vec![DEFAULT_ADDRESS, DEFAULT_ADDRESS, DEFAULT_ADDRESS],
            registry_addr: Some(DEFAULT_ADDRESS),
            registry_status: RegistryStatus::Disconnected,
            logs: vec![],
            focussed_panel: Focused::Main,
            connected_hosts: vec![
                Host {
                    id: 0,
                    status: HostStatus::Dead,
                    devices: vec![],
                    addr: DEFAULT_ADDRESS,
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
                Host {
                    id: 1,
                    status: HostStatus::Connected,
                    devices: vec![
                        Device {
                            id: 10,
                            dev_type: SourceType::TCP,
                            status: DeviceStatus::NotSubscribed,
                        },
                        Device {
                            id: 90,
                            dev_type: SourceType::Unknown,
                            status: DeviceStatus::Subscribed,
                        },
                        Device {
                            id: 91,
                            dev_type: SourceType::CSV,
                            status: DeviceStatus::NotSubscribed,
                        },
                        Device {
                            id: 42,
                            dev_type: SourceType::TCP,
                            status: DeviceStatus::NotSubscribed,
                        },
                        Device {
                            id: 911,
                            dev_type: SourceType::AX200,
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
impl Tui<OrgUpdate, ChannelMsg> for OrgTuiState {
    /// Draws the UI layout and content.
    fn draw_ui(&self, frame: &mut Frame) {
        ui(frame, self);
    }

    /// Handles keyboard input events and maps them to updates.
    fn handle_keyboard_event(&self, key_event: KeyEvent) -> Option<OrgUpdate> {
        let key = key_event.code;
        match key {
            KeyCode::Char('q') | KeyCode::Char('Q') => return Some(OrgUpdate::Exit),
            KeyCode::Esc => return Some(OrgUpdate::FocusChange(Focused::Main)),
            _ => {}
        };

        match &self.focussed_panel {
            Focused::Main => match key {
                KeyCode::Char('r') | KeyCode::Char('R') => Some(OrgUpdate::FocusChange(Focused::Registry(FocusedRegistry::RegistryAddress(0)))),
                KeyCode::Char('e') | KeyCode::Char('E') => Some(OrgUpdate::FocusChange(Focused::Experiments)),
                KeyCode::Char('h') | KeyCode::Char('H') => {
                    if !self.connected_hosts.is_empty() {
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
                    KeyCode::Char('a') | KeyCode::Char('A') => Some(OrgUpdate::FocusChange(Focused::Hosts(FocusedHosts::AddHost(0)))),
                    _ => None,
                },
                FocusedHosts::AddHost(host_idx) => None,
                FocusedHosts::HostTree(host_idx, 0) => match key {
                    KeyCode::Char('c') | KeyCode::Char('C') => self.connected_hosts.get(*host_idx).map(|host| OrgUpdate::Connect(host.addr)),

                    KeyCode::Char('d') | KeyCode::Char('D') => self.connected_hosts.get(*host_idx).map(|host| OrgUpdate::Disconnect(host.addr)),

                    KeyCode::Char('s') | KeyCode::Char('S') => self.connected_hosts.get(*host_idx).map(|host| OrgUpdate::SubscribeAll(host.addr)),
                    KeyCode::Char('u') | KeyCode::Char('U') => self.connected_hosts.get(*host_idx).map(|host| OrgUpdate::UnsubscribeAll(host.addr)),
                    KeyCode::Up => Some(OrgUpdate::Up),
                    KeyCode::Down => Some(OrgUpdate::Down),
                    KeyCode::Tab => Some(OrgUpdate::Tab),
                    KeyCode::BackTab => Some(OrgUpdate::BackTab),
                    _ => None,
                },
                FocusedHosts::HostTree(host_idx, device_idx) => match key {
                    KeyCode::Char('s') | KeyCode::Char('S') => {
                        if let Some(host) = self.connected_hosts.get(*host_idx) {
                            host.devices.get(*device_idx).map(|device| OrgUpdate::Subscribe(host.addr, device.id))
                        } else {
                            None
                        }
                    }
                    KeyCode::Char('u') | KeyCode::Char('U') => {
                        if let Some(host) = self.connected_hosts.get(*host_idx) {
                            host.devices.get(*device_idx).map(|device| OrgUpdate::Unsubscribe(host.addr, device.id))
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
            },
            Focused::Status => None,
            Focused::Experiments => None,
        }
    }

    /// Applies updates and potentially sends commands to background tasks.
    async fn handle_update(&mut self, update: OrgUpdate, command_send: &Sender<ChannelMsg>, update_recv: &mut Receiver<OrgUpdate>) {
        match update {
            OrgUpdate::Exit => {
                self.should_quit = true;
                command_send.send(ChannelMsg::Shutdown).await; // Graceful shutdown
            }
            OrgUpdate::Log(entry) => {
                self.logs.push(entry);
            }
            OrgUpdate::Connect(socket_addr) => {
                info!("Connecting to {socket_addr}");
                self.client.lock().await.connect(socket_addr);
                self.known_hosts.push(socket_addr);
            }
            OrgUpdate::Disconnect(socket_addr) => {
                info!("Disconnecting from {socket_addr}");
                self.client.lock().await.disconnect(socket_addr);
            }
            OrgUpdate::Subscribe(socket_addr, device_id) => {
                info!("Subscribing to device {device_id} on host {socket_addr}");
                let msg = Ctrl(CtrlMsg::Subscribe { device_id });
                self.client.lock().await.send_message(socket_addr, msg);
            }
            OrgUpdate::Unsubscribe(socket_addr, device_id) => {
                info!("Unsubscribing to device {device_id} on host {socket_addr}");
                let msg = Ctrl(CtrlMsg::Unsubscribe { device_id });
                self.client.lock().await.send_message(socket_addr, msg);
            }
            OrgUpdate::FocusChange(focused_panel) => self.focussed_panel = focused_panel,
            OrgUpdate::SubscribeAll(socket_addr) => {
                let mut client = self.client.lock().await;
                if let Some(host) = self.connected_hosts.iter().find(|a| a.addr == socket_addr) {
                    info!("Subscribing to {} devices on host {}", host.devices.len(), socket_addr);
                    for device in &host.devices {
                        let msg = Ctrl(CtrlMsg::Subscribe { device_id: device.id });
                        client.send_message(socket_addr, msg);
                    }
                }
            }
            OrgUpdate::UnsubscribeAll(socket_addr) => {
                let mut client = self.client.lock().await;
                if let Some(host) = self.connected_hosts.iter().find(|a| a.addr == socket_addr) {
                    info!("Subscribing from {} devices on host {}", host.devices.len(), socket_addr);
                    for device in &host.devices {
                        let msg = Ctrl(CtrlMsg::Unsubscribe { device_id: device.id });
                        client.send_message(socket_addr, msg);
                    }
                }
            }
            OrgUpdate::Up => {
                if let Focused::Hosts(host_focus) = &self.focussed_panel {
                    match host_focus {
                        FocusedHosts::HostTree(0, 0) => self.focussed_panel = Focused::Hosts(FocusedHosts::HostTree(0, 0)),
                        FocusedHosts::HostTree(h, 0) => {
                            if let Some(host) = self.connected_hosts.get(*h - 1) {
                                let d = host.devices.len();
                                self.focussed_panel = Focused::Hosts(FocusedHosts::HostTree(h - 1, d));
                            }
                        }
                        FocusedHosts::HostTree(h, d) => self.focussed_panel = Focused::Hosts(FocusedHosts::HostTree(*h, d - 1)),
                        _ => {}
                    }
                }
            }

            OrgUpdate::Down => {
                if let Focused::Hosts(host_focus) = &self.focussed_panel {
                    match host_focus {
                        FocusedHosts::HostTree(h, d) if *h < self.connected_hosts.len() => {
                            if let Some(host) = self.connected_hosts.get(*h) {
                                if *d < host.devices.len() {
                                    self.focussed_panel = Focused::Hosts(FocusedHosts::HostTree(*h, d + 1))
                                } else if *h < self.connected_hosts.len() - 1 {
                                    self.focussed_panel = Focused::Hosts(FocusedHosts::HostTree(h + 1, 0))
                                } else {
                                    self.focussed_panel = Focused::Hosts(FocusedHosts::AddHost(0))
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            OrgUpdate::BackTab => {
                if let Focused::Hosts(host_focus) = &self.focussed_panel {
                    match host_focus {
                        FocusedHosts::HostTree(0, 0) => self.focussed_panel = Focused::Hosts(FocusedHosts::HostTree(0, 0)),
                        FocusedHosts::HostTree(h, 0) => {
                            self.focussed_panel = Focused::Hosts(FocusedHosts::HostTree(h - 1, 0));
                        }
                        _ => {}
                    }
                }
            }
            OrgUpdate::Tab => {
                if let Focused::Hosts(host_focus) = &self.focussed_panel {
                    match host_focus {
                        FocusedHosts::HostTree(h, 0) if *h < self.connected_hosts.len() - 1 => {
                            self.focussed_panel = Focused::Hosts(FocusedHosts::HostTree(h + 1, 0));
                        }
                        _ => self.focussed_panel = Focused::Hosts(FocusedHosts::AddHost(0)),
                    }
                }
            }
            OrgUpdate::Left => {}
            OrgUpdate::Right => {}
        }
    }
    fn should_quit(&self) -> bool {
        self.should_quit
    }

    async fn on_tick(&mut self) {}
}
