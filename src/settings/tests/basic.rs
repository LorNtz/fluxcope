use super::*;
use crate::cli::ConfigSelection;
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;
use std::{
    net::{IpAddr, Ipv4Addr},
    process::Command,
};

#[test]
fn creates_default_config_when_missing() -> io::Result<()> {
    let path = temp_config_path();

    let manager = SettingsManager::load_from_path(&path)?;

    assert_eq!(DEFAULT_PROXY_PORT, manager.server_port());
    assert!(path.exists());

    let saved: AppSettings =
        serde_yaml::from_str(&fs::read_to_string(&path)?).map_err(yaml_error)?;
    assert_eq!(DEFAULT_PROXY_PORT, saved.server.port);
    assert_eq!(DEFAULT_CERTIFICATE_STORE_DIR, saved.certificate.store_dir);
    assert_eq!(
        DEFAULT_CERTIFICATE_PEM_FILENAME,
        saved.certificate.pem_filename
    );
    assert!(manager.recording_settings().start_record_on_launch);
    assert!(saved.recording.start_record_on_launch);
    assert!(saved.recording.prefilter.enable);
    assert!(saved.recording.prefilter.include_url_patterns.is_empty());
    assert!(!saved.ui.request_list.auto_expand);
    assert!(saved.proxy.is_none());
    let content = fs::read_to_string(&path)?;
    assert!(!content.contains("proxy:"));
    assert!(!content.contains("prefilter:"));
    Ok(())
}

#[test]
fn reads_existing_config_and_updates_file() -> io::Result<()> {
    let path = temp_config_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&path, "server:\n  port: 9000\n")?;

    let mut manager = SettingsManager::load_from_path(&path)?;
    assert_eq!(9000, manager.server_port());

    manager.set_server_port(9010)?;

    assert_eq!(9010, manager.server_port());
    assert_eq!(
        home_dir()?.join(".fluxcope/certificate/"),
        manager.certificate_store_dir()?
    );
    assert_eq!(
        DEFAULT_CERTIFICATE_PEM_FILENAME,
        manager.certificate_pem_filename()
    );
    assert!(manager.recording_settings().start_record_on_launch);
    assert!(!manager.ui_settings().request_list.auto_expand);

    let saved: AppSettings =
        serde_yaml::from_str(&fs::read_to_string(&path)?).map_err(yaml_error)?;
    assert_eq!(9010, saved.server.port);
    assert_eq!(DEFAULT_CERTIFICATE_STORE_DIR, saved.certificate.store_dir);
    assert_eq!(
        DEFAULT_CERTIFICATE_PEM_FILENAME,
        saved.certificate.pem_filename
    );
    assert!(saved.recording.start_record_on_launch);
    assert!(!saved.ui.request_list.auto_expand);
    assert!(saved.proxy.is_none());
    assert!(!fs::read_to_string(&path)?.contains("proxy:"));
    Ok(())
}

#[test]
fn exposes_loaded_settings_snapshot() -> io::Result<()> {
    let path = temp_config_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&path, "server:\n  port: 9012\n")?;

    let manager = SettingsManager::load_from_path(&path)?;

    assert_eq!(9012, manager.settings().server.port);
    Ok(())
}

#[test]
fn reads_request_list_auto_expand_setting() -> io::Result<()> {
    let path = temp_config_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&path, "ui:\n  request_list:\n    auto_expand: true\n")?;

    let manager = SettingsManager::load_from_path(&path)?;

    assert!(manager.ui_settings().request_list.auto_expand);

    let saved: AppSettings =
        serde_yaml::from_str(&fs::read_to_string(&path)?).map_err(yaml_error)?;
    assert!(saved.ui.request_list.auto_expand);
    Ok(())
}

#[test]
fn reads_start_record_on_launch_setting() -> io::Result<()> {
    let path = temp_config_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&path, "recording:\n  start_record_on_launch: false\n")?;

    let manager = SettingsManager::load_from_path(&path)?;

    assert!(!manager.recording_settings().start_record_on_launch);

    let saved: AppSettings =
        serde_yaml::from_str(&fs::read_to_string(&path)?).map_err(yaml_error)?;
    assert!(!saved.recording.start_record_on_launch);
    Ok(())
}

#[test]
fn default_config_path_uses_fluxcope_home_dir() -> io::Result<()> {
    assert_eq!(
        home_dir()?.join(".fluxcope/config.yml"),
        default_config_path()?
    );
    Ok(())
}

fn round_trip_yaml(yaml: &str) -> io::Result<String> {
    let path = temp_config_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&path, yaml)?;
    let mut manager = SettingsManager::load_from_path(&path)?;
    manager.update(|_| {})?;
    fs::read_to_string(&path)
}

#[test]
fn mcp_enable_defaults_to_disabled_and_is_omitted() -> io::Result<()> {
    let settings = AppSettings::default();

    assert!(!settings.mcp.enable);
    let saved = serde_yaml::to_string(&settings).map_err(yaml_error)?;
    assert!(!saved.contains("mcp:"), "{saved}");
    Ok(())
}

#[test]
fn explicit_default_mcp_enable_survives_round_trip() -> io::Result<()> {
    let saved = round_trip_yaml("mcp:\n  enable: false\n")?;

    assert!(saved.contains("mcp:\n  enable: false"), "{saved}");
    Ok(())
}

#[test]
fn read_only_file_commit_updates_memory_without_writing() -> io::Result<()> {
    let path = temp_config_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let original = "server:\n  port: 9007\nmcp:\n  enable: true\n";
    fs::write(&path, original)?;
    let canonical_path = fs::canonicalize(&path)?;
    let mut session = SettingsSession::load(&ConfigSelection::ReadOnlyFile(path.to_path_buf()))
        .map_err(|error| io::Error::other(error.to_string()))?;
    let mut next = session.snapshot().as_ref().clone();
    next.server.port = 9100;

    assert_eq!(session.commit(next)?, PersistenceMode::Ephemeral);
    assert_eq!(session.snapshot().server.port, 9100);
    assert_eq!(session.source_path(), Some(canonical_path.as_path()));
    assert_eq!(fs::read_to_string(&path)?, original);
    Ok(())
}

#[test]
fn temporary_commit_is_memory_only_and_creates_no_config_file() -> io::Result<()> {
    const CHILD_MARKER: &str = "WIRELENS_TEMPORARY_SETTINGS_CHILD";
    if std::env::var_os(CHILD_MARKER).is_some() {
        let mut session = SettingsSession::load(&ConfigSelection::Temporary {
            host: IpAddr::V4(Ipv4Addr::LOCALHOST),
            port: 9008,
        })
        .map_err(|error| io::Error::other(error.to_string()))?;
        assert_eq!(session.snapshot().server.port, 9008);
        let mut next = session.snapshot().as_ref().clone();
        next.server.port = 9100;
        assert_eq!(session.commit(next)?, PersistenceMode::Ephemeral);
        assert_eq!(session.snapshot().server.port, 9100);
        assert!(session.source_path().is_none());
        return Ok(());
    }

    let home = tempfile::tempdir()?;
    let output = Command::new(std::env::current_exe()?)
        .args([
            "--exact",
            "settings::tests::basic::temporary_commit_is_memory_only_and_creates_no_config_file",
            "--nocapture",
        ])
        .env("HOME", home.path())
        .env(CHILD_MARKER, "1")
        .output()?;

    assert!(
        output.status.success(),
        "temporary settings child failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!home.path().join(".wirelens").exists());
    Ok(())
}

#[test]
fn default_owned_session_persists_under_an_isolated_home() -> io::Result<()> {
    const CHILD_MARKER: &str = "WIRELENS_DEFAULT_SETTINGS_CHILD";
    if std::env::var_os(CHILD_MARKER).is_some() {
        let mut session = SettingsSession::load(&ConfigSelection::DefaultOwned)
            .map_err(|error| io::Error::other(error.to_string()))?;
        let expected_path = fs::canonicalize(default_config_path()?)?;
        assert_eq!(session.source_path(), Some(expected_path.as_path()));
        let mut next = session.snapshot().as_ref().clone();
        next.server.port = 9101;
        assert_eq!(session.commit(next)?, PersistenceMode::Persistent);
        return Ok(());
    }

    let home = tempfile::tempdir()?;
    let output = Command::new(std::env::current_exe()?)
        .args([
            "--exact",
            "settings::tests::basic::default_owned_session_persists_under_an_isolated_home",
            "--nocapture",
        ])
        .env("HOME", home.path())
        .env(CHILD_MARKER, "1")
        .output()?;

    assert!(
        output.status.success(),
        "default-owned settings child failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let saved: AppSettings = serde_yaml::from_str(&fs::read_to_string(
        home.path().join(".wirelens/config.yml"),
    )?)
    .map_err(yaml_error)?;
    assert_eq!(saved.server.port, 9101);
    Ok(())
}
#[cfg(unix)]
#[test]
fn default_owned_rejects_symlink_config_without_touching_target() -> io::Result<()> {
    const CHILD_MARKER: &str = "WIRELENS_SYMLINK_SETTINGS_CHILD";
    if std::env::var_os(CHILD_MARKER).is_some() {
        let error = SettingsSession::load(&ConfigSelection::DefaultOwned)
            .err()
            .expect("default-owned symlink configuration must be rejected");
        assert!(
            error.to_string().contains("symlink"),
            "unexpected startup error: {error:#}"
        );
        return Ok(());
    }

    let home = tempfile::tempdir()?;
    let wirelens_directory = home.path().join(".wirelens");
    fs::create_dir_all(&wirelens_directory)?;
    let target = home.path().join("symlink-target.yml");
    let original = "server:\n  port: 9120\n";
    fs::write(&target, original)?;
    let config = wirelens_directory.join("config.yml");
    std::os::unix::fs::symlink(&target, &config)?;
    let output = Command::new(std::env::current_exe()?)
        .args([
            "--exact",
            "settings::tests::basic::default_owned_rejects_symlink_config_without_touching_target",
            "--nocapture",
        ])
        .env("HOME", home.path())
        .env(CHILD_MARKER, "1")
        .output()?;

    assert!(
        output.status.success(),
        "symlink settings child failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(fs::symlink_metadata(&config)?.file_type().is_symlink());
    assert_eq!(fs::read_to_string(target)?, original);
    Ok(())
}

#[cfg(unix)]
#[test]
fn persistent_update_atomically_replaces_the_config_file() -> io::Result<()> {
    let path = temp_config_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&path, "server:\n  port: 9000\n")?;
    let mut manager = SettingsManager::load_from_path(&path)?;
    let original_inode = fs::metadata(&path)?.ino();

    manager.set_server_port(9102)?;

    assert_ne!(fs::metadata(&path)?.ino(), original_inode);
    assert_eq!(manager.server_port(), 9102);
    Ok(())
}

#[cfg(unix)]
#[test]
fn failed_atomic_update_preserves_file_and_memory() -> io::Result<()> {
    let path = temp_config_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&path, "server:\n  port: 9000\n")?;
    let mut manager = SettingsManager::load_from_path(&path)?;
    let previous_file = fs::read_to_string(&path)?;
    let parent = path
        .parent()
        .expect("temporary config path should have a parent");
    let _permissions = DirectoryPermissionsGuard::make_read_only(parent)?;

    let result = manager.set_server_port(9103);

    assert!(
        result.is_err(),
        "read-only directory must reject replacement"
    );
    assert_eq!(manager.server_port(), 9000);
    assert_eq!(fs::read_to_string(&path)?, previous_file);
    Ok(())
}
