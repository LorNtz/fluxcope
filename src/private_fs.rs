//! Owner-only storage for configuration, CA material, and logs.

use std::{
    fs::{self, File},
    io::{self, Write},
    os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt},
    path::Path,
};

use rustix::fs::{Mode, OFlags};

pub(crate) fn ensure_directory(path: &Path) -> io::Result<()> {
    if path.as_os_str().is_empty() {
        return Ok(());
    }
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)?;
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir() || metadata.uid() != rustix::process::geteuid().as_raw() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "private storage must be a directory owned by the current user",
        ));
    }
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
}

pub(crate) fn open_file(path: &Path, append: bool) -> io::Result<File> {
    let access = if append {
        OFlags::WRONLY | OFlags::CREATE | OFlags::APPEND
    } else {
        OFlags::RDONLY
    };
    let fd = rustix::fs::open(
        path,
        access | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
        Mode::RUSR | Mode::WUSR,
    )?;
    let file = File::from(fd);
    let metadata = file.metadata()?;
    if !metadata.is_file()
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.nlink() != 1
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "private storage must be a regular, singly linked file owned by the current user",
        ));
    }
    file.set_permissions(fs::Permissions::from_mode(0o600))?;
    Ok(file)
}

/// Replace a settings or CA file atomically without following an existing symlink.
pub(crate) fn write_file(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    match fs::symlink_metadata(path) {
        Ok(_) => {
            open_file(path, false)?;
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.write_all(bytes)?;
    temporary.as_file().sync_all()?;
    temporary.persist(path).map_err(|error| error.error)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    #[test]
    fn state_is_private_and_existing_permissions_are_repaired() -> io::Result<()> {
        let root = tempfile::tempdir()?;
        let directory = root.path().join("state");
        ensure_directory(&directory)?;
        assert_eq!(fs::metadata(&directory)?.mode() & 0o777, 0o700);
        let path = directory.join("key");
        write_file(&path, b"synthetic test material")?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644))?;
        open_file(&path, false)?;
        assert_eq!(fs::metadata(&path)?.mode() & 0o777, 0o600);
        write_file(&path, b"replacement")?;
        assert_eq!(fs::read(&path)?, b"replacement");
        assert_eq!(fs::metadata(&path)?.mode() & 0o777, 0o600);
        Ok(())
    }

    #[test]
    fn symlinks_and_hardlinks_do_not_modify_their_targets() -> io::Result<()> {
        let root = tempfile::tempdir()?;
        let target = root.path().join("target");
        fs::write(&target, b"untouched")?;
        let link = root.path().join("link");
        symlink(&target, &link)?;
        assert!(open_file(&link, true).is_err());
        assert!(write_file(&link, b"overwrite").is_err());
        fs::remove_file(&link)?;
        fs::hard_link(&target, &link)?;
        assert!(write_file(&link, b"overwrite").is_err());
        assert_eq!(fs::read(&target)?, b"untouched");
        Ok(())
    }

    #[test]
    fn fifo_is_rejected_without_waiting_for_a_peer() -> io::Result<()> {
        let root = tempfile::tempdir()?;
        let path = root.path().join("fifo");
        assert!(
            std::process::Command::new("mkfifo")
                .arg(&path)
                .status()?
                .success()
        );
        assert!(open_file(&path, false).is_err());
        assert!(open_file(&path, true).is_err());
        Ok(())
    }
}
