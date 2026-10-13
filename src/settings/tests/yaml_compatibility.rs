use super::*;

#[test]
fn preserves_non_string_metadata_keys_while_updating_settings() -> io::Result<()> {
    let path = temp_config_path();
    fs::write(
        &path,
        "extra:\n  1: number\n  true: boolean\n  null: empty\n  ? [left, right]\n  : sequence\n",
    )?;
    let mut manager = SettingsManager::load_from_path(&path)?;
    manager.set_server_port(9091)?;

    let reloaded = SettingsManager::load_from_path(&path)?;
    assert_eq!(reloaded.server_port(), 9091);
    let saved = yaml_from_path(&path)?;
    let metadata = mapping_entry(&saved, "extra")
        .and_then(yaml_serde::Value::as_mapping)
        .expect("metadata mapping survives a settings update");
    let expected = [
        (yaml_serde::Value::from(1), "number"),
        (yaml_serde::Value::Bool(true), "boolean"),
        (yaml_serde::Value::Null, "empty"),
        (
            yaml_serde::Value::Sequence(vec![
                yaml_serde::Value::from("left"),
                yaml_serde::Value::from("right"),
            ]),
            "sequence",
        ),
    ];
    assert_eq!(
        metadata.keys().collect::<Vec<_>>(),
        expected.iter().map(|(key, _)| key).collect::<Vec<_>>()
    );
    for (key, value) in expected {
        assert_eq!(
            metadata.get(&key).and_then(yaml_serde::Value::as_str),
            Some(value)
        );
    }
    Ok(())
}

#[test]
fn preserves_custom_metadata_tags_while_updating_settings() -> io::Result<()> {
    let path = temp_config_path();
    fs::write(
        &path,
        "extra:\n  scalar: !Label '界é'\n  mapping: !Options {enabled: true}\n",
    )?;
    let mut manager = SettingsManager::load_from_path(&path)?;
    manager.set_server_port(9092)?;

    let reloaded = SettingsManager::load_from_path(&path)?;
    assert_eq!(reloaded.server_port(), 9092);
    let saved = yaml_from_path(&path)?;
    let metadata = mapping_entry(&saved, "extra").expect("metadata survives a settings update");
    let Some(yaml_serde::Value::Tagged(scalar)) = mapping_entry(metadata, "scalar") else {
        panic!("custom scalar tag must be retained");
    };
    assert_eq!(scalar.tag, yaml_serde::value::Tag::new("Label"));
    assert_eq!(scalar.value, yaml_serde::Value::from("界é"));
    let Some(yaml_serde::Value::Tagged(mapping)) = mapping_entry(metadata, "mapping") else {
        panic!("custom mapping tag must be retained");
    };
    assert_eq!(mapping.tag, yaml_serde::value::Tag::new("Options"));
    assert_eq!(bool_entry(&mapping.value, "enabled"), Some(true));
    Ok(())
}

#[test]
fn preserves_literal_merge_entries_without_changing_effective_settings() -> io::Result<()> {
    let path = temp_config_path();
    fs::write(
        &path,
        "defaults: &defaults {port: 9090}\nserver:\n  <<: *defaults\n",
    )?;
    let mut manager = SettingsManager::load_from_path(&path)?;
    assert_eq!(manager.server_port(), DEFAULT_PROXY_PORT);
    manager.set_server_port(9093)?;

    let reloaded = SettingsManager::load_from_path(&path)?;
    assert_eq!(reloaded.server_port(), 9093);
    let saved = yaml_from_path(&path)?;
    let server = mapping_entry(&saved, "server").expect("server settings are saved");
    let merge = mapping_entry(server, "<<").expect("literal merge entry is retained");
    assert_eq!(
        mapping_entry(merge, "port").and_then(yaml_serde::Value::as_u64),
        Some(9090)
    );
    Ok(())
}
