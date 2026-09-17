use std::{net::SocketAddr, path::PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    capture::{ACTIVE_BODY_WORK_LIMIT, QUEUED_BODY_INPUT_LIMIT_BYTES, QUEUED_BODY_WORK_LIMIT},
    control::{
        BodyWorkRuntimeStatus, CaptureStoreRuntimeStatus, ControlRpcRuntimeStatus,
        DEFAULT_WAIT_TIMEOUT_MS, InstanceRuntimeMetrics, MAX_WAIT_TIMEOUT_MS, MappingRuntimeStatus,
        SearchWorkRuntimeStatus,
        audit::InstanceAuditSnapshot,
        body::{
            DEFAULT_BODY_PAGE_LENGTH, DEFAULT_BODY_SEARCH_CONTEXT_BYTES, DEFAULT_BODY_SEARCH_LIMIT,
            MAX_BODY_PAGE_LENGTH, MAX_BODY_SEARCH_CONTEXT_BYTES, MAX_BODY_SEARCH_LIMIT,
            MAX_BODY_SEARCH_QUERY_BYTES, MAX_DECODED_CONTENT_BYTES, MAX_INLINE_SELECTION_BYTES,
        },
        capture_query::{DEFAULT_CAPTURE_PAGE_LIMIT, MAX_CAPTURE_PAGE_LIMIT},
        json_walk::MAX_JSON_EXAMPLES,
    },
    control_rpc::framing::{
        ACTIVE_CALL_LIMIT, REQUEST_MAX_BYTES, RESPONSE_MAX_BYTES,
        RESPONSE_SERIALIZATION_BUDGET_BYTES,
    },
    instance::RunId,
    instance_registry::{MAX_DESCRIPTOR_BYTES, MAX_SCAN_FILES},
    runtime::{
        CAPTURE_SEARCH_LIMIT, DETAIL_MATERIALIZATION_LIMIT, RUNTIME_COMMAND_CHANNEL_CAPACITY,
    },
    settings::{ConfigMode, PersistenceMode},
};

#[derive(Clone, Debug, Default, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct InstanceSelector {
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    pub(crate) proxy_endpoint: Option<SocketAddr>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    pub(crate) run_id: Option<RunId>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, JsonSchema, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct GetStatusInput {
    pub(crate) instance: InstanceSelector,
}

#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize)]
pub(crate) struct InstanceSummary {
    pub(crate) instance: InstanceSelector,
    pub(crate) local_proxy_url: String,
    pub(crate) started_at: String,
    pub(crate) fluxcope_version: String,
    pub(crate) rpc_version: u16,
    #[schemars(with = "String")]
    pub(crate) config_mode: ConfigMode,
    #[schemars(with = "String")]
    pub(crate) persistence: PersistenceMode,
    #[schemars(with = "Option<String>")]
    pub(crate) config_source: Option<PathBuf>,
    pub(crate) recording_enabled: bool,
    pub(crate) retained_capture_count: usize,
    pub(crate) settings_revision: u64,
}

#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize)]
pub(crate) struct RejectedDescriptor {
    pub(crate) code: String,
    pub(crate) message: String,
}

#[derive(Clone, Debug, Default, Eq, JsonSchema, PartialEq, Serialize)]
pub(crate) struct DiscoverySummary {
    pub(crate) stale_count: usize,
    pub(crate) rejected: Vec<RejectedDescriptor>,
    pub(crate) omitted: usize,
}

#[derive(Clone, Debug, Default, Eq, JsonSchema, PartialEq, Serialize)]
pub(crate) struct ListInstancesResult {
    pub(crate) instances: Vec<InstanceSummary>,
    pub(crate) diagnostics: DiscoverySummary,
}

#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct GetStatusResult {
    pub(crate) instance: InstanceSelector,
    pub(crate) local_proxy_url: String,
    pub(crate) fluxcope_version: String,
    pub(crate) rpc_version: u16,
    pub(crate) config_source: Option<PathBuf>,
    #[schemars(with = "String")]
    pub(crate) config_mode: ConfigMode,
    #[schemars(with = "String")]
    pub(crate) persistence: PersistenceMode,
    pub(crate) recording_enabled: bool,
    pub(crate) retained_capture_count: usize,
    pub(crate) settings_revision: u64,
    pub(crate) mapping: MappingRuntimeStatus,
    pub(crate) capture_store: CaptureStoreRuntimeStatus,
    pub(crate) capture_change_epoch: u64,
    pub(crate) metrics: InstanceRuntimeMetrics,
    pub(crate) private_rpc: ControlRpcRuntimeStatus,
    pub(crate) body_work: BodyWorkRuntimeStatus,
    pub(crate) search_work: SearchWorkRuntimeStatus,
    pub(crate) audit: InstanceAuditSnapshot,
    pub(crate) limits: BrokerLimits,
    pub(crate) warnings: Vec<StatusWarning>,
}

#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize)]
pub(crate) struct StatusWarning {
    pub(crate) code: String,
    pub(crate) message: String,
}
#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize)]
pub(crate) struct BrokerVersions {
    pub(crate) broker: String,
    pub(crate) mcp: String,
    pub(crate) rpc: u16,
}

#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize)]
pub(crate) struct RegistryStatus {
    #[schemars(with = "String")]
    pub(crate) path: PathBuf,
    pub(crate) live_instances: usize,
    pub(crate) stale_descriptors: usize,
    pub(crate) rejected_descriptors: usize,
    pub(crate) omitted_descriptors: usize,
    pub(crate) connection_failures: u64,
}

#[derive(Clone, Copy, Debug, Default, Eq, JsonSchema, PartialEq, Serialize)]
pub(crate) struct TransportStatus {
    pub(crate) active_calls: u64,
    pub(crate) saturated_calls: u64,
    pub(crate) cancelled_calls: u64,
    pub(crate) active_probes: u64,
    pub(crate) bytes_relayed: u64,
}

#[derive(Clone, Copy, Debug, Eq, JsonSchema, PartialEq, Serialize)]
pub(crate) struct BrokerLimits {
    pub(crate) registry_descriptors: usize,
    pub(crate) descriptor_bytes: usize,
    pub(crate) public_calls: usize,
    pub(crate) liveness_probes: usize,
    pub(crate) private_rpc_request_bytes: usize,
    pub(crate) private_rpc_response_bytes: usize,
    pub(crate) private_rpc_calls_per_instance: usize,
    pub(crate) private_rpc_response_budget_bytes: usize,
    pub(crate) runtime_command_channel: usize,
    pub(crate) capture_search_workers_per_instance: usize,
    pub(crate) capture_detail_workers_per_instance: usize,
    pub(crate) ordinary_deadline_ms: u64,
    pub(crate) capture_search_default_rows: usize,
    pub(crate) capture_search_maximum_rows: usize,
    pub(crate) body_resource_default_bytes: usize,
    pub(crate) body_resource_maximum_bytes: usize,
    pub(crate) body_text_matches_default: usize,
    pub(crate) body_text_matches_maximum: usize,
    pub(crate) body_text_query_bytes: usize,
    pub(crate) match_context_default_bytes: usize,
    pub(crate) match_context_maximum_bytes_per_side: usize,
    pub(crate) inline_selected_structured_value_bytes: usize,
    pub(crate) json_pointer_shape_examples: usize,
    pub(crate) wait_default_ms: u64,
    pub(crate) wait_maximum_ms: u64,
    pub(crate) body_decoded_structured_input_bytes: usize,
    pub(crate) body_jobs_active_per_instance: usize,
    pub(crate) body_jobs_queued_per_instance: usize,
    pub(crate) body_jobs_queued_input_bytes: usize,
}

impl Default for BrokerLimits {
    fn default() -> Self {
        Self {
            registry_descriptors: MAX_SCAN_FILES,
            descriptor_bytes: usize::try_from(MAX_DESCRIPTOR_BYTES)
                .expect("descriptor limit fits usize"),
            public_calls: super::broker::PUBLIC_CALL_LIMIT,
            liveness_probes: super::broker::LIVENESS_PROBE_LIMIT,
            private_rpc_request_bytes: REQUEST_MAX_BYTES,
            private_rpc_response_bytes: RESPONSE_MAX_BYTES,
            private_rpc_calls_per_instance: ACTIVE_CALL_LIMIT,
            private_rpc_response_budget_bytes: RESPONSE_SERIALIZATION_BUDGET_BYTES,
            runtime_command_channel: RUNTIME_COMMAND_CHANNEL_CAPACITY,
            capture_search_workers_per_instance: CAPTURE_SEARCH_LIMIT,
            capture_detail_workers_per_instance: DETAIL_MATERIALIZATION_LIMIT,
            ordinary_deadline_ms: 30_000,
            capture_search_default_rows: DEFAULT_CAPTURE_PAGE_LIMIT,
            capture_search_maximum_rows: MAX_CAPTURE_PAGE_LIMIT,
            body_resource_default_bytes: DEFAULT_BODY_PAGE_LENGTH,
            body_resource_maximum_bytes: MAX_BODY_PAGE_LENGTH,
            body_text_matches_default: DEFAULT_BODY_SEARCH_LIMIT,
            body_text_matches_maximum: MAX_BODY_SEARCH_LIMIT,
            body_text_query_bytes: MAX_BODY_SEARCH_QUERY_BYTES,
            match_context_default_bytes: DEFAULT_BODY_SEARCH_CONTEXT_BYTES,
            match_context_maximum_bytes_per_side: MAX_BODY_SEARCH_CONTEXT_BYTES,
            inline_selected_structured_value_bytes: MAX_INLINE_SELECTION_BYTES,
            json_pointer_shape_examples: MAX_JSON_EXAMPLES,
            wait_default_ms: DEFAULT_WAIT_TIMEOUT_MS,
            wait_maximum_ms: MAX_WAIT_TIMEOUT_MS,
            body_decoded_structured_input_bytes: MAX_DECODED_CONTENT_BYTES,
            body_jobs_active_per_instance: ACTIVE_BODY_WORK_LIMIT,
            body_jobs_queued_per_instance: QUEUED_BODY_WORK_LIMIT,
            body_jobs_queued_input_bytes: QUEUED_BODY_INPUT_LIMIT_BYTES,
        }
    }
}

#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize)]
pub(crate) struct BrokerStatusResult {
    pub(crate) versions: BrokerVersions,
    pub(crate) registry: RegistryStatus,
    pub(crate) transport: TransportStatus,
    pub(crate) limits: BrokerLimits,
    pub(crate) warnings: Vec<StatusWarning>,
}

#[cfg(test)]
mod tests {
    use std::{net::SocketAddr, path::PathBuf, str::FromStr};

    use schemars::schema_for;
    use serde_json::{Value, json};

    use super::{
        BrokerLimits, BrokerStatusResult, BrokerVersions, DiscoverySummary, GetStatusInput,
        GetStatusResult, InstanceSelector, InstanceSummary, ListInstancesResult, RegistryStatus,
        StatusWarning, TransportStatus,
    };
    use crate::{
        capture::QUEUED_BODY_INPUT_LIMIT_BYTES,
        instance::RunId,
        instance_registry::MAX_SCAN_FILES,
        runtime::{CAPTURE_SEARCH_LIMIT, RUNTIME_COMMAND_CHANNEL_CAPACITY},
        settings::{ConfigMode, PersistenceMode},
    };

    const RUN_ID: &str = "AAAAAAAAAAAAAAAAAAAAAA";

    fn endpoint() -> SocketAddr {
        "127.0.0.1:19001".parse().expect("endpoint")
    }

    fn selector() -> InstanceSelector {
        InstanceSelector {
            proxy_endpoint: Some(endpoint()),
            run_id: Some(RunId::from_str(RUN_ID).expect("run ID")),
        }
    }

    #[test]
    fn selector_accepts_omission_but_rejects_unknown_or_malformed_identity_fields() {
        assert_eq!(
            serde_json::from_value::<GetStatusInput>(json!({})).expect("omitted selector"),
            GetStatusInput {
                instance: InstanceSelector::default(),
            }
        );
        assert!(
            serde_json::from_value::<GetStatusInput>(json!({
                "instance": {"proxy_endpoint": "not-an-endpoint"}
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<GetStatusInput>(json!({
                "instance": {"run_id": "not-a-run-id"}
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<GetStatusInput>(json!({
                "instance": {},
                "unexpected": true
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<GetStatusInput>(json!({
                "instance": {"proxy_endpoint": "127.0.0.1:19001", "unexpected": true}
            }))
            .is_err()
        );
    }

    #[test]
    fn selector_schema_is_closed_and_exposes_only_endpoint_and_run_identity() {
        let schema = serde_json::to_value(schema_for!(GetStatusInput)).expect("schema JSON");
        assert_eq!(schema["type"], "object");
        assert_eq!(schema["additionalProperties"], false);
        assert!(schema["properties"]["instance"].is_object());

        let definitions = schema["$defs"].as_object().expect("schema definitions");
        let selector = definitions
            .values()
            .find(|definition| {
                definition["properties"]["proxy_endpoint"].is_object()
                    && definition["properties"]["run_id"].is_object()
            })
            .expect("instance selector definition");
        assert_eq!(selector["additionalProperties"], false);
        assert_eq!(
            selector["properties"]
                .as_object()
                .expect("selector properties")
                .keys()
                .cloned()
                .collect::<Vec<_>>(),
            vec!["proxy_endpoint", "run_id"]
        );
    }

    #[test]
    fn list_instances_shape_separates_live_instances_from_bounded_diagnostics() {
        let result = ListInstancesResult {
            instances: vec![InstanceSummary {
                instance: selector(),
                local_proxy_url: "http://127.0.0.1:19001".to_owned(),
                started_at: "2026-08-24T00:00:00Z".to_owned(),
                fluxcope_version: "9.8.7".to_owned(),
                rpc_version: crate::control_rpc::protocol::RPC_VERSION,
                config_mode: ConfigMode::Temporary,
                persistence: PersistenceMode::Ephemeral,
                config_source: None,
                recording_enabled: true,
                retained_capture_count: 12,
                settings_revision: 41,
            }],
            diagnostics: DiscoverySummary {
                stale_count: 2,
                rejected: vec![super::RejectedDescriptor {
                    code: "descriptor_invalid_json".to_owned(),
                    message: "invalid descriptor JSON".to_owned(),
                }],
                omitted: 7,
            },
        };

        assert_eq!(
            serde_json::to_value(result).expect("list result JSON"),
            json!({
                "instances": [{
                    "instance": {
                        "proxy_endpoint": "127.0.0.1:19001",
                        "run_id": RUN_ID
                    },
                    "local_proxy_url": "http://127.0.0.1:19001",
                    "started_at": "2026-08-24T00:00:00Z",
                    "fluxcope_version": "9.8.7",
                    "rpc_version": crate::control_rpc::protocol::RPC_VERSION,
                    "config_mode": "temporary",
                    "persistence": "ephemeral",
                    "config_source": null,
                    "recording_enabled": true,
                    "retained_capture_count": 12,
                    "settings_revision": 41
                }],
                "diagnostics": {
                    "stale_count": 2,
                    "rejected": [{
                        "code": "descriptor_invalid_json",
                        "message": "invalid descriptor JSON"
                    }],
                    "omitted": 7
                }
            })
        );
    }

    #[test]
    fn get_status_shape_repeats_resolved_identity_and_current_describe_fields() {
        let result = GetStatusResult {
            local_proxy_url: "http://127.0.0.1:19001".to_owned(),
            fluxcope_version: "0.1.0-test".to_owned(),
            rpc_version: crate::control_rpc::protocol::RPC_VERSION,
            config_source: None,
            instance: selector(),
            config_mode: ConfigMode::Temporary,
            persistence: PersistenceMode::Ephemeral,
            recording_enabled: false,
            retained_capture_count: 3,
            settings_revision: 9,
            mapping: Default::default(),
            capture_store: Default::default(),
            capture_change_epoch: 0,
            private_rpc: Default::default(),
            metrics: Default::default(),
            search_work: Default::default(),
            body_work: Default::default(),
            audit: Default::default(),
            limits: BrokerLimits::default(),
            warnings: Vec::new(),
        };

        assert_eq!(
            serde_json::to_value(result).expect("status JSON"),
            json!({
                "instance": {
                    "proxy_endpoint": "127.0.0.1:19001",
                    "run_id": RUN_ID
                },
                "local_proxy_url": "http://127.0.0.1:19001",
                "fluxcope_version": "0.1.0-test",
                "rpc_version": crate::control_rpc::protocol::RPC_VERSION,
                "config_source": null,
                "config_mode": "temporary",
                "persistence": "ephemeral",
                "recording_enabled": false,
                "retained_capture_count": 3,
                "mapping": {
                    "configured": false,
                    "enabled": false,
                    "active_preset": null,
                    "map_remote_enabled": null,
                    "map_local_enabled": null
                },
                "capture_store": {
                    "revision": 0,
                    "retained_bytes": 0,
                    "maximum_retained_bytes": 0,
                    "maximum_retained_records": 0
                },
                "capture_change_epoch": 0,
                "metrics": {
                    "capture": {
                        "exchanges_not_admitted": 0,
                        "memory_pressure": 0,
                        "previews_per_body_limited": 0,
                        "previews_memory_limited": 0,
                        "metadata_truncated": 0
                    },
                    "decode": {
                        "rejected": 0,
                        "superseded": 0,
                        "output_limited": 0,
                        "failed": 0
                    },
                    "logging": {
                        "producer_dropped": 0,
                        "tui_dropped": 0,
                        "records_truncated": 0
                    }
                },
                "private_rpc": {
                    "active": 0,
                    "maximum_active": 0
                },
                "body_work": {
                    "active": 0,
                    "queued": 0,
                    "queued_bytes": 0,
                    "maximum_active": 0,
                    "maximum_queued": 0,
                    "maximum_queued_bytes": 0,
                    "rejected": 0
                },
                "search_work": {
                    "capture_searches_active": 0,
                    "maximum_capture_searches": 0,
                    "detail_materializations_active": 0,
                    "maximum_detail_materializations": 0
                },
                "audit": {
                    "reads": [],
                    "response_delivery_failures": 0,
                    "mutations": []
                },
                "limits": serde_json::to_value(BrokerLimits::default()).expect("limits JSON"),
                "warnings": [],
                "settings_revision": 9
            })
        );
    }

    #[test]
    fn broker_status_contains_only_versions_registry_transport_and_fixed_limits() {
        let result = BrokerStatusResult {
            versions: BrokerVersions {
                broker: "0.1.0".to_owned(),
                mcp: "2025-03-26".to_owned(),
                rpc: 1,
            },
            registry: RegistryStatus {
                path: PathBuf::from("/home/test/.fluxcope/run/instances"),
                live_instances: 2,
                stale_descriptors: 1,
                rejected_descriptors: 3,
                omitted_descriptors: 4,
                connection_failures: 5,
            },
            transport: TransportStatus {
                active_calls: 6,
                saturated_calls: 7,
                cancelled_calls: 8,
                active_probes: 9,
                bytes_relayed: 10,
            },
            limits: BrokerLimits::default(),
            warnings: vec![StatusWarning {
                code: "local_unauthenticated_access".to_owned(),
                message: "current-user access".to_owned(),
            }],
        };
        let value = serde_json::to_value(result).expect("broker status JSON");

        assert_eq!(
            value
                .as_object()
                .expect("status object")
                .keys()
                .cloned()
                .collect::<Vec<_>>(),
            vec!["limits", "registry", "transport", "versions", "warnings"]
        );
        let limits = value["limits"].as_object().expect("limits object");
        assert_eq!(limits.len(), 29);
        assert_eq!(limits["registry_descriptors"], json!(MAX_SCAN_FILES));
        assert_eq!(limits["public_calls"], json!(32));
        assert_eq!(
            limits["runtime_command_channel"],
            json!(RUNTIME_COMMAND_CHANNEL_CAPACITY)
        );
        assert_eq!(
            limits["capture_search_workers_per_instance"],
            json!(CAPTURE_SEARCH_LIMIT)
        );
        assert_eq!(
            limits["body_jobs_queued_input_bytes"],
            json!(QUEUED_BODY_INPUT_LIMIT_BYTES)
        );
        for forbidden in [
            "captures",
            "headers",
            "bodies",
            "queries",
            "mappings",
            "recording_enabled",
            "settings_revision",
        ] {
            assert!(
                !contains_key(&value, forbidden),
                "broker status leaked {forbidden}"
            );
        }
    }

    fn contains_key(value: &Value, needle: &str) -> bool {
        match value {
            Value::Object(object) => {
                object.contains_key(needle)
                    || object.values().any(|value| contains_key(value, needle))
            }
            Value::Array(values) => values.iter().any(|value| contains_key(value, needle)),
            _ => false,
        }
    }
}
