use super::*;

#[test]
fn reads_optional_proxy_settings_without_filling_empty_sections() -> io::Result<()> {
    let path = temp_config_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(
        &path,
        r#"
proxy:
  active_preset: dev
  presets:
    - name: dev
      map_remote:
        rules:
          - from: "https://a.com"
            to: "http://b.test.com"
"#,
    )?;

    let manager = SettingsManager::load_from_path(&path)?;
    let proxy = manager
        .proxy_settings()
        .expect("proxy settings should deserialize");

    assert!(proxy.enable);
    assert_eq!(proxy.active_preset.as_deref(), Some("dev"));
    assert_eq!(proxy.presets.len(), 1);
    assert!(proxy.presets[0].map_remote.enable);
    assert_eq!(proxy.presets[0].map_remote.rules.len(), 1);
    assert!(proxy.presets[0].map_remote.rules[0].enable);
    assert!(proxy.presets[0].map_local.rules.is_empty());

    let saved = yaml_from_path(&path)?;
    let proxy = mapping_entry(&saved, "proxy").expect("proxy should be serialized");
    assert!(mapping_entry(proxy, "enable").is_none());
    let presets = sequence_entry(proxy, "presets").expect("presets should be serialized");
    let preset = presets.first().expect("preset should be serialized");
    let map_remote = mapping_entry(preset, "map_remote").expect("map_remote should be serialized");
    assert!(mapping_entry(map_remote, "enable").is_none());
    let rules = sequence_entry(map_remote, "rules").expect("rules should be serialized");
    assert!(mapping_entry(&rules[0], "enable").is_none());
    assert!(mapping_entry(preset, "map_local").is_none());
    Ok(())
}

#[test]
fn preserves_explicit_default_true_proxy_enable_fields() -> io::Result<()> {
    let path = temp_config_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(
        &path,
        r#"
proxy:
  enable: true
  active_preset: dev
  presets:
    - name: dev
      map_remote:
        enable: true
        rules:
          - from: "https://a.com"
            to: "http://b.test.com"
            enable: true
      map_local:
        enable: true
        rules:
          - from: "http://b.test.com"
            to: "~/api.json"
            enable: true
"#,
    )?;

    SettingsManager::load_from_path(&path)?;

    let saved = yaml_from_path(&path)?;
    let proxy = mapping_entry(&saved, "proxy").expect("proxy should be serialized");
    assert_eq!(bool_entry(proxy, "enable"), Some(true));
    let presets = sequence_entry(proxy, "presets").expect("presets should be serialized");
    let preset = presets.first().expect("preset should be serialized");
    let map_remote = mapping_entry(preset, "map_remote").expect("map_remote should be serialized");
    assert_eq!(bool_entry(map_remote, "enable"), Some(true));
    let remote_rules =
        sequence_entry(map_remote, "rules").expect("remote rules should be serialized");
    assert_eq!(bool_entry(&remote_rules[0], "enable"), Some(true));
    let map_local = mapping_entry(preset, "map_local").expect("map_local should be serialized");
    assert_eq!(mapping_keys(map_local), vec!["enable", "rules"]);
    assert_eq!(bool_entry(map_local, "enable"), Some(true));
    let local_rules = sequence_entry(map_local, "rules").expect("local rules should be serialized");
    assert_eq!(bool_entry(&local_rules[0], "enable"), Some(true));
    Ok(())
}

#[test]
fn preserves_existing_mapping_order_at_all_config_levels() -> io::Result<()> {
    let path = temp_config_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(
        &path,
        r#"
proxy:
  presets:
    - map_local:
        enable: true
        rules:
          - to: "~/api.json"
            enable: true
            from: "http://b.test.com"
      map_remote:
        rules:
          - enable: true
            to: "http://b.test.com"
            from: "https://a.com"
        enable: true
      name: dev
  active_preset: dev
  enable: true
server:
  port: 9000
"#,
    )?;

    SettingsManager::load_from_path(&path)?;

    let saved = yaml_from_path(&path)?;
    let root_keys = mapping_keys(&saved);
    assert!(root_keys.starts_with(&["proxy", "server"]));
    assert!(root_keys.contains(&"certificate"));
    assert!(root_keys.contains(&"recording"));
    assert!(root_keys.contains(&"ui"));
    let proxy = mapping_entry(&saved, "proxy").expect("proxy should be serialized");
    assert_eq!(
        mapping_keys(proxy),
        vec!["presets", "active_preset", "enable"]
    );
    let presets = sequence_entry(proxy, "presets").expect("presets should be serialized");
    let preset = presets.first().expect("preset should be serialized");
    assert_eq!(
        mapping_keys(preset),
        vec!["map_local", "map_remote", "name"]
    );

    let map_local = mapping_entry(preset, "map_local").expect("map_local should be serialized");
    assert_eq!(mapping_keys(map_local), vec!["enable", "rules"]);
    let local_rules = sequence_entry(map_local, "rules").expect("local rule should exist");
    assert_eq!(mapping_keys(&local_rules[0]), vec!["to", "enable", "from"]);

    let map_remote = mapping_entry(preset, "map_remote").expect("map_remote should be serialized");
    assert_eq!(mapping_keys(map_remote), vec!["rules", "enable"]);
    let remote_rules = sequence_entry(map_remote, "rules").expect("remote rule should exist");
    assert_eq!(mapping_keys(&remote_rules[0]), vec!["enable", "to", "from"]);
    Ok(())
}

#[test]
fn preserves_present_default_proxy_fields_across_value_types() -> io::Result<()> {
    let path = temp_config_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(
        &path,
        r#"
proxy:
  enable: true
  active_preset: null
  presets:
    - name: ""
      map_remote:
        enable: true
        rules: []
      map_local: {}
"#,
    )?;

    SettingsManager::load_from_path(&path)?;

    let saved = yaml_from_path(&path)?;
    let proxy = mapping_entry(&saved, "proxy").expect("proxy should be serialized");
    assert_eq!(bool_entry(proxy, "enable"), Some(true));
    assert!(matches!(
        mapping_entry(proxy, "active_preset"),
        Some(serde_yaml::Value::Null)
    ));
    let presets = sequence_entry(proxy, "presets").expect("presets should be serialized");
    let preset = presets.first().expect("preset should be serialized");
    assert_eq!(string_entry(preset, "name"), Some(""));
    let map_remote = mapping_entry(preset, "map_remote").expect("map_remote should be serialized");
    assert_eq!(bool_entry(map_remote, "enable"), Some(true));
    assert_eq!(sequence_entry(map_remote, "rules").map(Vec::len), Some(0));
    let map_local = mapping_entry(preset, "map_local").expect("map_local should be preserved");
    assert!(matches!(map_local, serde_yaml::Value::Mapping(mapping) if mapping.is_empty()));
    Ok(())
}

#[test]
fn does_not_restore_previous_config_that_changes_settings() -> io::Result<()> {
    let path = temp_config_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(
        &path,
        r#"
proxy:
  active_preset: dev
  presets:
    - name: dev
"#,
    )?;

    let mut manager = SettingsManager::load_from_path(&path)?;
    manager.update(|settings| {
        settings.proxy = None;
    })?;

    let saved = yaml_from_path(&path)?;
    assert!(mapping_entry(&saved, "proxy").is_none());
    Ok(())
}
