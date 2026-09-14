use super::*;
use crate::{
    control_rpc::test_support::{ENDPOINT, RUN_ID, instance_scope, run_id, scope_value},
    settings::{
        AppSettings, ConfigMode, PersistenceMode, ProxyMapLocalRule, ProxyMapRemoteRule,
        ProxyPresetSettings, ProxySettings,
        mapping_ops::{
            MappingExplanation, MappingGateState, MappingMutation, MappingObjectRef,
            MappingRuleField, MappingValidationResult, ProxyRuleTable,
        },
    },
};
use serde_json::json;
use std::{sync::Arc, time::Instant};

fn request(operation: &str, arguments: serde_json::Value) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "protocol_version": RPC_VERSION,
        "request_id": "task-14-request",
        "run_id": RUN_ID,
        "deadline_ms": 5_000,
        "client": {"name": "mapping-test", "version": "1.0"},
        "operation": operation,
        "arguments": arguments
    }))
    .expect("request JSON")
}

fn remote_rule() -> ProxyMapRemoteRule {
    ProxyMapRemoteRule {
        from: "https://api.example.com".to_string(),
        to: "http://127.0.0.1:4310".to_string(),
        enable: false,
    }
}

fn local_rule() -> ProxyMapLocalRule {
    ProxyMapLocalRule {
        from: "https://cdn.example.com/assets".to_string(),
        to: "/tmp/assets".to_string(),
        enable: true,
    }
}

fn mutations() -> Vec<MappingMutation> {
    vec![
        MappingMutation::CreatePreset {
            name: "dev".to_string(),
            initial: Some(ProxyPresetSettings {
                name: "dev".to_string(),
                map_remote: crate::settings::ProxyMapRemoteSettings {
                    enable: false,
                    rules: vec![remote_rule()],
                },
                map_local: crate::settings::ProxyMapLocalSettings {
                    enable: true,
                    rules: vec![local_rule()],
                },
            }),
        },
        MappingMutation::RenamePreset {
            name: "dev".to_string(),
            new_name: "staging".to_string(),
        },
        MappingMutation::DeletePreset {
            name: "staging".to_string(),
        },
        MappingMutation::SetActivePreset {
            name: Some("dev".to_string()),
        },
        MappingMutation::SetGlobalEnabled { enabled: false },
        MappingMutation::SetTableEnabled {
            preset: "dev".to_string(),
            table: ProxyRuleTable::Remote,
            enabled: false,
        },
        MappingMutation::InsertRemoteRule {
            preset: "dev".to_string(),
            index: 0,
            rule: remote_rule(),
        },
        MappingMutation::AppendLocalRule {
            preset: "dev".to_string(),
            rule: local_rule(),
        },
        MappingMutation::UpdateRemoteRule {
            preset: "dev".to_string(),
            index: 0,
            from: "https://new.example.com".to_string(),
            to: "http://127.0.0.1:4320".to_string(),
        },
        MappingMutation::DeleteRule {
            preset: "dev".to_string(),
            table: ProxyRuleTable::Local,
            index: 1,
        },
        MappingMutation::MoveRule {
            preset: "dev".to_string(),
            table: ProxyRuleTable::Remote,
            from: 0,
            to: 2,
        },
        MappingMutation::SetRuleEnabled {
            preset: "dev".to_string(),
            table: ProxyRuleTable::Remote,
            index: 0,
            enabled: false,
        },
    ]
}

#[test]
fn mapping_read_validate_and_explain_decode_to_strict_typed_operations() {
    let current = ProxySettings::default();
    let cases = [
        (
            "get_mapping_settings",
            json!({}),
            ControlOperation::GetMappingSettings {
                scope: Default::default(),
            },
        ),
        (
            "validate_mapping_settings",
            json!({"proxy": current}),
            ControlOperation::ValidateMappingSettings {
                proxy: Box::new(ProxySettings::default()),
            },
        ),
        (
            "explain_mapping",
            json!({"url": "https://api.example.com/v1"}),
            ControlOperation::ExplainMapping {
                url: "https://api.example.com/v1".to_string(),
                proposed_proxy: None,
            },
        ),
        (
            "explain_mapping",
            json!({
                "url": "https://api.example.com/v1",
                "proposed_proxy": ProxySettings::default()
            }),
            ControlOperation::ExplainMapping {
                url: "https://api.example.com/v1".to_string(),
                proposed_proxy: Some(Box::new(ProxySettings::default())),
            },
        ),
    ];

    for (kind, arguments, expected) in cases {
        let decoded = decode_request_payload(&request(kind, arguments), Instant::now())
            .expect("valid mapping RPC request");
        assert_eq!(decoded.operation, expected);
    }
}

#[test]
fn every_mapping_mutation_round_trips_through_the_production_private_encoder() {
    let run_id = run_id();
    let client = DeclaredClient {
        name: "mapping-test".to_string(),
        version: "1.0".to_string(),
    };
    for mutation in mutations() {
        let expected = ControlOperation::MutateMapping {
            expected_revision: crate::runtime::settings::SettingsRevision::new(7),
            mutation: Box::new(mutation),
        };
        let envelope =
            OutboundRequestEnvelope::new("mapping-round-trip", &run_id, 5_000, &client, &expected);
        let frame = crate::control_rpc::framing::encode_json_frame(
            &envelope,
            crate::control_rpc::framing::REQUEST_MAX_BYTES,
        )
        .expect("encode mutation");
        let declared = u32::from_be_bytes(frame[..4].try_into().expect("frame prefix")) as usize;
        assert_eq!(declared, frame.len() - 4);
        let decoded = decode_request_payload(&frame[4..], Instant::now()).expect("decode mutation");

        assert_eq!(decoded.operation, expected);
        assert_eq!(
            decoded.operation.kind(),
            ControlOperationKind::MutateMapping
        );
    }
}

#[test]
fn mapping_rpc_arguments_reject_unknown_fields_and_missing_revision() {
    for (operation, arguments) in [
        ("get_mapping_settings", json!({"unexpected": true})),
        (
            "validate_mapping_settings",
            json!({"proxy": {}, "unexpected": true}),
        ),
        (
            "explain_mapping",
            json!({"url": "https://example.com", "unexpected": true}),
        ),
        (
            "mutate_mapping",
            json!({"mutation": {"kind": "delete_preset", "name": "dev"}}),
        ),
        (
            "mutate_mapping",
            json!({
                "expected_revision": 1,
                "mutation": {"kind": "delete_preset", "name": "dev"},
                "unexpected": true
            }),
        ),
    ] {
        let error = decode_request_payload(&request(operation, arguments), Instant::now())
            .expect_err("strict mapping request");
        assert_eq!(error.code(), ControlErrorCode::InvalidArgument);
    }
}

#[test]
fn mapping_rpc_arguments_reject_unknown_nested_mapping_fields() {
    let cases = [
        (
            "validate_mapping_settings",
            json!({"proxy": {"enabel": false}}),
        ),
        (
            "validate_mapping_settings",
            json!({"proxy": {"presets": [{"name": "dev", "unexpected": true}]}}),
        ),
        (
            "validate_mapping_settings",
            json!({
                "proxy": {
                    "presets": [{
                        "name": "dev",
                        "map_remote": {"enabel": false}
                    }]
                }
            }),
        ),
        (
            "validate_mapping_settings",
            json!({
                "proxy": {
                    "presets": [{
                        "name": "dev",
                        "map_remote": {
                            "rules": [{
                                "from": "https://a.example",
                                "to": "https://b.example",
                                "unexpected": true
                            }]
                        }
                    }]
                }
            }),
        ),
        (
            "explain_mapping",
            json!({
                "url": "https://a.example",
                "proposed_proxy": {"enabel": false}
            }),
        ),
        (
            "mutate_mapping",
            json!({
                "expected_revision": 1,
                "mutation": {
                    "kind": "create_preset",
                    "name": "dev",
                    "initial": {"map_remote": {"enabel": false}}
                }
            }),
        ),
        (
            "mutate_mapping",
            json!({
                "expected_revision": 1,
                "mutation": {
                    "kind": "insert_remote_rule",
                    "preset": "dev",
                    "index": 0,
                    "rule": {
                        "from": "https://a.example",
                        "to": "https://b.example",
                        "enabel": false
                    }
                }
            }),
        ),
    ];

    for (operation, arguments) in cases {
        let error = decode_request_payload(&request(operation, arguments), Instant::now())
            .expect_err("unknown nested mapping field");
        assert_eq!(error.code(), ControlErrorCode::InvalidArgument);
    }
}

#[test]
fn mapping_results_have_exact_identity_mode_persistence_revision_and_affected_shapes() {
    let mutation = ControlResult::MutateMapping {
        instance: instance_scope(),
        settings_revision: crate::runtime::settings::SettingsRevision::new(8),
        config_mode: ConfigMode::DefaultOwned,
        persistence: PersistenceMode::Persistent,
        outcome: crate::runtime::settings::SettingsTransactionOutcome::Committed,
        affected: MappingObjectRef::RuleField {
            preset: "dev".to_string(),
            table: ProxyRuleTable::Remote,
            index: 2,
            field: MappingRuleField::To,
        },
    };
    assert_eq!(
        serde_json::to_value(mutation).expect("mutation result"),
        json!({
            "operation": "mutate_mapping",
            "instance": scope_value(ENDPOINT, RUN_ID),
            "settings_revision": 8,
            "config_mode": "default_owned",
            "persistence": "persistent",
            "outcome": "committed",
            "affected": {
                "kind": "rule_field",
                "preset": "dev",
                "table": "remote",
                "index": 2,
                "field": "to"
            }
        })
    );
}

fn large_proxy(rule_count: usize) -> ProxySettings {
    let mut preset = ProxyPresetSettings {
        name: "large".to_string(),
        ..ProxyPresetSettings::default()
    };
    preset.map_remote.rules = (0..rule_count)
        .map(|index| ProxyMapRemoteRule {
            from: format!(
                "https://source-{index}.example.com/a/long/path/used/to/stress/the/private/wire"
            ),
            to: format!(
                "https://target-{index}.example.com/a/long/path/used/to/stress/the/private/wire"
            ),
            enable: true,
        })
        .collect();
    ProxySettings {
        active_preset: Some("large".to_string()),
        presets: vec![preset],
        ..ProxySettings::default()
    }
}

#[test]
fn oversized_mapping_request_is_rejected_by_one_pass_capped_framing() {
    let operation = ControlOperation::ValidateMappingSettings {
        proxy: Box::new(large_proxy(10_000)),
    };
    let client = DeclaredClient {
        name: "mapping-test".to_string(),
        version: "1.0".to_string(),
    };
    let run_id = run_id();
    let envelope =
        OutboundRequestEnvelope::new("oversized-mapping", &run_id, 5_000, &client, &operation);
    let error = crate::control_rpc::framing::encode_json_frame(
        &envelope,
        crate::control_rpc::framing::REQUEST_MAX_BYTES,
    )
    .expect_err("mapping request must be capped while it is encoded");

    assert_eq!(error.code(), ControlErrorCode::RpcFrameTooLarge);
}

#[test]
fn large_mapping_snapshot_retains_worker_permit_through_serialization() {
    let settings = Arc::new(AppSettings {
        proxy: Some(large_proxy(10_000)),
        ..AppSettings::default()
    });
    let admission = Arc::new(tokio::sync::Semaphore::new(1));
    let permit = Arc::clone(&admission)
        .try_acquire_owned()
        .expect("mapping worker permit");
    let result = ControlResult::GetMappingSettings {
        instance: instance_scope(),
        settings_revision: crate::runtime::settings::SettingsRevision::new(7),
        config_mode: ConfigMode::Temporary,
        persistence: PersistenceMode::Ephemeral,
        proxy: MappingSettingsPayload::from_snapshot(
            crate::control::settings::mapping::MappingSettingsView::scoped(
                settings.proxy.as_ref().expect("proxy"),
                Default::default(),
            )
            .expect("full view"),
            Arc::new(permit),
        ),
    };

    let encoded = serde_json::to_vec(&result).expect("large mapping result");
    assert!(
        encoded.len() > 1_000_000,
        "fixture must exercise a large result"
    );
    assert!(encoded.len() < 8 * 1024 * 1024, "private response limit");
    assert!(
        Arc::clone(&admission).try_acquire_owned().is_err(),
        "serialization must retain worker admission"
    );

    drop(result);
    drop(
        Arc::clone(&admission)
            .try_acquire_owned()
            .expect("worker permit released after result delivery"),
    );
}

#[test]
fn validate_and_explain_results_are_strict_tagged_settings_results() {
    let validate = ControlResult::ValidateMappingSettings {
        instance: instance_scope(),
        settings_revision: crate::runtime::settings::SettingsRevision::new(7),
        config_mode: ConfigMode::Temporary,
        persistence: PersistenceMode::Ephemeral,
        validation: Box::new(MappingValidationResult {
            gates: MappingGateState::default(),
            diagnostics: Vec::new(),
            diagnostics_total: 0,
            diagnostics_omitted: 0,
        }),
    };
    assert_eq!(
        serde_json::to_value(validate).expect("validation result"),
        json!({
            "operation": "validate_mapping_settings",
            "instance": scope_value(ENDPOINT, RUN_ID),
            "settings_revision": 7,
            "config_mode": "temporary",
            "persistence": "ephemeral",
            "validation": {
                "gates": {
                    "proxy_present": false,
                    "global_enabled": false,
                    "active_preset": null,
                    "remote_enabled": null,
                    "local_enabled": null
                },
                "diagnostics": [],
                "diagnostics_total": 0,
                "diagnostics_omitted": 0
            }
        })
    );

    let explain = ControlResult::ExplainMapping {
        instance: instance_scope(),
        settings_revision: crate::runtime::settings::SettingsRevision::new(7),
        config_mode: ConfigMode::Temporary,
        persistence: PersistenceMode::Ephemeral,
        explanation: Box::new(MappingExplanation {
            original_url: Some("https://api.example.com/v1".parse().expect("original URI")),
            effective_url: Some("http://127.0.0.1:4310/v1".parse().expect("effective URI")),
            local_path: None,
            matches: Vec::new(),
            gates: MappingGateState::default(),
            diagnostics: Vec::new(),
            diagnostics_total: 0,
            diagnostics_omitted: 0,
        }),
    };
    let value = serde_json::to_value(explain).expect("explanation result");
    assert_eq!(value["operation"], json!("explain_mapping"));
    assert_eq!(value["instance"], scope_value(ENDPOINT, RUN_ID));
    assert_eq!(value["settings_revision"], json!(7));
    assert_eq!(value["config_mode"], json!("temporary"));
    assert_eq!(value["persistence"], json!("ephemeral"));
    assert_eq!(
        value["explanation"]["original_url"],
        json!("https://api.example.com/v1")
    );
    assert_eq!(
        value["explanation"]["effective_url"],
        json!("http://127.0.0.1:4310/v1")
    );
    assert_eq!(value["explanation"]["matches"], json!([]));
    assert_eq!(value["explanation"]["diagnostics"], json!([]));
}

#[test]
fn settings_conflict_error_codes_are_stable_private_rpc_values() {
    assert_eq!(
        serde_json::to_value(ControlErrorCode::SettingsRevisionConflict)
            .expect("revision conflict code"),
        json!("settings_revision_conflict")
    );
    assert_eq!(
        serde_json::to_value(ControlErrorCode::TuiDraftConflict).expect("draft conflict code"),
        json!("tui_draft_conflict")
    );
    assert_eq!(
        serde_json::to_value(ControlErrorCode::MappingValidationFailed)
            .expect("mapping validation code"),
        json!("mapping_validation_failed")
    );
}
