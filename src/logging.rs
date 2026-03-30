use crate::app::AppEvent;
use log::{Level, Metadata, Record, SetLoggerError};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::sync::Mutex;
use tokio::sync::mpsc;

pub struct AppLogger {
    file: Mutex<File>,
    tx: mpsc::UnboundedSender<AppEvent>,
}

impl AppLogger {
    pub fn new(tx: mpsc::UnboundedSender<AppEvent>) -> Self {
        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .append(true)
            .open("debug.log")
            .unwrap();

        Self {
            file: Mutex::new(file),
            tx,
        }
    }

    pub fn init(tx: mpsc::UnboundedSender<AppEvent>) -> Result<(), SetLoggerError> {
        let logger = Box::new(AppLogger::new(tx));
        log::set_logger(Box::leak(logger)).map(|()| log::set_max_level(log::LevelFilter::Info))
    }
}

impl log::Log for AppLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        let is_crate_log = metadata.target().starts_with("proxy_tui");
        if is_crate_log {
            metadata.level() <= Level::Info
        } else {
            metadata.level() <= Level::Warn
        }
    }

    fn log(&self, record: &Record) {
        if self.enabled(record.metadata()) {
            let log_msg = format!(
                "{} - [{}] {}",
                record.level(),
                record.target(),
                record.args()
            );

            // Write to file
            if let Ok(mut file) = self.file.lock() {
                if let Err(e) = writeln!(file, "{}", log_msg) {
                    eprintln!("Failed to write to log file: {}", e);
                }
            }

            // Send to TUI
            let _ = self.tx.send(AppEvent::LogMessage(log_msg));
        }
    }

    fn flush(&self) {}
}
