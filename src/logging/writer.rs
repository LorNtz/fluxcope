use std::{io, net::SocketAddr, path::Path, path::PathBuf, sync::Arc};

use std::sync::atomic::Ordering;
use tokio::{
    fs::{self, File},
    io::AsyncWriteExt,
    sync::mpsc,
};
use tokio_util::sync::CancellationToken;

use super::{LogRecord, LoggingMetrics, LoggingPolicy};
use crate::instance::endpoint_hash;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum LoggingStatus {
    Degraded(String),
}

pub(crate) fn endpoint_log_path(wirelens_home: &Path, endpoint: SocketAddr) -> PathBuf {
    wirelens_home
        .join("logs")
        .join(format!("{}.log", endpoint_hash(endpoint)))
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
    let path = path.to_owned();
    let file = tokio::task::spawn_blocking(move || {
        let directory = path.parent().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "log path has no parent directory",
            )
        })?;
        crate::private_fs::ensure_directory(directory)?;
        crate::private_fs::open_file(&path, true)
    })
    .await
    .map_err(io::Error::other)??;
    Ok(File::from_std(file))
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

#[cfg(feature = "benchmark")]
pub(crate) async fn benchmark_rotate(path: &Path, retained_files: usize) -> io::Result<()> {
    fs::write(path, b"benchmark log record\n").await?;
    rotate(path, retained_files).await
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

    #[test]
    fn endpoint_log_paths_are_stable_distinct_and_bounded() {
        let home = tempfile::tempdir().expect("temporary Wirelens home");
        let first_endpoint = "127.0.0.1:8989".parse().expect("first endpoint");
        let second_endpoint = "127.0.0.1:8990".parse().expect("second endpoint");

        let first = endpoint_log_path(home.path(), first_endpoint);
        let repeated = endpoint_log_path(home.path(), first_endpoint);
        let second = endpoint_log_path(home.path(), second_endpoint);

        assert_eq!(first, repeated);
        assert_ne!(first, second);
        assert_eq!(
            first.parent().expect("logs directory"),
            home.path().join("logs")
        );
        let name = first
            .file_name()
            .expect("endpoint log filename")
            .to_string_lossy();
        assert!(name.ends_with(".log"));
        assert!(name.len() <= 68);
    }

    #[tokio::test]
    async fn different_endpoint_logs_rotate_independently() -> io::Result<()> {
        let home = tempfile::tempdir()?;
        let first = endpoint_log_path(
            home.path(),
            "127.0.0.1:19001".parse().expect("first endpoint"),
        );
        let second = endpoint_log_path(
            home.path(),
            "127.0.0.1:19002".parse().expect("second endpoint"),
        );
        fs::create_dir_all(first.parent().expect("logs directory")).await?;
        fs::write(&first, b"first-endpoint-active").await?;
        fs::write(rotated_path(&first, 1), b"first-endpoint-old").await?;
        fs::write(&second, b"second-endpoint-active").await?;
        fs::write(rotated_path(&second, 1), b"second-endpoint-old").await?;

        let (first_result, second_result) = tokio::join!(rotate(&first, 1), rotate(&second, 1));
        first_result?;
        second_result?;

        assert_eq!(
            fs::read(rotated_path(&first, 1)).await?,
            b"first-endpoint-active"
        );
        assert_eq!(
            fs::read(rotated_path(&second, 1)).await?,
            b"second-endpoint-active"
        );
        assert!(!first.exists());
        assert!(!second.exists());
        Ok(())
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn endpoint_log_directory_and_file_are_owner_only() -> io::Result<()> {
        use std::os::unix::fs::PermissionsExt as _;

        let home = tempfile::tempdir()?;
        let path = endpoint_log_path(home.path(), "127.0.0.1:19003".parse().expect("endpoint"));

        drop(open_log_file(&path).await?);

        assert_eq!(
            fs::metadata(path.parent().expect("logs directory"))
                .await?
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(&path).await?.permissions().mode() & 0o777,
            0o600
        );
        Ok(())
    }

    fn temporary_log_path() -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("test clock should follow the Unix epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("fluxcope-log-{nanos}.log"))
    }
}
