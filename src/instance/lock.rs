use std::{
    fs::{self, File, OpenOptions},
    io,
    path::Path,
};

use fs2::FileExt;

pub(crate) struct DefaultConfigLease {
    _file: File,
}

impl DefaultConfigLease {
    pub(crate) fn acquire(path: &Path) -> io::Result<Self> {
        let file = open_owner_only_lock_without_following_symlinks(path)?;
        FileExt::try_lock_exclusive(&file).map_err(|error| {
            if error.kind() == io::ErrorKind::WouldBlock {
                io::Error::new(io::ErrorKind::AlreadyExists, "default_config_already_owned")
            } else {
                error
            }
        })?;
        Ok(Self { _file: file })
    }
}

fn ensure_lock_directory(path: &Path) -> io::Result<()> {
    let directory = path.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "default config lock path has no parent directory",
        )
    })?;
    let created = match create_owner_only_directory(directory) {
        Ok(()) => true,
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => false,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let parent = directory.parent().ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "default config run directory has no parent",
                )
            })?;
            fs::create_dir_all(parent)?;
            match create_owner_only_directory(directory) {
                Ok(()) => true,
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => false,
                Err(error) => return Err(error),
            }
        }
        Err(error) => return Err(error),
    };

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        if created {
            fs::set_permissions(directory, fs::Permissions::from_mode(0o700))?;
        }
        validate_owner_only_directory(directory)?;
    }

    Ok(())
}

#[cfg(unix)]
fn create_owner_only_directory(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;

    let mut builder = fs::DirBuilder::new();
    builder.mode(0o700);
    builder.create(path)
}

#[cfg(not(unix))]
fn create_owner_only_directory(path: &Path) -> io::Result<()> {
    fs::create_dir(path)
}

#[cfg(unix)]
fn validate_owner_only_directory(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::MetadataExt;

    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "default config run path must be a real directory",
        ));
    }
    if metadata.uid() != rustix::process::geteuid().as_raw() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "default config run directory must be owned by the effective user",
        ));
    }
    if metadata.mode() & 0o777 != 0o700 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "default config run directory must have mode 0700",
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn open_owner_only_lock_without_following_symlinks(path: &Path) -> io::Result<File> {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};

    ensure_lock_directory(path)?;
    let nofollow = rustix::fs::OFlags::NOFOLLOW.bits() as i32;
    let open_existing = || {
        OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(nofollow)
            .open(path)
    };
    let (file, created) = match open_existing() {
        Ok(file) => (file, false),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            match OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .mode(0o600)
                .custom_flags(nofollow)
                .open(path)
            {
                Ok(file) => (file, true),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    (open_existing()?, false)
                }
                Err(error) => return Err(error),
            }
        }
        Err(error) => return Err(error),
    };
    if created {
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    let metadata = file.metadata()?;
    if !metadata.file_type().is_file() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "default config lock must be a regular file",
        ));
    }
    if metadata.uid() != rustix::process::geteuid().as_raw() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "default config lock must be owned by the effective user",
        ));
    }
    if metadata.mode() & 0o777 != 0o600 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "default config lock must have mode 0600",
        ));
    }
    Ok(file)
}

#[cfg(not(unix))]
fn open_owner_only_lock_without_following_symlinks(path: &Path) -> io::Result<File> {
    ensure_lock_directory(path)?;
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .open(path)
}

#[cfg(test)]
mod tests {
    use super::DefaultConfigLease;
    use std::{
        fs, io,
        path::{Path, PathBuf},
        process::{Child, Command, ExitStatus},
        thread,
        time::{Duration, Instant},
    };

    struct LeaseChild {
        child: Option<Child>,
        release_path: PathBuf,
    }

    impl LeaseChild {
        fn wait_until_ready(&mut self, ready_path: &Path) -> io::Result<()> {
            let deadline = Instant::now() + Duration::from_secs(5);
            while !ready_path.exists() {
                if let Some(status) = self
                    .child
                    .as_mut()
                    .expect("lease child should be running")
                    .try_wait()?
                {
                    return Err(io::Error::other(format!(
                        "lease child exited before readiness: {status}"
                    )));
                }
                if Instant::now() >= deadline {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "lease child did not become ready",
                    ));
                }
                thread::sleep(Duration::from_millis(10));
            }
            Ok(())
        }

        fn release_and_wait(&mut self) -> io::Result<ExitStatus> {
            fs::write(&self.release_path, b"release")?;
            self.child
                .take()
                .expect("lease child should be running")
                .wait()
        }
    }

    impl Drop for LeaseChild {
        fn drop(&mut self) {
            let _ = fs::write(&self.release_path, b"release");
            if let Some(mut child) = self.child.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }

    #[cfg(unix)]
    struct UmaskGuard(rustix::fs::Mode);

    #[cfg(unix)]
    impl UmaskGuard {
        fn set(mask: rustix::fs::Mode) -> Self {
            Self(rustix::process::umask(mask))
        }
    }

    #[cfg(unix)]
    impl Drop for UmaskGuard {
        fn drop(&mut self) {
            rustix::process::umask(self.0);
        }
    }

    #[test]
    fn default_config_lease_rejects_a_concurrent_process_until_release() -> io::Result<()> {
        const CHILD_MARKER: &str = "WIRELENS_LEASE_CONTENTION_CHILD";
        const ROOT_ENV: &str = "WIRELENS_LEASE_CONTENTION_ROOT";
        if std::env::var_os(CHILD_MARKER).is_some() {
            let root = PathBuf::from(
                std::env::var_os(ROOT_ENV)
                    .expect("lease contention child should receive its fixture root"),
            );
            let lock_path = root.join(".wirelens/run/default-config.lock");
            let _lease = DefaultConfigLease::acquire(&lock_path)?;
            fs::write(root.join("ready"), b"ready")?;
            let deadline = Instant::now() + Duration::from_secs(5);
            while !root.join("release").exists() {
                assert!(
                    Instant::now() < deadline,
                    "lease contention child timed out waiting for release"
                );
                thread::sleep(Duration::from_millis(10));
            }
            return Ok(());
        }

        let temporary_home = tempfile::tempdir()?;
        let ready_path = temporary_home.path().join("ready");
        let release_path = temporary_home.path().join("release");
        let child = Command::new(std::env::current_exe()?)
            .args([
                "--exact",
                "instance::lock::tests::default_config_lease_rejects_a_concurrent_process_until_release",
                "--nocapture",
            ])
            .env(CHILD_MARKER, "1")
            .env(ROOT_ENV, temporary_home.path())
            .spawn()?;
        let mut child = LeaseChild {
            child: Some(child),
            release_path,
        };
        child.wait_until_ready(&ready_path)?;
        let lock_path = temporary_home
            .path()
            .join(".wirelens/run/default-config.lock");

        let error = DefaultConfigLease::acquire(&lock_path)
            .err()
            .expect("a concurrent process must retain default configuration ownership");

        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(error.to_string(), "default_config_already_owned");
        assert!(child.release_and_wait()?.success());
        let _next = DefaultConfigLease::acquire(&lock_path)?;
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn newly_created_lock_is_chmodded_to_0600_under_restrictive_umask() -> io::Result<()> {
        use std::os::unix::fs::PermissionsExt;

        const CHILD_MARKER: &str = "WIRELENS_RESTRICTIVE_UMASK_CHILD";
        const ROOT_ENV: &str = "WIRELENS_RESTRICTIVE_UMASK_ROOT";
        if std::env::var_os(CHILD_MARKER).is_some() {
            let root = PathBuf::from(
                std::env::var_os(ROOT_ENV)
                    .expect("restrictive-umask child should receive its fixture root"),
            );
            let _umask = UmaskGuard::set(rustix::fs::Mode::from_bits_retain(0o200));
            let lock_path = root.join(".wirelens/run/default-config.lock");

            let _lease = DefaultConfigLease::acquire(&lock_path)?;

            assert_eq!(fs::metadata(lock_path)?.permissions().mode() & 0o777, 0o600);
            return Ok(());
        }

        let temporary_home = tempfile::tempdir()?;
        let wirelens_directory = temporary_home.path().join(".wirelens");
        fs::create_dir(&wirelens_directory)?;
        fs::set_permissions(&wirelens_directory, fs::Permissions::from_mode(0o700))?;
        let output = Command::new(std::env::current_exe()?)
            .args([
                "--exact",
                "instance::lock::tests::newly_created_lock_is_chmodded_to_0600_under_restrictive_umask",
                "--nocapture",
            ])
            .env(CHILD_MARKER, "1")
            .env(ROOT_ENV, temporary_home.path())
            .output()?;

        assert!(
            output.status.success(),
            "restrictive-umask child failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(())
    }
}
