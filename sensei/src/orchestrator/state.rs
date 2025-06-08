use std::net::SocketAddr;
use std::sync::Arc;

use async_trait::async_trait;
use crossterm::event::{KeyCode, KeyEvent};
use lib::network::rpc_message::CtrlMsg;
use lib::network::rpc_message::RpcMessageKind::Ctrl;
use lib::network::tcp::ChannelMsg;
use lib::network::tcp::client::TcpClient;
use lib::tui::Tui;
use lib::tui::logs::{FromLog, LogEntry};
use ratatui::Frame;
use tokio::sync::Mutex;
use tokio::sync::mpsc::{Receiver, Sender};

use super::tui::ui;

pub struct OrgTuiState {
    should_quit: bool,
    client: Arc<Mutex<TcpClient>>,
    known_clients: Vec<SocketAddr>,
}

pub enum OrgCommand {}

type DeviceID = u64;
pub enum OrgUpdate {
    Log(LogEntry),
    Connect(SocketAddr),
    Disconnect(SocketAddr),
    Subscribe(SocketAddr, DeviceID),
    Unsubscribe(SocketAddr, DeviceID),
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
            known_clients: vec![],
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
        match key_event.code {
            KeyCode::Char('q') | KeyCode::Char('Q') => Some(OrgUpdate::Exit),
            _ => None,
        }
    }

    /// Applies updates and potentially sends commands to background tasks.
    async fn handle_update(&mut self, update: OrgUpdate, command_send: &Sender<ChannelMsg>, update_recv: &mut Receiver<OrgUpdate>) {
        match update {
            OrgUpdate::Exit => {
                self.should_quit = true;
            }
            OrgUpdate::Log(entry) => {}
            OrgUpdate::Connect(socket_addr) => {
                self.client.lock().await.connect(socket_addr);
                self.known_clients.push(socket_addr);
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
            }
        }
    }
    fn should_quit(&self) -> bool {
        self.should_quit
    }

    async fn on_tick(&mut self) {}
}
