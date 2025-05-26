pub mod logs;
pub mod example;

use crate::sources::controllers::esp32_controller::Esp32Controller;
use async_trait::async_trait;
use crossterm::{
    event::{Event, EventStream, KeyCode, KeyEvent},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use futures::StreamExt;
use log::info;
use log::{LevelFilter, debug};
use logs::{FromLog, LogEntry, init_logger};
use ratatui::{
    Frame, Terminal,
    layout::{Constraint, Direction, Layout},
    prelude::CrosstermBackend,
    widgets::{Block, Borders},
};
use std::{
    error::Error,
    io::{self, stdout},
    sync::{Arc, atomic::AtomicBool},
    thread::sleep,
    time::Duration,
    vec,
};
use tokio::{
    sync::mpsc::{self, Receiver, Sender},
    task::JoinHandle,
};

pub struct TuiRunner<T: Tui<Update, Command>, Update, Command> {
    app: T,
    command_send: Sender<Command>,
    update_recv: Receiver<Update>,
    update_send: Sender<Update>,
    even_stream: EventStream,
    log_level: LevelFilter,
}

const LOG_BUFFER_CAPACITY: usize = 100;

impl<T, U, C> TuiRunner<T, U, C>
where
    U: FromLog + Send + 'static, // Required for extracting a LogEntry from the Update enum
    T: Tui<U, C>,
{
    pub fn new(
        app: T,
        command_send: Sender<C>,
        update_recv: Receiver<U>,
        update_send: Sender<U>,
        log_level: LevelFilter,
    ) -> Self {
        Self {
            app,
            command_send,
            update_recv,
            update_send,
            even_stream: EventStream::new(),
            log_level,
        }
    }

    pub async fn run(mut self) -> Result<(), Box<dyn Error>> {
        let mut terminal = Self::setup_terminal().unwrap(); //TODO remove unwrap
        let (log_send, log_recv) = mpsc::channel::<LogEntry>(LOG_BUFFER_CAPACITY);

        Self::log_handler_task(log_recv, self.update_send).await;
        init_logger(self.log_level, log_send.clone());

        loop {
            terminal.draw(|f| self.app.draw_ui(f))?;

            tokio::select! {
                Some(Ok(Event::Key(key))) = self.even_stream.next() => {
                    if let Some(update) = T::handle_keyboard_event(key) {
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
        drop(log_send);
        Self::restore_terminal(&mut terminal);

        Ok(())
    }

     
    fn setup_terminal() -> Result<Terminal<CrosstermBackend<io::Stdout>>, Box<dyn Error>> {
        enable_raw_mode()?;
        let mut stdout = stdout();
        execute!(stdout, EnterAlternateScreen)?;
        let backend = CrosstermBackend::new(stdout);
        Terminal::new(backend).map_err(Into::into)
    }

    fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<(), Box<dyn Error>> {
        disable_raw_mode()?;
        execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
        terminal.show_cursor()?;
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
}

#[async_trait]
pub trait Tui<Update, Cmd> {
    fn draw_ui(&self, f: &mut Frame);

    fn handle_keyboard_event(key_event: KeyEvent) -> Option<Update>;

    async fn handle_update(&mut self, update: Update, command_send: &Sender<Cmd>, update_recv: &mut Receiver<Update>);

    fn should_quit(&self) -> bool;

    async fn on_tick(&mut self);

}
