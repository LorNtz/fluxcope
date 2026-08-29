use super::*;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::{
    ops::Deref,
    path::{Path, PathBuf},
};
use tempfile::TempDir;

struct TempConfigPath {
    _directory: TempDir,
    path: PathBuf,
}

impl Deref for TempConfigPath {
    type Target = Path;

    fn deref(&self) -> &Self::Target {
        &self.path
    }
}

impl AsRef<Path> for TempConfigPath {
    fn as_ref(&self) -> &Path {
        &self.path
    }
}

fn temp_config_path() -> TempConfigPath {
    let directory = tempfile::tempdir().expect("temporary settings directory should be created");
    let path = directory.path().join("config.yml");
    TempConfigPath {
        _directory: directory,
        path,
    }
}
#[cfg(unix)]
struct DirectoryPermissionsGuard {
    path: PathBuf,
    original_mode: u32,
}

#[cfg(unix)]
impl DirectoryPermissionsGuard {
    fn make_read_only(path: &Path) -> io::Result<Self> {
        let original_mode = fs::metadata(path)?.permissions().mode();
        fs::set_permissions(path, fs::Permissions::from_mode(0o500))?;
        Ok(Self {
            path: path.to_path_buf(),
            original_mode,
        })
    }
}

#[cfg(unix)]
impl Drop for DirectoryPermissionsGuard {
    fn drop(&mut self) {
        let _ = fs::set_permissions(&self.path, fs::Permissions::from_mode(self.original_mode));
    }
}

fn yaml_from_path(path: &Path) -> io::Result<serde_yaml::Value> {
    serde_yaml::from_str(&fs::read_to_string(path)?).map_err(yaml_error)
}

fn mapping_entry<'a>(value: &'a serde_yaml::Value, key: &str) -> Option<&'a serde_yaml::Value> {
    let serde_yaml::Value::Mapping(mapping) = value else {
        return None;
    };
    mapping.get(key)
}

fn sequence_entry<'a>(
    value: &'a serde_yaml::Value,
    key: &str,
) -> Option<&'a Vec<serde_yaml::Value>> {
    let serde_yaml::Value::Sequence(sequence) = mapping_entry(value, key)? else {
        return None;
    };
    Some(sequence)
}

fn bool_entry(value: &serde_yaml::Value, key: &str) -> Option<bool> {
    let serde_yaml::Value::Bool(field) = mapping_entry(value, key)? else {
        return None;
    };
    Some(*field)
}

fn string_entry<'a>(value: &'a serde_yaml::Value, key: &str) -> Option<&'a str> {
    let serde_yaml::Value::String(field) = mapping_entry(value, key)? else {
        return None;
    };
    Some(field)
}

fn mapping_keys(value: &serde_yaml::Value) -> Vec<&str> {
    let serde_yaml::Value::Mapping(mapping) = value else {
        return Vec::new();
    };
    mapping
        .keys()
        .filter_map(|key| match key {
            serde_yaml::Value::String(key) => Some(key.as_str()),
            _ => None,
        })
        .collect()
}

mod basic;
mod prefilter;
mod proxy;
