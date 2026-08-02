use super::*;

#[test]
fn migrates_legacy_prefilter_patterns_and_preserves_flags_and_order() -> io::Result<()> {
    let path = temp_config_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(
        &path,
        r#"
recording:
  prefilter:
    enable: false
    include_url_patterns:
      - "https://api.example.com/*"
      - pattern: "*://cdn.example.com/assets/*"
        enable: false
      - pattern: "https://static.example.com/*"
"#,
    )?;

    let manager = SettingsManager::load_from_path(&path)?;

    assert!(!manager.recording_settings().prefilter.enable);
    assert_eq!(
        manager.recording_settings().prefilter.include_url_patterns,
        vec![
            RecordingPrefilterPatternSettings::new("https://api.example.com/*"),
            RecordingPrefilterPatternSettings {
                pattern: "*://cdn.example.com/assets/*".to_string(),
                enable: false,
            },
            RecordingPrefilterPatternSettings::new("https://static.example.com/*"),
        ]
    );
    let saved = fs::read_to_string(&path)?;
    let first = saved
        .find("https://api.example.com/*")
        .expect("first pattern should be serialized");
    let second = saved
        .find("*://cdn.example.com/assets/*")
        .expect("second pattern should be serialized");
    let third = saved
        .find("https://static.example.com/*")
        .expect("third pattern should be serialized");
    assert!(first < second && second < third);
    assert_eq!(saved.matches("pattern:").count(), 3);
    assert_eq!(saved.matches("enable: true").count(), 2);
    assert_eq!(saved.matches("enable: false").count(), 2);
    Ok(())
}

#[test]
fn ignores_malformed_prefilter_pattern_entries_and_reports_their_indexes() -> io::Result<()> {
    let path = temp_config_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let content = r#"recording:
  prefilter:
    include_url_patterns:
      - enable: true
      - pattern: "https://api.example.com/*"
        enable: false
      - 42
      - "https://legacy.example.com/*"
"#;
    fs::write(&path, content)?;

    let mut manager = SettingsManager::load_from_path(&path)?;

    assert_eq!(
        manager.recording_settings().prefilter.include_url_patterns,
        vec![
            RecordingPrefilterPatternSettings {
                pattern: "https://api.example.com/*".to_string(),
                enable: false,
            },
            RecordingPrefilterPatternSettings::new("https://legacy.example.com/*"),
        ]
    );
    let diagnostics = manager.take_load_diagnostics();
    assert_eq!(diagnostics.len(), 2);
    assert!(diagnostics[0].message.contains("[0]"));
    assert!(diagnostics[1].message.contains("[2]"));
    assert_eq!(fs::read_to_string(&path)?, content);

    manager.update(|settings| settings.server.port = 9018)?;
    let repaired = yaml_from_path(&path)?;
    let recording =
        mapping_entry(&repaired, "recording").expect("recording settings should be serialized");
    let prefilter = mapping_entry(recording, "prefilter").expect("prefilter should be serialized");
    let patterns = sequence_entry(prefilter, "include_url_patterns")
        .expect("prefilter patterns should be serialized");
    assert_eq!(patterns.len(), 2);
    assert!(
        patterns
            .iter()
            .all(|pattern| mapping_keys(pattern) == vec!["pattern", "enable"])
    );
    Ok(())
}

#[test]
fn preserves_explicit_default_prefilter_fields() -> io::Result<()> {
    let path = temp_config_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(
        &path,
        "recording:\n  prefilter:\n    enable: true\n    include_url_patterns: []\n",
    )?;

    SettingsManager::load_from_path(&path)?;

    let saved = fs::read_to_string(&path)?;
    assert!(saved.contains("prefilter:"));
    assert!(saved.contains("enable: true"));
    assert!(saved.contains("include_url_patterns: []"));
    Ok(())
}

#[test]
fn malformed_prefilter_is_disabled_without_rewriting_file() -> io::Result<()> {
    let path = temp_config_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let content = "server:\n  port: 9017\nrecording:\n  prefilter: invalid\n";
    fs::write(&path, content)?;

    let mut manager = SettingsManager::load_from_path(&path)?;

    assert_eq!(manager.server_port(), 9017);
    assert!(!manager.recording_settings().prefilter.enable);
    assert!(
        manager
            .recording_settings()
            .prefilter
            .include_url_patterns
            .is_empty()
    );
    let diagnostics = manager.take_load_diagnostics();
    assert_eq!(diagnostics.len(), 1);
    assert!(diagnostics[0].message.contains("has been disabled"));
    assert_eq!(fs::read_to_string(&path)?, content);

    manager.update(|settings| settings.server.port = 9018)?;
    let repaired = fs::read_to_string(&path)?;
    assert!(repaired.contains("port: 9018"));
    assert!(repaired.contains("prefilter:"));
    assert!(repaired.contains("enable: false"));
    assert!(!repaired.contains("prefilter: invalid"));
    Ok(())
}

#[test]
fn malformed_unrelated_setting_still_fails_when_prefilter_is_malformed() -> io::Result<()> {
    let path = temp_config_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(
        &path,
        "server:\n  port: invalid\nrecording:\n  prefilter: invalid\n",
    )?;

    let error = SettingsManager::load_from_path(&path)
        .err()
        .expect("invalid server setting should fail");

    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    Ok(())
}
