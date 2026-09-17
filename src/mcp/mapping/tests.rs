use schemars::{JsonSchema, schema_for};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use super::super::capture::RequiredInstanceSelector;
use super::{
    CreateMappingRuleInput, CreatePresetInput, DeleteMappingRuleInput, DeletePresetInput,
    ExplainMappingInput, GetMappingSettingsInput, MappingGateTarget, MappingRuleInput,
    MoveMappingRuleInput, RenamePresetInput, SetActivePresetInput, SetMappingGateInput,
    SetMappingRuleEnabledInput, UpdateMappingRuleInput, ValidateMappingSettingsInput,
    get_mapping_result, mapping_mutation_result,
};
use crate::{
    control_rpc::protocol::{
        ControlOperation, ControlResult, InstanceScope, MappingSettingsPayload,
    },
    instance::RunId,
    settings::{
        ConfigMode, PersistenceMode, ProxyMapLocalRule, ProxyMapRemoteRule, ProxyMapRemoteSettings,
        ProxyPresetSettings, ProxySettings,
        mapping_ops::{
            MAX_MAPPING_DIAGNOSTICS, MappingMutation, MappingObjectRef, ProxyRuleTable,
            validate_mapping_candidate,
        },
    },
};
use std::{net::SocketAddr, str::FromStr};

const RUN_ID: &str = "AAAAAAAAAAAAAAAAAAAAAA";

fn instance_json() -> Value {
    json!({"proxy_endpoint": "127.0.0.1:19014", "run_id": RUN_ID})
}

fn instance() -> RequiredInstanceSelector {
    RequiredInstanceSelector {
        proxy_endpoint: SocketAddr::from_str("127.0.0.1:19014").expect("endpoint"),
        run_id: RunId::from_str(RUN_ID).expect("run ID"),
    }
}

fn with_identity(fields: Value) -> Value {
    let mut value = json!({
        "instance": instance_json(),
        "expected_settings_revision": 7
    });
    value
        .as_object_mut()
        .expect("object")
        .extend(fields.as_object().expect("fields").clone());
    value
}

fn decode<T: DeserializeOwned>(value: Value) -> T {
    serde_json::from_value(value).expect("valid mapping input")
}

#[test]
fn read_validate_and_explain_inputs_are_closed_and_side_effect_free_operations() {
    let get: GetMappingSettingsInput = decode(json!({"instance": instance_json()}));
    assert_eq!(get.instance, instance().selector());
    assert_eq!(
        get.into_operation(),
        ControlOperation::GetMappingSettings {
            scope: Default::default()
        }
    );

    let validate: ValidateMappingSettingsInput = decode(json!({
        "instance": instance_json(),
        "proxy": {
            "enabled": true,
            "active_preset": "dev",
            "presets": [{"name": "dev"}]
        }
    }));
    assert_eq!(
        validate.into_operation(),
        ControlOperation::ValidateMappingSettings {
            proxy: Box::new(ProxySettings {
                enable: true,
                active_preset: Some("dev".to_string()),
                presets: vec![ProxyPresetSettings {
                    name: "dev".to_string(),
                    ..ProxyPresetSettings::default()
                }],
            })
        }
    );

    let current: ExplainMappingInput = decode(json!({
        "instance": instance_json(),
        "url": "https://api.example.com/v1"
    }));
    assert_eq!(
        current.into_operation(),
        ControlOperation::ExplainMapping {
            url: "https://api.example.com/v1".to_string(),
            proposed_proxy: None,
        }
    );
    let proposed: ExplainMappingInput = decode(json!({
        "instance": instance_json(),
        "url": "https://api.example.com/v1",
        "proposed_proxy": {"enabled": false}
    }));
    assert_eq!(
        proposed.into_operation(),
        ControlOperation::ExplainMapping {
            url: "https://api.example.com/v1".to_string(),
            proposed_proxy: Some(Box::new(ProxySettings {
                enable: false,
                ..ProxySettings::default()
            })),
        }
    );

    assert!(
        serde_json::from_value::<GetMappingSettingsInput>(
            json!({"instance": instance_json(), "unexpected": true})
        )
        .is_err()
    );
    assert!(
        serde_json::from_value::<ValidateMappingSettingsInput>(
            json!({"instance": instance_json(), "proxy": {}, "unexpected": true})
        )
        .is_err()
    );
    assert!(
        serde_json::from_value::<ExplainMappingInput>(json!({
            "instance": instance_json(),
            "url": "https://example.com",
            "unexpected": true
        }))
        .is_err()
    );
}
#[test]
fn mapping_reads_allow_snapshot_selector_omission_and_partial_targeting() {
    for instance in [
        None,
        Some(json!({"proxy_endpoint": "127.0.0.1:19014"})),
        Some(json!({"run_id": RUN_ID})),
    ] {
        let mut get = json!({});
        let mut validate = json!({"proxy": {}});
        let mut explain = json!({"url": "https://example.com"});
        if let Some(instance) = instance {
            get["instance"] = instance.clone();
            validate["instance"] = instance.clone();
            explain["instance"] = instance;
        }
        assert!(serde_json::from_value::<GetMappingSettingsInput>(get).is_ok());
        assert!(serde_json::from_value::<ValidateMappingSettingsInput>(validate).is_ok());
        assert!(serde_json::from_value::<ExplainMappingInput>(explain).is_ok());
    }
}

#[test]
fn create_preset_accepts_a_complete_optional_preset_without_implicit_activation() {
    let input: CreatePresetInput = decode(with_identity(json!({
        "name": "dev",
        "initial": {
            "map_remote": {
                "enabled": false,
                "rules": [{
                    "from": "https://api.example.com",
                    "to": "http://127.0.0.1:4310",
                    "enabled": false
                }]
            },
            "map_local": {
                "enabled": false,
                "rules": [{
                    "from": "https://cdn.example.com/assets",
                    "to": "/tmp/assets",
                    "enabled": true
                }]
            }
        }
    })));

    assert_eq!(
        input.into_operation(),
        ControlOperation::MutateMapping {
            expected_revision: crate::runtime::settings::SettingsRevision::new(7),
            mutation: Box::new(MappingMutation::CreatePreset {
                name: "dev".to_string(),
                initial: Some(ProxyPresetSettings {
                    name: String::new(),
                    map_remote: crate::settings::ProxyMapRemoteSettings {
                        enable: false,
                        rules: vec![ProxyMapRemoteRule {
                            from: "https://api.example.com".to_string(),
                            to: "http://127.0.0.1:4310".to_string(),
                            enable: false,
                        }],
                    },
                    map_local: crate::settings::ProxyMapLocalSettings {
                        enable: false,
                        rules: vec![ProxyMapLocalRule {
                            from: "https://cdn.example.com/assets".to_string(),
                            to: "/tmp/assets".to_string(),
                            enable: true,
                        }],
                    },
                }),
            }),
        }
    );
}

#[test]
fn all_public_mapping_writes_convert_to_exactly_one_domain_mutation() {
    let cases: Vec<(Box<dyn Fn() -> ControlOperation>, ControlOperation)> = vec![
        (
            Box::new(|| {
                decode::<RenamePresetInput>(with_identity(json!({
                    "name": "dev", "new_name": "staging"
                })))
                .into_operation()
            }),
            mutation(MappingMutation::RenamePreset {
                name: "dev".to_string(),
                new_name: "staging".to_string(),
            }),
        ),
        (
            Box::new(|| {
                decode::<DeletePresetInput>(with_identity(json!({"name": "dev"}))).into_operation()
            }),
            mutation(MappingMutation::DeletePreset {
                name: "dev".to_string(),
            }),
        ),
        (
            Box::new(|| {
                decode::<SetActivePresetInput>(with_identity(json!({"name": null})))
                    .into_operation()
            }),
            mutation(MappingMutation::SetActivePreset { name: None }),
        ),
        (
            Box::new(|| {
                decode::<SetMappingGateInput>(with_identity(json!({
                    "gate": {"kind": "global"}, "enabled": false
                })))
                .into_operation()
            }),
            mutation(MappingMutation::SetGlobalEnabled { enabled: false }),
        ),
        (
            Box::new(|| {
                decode::<SetMappingGateInput>(with_identity(json!({
                    "gate": {"kind": "table", "preset": "dev", "table": "remote"},
                    "enabled": false
                })))
                .into_operation()
            }),
            mutation(MappingMutation::SetTableEnabled {
                preset: "dev".to_string(),
                table: ProxyRuleTable::Remote,
                enabled: false,
            }),
        ),
        (
            Box::new(|| {
                decode::<CreateMappingRuleInput>(with_identity(json!({
                    "preset": "dev",
                    "table": "local",
                    "index": 1,
                    "rule": {
                        "from": "https://cdn.example.com/assets",
                        "to": "/tmp/assets",
                        "enabled": false
                    }
                })))
                .into_operation()
            }),
            mutation(MappingMutation::InsertLocalRule {
                preset: "dev".to_string(),
                index: 1,
                rule: ProxyMapLocalRule {
                    from: "https://cdn.example.com/assets".to_string(),
                    to: "/tmp/assets".to_string(),
                    enable: false,
                },
            }),
        ),
        (
            Box::new(|| {
                decode::<CreateMappingRuleInput>(with_identity(json!({
                    "preset": "dev",
                    "table": "remote",
                    "rule": {
                        "from": "https://api.example.com",
                        "to": "http://127.0.0.1:4310",
                        "enabled": true
                    }
                })))
                .into_operation()
            }),
            mutation(MappingMutation::AppendRemoteRule {
                preset: "dev".to_string(),
                rule: ProxyMapRemoteRule {
                    from: "https://api.example.com".to_string(),
                    to: "http://127.0.0.1:4310".to_string(),
                    enable: true,
                },
            }),
        ),
        (
            Box::new(|| {
                decode::<UpdateMappingRuleInput>(with_identity(json!({
                    "preset": "dev", "table": "remote", "index": 2,
                    "from": "https://new.example.com", "to": "http://127.0.0.1:4320"
                })))
                .into_operation()
            }),
            mutation(MappingMutation::UpdateRemoteRule {
                preset: "dev".to_string(),
                index: 2,
                from: "https://new.example.com".to_string(),
                to: "http://127.0.0.1:4320".to_string(),
            }),
        ),
        (
            Box::new(|| {
                decode::<DeleteMappingRuleInput>(with_identity(json!({
                    "preset": "dev", "table": "local", "index": 3
                })))
                .into_operation()
            }),
            mutation(MappingMutation::DeleteRule {
                preset: "dev".to_string(),
                table: ProxyRuleTable::Local,
                index: 3,
            }),
        ),
        (
            Box::new(|| {
                decode::<MoveMappingRuleInput>(with_identity(json!({
                    "preset": "dev", "table": "remote", "from": 3, "to": 1
                })))
                .into_operation()
            }),
            mutation(MappingMutation::MoveRule {
                preset: "dev".to_string(),
                table: ProxyRuleTable::Remote,
                from: 3,
                to: 1,
            }),
        ),
        (
            Box::new(|| {
                decode::<SetMappingRuleEnabledInput>(with_identity(json!({
                    "preset": "dev", "table": "remote", "index": 1, "enabled": false
                })))
                .into_operation()
            }),
            mutation(MappingMutation::SetRuleEnabled {
                preset: "dev".to_string(),
                table: ProxyRuleTable::Remote,
                index: 1,
                enabled: false,
            }),
        ),
    ];

    for (actual, expected) in cases {
        assert_eq!(actual(), expected);
    }
}

#[test]
fn create_mapping_rule_defaults_omitted_enabled_to_true() {
    let input: CreateMappingRuleInput = decode(with_identity(json!({
        "preset": "dev",
        "table": "remote",
        "rule": {
            "from": "https://a.example",
            "to": "https://b.example"
        }
    })));

    assert_eq!(
        input.into_operation(),
        mutation(MappingMutation::AppendRemoteRule {
            preset: "dev".to_string(),
            rule: ProxyMapRemoteRule {
                from: "https://a.example".to_string(),
                to: "https://b.example".to_string(),
                enable: true,
            },
        })
    );
}

#[test]
fn get_mapping_settings_uses_a_normalized_complete_public_view() {
    let proxy = ProxySettings {
        enable: false,
        active_preset: Some("dev".to_string()),
        presets: vec![ProxyPresetSettings {
            name: "dev".to_string(),
            map_remote: ProxyMapRemoteSettings {
                enable: false,
                rules: vec![ProxyMapRemoteRule {
                    from: "https://a.example".to_string(),
                    to: "https://b.example".to_string(),
                    enable: true,
                }],
            },
            ..ProxyPresetSettings::default()
        }],
    };
    let result = get_mapping_result(ControlResult::GetMappingSettings {
        instance: InstanceScope {
            proxy_endpoint: instance().proxy_endpoint,
            run_id: instance().run_id,
        },
        settings_revision: crate::runtime::settings::SettingsRevision::new(7),
        config_mode: ConfigMode::Temporary,
        persistence: PersistenceMode::Ephemeral,
        proxy: MappingSettingsPayload::from_proxy(proxy),
    })
    .expect("mapping result");
    let value = serde_json::to_value(result).expect("public result");

    assert!(value.get("proxy").is_none());
    assert_eq!(value["mapping"]["enabled"], false);
    assert_eq!(value["mapping"]["active_preset"], "dev");
    assert_eq!(
        value["mapping"]["presets"][0]["map_remote"]["enabled"],
        false
    );
    assert_eq!(
        value["mapping"]["presets"][0]["map_remote"]["rules"][0]["index"],
        0
    );
    assert_eq!(value["mapping"]["presets"][0]["map_local"]["enabled"], true);
    assert_eq!(
        value["mapping"]["presets"][0]["map_local"]["rules"],
        json!([])
    );
}

#[test]
fn mapping_diagnostics_are_capped_with_explicit_omission_counts() {
    let proxy = ProxySettings {
        presets: vec![ProxyPresetSettings {
            name: "dev".to_string(),
            map_remote: ProxyMapRemoteSettings {
                rules: (0..MAX_MAPPING_DIAGNOSTICS + 10)
                    .map(|index| ProxyMapRemoteRule {
                        from: format!("not a URL {index}"),
                        to: "also not a URL".to_string(),
                        enable: true,
                    })
                    .collect(),
                ..ProxyMapRemoteSettings::default()
            },
            ..ProxyPresetSettings::default()
        }],
        active_preset: Some("dev".to_string()),
        ..ProxySettings::default()
    };
    let validation = validate_mapping_candidate(Some(&proxy));

    assert_eq!(validation.diagnostics.len(), MAX_MAPPING_DIAGNOSTICS);
    assert!(validation.diagnostics_total > validation.diagnostics.len());
    assert_eq!(
        validation.diagnostics_omitted,
        validation.diagnostics_total - validation.diagnostics.len()
    );
}

fn mutation(mutation: MappingMutation) -> ControlOperation {
    ControlOperation::MutateMapping {
        expected_revision: crate::runtime::settings::SettingsRevision::new(7),
        mutation: Box::new(mutation),
    }
}

#[test]
fn every_mapping_write_requires_endpoint_run_and_expected_revision() {
    let valid_values = [
        ("create_preset", with_identity(json!({"name": "dev"}))),
        (
            "rename_preset",
            with_identity(json!({"name": "dev", "new_name": "next"})),
        ),
        ("delete_preset", with_identity(json!({"name": "dev"}))),
        ("set_active_preset", with_identity(json!({"name": null}))),
        (
            "set_mapping_gate",
            with_identity(json!({"gate": {"kind": "global"}, "enabled": true})),
        ),
        (
            "create_mapping_rule",
            with_identity(json!({
                "preset": "dev", "table": "remote",
                "rule": {"from": "https://a.example", "to": "https://b.example", "enabled": true}
            })),
        ),
        (
            "update_mapping_rule",
            with_identity(json!({
                "preset": "dev", "table": "remote", "index": 0,
                "from": "https://a.example", "to": "https://b.example"
            })),
        ),
        (
            "delete_mapping_rule",
            with_identity(json!({"preset": "dev", "table": "remote", "index": 0})),
        ),
        (
            "move_mapping_rule",
            with_identity(json!({"preset": "dev", "table": "remote", "from": 0, "to": 1})),
        ),
        (
            "set_mapping_rule_enabled",
            with_identity(json!({
                "preset": "dev", "table": "remote", "index": 0, "enabled": true
            })),
        ),
    ];

    for (name, value) in valid_values {
        assert_rejects_missing_identity_or_revision(name, value);
    }
}

fn assert_rejects_missing_identity_or_revision(name: &str, valid: Value) {
    for path in [
        "/instance/proxy_endpoint",
        "/instance/run_id",
        "/expected_settings_revision",
    ] {
        let mut missing = valid.clone();
        let (parent, field) = path.rsplit_once('/').expect("JSON path");
        let parent = if parent.is_empty() {
            &mut missing
        } else {
            missing
                .pointer_mut(parent)
                .unwrap_or_else(|| panic!("missing parent {parent}"))
        };
        parent.as_object_mut().expect("parent object").remove(field);
        let accepted = mapping_input_accepts(name, missing);
        assert!(!accepted, "{name} accepted missing {path}");
    }
    let mut unknown = valid;
    unknown
        .as_object_mut()
        .expect("mapping input object")
        .insert("unexpected".to_string(), json!(true));
    assert!(
        !mapping_input_accepts(name, unknown),
        "{name} accepted an unknown field"
    );
}

fn mapping_input_accepts(name: &str, value: Value) -> bool {
    match name {
        "create_preset" => serde_json::from_value::<CreatePresetInput>(value).is_ok(),
        "rename_preset" => serde_json::from_value::<RenamePresetInput>(value).is_ok(),
        "delete_preset" => serde_json::from_value::<DeletePresetInput>(value).is_ok(),
        "set_active_preset" => serde_json::from_value::<SetActivePresetInput>(value).is_ok(),
        "set_mapping_gate" => serde_json::from_value::<SetMappingGateInput>(value).is_ok(),
        "create_mapping_rule" => serde_json::from_value::<CreateMappingRuleInput>(value).is_ok(),
        "update_mapping_rule" => serde_json::from_value::<UpdateMappingRuleInput>(value).is_ok(),
        "delete_mapping_rule" => serde_json::from_value::<DeleteMappingRuleInput>(value).is_ok(),
        "move_mapping_rule" => serde_json::from_value::<MoveMappingRuleInput>(value).is_ok(),
        "set_mapping_rule_enabled" => {
            serde_json::from_value::<SetMappingRuleEnabledInput>(value).is_ok()
        }
        _ => unreachable!("known mutation"),
    }
}

#[test]
fn mapping_tool_schemas_are_closed_and_require_contract_fields() {
    assert_schema::<GetMappingSettingsInput>(&[], false);
    assert_schema::<ValidateMappingSettingsInput>(&["proxy"], false);
    assert_schema::<ExplainMappingInput>(&["url"], false);
    assert_schema::<CreatePresetInput>(&["instance", "expected_settings_revision", "name"], true);
    assert_schema::<RenamePresetInput>(
        &["instance", "expected_settings_revision", "name", "new_name"],
        true,
    );
    assert_schema::<DeletePresetInput>(&["instance", "expected_settings_revision", "name"], true);
    assert_schema::<SetActivePresetInput>(
        &["instance", "expected_settings_revision", "name"],
        true,
    );
    assert_schema::<SetMappingGateInput>(
        &["instance", "expected_settings_revision", "gate", "enabled"],
        true,
    );
    assert_schema::<CreateMappingRuleInput>(
        &[
            "instance",
            "expected_settings_revision",
            "preset",
            "table",
            "rule",
        ],
        true,
    );
    assert_schema::<UpdateMappingRuleInput>(
        &[
            "instance",
            "expected_settings_revision",
            "preset",
            "table",
            "index",
            "from",
            "to",
        ],
        true,
    );
    assert_schema::<DeleteMappingRuleInput>(
        &[
            "instance",
            "expected_settings_revision",
            "preset",
            "table",
            "index",
        ],
        true,
    );
    assert_schema::<MoveMappingRuleInput>(
        &[
            "instance",
            "expected_settings_revision",
            "preset",
            "table",
            "from",
            "to",
        ],
        true,
    );
    assert_schema::<SetMappingRuleEnabledInput>(
        &[
            "instance",
            "expected_settings_revision",
            "preset",
            "table",
            "index",
            "enabled",
        ],
        true,
    );
}

fn assert_schema<T: JsonSchema>(required_fields: &[&str], exact_instance: bool) {
    let schema = serde_json::to_value(schema_for!(T)).expect("JSON schema");
    assert_eq!(schema["type"], json!("object"));
    assert_eq!(schema["additionalProperties"], json!(false));
    let required = schema["required"].as_array().cloned().unwrap_or_default();
    for field in required_fields {
        assert!(
            required.contains(&json!(field)),
            "missing {field}: {schema}"
        );
    }
    let instance = resolve_schema(&schema, &schema["properties"]["instance"]);
    assert_eq!(instance["additionalProperties"], json!(false));
    if exact_instance {
        assert_eq!(instance["required"], json!(["proxy_endpoint", "run_id"]));
    } else {
        assert!(instance.get("required").is_none());
    }
}

fn resolve_schema<'a>(root: &'a Value, schema: &'a Value) -> &'a Value {
    let Some(reference) = schema.get("$ref").and_then(Value::as_str) else {
        return schema;
    };
    root.pointer(reference.trim_start_matches('#'))
        .unwrap_or_else(|| panic!("unresolved schema reference {reference}"))
}

#[test]
fn mutation_result_serialization_repeats_exact_metadata_and_affected_object() {
    let result = mapping_mutation_result(ControlResult::MutateMapping {
        instance: InstanceScope {
            proxy_endpoint: "127.0.0.1:19014".parse().expect("endpoint"),
            run_id: RunId::from_str(RUN_ID).expect("run ID"),
        },
        settings_revision: crate::runtime::settings::SettingsRevision::new(8),
        config_mode: ConfigMode::DefaultOwned,
        persistence: PersistenceMode::Persistent,
        outcome: crate::runtime::settings::SettingsTransactionOutcome::Committed,
        affected: MappingObjectRef::Rule {
            preset: "dev".to_string(),
            table: ProxyRuleTable::Remote,
            index: 2,
        },
    })
    .expect("mapping mutation result");

    assert_eq!(
        serde_json::to_value(result).expect("serialize result"),
        json!({
            "instance": instance_json(),
            "settings_revision": 8,
            "config_mode": "default_owned",
            "persistence": "persistent",
            "outcome": "committed",
            "affected": {
                "kind": "rule",
                "preset": "dev",
                "table": "remote",
                "index": 2
            }
        })
    );
}

#[test]
fn public_gate_and_rule_shapes_are_typed_not_stringly_encoded() {
    let global: MappingGateTarget = decode(json!({"kind": "global"}));
    let table: MappingGateTarget = decode(json!({
        "kind": "table", "preset": "dev", "table": "local"
    }));
    assert_eq!(global, MappingGateTarget::Global);
    assert_eq!(
        table,
        MappingGateTarget::Table {
            preset: "dev".to_string(),
            table: ProxyRuleTable::Local,
        }
    );

    let rule: MappingRuleInput = decode(json!({
        "from": "https://a.example",
        "to": "https://b.example",
        "enabled": false
    }));
    assert_eq!(rule.from, "https://a.example");
    assert_eq!(rule.to, "https://b.example");
    assert!(!rule.enabled);
}
