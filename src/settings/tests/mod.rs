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

fn yaml_from_path(path: &Path) -> io::Result<yaml_serde::Value> {
    yaml_serde::from_str(&fs::read_to_string(path)?).map_err(yaml_error)
}

fn mapping_entry<'a>(value: &'a yaml_serde::Value, key: &str) -> Option<&'a yaml_serde::Value> {
    let yaml_serde::Value::Mapping(mapping) = value else {
        return None;
    };
    mapping.get(key)
}

fn sequence_entry<'a>(
    value: &'a yaml_serde::Value,
    key: &str,
) -> Option<&'a Vec<yaml_serde::Value>> {
    let yaml_serde::Value::Sequence(sequence) = mapping_entry(value, key)? else {
        return None;
    };
    Some(sequence)
}

fn bool_entry(value: &yaml_serde::Value, key: &str) -> Option<bool> {
    let yaml_serde::Value::Bool(field) = mapping_entry(value, key)? else {
        return None;
    };
    Some(*field)
}

fn string_entry<'a>(value: &'a yaml_serde::Value, key: &str) -> Option<&'a str> {
    let yaml_serde::Value::String(field) = mapping_entry(value, key)? else {
        return None;
    };
    Some(field)
}

fn mapping_keys(value: &yaml_serde::Value) -> Vec<&str> {
    let yaml_serde::Value::Mapping(mapping) = value else {
        return Vec::new();
    };
    mapping
        .keys()
        .filter_map(|key| match key {
            yaml_serde::Value::String(key) => Some(key.as_str()),
            _ => None,
        })
        .collect()
}

mod basic;
mod prefilter;
mod proxy;
mod yaml_compatibility;
