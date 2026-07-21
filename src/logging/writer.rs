use std::{io, path::Path, path::PathBuf, sync::Arc};

use tokio::{
    fs::{self, File, OpenOptions},
    io::AsyncWriteExt,
    sync::mpsc,
};
use tokio_util::sync::CancellationToken;

use super::{LogRecord, LoggingMetrics, LoggingPolicy};
use std::sync::atomic::Ordering;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum LoggingStatus {
    Degraded(String),
}

pub(super) async fn run(
    path: PathBuf,
    mut records: mpsc::Receiver<LogRecord>,
    tui_tx: mpsc::Sender<LogRecord>,
    status_tx: mpsc::Sender<LoggingStatus>,
    policy: LoggingPolicy,
    metrics: Arc<LoggingMetrics>,
    shutdown: CancellationToken,
) {
    let mut file = match open_log_file(&path).await {
        Ok(file) => Some(file),
        Err(error) => {
            send_degraded(&status_tx, &shutdown, &path, error).await;
            None
        }
    };
    let mut file_bytes = file_size(&path).await.unwrap_or_default();

    loop {
        let record = tokio::select! {
            _ = shutdown.cancelled() => break,
            record = records.recv() => match record {
                Some(record) => record,
                None => break,
            },
        };

        if file.is_some() {
            let append_bytes = record.len().saturating_add(1) as u64;
            if file_bytes.saturating_add(append_bytes) > policy.max_file_bytes {
                file = None;
                if let Err(error) = rotate(&path, policy.retained_files).await {
                    send_degraded(&status_tx, &shutdown, &path, error).await;
                } else {
                    match open_log_file(&path).await {
                        Ok(new_file) => {
                            file = Some(new_file);
                            file_bytes = 0;
                        }
                        Err(error) => {
                            send_degraded(&status_tx, &shutdown, &path, error).await;
                        }
                    }
                }
            }

            if let Some(active_file) = file.as_mut() {
                let write_result = async {
                    active_file.write_all(record.as_str().as_bytes()).await?;
                    active_file.write_all(b"\n").await
                }
                .await;
                match write_result {
                    Ok(()) => file_bytes = file_bytes.saturating_add(append_bytes),
                    Err(error) => {
                        file = None;
                        send_degraded(&status_tx, &shutdown, &path, error).await;
                    }
                }
            }
        }

        if tui_tx.try_send(record).is_err() {
            metrics.tui_dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    if let Some(mut file) = file {
        let _ = file.flush().await;
    }
}

async fn open_log_file(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .await
}

async fn file_size(path: &Path) -> io::Result<u64> {
    fs::metadata(path).await.map(|metadata| metadata.len())
}

async fn rotate(path: &Path, retained_files: usize) -> io::Result<()> {
    if retained_files == 0 {
        remove_if_exists(path).await?;
        return Ok(());
    }

    remove_if_exists(&rotated_path(path, retained_files)).await?;
    for index in (1..retained_files).rev() {
        rename_if_exists(&rotated_path(path, index), &rotated_path(path, index + 1)).await?;
    }
    rename_if_exists(path, &rotated_path(path, 1)).await
}

fn rotated_path(path: &Path, index: usize) -> PathBuf {
    let mut value = path.as_os_str().to_os_string();
    value.push(format!(".{index}"));
    PathBuf::from(value)
}

async fn remove_if_exists(path: &Path) -> io::Result<()> {
    match fs::remove_file(path).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

async fn rename_if_exists(from: &Path, to: &Path) -> io::Result<()> {
    match fs::rename(from, to).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

async fn send_degraded(
    status_tx: &mpsc::Sender<LoggingStatus>,
    shutdown: &CancellationToken,
    path: &Path,
    error: io::Error,
) {
    let status = LoggingStatus::Degraded(format!(
        "disk logging disabled for {}: {error}",
        path.display()
    ));
    tokio::select! {
        _ = shutdown.cancelled() => {}
        result = status_tx.send(status) => {
            let _ = result;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[tokio::test]
    async fn rotation_keeps_configured_file_count() -> io::Result<()> {
        let path = temporary_log_path();
        fs::write(&path, b"active").await?;
        fs::write(rotated_path(&path, 1), b"one").await?;
        fs::write(rotated_path(&path, 2), b"two").await?;

        rotate(&path, 2).await?;

        assert!(!path.exists());
        assert_eq!(fs::read(rotated_path(&path, 1)).await?, b"active");
        assert_eq!(fs::read(rotated_path(&path, 2)).await?, b"one");
        let _ = fs::remove_file(rotated_path(&path, 1)).await;
        let _ = fs::remove_file(rotated_path(&path, 2)).await;
        Ok(())
    }

    fn temporary_log_path() -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("test clock should follow the Unix epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("wirelens-log-{nanos}.log"))
    }
}
