use chrono::{DateTime, Local};
use log::{Level, LevelFilter, Metadata, Record, SetLoggerError, info};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use tokio::sync::mpsc::{Receiver, Sender};

#[derive(Debug, Clone)] // Added derive Debug
pub struct LogEntry {
    pub timestamp: DateTime<Local>,
    pub level: log::Level,
    pub message: String,
}

impl LogEntry {
    // Nicely formats the logs for printing to the TUI
    pub fn format(&self) -> Line {
        let timestamp_str = self.timestamp.format("%H:%M:%S").to_string();
        let level_str = format!("[{}]", self.level);
        let message_str = &self.message;

        let style = match self.level {
            log::Level::Error => Style::default().fg(Color::Red),
            log::Level::Warn => Style::default().fg(Color::Yellow),
            log::Level::Info => Style::default().fg(Color::Cyan),
            log::Level::Debug => Style::default().fg(Color::Blue),
            log::Level::Trace => Style::default().fg(Color::Magenta),
        };
        Line::from(vec![
            Span::raw(format!("{timestamp_str} ")),
            Span::styled(level_str, style),
            Span::styled(format!(" {message_str}"), style),
        ])
    }
}

pub struct TuiLogger {
    pub log_sender: Sender<LogEntry>,
    pub level: Level,
}

impl log::Log for TuiLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= self.level
    }

    fn log(&self, record: &Record) {
        if self.enabled(record.metadata()) {
            let log_entry = LogEntry {
                timestamp: Local::now(),
                level: record.level(),
                message: format!("{}", record.args()),
            };
            if self.log_sender.try_send(log_entry.clone()).is_err() {
                eprintln!(
                    "[TUI_LOG_FALLBACK] {}: {} [{}] - {}",
                    Local::now().format("%Y-%m-%d %H:%M:%S%.3f"),
                    std::thread::current().name().unwrap_or("unknown_thread"),
                    record.level(),
                    record.args()
                );
            }     
        }
    }
    fn flush(&self) {}
}

// Configures the global logger such that all logs get routed to our custom TuiLogger struct
// which sends them over a channel to a log handler task
pub fn init_logger(log_level_filter: LevelFilter, sender: Sender<LogEntry>) -> Result<(), SetLoggerError> {
    let logger = TuiLogger {
        log_sender: sender,
        level: log_level_filter.to_level().unwrap_or(log::Level::Error),
    };

    log::set_boxed_logger(Box::new(logger))?;
    log::set_max_level(log_level_filter);
    Ok(())
}

// Makes sure a LogEntry carrying struct is available on the Update enum
pub trait FromLog {
    fn from_log(log: LogEntry) -> Self;
}
