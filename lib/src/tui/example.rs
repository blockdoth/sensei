use std::error::Error;
use std::io::{self, stdout};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::thread::sleep;
use std::time::Duration;
use std::vec;

use async_trait::async_trait;
use crossterm::event::{Event, EventStream, KeyCode, KeyEvent};
use crossterm::execute;
use crossterm::terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode};
use futures::StreamExt;
use log::{Log, debug, info, warn};
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::prelude::CrosstermBackend;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::{Frame, Terminal};
use tokio::sync::mpsc::{self, Receiver, Sender};
use tokio::task::JoinHandle;

use super::{FromLog, LogEntry, Tui, TuiRunner};
use crate::sources::controllers::esp32_controller::Esp32ControllerParams;

pub struct TuiState {
    should_quit: bool,
    toggled: bool,
    pub logs: Vec<LogEntry>,
}

impl Default for TuiState {
    fn default() -> Self {
        Self::new()
    }
}

impl TuiState {
    pub fn new() -> Self {
        Self {
            logs: vec![],
            should_quit: false,
            toggled: false,
        }
    }
}

pub enum Command {
    TouchedBalls,
}
pub enum Update {
    Quit,
    Toggle,
    Balls,
    Log(LogEntry),
}

impl FromLog for Update {
    fn from_log(log: LogEntry) -> Self {
        Update::Log(log)
    }
}

#[async_trait]
impl Tui<Update, Command> for TuiState {
    fn draw_ui(&self, frame: &mut Frame) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .margin(2)
            .constraints([Constraint::Length(5), Constraint::Min(5)])
            .split(frame.area());

        let toggle_block = Block::default()
            .title(if self.toggled { "Toggled On" } else { "Toggled Off" })
            .borders(Borders::ALL);

        frame.render_widget(toggle_block, chunks[0]);

        // Log block
        let log_lines: Vec<Line> = self.logs.iter().rev().map(|entry| entry.format()).collect();

        let log_count = self.logs.len();
        let logs_widget = Paragraph::new(log_lines).block(
            Block::default()
                .title(format!("Logs ({log_count})")) // <--- Count shown here
                .borders(Borders::ALL),
        );

        frame.render_widget(logs_widget, chunks[1]);
    }

    fn handle_keyboard_event(&self, key_event: KeyEvent) -> Option<Update> {
        match key_event.code {
            KeyCode::Char('q') => Some(Update::Quit),
            KeyCode::Char('t') => Some(Update::Toggle),
            _ => None,
        }
    }

    fn should_quit(&self) -> bool {
        self.should_quit
    }

    async fn on_tick(&mut self) {}

    async fn handle_update(&mut self, update: Update, command_send: &Sender<Command>, update_recv: &mut Receiver<Update>) {
        match update {
            Update::Quit => {
                self.should_quit = true;
            }
            Update::Toggle => {
                self.toggled = !self.toggled;
                info!("Toggled");
                let _ = command_send.try_send(Command::TouchedBalls);
            }
            Update::Balls => {
                info!("balls")
            }
            Update::Log(entry) => {
                self.logs.push(entry);
            }
        }
    }
}

pub async fn run_example() {
    let (command_send, mut command_recv) = mpsc::channel::<Command>(10);
    let (update_send, mut update_recv) = mpsc::channel::<Update>(10);

    let update_send_clone = update_send.clone();

    // Showcases messages going both ways
    let other_task = vec![async move {
        loop {
            update_send_clone.send(Update::Balls).await;
            if command_recv.try_recv().is_ok() {
                warn!("Balls have been touched");
            }
        }
    }];

    let tui = TuiState::new();
    let tui_runner = TuiRunner::new(tui, command_send, update_recv, update_send, log::LevelFilter::Info);

    tui_runner.run(other_task).await;
}
