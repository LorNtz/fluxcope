use std::{net::SocketAddr, path::PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    instance::RunId,
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
    pub(crate) wirelens_version: String,
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
pub(crate) struct GetStatusResult {
    pub(crate) instance: InstanceSelector,
    #[schemars(with = "String")]
    pub(crate) config_mode: ConfigMode,
    #[schemars(with = "String")]
    pub(crate) persistence: PersistenceMode,
    pub(crate) recording_enabled: bool,
    pub(crate) retained_capture_count: usize,
    pub(crate) settings_revision: u64,
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
    pub(crate) ordinary_deadline_ms: u64,
}

impl Default for BrokerLimits {
    fn default() -> Self {
        Self {
            registry_descriptors: 256,
            descriptor_bytes: 64 * 1024,
            public_calls: 32,
            liveness_probes: 16,
            ordinary_deadline_ms: 30_000,
        }
    }
}

#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize)]
pub(crate) struct BrokerStatusResult {
    pub(crate) versions: BrokerVersions,
    pub(crate) registry: RegistryStatus,
    pub(crate) transport: TransportStatus,
    pub(crate) limits: BrokerLimits,
}

#[cfg(test)]
mod tests {
    use std::{net::SocketAddr, path::PathBuf, str::FromStr};

    use schemars::schema_for;
    use serde_json::{Value, json};

    use super::{
        BrokerLimits, BrokerStatusResult, BrokerVersions, DiscoverySummary, GetStatusInput,
        GetStatusResult, InstanceSelector, InstanceSummary, ListInstancesResult, RegistryStatus,
        TransportStatus,
    };
    use crate::{
        instance::RunId,
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
                wirelens_version: "9.8.7".to_owned(),
                rpc_version: 1,
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
                    "wirelens_version": "9.8.7",
                    "rpc_version": 1,
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
            instance: selector(),
            config_mode: ConfigMode::Temporary,
            persistence: PersistenceMode::Ephemeral,
            recording_enabled: false,
            retained_capture_count: 3,
            settings_revision: 9,
        };

        assert_eq!(
            serde_json::to_value(result).expect("status JSON"),
            json!({
                "instance": {
                    "proxy_endpoint": "127.0.0.1:19001",
                    "run_id": RUN_ID
                },
                "config_mode": "temporary",
                "persistence": "ephemeral",
                "recording_enabled": false,
                "retained_capture_count": 3,
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
                path: PathBuf::from("/home/test/.wirelens/run/instances"),
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
        };
        let value = serde_json::to_value(result).expect("broker status JSON");

        assert_eq!(
            value
                .as_object()
                .expect("status object")
                .keys()
                .cloned()
                .collect::<Vec<_>>(),
            vec!["limits", "registry", "transport", "versions"]
        );
        assert_eq!(
            value["limits"],
            json!({
                "registry_descriptors": 256,
                "descriptor_bytes": 65_536,
                "public_calls": 32,
                "liveness_probes": 16,
                "ordinary_deadline_ms": 30_000
            })
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
