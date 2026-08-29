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
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};

    ensure_lock_directory(path)?;
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .mode(0o600)
        .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
        .open(path)?;
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
    use std::io;

    #[test]
    fn default_config_lease_allows_only_one_owner_for_its_lifetime() -> io::Result<()> {
        let temporary_home = tempfile::tempdir()?;
        let lock_path = temporary_home
            .path()
            .join(".wirelens/run/default-config.lock");
        let first = DefaultConfigLease::acquire(&lock_path)?;

        assert!(DefaultConfigLease::acquire(&lock_path).is_err());

        drop(first);
        let _next = DefaultConfigLease::acquire(&lock_path)?;
        Ok(())
    }
}
