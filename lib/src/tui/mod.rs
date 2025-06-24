pub mod example;
pub mod logs;

use std::error::Error;
use std::io::{self, stdout};
use std::time::Duration;
use std::vec;

use async_trait::async_trait;
use crossterm::event::{Event, EventStream, KeyEvent};
use crossterm::execute;
use crossterm::terminal::{Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode};
use futures::StreamExt;
use log::{LevelFilter, debug, info, trace};
use logs::{FromLog, LogEntry, init_logger};
use ratatui::prelude::CrosstermBackend;
use ratatui::{Frame, Terminal};
use tokio::sync::mpsc::{self, Receiver, Sender};
use tokio::task::JoinHandle;
use tokio::time::sleep;

/// A configurable and generic runner that manages the entire lifecycle of a TUI application.
/// It handles input events, log streaming, periodic ticks, and state updates.
pub struct TuiRunner<T: Tui<Update, Command>, Update, Command> {
    app: T,
    command_send: Sender<Command>,
    update_recv: Receiver<Update>,
    update_send: Sender<Update>,
    log_send: Sender<LogEntry>,
    log_recv: Receiver<LogEntry>,
    even_stream: EventStream,
    log_level: LevelFilter,
}

const LOG_BUFFER_CAPACITY: usize = 100;

impl<T, U, C> TuiRunner<T, U, C>
where
    U: FromLog + Send + 'static, // Required for extracting a LogEntry from the Update enum
    T: Tui<U, C>,
{
    /// Creates a new `TuiRunner` with required communication channels and configuration.
    ///
    /// # Parameters
    /// - `app`: The application state implementing the `Tui` trait.
    /// - `command_send`: Channel to send commands to async task handlers.
    /// - `update_recv`: Channel to receive updates for the TUI.
    /// - `update_send`: Channel to send updates (e.g., from logs or external sources).
    /// - `log_level`: Logging level for filtering logs.
    pub fn new(app: T, command_send: Sender<C>, update_recv: Receiver<U>, update_send: Sender<U>, log_level: LevelFilter) -> Self {
        let (log_send, log_recv) = mpsc::channel::<LogEntry>(LOG_BUFFER_CAPACITY);
        Self {
            app,
            command_send,
            update_recv,
            update_send,
            log_send,
            log_recv,
            even_stream: EventStream::new(),
            log_level,
        }
    }

    /// Starts the main event loop for the TUI and runs any background async tasks.
    ///
    /// This function sets up the terminal, handles logs, polls for keyboard events,
    /// and applies periodic updates.
    ///
    /// # Arguments
    /// - `tasks`: Background async tasks to spawn during runtime.
    ///
    /// # Returns
    /// A `Result` indicating success or any terminal-related errors.
    pub async fn run<F>(mut self, tasks: Vec<F>) -> Result<(), Box<dyn Error>>
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let mut terminal = Self::setup_terminal().unwrap(); //TODO remove unwrap

        Self::log_handler_task(self.log_recv, self.update_send).await;
        init_logger(self.log_level, self.log_send.clone())?;

        let mut handles: Vec<JoinHandle<()>> = vec![];
        for task in tasks {
            handles.push(tokio::spawn(task));
        }

        loop {
            terminal.draw(|f| self.app.draw_ui(f))?;

            tokio::select! {
              Some(Ok(Event::Key(key))) = self.even_stream.next() => {
                if let Some(update) = self.app.handle_keyboard_event(key) {
                  self.app.handle_update(update, &self.command_send, &mut self.update_recv).await;
                }

                if self.app.should_quit() {
                  break;
                }
              }

              Some(update) = self.update_recv.recv() => {
                self.app.handle_update(update, &self.command_send, &mut self.update_recv).await;
              }

              _ = tokio::time::sleep(Duration::from_millis(10)) => {
                self.app.on_tick().await;
              }
            }
        }

        for handle in &handles {
            handle.abort();
        }

        tokio::time::sleep(Duration::from_millis(100)).await; // TODO graceful shutdown
        Self::restore_terminal(&mut terminal)?;

        Ok(())
    }

    /// Launches an async task that listens for log entries and converts them to updates
    /// using the `FromLog` trait. These updates are then forwarded into the update stream.
    async fn log_handler_task(mut log_recv_channel: Receiver<LogEntry>, update_send_channel: Sender<U>) -> JoinHandle<()> {
        tokio::spawn(async move {
            info!("Started log processor task");
            loop {
                tokio::select! {
                    log = log_recv_channel.recv() => {
                        match log {
                            Some(log_msg) => {
                                if update_send_channel.send(U::from_log(log_msg)).await.is_err() {
                                    debug!("Log processor: Update channel closed.");
                                    break
                                }
                            }
                            None => {
                                info!("Log processor: Log entry channel disconnected.");
                                break
                            }
                        }
                    }
                    _ = sleep(Duration::from_millis(50)) => {
                      trace!("tick");
                    }
                }
            }
            info!("Log processor task stopped.")
        })
    }

    /// Prepares the terminal for raw mode and alternate screen usage.
    fn setup_terminal() -> Result<Terminal<CrosstermBackend<io::Stdout>>, Box<dyn Error>> {
        enable_raw_mode()?;
        let mut stdout = stdout();
        execute!(stdout, EnterAlternateScreen, Clear(ClearType::All))?;
        let backend = CrosstermBackend::new(stdout);
        Terminal::new(backend).map_err(Into::into)
    }

    /// Restores the terminal to its original state after exiting the application.
    fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<(), Box<dyn Error>> {
        disable_raw_mode()?;
        execute!(terminal.backend_mut(), Clear(ClearType::All), LeaveAlternateScreen)?;
        terminal.show_cursor()?;
        Ok(())
    }
}

/// Trait that any TUI application must implement to work with `TuiRunner`.
#[async_trait]
pub trait Tui<Update, Cmd> {
    /// Draws the UI using the current state. Should be purely visual with no side effects.
    fn draw_ui(&self, f: &mut Frame);

    /// Handles a keyboard event and optionally returns an update to process.
    /// Should not mutate state directly.
    fn handle_keyboard_event(&self, key_event: KeyEvent) -> Option<Update>;

    /// Main update handler that reacts to updates from events, logs, or commands.
    /// This is where all state mutations should occur.
    async fn handle_update(&mut self, update: Update, command_send: &Sender<Cmd>, update_recv: &mut Receiver<Update>);

    /// Periodic tick handler that gets called every loop iteration.
    /// Suitable for lightweight background updates like animations or polling.
    async fn on_tick(&mut self);

    /// Determines if the TUI application should terminate.
    fn should_quit(&self) -> bool;
}
