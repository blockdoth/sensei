use chrono::{DateTime, Local};
use log::Level;
use log::{LevelFilter, Metadata, Record, SetLoggerError, info};
use tokio::sync::mpsc::Sender;

#[derive(Debug, Clone)] // Added derive Debug
pub struct LogEntry {
    pub timestamp: DateTime<Local>,
    pub level: log::Level,
    pub message: String,
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
            if self.log_sender.try_send(log_entry).is_err() {
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
