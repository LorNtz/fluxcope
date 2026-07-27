mod writer;

use std::{
    fmt::{self, Write as _},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use log::{Level, Metadata, Record, SetLoggerError};
use tokio::{sync::mpsc, task::JoinHandle};
use tokio_util::sync::CancellationToken;

pub(crate) use writer::LoggingStatus;

const TRUNCATION_SUFFIX: &str = " … [truncated]";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogRecord {
    message: String,
}

impl LogRecord {
    fn new(message: String) -> Self {
        Self { message }
    }

    pub fn as_str(&self) -> &str {
        &self.message
    }

    pub fn len(&self) -> usize {
        self.message.len()
    }

    pub(crate) fn system(message: String) -> Self {
        Self::new(message)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct LogRetentionPolicy {
    pub max_records: usize,
    pub max_bytes: usize,
}

impl Default for LogRetentionPolicy {
    fn default() -> Self {
        Self {
            max_records: 5_000,
            max_bytes: 4 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Debug)]
pub struct LoggingPolicy {
    pub writer_queue_capacity: usize,
    pub tui_queue_capacity: usize,
    pub status_queue_capacity: usize,
    pub max_record_bytes: usize,
    pub retention: LogRetentionPolicy,
    pub max_file_bytes: u64,
    pub retained_files: usize,
}

impl Default for LoggingPolicy {
    fn default() -> Self {
        Self {
            writer_queue_capacity: 1_024,
            tui_queue_capacity: 1_024,
            status_queue_capacity: 8,
            max_record_bytes: 64 * 1024,
            retention: LogRetentionPolicy::default(),
            max_file_bytes: 10 * 1024 * 1024,
            retained_files: 5,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LoggingMetricsSnapshot {
    pub producer_dropped: u64,
    pub tui_dropped: u64,
    pub records_truncated: u64,
}

#[derive(Debug, Default)]
pub struct LoggingMetrics {
    producer_dropped: AtomicU64,
    tui_dropped: AtomicU64,
    records_truncated: AtomicU64,
}

impl LoggingMetrics {
    pub fn snapshot(&self) -> LoggingMetricsSnapshot {
        LoggingMetricsSnapshot {
            producer_dropped: self.producer_dropped.load(Ordering::Relaxed),
            tui_dropped: self.tui_dropped.load(Ordering::Relaxed),
            records_truncated: self.records_truncated.load(Ordering::Relaxed),
        }
    }
}

pub(crate) struct LoggingService {
    pub records: mpsc::Receiver<LogRecord>,
    pub statuses: mpsc::Receiver<LoggingStatus>,
    pub task: JoinHandle<()>,
    pub metrics: Arc<LoggingMetrics>,
}

pub struct AppLogger {
    tx: mpsc::Sender<LogRecord>,
    max_record_bytes: usize,
    metrics: Arc<LoggingMetrics>,
}

impl AppLogger {
    pub(crate) fn init(
        log_path: impl AsRef<Path>,
        policy: LoggingPolicy,
        shutdown: CancellationToken,
    ) -> Result<LoggingService, SetLoggerError> {
        let metrics = Arc::new(LoggingMetrics::default());
        let (writer_tx, writer_rx) = mpsc::channel(policy.writer_queue_capacity.max(1));
        let (tui_tx, tui_rx) = mpsc::channel(policy.tui_queue_capacity.max(1));
        let (status_tx, status_rx) = mpsc::channel(policy.status_queue_capacity.max(1));
        let logger = Box::new(Self {
            tx: writer_tx,
            max_record_bytes: policy.max_record_bytes.max(TRUNCATION_SUFFIX.len()),
            metrics: Arc::clone(&metrics),
        });

        log::set_logger(Box::leak(logger))?;
        log::set_max_level(log::LevelFilter::Info);

        let log_path = PathBuf::from(log_path.as_ref());
        let task_metrics = Arc::clone(&metrics);
        let task = tokio::spawn(async move {
            writer::run(
                log_path,
                writer_rx,
                tui_tx,
                status_tx,
                policy,
                task_metrics,
                shutdown,
            )
            .await;
        });

        Ok(LoggingService {
            records: tui_rx,
            statuses: status_rx,
            task,
            metrics,
        })
    }

    fn format_record(&self, record: &Record<'_>) -> LogRecord {
        let payload_limit = self
            .max_record_bytes
            .saturating_sub(TRUNCATION_SUFFIX.len());
        let mut output = LimitedString::new(payload_limit);
        let _ = write!(output, "{} - [{}] ", record.level(), record.target());
        let _ = fmt::write(&mut output, *record.args());

        let (mut message, truncated) = output.finish();
        if truncated {
            message.push_str(TRUNCATION_SUFFIX);
            self.metrics
                .records_truncated
                .fetch_add(1, Ordering::Relaxed);
        }
        LogRecord::new(message)
    }
}

impl log::Log for AppLogger {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        let is_crate_log = metadata.target().starts_with("wirelens");
        if is_crate_log {
            metadata.level() <= Level::Info
        } else {
            metadata.level() <= Level::Warn
        }
    }

    fn log(&self, record: &Record<'_>) {
        if !self.enabled(record.metadata()) {
            return;
        }

        match self.tx.try_reserve() {
            Ok(permit) => permit.send(self.format_record(record)),
            Err(_) => {
                self.metrics
                    .producer_dropped
                    .fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    fn flush(&self) {}
}

struct LimitedString {
    value: String,
    limit: usize,
    truncated: bool,
}

impl LimitedString {
    fn new(limit: usize) -> Self {
        Self {
            value: String::with_capacity(limit.min(1024)),
            limit,
            truncated: false,
        }
    }

    fn finish(self) -> (String, bool) {
        (self.value, self.truncated)
    }
}

impl fmt::Write for LimitedString {
    fn write_str(&mut self, value: &str) -> fmt::Result {
        let remaining = self.limit.saturating_sub(self.value.len());
        if value.len() <= remaining {
            self.value.push_str(value);
            return Ok(());
        }

        let mut boundary = remaining.min(value.len());
        while boundary > 0 && !value.is_char_boundary(boundary) {
            boundary -= 1;
        }
        self.value.push_str(&value[..boundary]);
        self.truncated = true;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use log::Log as _;

    #[test]
    fn limited_string_never_splits_utf8() {
        let mut output = LimitedString::new(5);
        write!(output, "a界bc").expect("formatting should succeed");

        assert_eq!(output.finish(), ("a界b".to_string(), true));
    }

    #[test]
    fn retention_defaults_match_runtime_policy() {
        let policy = LoggingPolicy::default();

        assert_eq!(policy.writer_queue_capacity, 1_024);
        assert_eq!(policy.max_record_bytes, 64 * 1024);
        assert_eq!(policy.retention.max_records, 5_000);
        assert_eq!(policy.retention.max_bytes, 4 * 1024 * 1024);
    }

    #[test]
    fn formatter_caps_record_and_marks_truncation() {
        let (tx, _rx) = mpsc::channel(1);
        let metrics = Arc::new(LoggingMetrics::default());
        let logger = AppLogger {
            tx,
            max_record_bytes: 32,
            metrics: Arc::clone(&metrics),
        };
        let record = Record::builder()
            .level(Level::Info)
            .target("wirelens::test")
            .args(format_args!("abcdefghijklmnopqrstuvwxyz"))
            .build();

        let formatted = logger.format_record(&record);

        assert!(formatted.len() <= 32);
        assert!(formatted.as_str().ends_with(TRUNCATION_SUFFIX));
        assert_eq!(metrics.snapshot().records_truncated, 1);
    }

    #[test]
    fn saturated_producer_queue_drops_without_waiting() {
        let (tx, mut rx) = mpsc::channel(1);
        let metrics = Arc::new(LoggingMetrics::default());
        let logger = AppLogger {
            tx,
            max_record_bytes: 64,
            metrics: Arc::clone(&metrics),
        };
        let first = Record::builder()
            .level(Level::Info)
            .target("wirelens::test")
            .args(format_args!("first"))
            .build();
        let second = Record::builder()
            .level(Level::Info)
            .target("wirelens::test")
            .args(format_args!(
                "this record is deliberately long enough to require truncation if formatted"
            ))
            .build();

        logger.log(&first);
        logger.log(&second);

        assert_eq!(metrics.snapshot().producer_dropped, 1);
        assert_eq!(metrics.snapshot().records_truncated, 0);
        assert_eq!(
            rx.try_recv().unwrap().as_str(),
            "INFO - [wirelens::test] first"
        );
    }
}
