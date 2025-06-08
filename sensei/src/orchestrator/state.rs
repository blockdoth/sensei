use std::net::SocketAddr;
use std::sync::Arc;
use std::usize;

use async_trait::async_trait;
use crossterm::event::{KeyCode, KeyEvent};
use lib::network::rpc_message::RpcMessageKind::Ctrl;
use lib::network::rpc_message::{CtrlMsg, HostId, SourceType};
use lib::network::tcp::ChannelMsg;
use lib::network::tcp::client::TcpClient;
use lib::tui::Tui;
use lib::tui::logs::{FromLog, LogEntry};
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
    pub focussed_panel: FocusedPanel,
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

pub enum FocusedPanel {
    Main,
    Registry(FocusedRegistryPanel),
    Hosts(FocusedHostsPanel),
    Status,
}

pub enum FocusedRegistryPanel {
  RegistryAddress(usize),
  AvailableHosts(usize),
}

pub enum FocusedHostsPanel {
    Host(usize, usize),
}

impl FocusedHostsPanel {
  fn up(self) -> Self {
    match self {
        FocusedHostsPanel::Host(0,0) => FocusedHostsPanel::Host(0,0),
        FocusedHostsPanel::Host(h, 0) => FocusedHostsPanel::Host(h - 1, 0),
        FocusedHostsPanel::Host(h, d) => FocusedHostsPanel::Host(h, d - 1),
    }
  }

  fn down(self) -> Self {
    match self {
      FocusedHostsPanel::Host(h,_) => FocusedHostsPanel::Host(h+1, 0),
    }
  }
}

pub enum OrgCommand {}

type DeviceID = u64;
pub enum OrgUpdate {
    Log(LogEntry),
    Connect(SocketAddr),
    Disconnect(SocketAddr),
    Subscribe(SocketAddr, DeviceID),
    Unsubscribe(SocketAddr, DeviceID),
    FocusChange(FocusedPanel),
    Exit,
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
            focussed_panel: FocusedPanel::Main,
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
          KeyCode::Esc => return Some(OrgUpdate::FocusChange(FocusedPanel::Main)),
          _ => {}
        };
        
        match &self.focussed_panel {
            FocusedPanel::Main => 
              match key {
                KeyCode::Char('r') | KeyCode::Char('R') => Some(OrgUpdate::FocusChange(FocusedPanel::Registry(FocusedRegistryPanel::RegistryAddress(0)))),
                KeyCode::Char('h') | KeyCode::Char('H') => Some(OrgUpdate::FocusChange(FocusedPanel::Hosts(FocusedHostsPanel::Host(0,0)))),
                _ => None,
            },
            
            FocusedPanel::Registry(focused_registry_panel) => match focused_registry_panel {
                FocusedRegistryPanel::RegistryAddress(_) =>  match key {
                  _ => None
                },
                FocusedRegistryPanel::AvailableHosts(_) =>  match key {
                  _ => None
                },
            }
            FocusedPanel::Hosts(focused_hosts_panel) => match focused_hosts_panel {
                FocusedHostsPanel::Host(_,_) => match key {
                  _ => None
                },
            }
            FocusedPanel::Status => match key {
              _ => None
            },
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
                self.client.lock().await.connect(socket_addr);
                self.known_hosts.push(socket_addr);
            }
            OrgUpdate::Disconnect(socket_addr) => {
                self.client.lock().await.disconnect(socket_addr);
            }
            OrgUpdate::Subscribe(socket_addr, device_id) => {
                let msg = Ctrl(CtrlMsg::Subscribe { device_id });
                self.client.lock().await.send_message(socket_addr, msg);
            }
            OrgUpdate::Unsubscribe(socket_addr, device_id) => {
                let msg = Ctrl(CtrlMsg::Unsubscribe { device_id });
                self.client.lock().await.send_message(socket_addr, msg);
            },
            OrgUpdate::FocusChange(focused_panel) => {
              self.focussed_panel = focused_panel
            },

        }
    }
    fn should_quit(&self) -> bool {
        self.should_quit
    }

    async fn on_tick(&mut self) {}
}
