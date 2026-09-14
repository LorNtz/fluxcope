use super::*;

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
