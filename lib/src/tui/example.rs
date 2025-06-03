//! A minimal example implementation of a TUI (Terminal User Interface) using the custom TUI framework
//!
//! This module includes a basic example TUI application (`TuiState`) that supports toggling a UI state,
//! quitting the application, and logging. It is designed to demonstrate how the `TuiRunner` and `Tui`
//! trait can be used to manage event-driven TUI workflows.

use std::vec;

use async_trait::async_trait;
use crossterm::event::{KeyCode, KeyEvent};
use futures::StreamExt;
use log::{info, warn};
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph};
use tokio::sync::mpsc::{self, Receiver, Sender};

use super::{FromLog, LogEntry, Tui, TuiRunner};

/// Represents the state of the TUI application.
pub struct TuiState {
    /// Flag to indicate whether the application should quit.
    should_quit: bool,

    /// Toggles a boolean state which affects the displayed UI.
    toggled: bool,

    /// A log of recent log entries displayed in the UI.
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

/// Commands that can be sent to background tasks from the UI.
pub enum Command {
    /// Command indicating some user interaction occurred (e.g., toggle).
    TouchedBalls,
}

/// Updates that can be applied to the TUI state.
pub enum Update {
    /// User requested to quit the application.
    Quit,

    /// User toggled the UI toggle.
    Toggle,

    /// A message sent to demonstrate background-to-UI communication.
    Balls,

    /// A log message to display in the log section of the UI.
    Log(LogEntry),
}

impl FromLog for Update {
    fn from_log(log: LogEntry) -> Self {
        Update::Log(log)
    }
}

#[async_trait]
impl Tui<Update, Command> for TuiState {
    /// Draws the UI layout and content.
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

    /// Handles keyboard input events and maps them to updates.
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

    /// Applies updates and potentially sends commands to background tasks.
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

/// Launches and runs the example TUI application.
///
/// This function demonstrates communication between a background task and the TUI.
/// The background task sends `Update::Balls` periodically, and reacts to `Command::TouchedBalls`.
///
/// # Example
/// ```rust,ignore
/// tokio::main
/// async fn main() {
///     run_example().await;
/// }
/// ```
pub async fn run_example() {
    let (command_send, mut command_recv) = mpsc::channel::<Command>(10);
    let (update_send, update_recv) = mpsc::channel::<Update>(10);

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
