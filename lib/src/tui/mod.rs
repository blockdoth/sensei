pub mod example;
pub mod logs;

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
use crossterm::terminal::{Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode};
use futures::StreamExt;
use log::{LevelFilter, debug, info};
use logs::{FromLog, LogEntry, init_logger};
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::prelude::CrosstermBackend;
use ratatui::widgets::{Block, Borders};
use ratatui::{Frame, Terminal};
use tokio::sync::mpsc::{self, Receiver, Sender};
use tokio::task::JoinHandle;

use crate::sources::controllers::esp32_controller::Esp32Controller;

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

    // Manages the entire TUI lifecycle
    pub async fn run<F>(mut self, tasks: Vec<F>) -> Result<(), Box<dyn Error>>
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let mut terminal = Self::setup_terminal().unwrap(); //TODO remove unwrap

        Self::log_handler_task(self.log_recv, self.update_send).await;
        init_logger(self.log_level, self.log_send.clone());

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

              _ = tokio::time::sleep(Duration::from_millis(100)) => {
                self.app.on_tick().await;
              }
            }
        }

        for handle in &handles {
            handle.abort();
        }

        Self::restore_terminal(&mut terminal);

        Ok(())
    }

    // Processes all incoming logs by routing them through a channel connected to the TUI, this allows for complete flexibility in where to display logs
    async fn log_handler_task(mut log_recv_channel: Receiver<LogEntry>, update_send_channel: Sender<U>) -> JoinHandle<()> {
        tokio::spawn(async move {
            info!("Log processor task started.");
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
                    // This breaks it for some reason
                    // _ = sleep(Duration::from_millis(10)) => {
                    //   trace!("tick");
                    // }
                }
            }
            info!("Log processor task stopped.")
        })
    }

    fn setup_terminal() -> Result<Terminal<CrosstermBackend<io::Stdout>>, Box<dyn Error>> {
        enable_raw_mode()?;
        let mut stdout = stdout();
        execute!(stdout, EnterAlternateScreen, Clear(ClearType::All))?;
        let backend = CrosstermBackend::new(stdout);
        Terminal::new(backend).map_err(Into::into)
    }

    fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<(), Box<dyn Error>> {
        disable_raw_mode()?;
        execute!(terminal.backend_mut(), Clear(ClearType::All), LeaveAlternateScreen)?;
        terminal.show_cursor()?;
        Ok(())
    }
}

#[async_trait]
pub trait Tui<Update, Cmd> {
    // Draws the UI based on the state of the TUI, should not change any state by itself
    fn draw_ui(&self, f: &mut Frame);

    // Handles a single keyboard event and produces and Update, should not change any state
    fn handle_keyboard_event(&self, key_event: KeyEvent) -> Option<Update>;

    // Handles incoming Updates produced from any source, this is the only place where state should change
    async fn handle_update(&mut self, update: Update, command_send: &Sender<Cmd>, update_recv: &mut Receiver<Update>);

    // Gets called each tick of the main loop, useful for updating graphs and live views, should only make small changes to state
    async fn on_tick(&mut self);

    // Whether the tui should quit
    fn should_quit(&self) -> bool;
}
