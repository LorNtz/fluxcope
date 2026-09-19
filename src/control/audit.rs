use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex},
};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    control_rpc::{
        protocol::{
            ControlErrorCode, ControlOperationKind, InstanceScope, MutationAuditOperationKind,
        },
        server::{ControlCallAudit, ControlCallOutcome},
    },
    settings::{ConfigMode, PersistenceMode, mapping_ops::MappingObjectRef},
};
pub(crate) const MAX_AUDIT_TARGET_NAME_BYTES: usize = 256;
const AUDIT_WINDOW_SECONDS: u64 = 60;
pub(crate) const MAX_READ_AUDIT_KEYS: usize = 256;
const MAX_MUTATION_AUDIT_RECORDS: usize = 512;

#[derive(Clone, Debug, Default, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub(crate) struct ReadLatencyBuckets {
    pub(crate) under_10_ms: u64,
    pub(crate) under_100_ms: u64,
    pub(crate) under_1_second: u64,
    pub(crate) under_10_seconds: u64,
    pub(crate) at_least_10_seconds: u64,
}

impl ReadLatencyBuckets {
    fn record(&mut self, duration: std::time::Duration) {
        let bucket = if duration < std::time::Duration::from_millis(10) {
            &mut self.under_10_ms
        } else if duration < std::time::Duration::from_millis(100) {
            &mut self.under_100_ms
        } else if duration < std::time::Duration::from_secs(1) {
            &mut self.under_1_second
        } else if duration < std::time::Duration::from_secs(10) {
            &mut self.under_10_seconds
        } else {
            &mut self.at_least_10_seconds
        };
        *bucket = bucket.saturating_add(1);
    }
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub(crate) struct ReadAuditBucket {
    pub(crate) window_started_unix_seconds: u64,
    pub(crate) proxy_endpoint: String,
    pub(crate) run_id: String,
    pub(crate) client_name: String,
    pub(crate) client_version: String,
    pub(crate) operation: Option<ControlOperationKind>,
    pub(crate) overflow: bool,
    pub(crate) calls: u64,
    pub(crate) failures: u64,
    pub(crate) cancellations: u64,
    pub(crate) bytes_returned: u64,
    pub(crate) latency: ReadLatencyBuckets,
}

#[derive(Clone, Debug)]
struct StoredReadAuditBucket {
    window_started_unix_seconds: u64,
    proxy_endpoint: Arc<str>,
    run_id: Arc<str>,
    client_name: Arc<str>,
    client_version: Arc<str>,
    operation: Option<ControlOperationKind>,
    overflow: bool,
    calls: u64,
    failures: u64,
    cancellations: u64,
    bytes_returned: u64,
    latency: ReadLatencyBuckets,
}

impl From<StoredReadAuditBucket> for ReadAuditBucket {
    fn from(bucket: StoredReadAuditBucket) -> Self {
        Self {
            window_started_unix_seconds: bucket.window_started_unix_seconds,
            proxy_endpoint: bucket.proxy_endpoint.to_string(),
            run_id: bucket.run_id.to_string(),
            client_name: bucket.client_name.to_string(),
            client_version: bucket.client_version.to_string(),
            operation: bucket.operation,
            overflow: bucket.overflow,
            calls: bucket.calls,
            failures: bucket.failures,
            cancellations: bucket.cancellations,
            bytes_returned: bucket.bytes_returned,
            latency: bucket.latency,
        }
    }
}
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub(crate) struct MutationAuditRecord {
    pub(crate) completed_unix_seconds: u64,
    pub(crate) proxy_endpoint: String,
    pub(crate) run_id: String,
    pub(crate) config_mode: Option<String>,
    pub(crate) persistence: Option<String>,
    pub(crate) client_name: String,
    pub(crate) client_version: String,
    pub(crate) operation: MutationAuditOperationKind,
    pub(crate) target: Option<MappingObjectRef>,
    pub(crate) target_truncated: bool,
    pub(crate) success: bool,
    pub(crate) prior_settings_revision: Option<u64>,
    pub(crate) resulting_settings_revision: Option<u64>,
    pub(crate) result_code: Option<ControlErrorCode>,
    pub(crate) retryable: bool,
    pub(crate) duration_micros: u64,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub(crate) struct InstanceAuditSnapshot {
    pub(crate) reads: Vec<ReadAuditBucket>,
    pub(crate) mutations: Vec<MutationAuditRecord>,
    pub(crate) response_delivery_failures: u64,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct ReadKey {
    window: u64,
    client_name: String,
    client_version: String,
    operation: ControlOperationKind,
}

#[derive(Clone, Debug, Default)]
struct KnownInstance {
    endpoint: String,
    run_id: String,
    config_mode: Option<String>,
    persistence: Option<String>,
}

#[derive(Default)]
struct AuditState {
    reads: HashMap<ReadKey, StoredReadAuditBucket>,
    overflow: HashMap<u64, StoredReadAuditBucket>,
    minimum_window: Option<u64>,
    mutations: VecDeque<Arc<MutationAuditRecord>>,
    response_delivery_failures: u64,
    known: KnownInstance,
}

#[derive(Clone, Default)]
pub(crate) struct InstanceAudit {
    state: Arc<Mutex<AuditState>>,
}

impl InstanceAudit {
    pub(crate) fn new(
        instance: InstanceScope,
        config_mode: ConfigMode,
        persistence: PersistenceMode,
    ) -> Self {
        Self {
            state: Arc::new(Mutex::new(AuditState {
                known: KnownInstance {
                    endpoint: instance.proxy_endpoint.to_string(),
                    run_id: instance.run_id.to_string(),
                    config_mode: enum_string(&config_mode),
                    persistence: enum_string(&persistence),
                },
                ..AuditState::default()
            })),
        }
    }

    pub(crate) fn record(&self, call: ControlCallAudit) {
        let minimum_window = minimum_window(call.completed_unix_seconds);
        let mutation_record = {
            let mut state = self.state.lock().expect("instance audit lock poisoned");
            update_known_instance(&mut state.known, &call.outcome);
            if call.response_delivery_failed {
                state.response_delivery_failures =
                    state.response_delivery_failures.saturating_add(1);
            }
            prune_read_windows(&mut state, minimum_window);
            if call.mutation.is_some() {
                prune_mutation_history(
                    &mut state,
                    call.completed_unix_seconds
                        .saturating_sub(AUDIT_WINDOW_SECONDS),
                );
                Some(record_mutation(&mut state, call))
            } else {
                record_read(&mut state, call);
                None
            }
        };
        if let Some(record) = mutation_record {
            persist_mutation_record(&record);
        }
    }

    pub(crate) fn snapshot_now(&self) -> InstanceAuditSnapshot {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        self.snapshot(now)
    }

    pub(crate) fn snapshot(&self, now_unix_seconds: u64) -> InstanceAuditSnapshot {
        let minimum_window = minimum_window(now_unix_seconds);
        let (stored_reads, stored_mutations, response_delivery_failures) = {
            let mut state = self.state.lock().expect("instance audit lock poisoned");
            prune_read_windows(&mut state, minimum_window);
            prune_mutation_history(
                &mut state,
                now_unix_seconds.saturating_sub(AUDIT_WINDOW_SECONDS),
            );
            let mut reads = Vec::with_capacity(state.reads.len() + state.overflow.len());
            reads.extend(state.reads.values().cloned());
            reads.extend(state.overflow.values().cloned());
            let mutations = state.mutations.iter().cloned().collect::<Vec<_>>();
            (reads, mutations, state.response_delivery_failures)
        };
        let mut reads = stored_reads
            .into_iter()
            .map(ReadAuditBucket::from)
            .collect::<Vec<_>>();
        let mutations = stored_mutations
            .into_iter()
            .map(|record| record.as_ref().clone())
            .collect();
        reads.sort_by(|left, right| {
            left.window_started_unix_seconds
                .cmp(&right.window_started_unix_seconds)
                .then_with(|| left.client_name.cmp(&right.client_name))
                .then_with(|| operation_name(left.operation).cmp(operation_name(right.operation)))
        });
        InstanceAuditSnapshot {
            reads,
            mutations,
            response_delivery_failures,
        }
    }
}

fn prune_read_windows(state: &mut AuditState, minimum_window: u64) {
    if state.minimum_window == Some(minimum_window) {
        return;
    }
    state.reads.retain(|key, _| key.window >= minimum_window);
    state.overflow.retain(|window, _| *window >= minimum_window);
    state.minimum_window = Some(minimum_window);
}

fn prune_mutation_history(state: &mut AuditState, minimum_unix_seconds: u64) {
    state
        .mutations
        .retain(|record| record.completed_unix_seconds >= minimum_unix_seconds);
}

fn update_known_instance(known: &mut KnownInstance, outcome: &ControlCallOutcome) {
    let ControlCallOutcome::Success {
        instance,
        config_mode,
        persistence,
        ..
    } = outcome
    else {
        return;
    };
    if let Some(instance) = instance {
        if known.endpoint.is_empty() {
            known.endpoint = instance.proxy_endpoint.to_string();
        }
        if known.run_id.is_empty() {
            known.run_id = instance.run_id.to_string();
        }
    }
    if known.config_mode.is_none() {
        known.config_mode = config_mode.as_ref().and_then(enum_string);
    }
    if known.persistence.is_none() {
        known.persistence = persistence.as_ref().and_then(enum_string);
    }
}

fn record_read(state: &mut AuditState, call: ControlCallAudit) {
    let window = window_start(call.completed_unix_seconds);
    let key = ReadKey {
        window,
        client_name: call.client.name.clone(),
        client_version: call.client.version.clone(),
        operation: call.operation,
    };
    if let Some(bucket) = state.reads.get_mut(&key) {
        update_read_bucket(bucket, &call);
        return;
    }
    if state.reads.len() < MAX_READ_AUDIT_KEYS {
        let mut bucket = new_read_bucket(&state.known, &call, window, false);
        update_read_bucket(&mut bucket, &call);
        state.reads.insert(key, bucket);
        return;
    }
    let known = state.known.clone();
    let bucket = state
        .overflow
        .entry(window)
        .or_insert_with(|| new_read_bucket(&known, &call, window, true));
    update_read_bucket(bucket, &call);
}

fn new_read_bucket(
    known: &KnownInstance,
    call: &ControlCallAudit,
    window: u64,
    overflow: bool,
) -> StoredReadAuditBucket {
    StoredReadAuditBucket {
        window_started_unix_seconds: window,
        proxy_endpoint: Arc::from(known.endpoint.as_str()),
        run_id: Arc::from(known.run_id.as_str()),
        client_name: if overflow {
            Arc::from("__overflow__")
        } else {
            Arc::from(call.client.name.as_str())
        },
        client_version: if overflow {
            Arc::from("")
        } else {
            Arc::from(call.client.version.as_str())
        },
        operation: (!overflow).then_some(call.operation),
        overflow,
        calls: 0,
        failures: 0,
        cancellations: 0,
        bytes_returned: 0,
        latency: ReadLatencyBuckets::default(),
    }
}

fn update_read_bucket(bucket: &mut StoredReadAuditBucket, call: &ControlCallAudit) {
    bucket.calls = bucket.calls.saturating_add(1);
    bucket.bytes_returned = bucket
        .bytes_returned
        .saturating_add(u64::try_from(call.response_bytes).unwrap_or(u64::MAX));
    if let ControlCallOutcome::Error { code, .. } = call.outcome {
        bucket.failures = bucket.failures.saturating_add(1);
        if code == ControlErrorCode::Cancelled {
            bucket.cancellations = bucket.cancellations.saturating_add(1);
        }
    }
    bucket.latency.record(call.duration);
}

fn record_mutation(state: &mut AuditState, call: ControlCallAudit) -> Arc<MutationAuditRecord> {
    let mutation = call
        .mutation
        .expect("mutation audit records include mutation metadata");
    let (success, resulting_settings_revision, result_code, retryable, target) = match call.outcome
    {
        ControlCallOutcome::Success {
            settings_revision,
            affected,
            ..
        } => (true, settings_revision, None, false, affected),
        ControlCallOutcome::Error { code, retryable } => (false, None, Some(code), retryable, None),
    };
    let (target, target_truncated) = target
        .or(mutation.target)
        .map(|target| (Some(target.target), target.truncated))
        .unwrap_or((None, false));
    let record = Arc::new(MutationAuditRecord {
        completed_unix_seconds: call.completed_unix_seconds,
        proxy_endpoint: state.known.endpoint.clone(),
        run_id: state.known.run_id.clone(),
        config_mode: state.known.config_mode.clone(),
        persistence: state.known.persistence.clone(),
        client_name: call.client.name,
        client_version: call.client.version,
        operation: mutation.operation,
        prior_settings_revision: mutation.prior_settings_revision,
        resulting_settings_revision,
        target,
        target_truncated,
        result_code,
        success,
        retryable,
        duration_micros: u64::try_from(call.duration.as_micros()).unwrap_or(u64::MAX),
    });
    if state.mutations.len() == MAX_MUTATION_AUDIT_RECORDS {
        state.mutations.pop_front();
    }
    state.mutations.push_back(record.clone());
    record
}

fn persist_mutation_record(record: &MutationAuditRecord) {
    match serde_json::to_string(record) {
        Ok(record) => log::info!(target: "fluxcope::mcp_audit", "{record}"),
        Err(error) => {
            log::error!(target: "fluxcope::mcp_audit", "failed to serialize MCP mutation audit record: {error}");
        }
    }
}

fn minimum_window(unix_seconds: u64) -> u64 {
    window_start(unix_seconds).saturating_sub(AUDIT_WINDOW_SECONDS)
}

fn window_start(unix_seconds: u64) -> u64 {
    unix_seconds / AUDIT_WINDOW_SECONDS * AUDIT_WINDOW_SECONDS
}

fn enum_string(value: &impl Serialize) -> Option<String> {
    serde_json::to_value(value)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
}

fn operation_name(operation: Option<ControlOperationKind>) -> &'static str {
    operation.map_or("", ControlOperationKind::as_str)
}

#[cfg(test)]
mod tests {
    use std::{net::SocketAddr, str::FromStr, time::Duration};

    use serde_json::json;

    use super::*;
    use crate::{
        control_rpc::{
            protocol::{ControlErrorCode, ControlOperationKind, DeclaredClient, InstanceScope},
            server::{
                BoundedAuditTarget, ControlCallAudit, ControlCallOutcome, ControlMutationAudit,
            },
        },
        instance::RunId,
        settings::mapping_ops::{MappingObjectRef, ProxyRuleTable},
        settings::{ConfigMode, PersistenceMode},
    };

    const RUN_ID: &str = "AAAAAAAAAAAAAAAAAAAAAA";

    fn instance() -> InstanceScope {
        InstanceScope {
            proxy_endpoint: SocketAddr::from(([127, 0, 0, 1], 8_989)),
            run_id: RunId::from_str(RUN_ID).expect("run ID"),
        }
    }

    fn client(name: &str) -> DeclaredClient {
        DeclaredClient {
            name: name.to_owned(),
            version: "1.0".to_owned(),
        }
    }

    fn bounded_target(target: MappingObjectRef) -> BoundedAuditTarget {
        let (target, truncated) = target.bounded_clone(MAX_AUDIT_TARGET_NAME_BYTES);
        BoundedAuditTarget { target, truncated }
    }

    fn failed_preset_mutation_call(name: &str, second: u64) -> ControlCallAudit {
        ControlCallAudit {
            response_delivery_failed: false,
            client: client("agent"),
            operation: ControlOperationKind::MutateMapping,
            mutation: Some(ControlMutationAudit {
                operation: MutationAuditOperationKind::CreatePreset,
                prior_settings_revision: Some(8),
                target: Some(bounded_target(MappingObjectRef::Preset {
                    name: name.to_owned(),
                })),
            }),
            outcome: ControlCallOutcome::Error {
                code: ControlErrorCode::MappingValidationFailed,
                retryable: false,
            },
            duration: Duration::from_millis(1),
            response_bytes: 64,
            completed_unix_seconds: second,
        }
    }
    fn read_call(client: DeclaredClient, second: u64, bytes: usize) -> ControlCallAudit {
        ControlCallAudit {
            response_delivery_failed: false,
            client,
            operation: ControlOperationKind::SearchCaptures,
            mutation: None,
            outcome: ControlCallOutcome::Success {
                instance: Some(instance()),
                config_mode: Some(ConfigMode::Temporary),
                persistence: Some(PersistenceMode::Ephemeral),
                settings_revision: None,
                affected: None,
            },
            duration: Duration::from_millis(7),
            response_bytes: bytes,
            completed_unix_seconds: second,
        }
    }

    #[test]
    fn response_delivery_failures_are_counted_without_reclassifying_domain_outcomes() {
        let audit = InstanceAudit::default();
        let mut call = read_call(client("agent"), 120, 0);
        call.response_delivery_failed = true;
        audit.record(call);

        let snapshot = audit.snapshot(120);
        assert_eq!(snapshot.response_delivery_failures, 1);
        assert_eq!(snapshot.reads.len(), 1);
        assert_eq!(snapshot.reads[0].failures, 0);
    }
    #[test]
    fn mutation_audit_excludes_traffic_and_mapping_values() {
        let audit = InstanceAudit::default();
        audit.record(ControlCallAudit {
            response_delivery_failed: false,
            client: client("agent"),
            operation: ControlOperationKind::MutateMapping,
            mutation: Some(ControlMutationAudit {
                operation: MutationAuditOperationKind::UpdateMappingRule,
                prior_settings_revision: Some(8),
                target: Some(bounded_target(MappingObjectRef::Rule {
                    preset: "dev".to_owned(),
                    table: ProxyRuleTable::Remote,
                    index: 2,
                })),
            }),
            outcome: ControlCallOutcome::Success {
                instance: Some(instance()),
                config_mode: Some(ConfigMode::DefaultOwned),
                persistence: Some(PersistenceMode::Persistent),
                settings_revision: Some(9),
                affected: Some(bounded_target(MappingObjectRef::Rule {
                    preset: "dev".to_owned(),
                    table: ProxyRuleTable::Remote,
                    index: 2,
                })),
            },
            duration: Duration::from_millis(12),
            response_bytes: 321,
            completed_unix_seconds: 120,
        });

        let snapshot = audit.snapshot(120);
        assert_eq!(snapshot.mutations.len(), 1);
        let record = &snapshot.mutations[0];
        assert_eq!(record.prior_settings_revision, Some(8));
        assert_eq!(record.resulting_settings_revision, Some(9));
        assert_eq!(record.result_code, None);
        assert!(record.success);
        let encoded = serde_json::to_string(record).expect("audit JSON");
        assert!(encoded.contains("update_mapping_rule"));
        assert!(encoded.contains("dev"));
        assert!(!encoded.contains("Authorization"));
        assert!(!encoded.contains("https://secret.example"));
        assert_eq!(audit.snapshot(180).mutations.len(), 1);
        assert!(audit.snapshot(181).mutations.is_empty());
    }

    #[test]
    fn mutation_retention_prunes_out_of_order_completions() {
        let audit = InstanceAudit::new(
            instance(),
            ConfigMode::Temporary,
            PersistenceMode::Ephemeral,
        );
        audit.record(failed_preset_mutation_call("newer", 121));
        audit.record(failed_preset_mutation_call("older", 120));

        let snapshot = audit.snapshot(181);
        assert_eq!(snapshot.mutations.len(), 1);
        assert_eq!(
            snapshot.mutations[0].target,
            Some(MappingObjectRef::Preset {
                name: "newer".to_owned()
            })
        );
    }

    #[test]
    fn failed_first_mutation_keeps_instance_identity_and_declared_target() {
        let audit = InstanceAudit::new(
            instance(),
            ConfigMode::ReadOnlyFile,
            PersistenceMode::Ephemeral,
        );
        audit.record(ControlCallAudit {
            response_delivery_failed: false,
            client: client("agent"),
            operation: ControlOperationKind::MutateMapping,
            mutation: Some(ControlMutationAudit {
                operation: MutationAuditOperationKind::UpdateMappingRule,
                prior_settings_revision: Some(8),
                target: Some(bounded_target(MappingObjectRef::Preset {
                    name: "dev".to_owned(),
                })),
            }),
            outcome: ControlCallOutcome::Error {
                code: ControlErrorCode::SettingsRevisionConflict,
                retryable: true,
            },
            duration: Duration::from_millis(4),
            response_bytes: 64,
            completed_unix_seconds: 120,
        });

        let record = &audit.snapshot(120).mutations[0];
        assert!(!record.success);
        assert_eq!(record.proxy_endpoint, "127.0.0.1:8989");
        assert_eq!(record.run_id, RUN_ID);
        assert_eq!(record.config_mode.as_deref(), Some("read_only_file"));
        assert_eq!(record.persistence.as_deref(), Some("ephemeral"));
        assert_eq!(
            record.target,
            Some(MappingObjectRef::Preset {
                name: "dev".to_owned()
            })
        );
        assert_eq!(
            record.result_code,
            Some(ControlErrorCode::SettingsRevisionConflict)
        );
        assert!(record.retryable);
    }

    #[test]
    fn reads_aggregate_in_sixty_second_buckets_across_connections() {
        let audit = InstanceAudit::default();
        audit.record(read_call(client("agent"), 120, 100));
        audit.record(read_call(client("agent"), 179, 250));
        audit.record(ControlCallAudit {
            response_delivery_failed: false,
            outcome: ControlCallOutcome::Error {
                code: ControlErrorCode::DeadlineExceeded,
                retryable: true,
            },
            ..read_call(client("agent"), 180, 0)
        });

        let snapshot = audit.snapshot(180);
        assert_eq!(snapshot.reads.len(), 2);
        let first = snapshot
            .reads
            .iter()
            .find(|bucket| bucket.window_started_unix_seconds == 120)
            .expect("first bucket");
        assert_eq!(first.calls, 2);
        assert_eq!(first.failures, 0);
        assert_eq!(first.bytes_returned, 350);
        let second = snapshot
            .reads
            .iter()
            .find(|bucket| bucket.window_started_unix_seconds == 180)
            .expect("second bucket");
        assert_eq!(second.calls, 1);
        assert_eq!(second.failures, 1);
        assert_eq!(second.cancellations, 0);
    }
    #[test]
    fn mutation_target_names_are_bounded_before_retention() {
        let audit = InstanceAudit::new(
            instance(),
            ConfigMode::Temporary,
            PersistenceMode::Ephemeral,
        );
        audit.record(ControlCallAudit {
            response_delivery_failed: false,
            client: client("agent"),
            operation: ControlOperationKind::MutateMapping,
            mutation: Some(ControlMutationAudit {
                operation: MutationAuditOperationKind::CreatePreset,
                prior_settings_revision: Some(8),
                target: Some(bounded_target(MappingObjectRef::Preset {
                    name: "x".repeat(1024 * 1024),
                })),
            }),
            outcome: ControlCallOutcome::Error {
                code: ControlErrorCode::MappingValidationFailed,
                retryable: false,
            },
            duration: Duration::from_millis(1),
            response_bytes: 64,
            completed_unix_seconds: 120,
        });

        let snapshot = audit.snapshot(120);
        let record = &snapshot.mutations[0];
        assert!(record.target_truncated);
        let MappingObjectRef::Preset { name } = record.target.as_ref().expect("target") else {
            panic!("preset target");
        };
        assert!(name.len() <= MAX_AUDIT_TARGET_NAME_BYTES);
        let state = audit.state.lock().expect("audit state");
        let MappingObjectRef::Preset { name } =
            state.mutations[0].target.as_ref().expect("retained target")
        else {
            panic!("retained preset target");
        };
        assert!(name.capacity() <= MAX_AUDIT_TARGET_NAME_BYTES);
    }

    #[test]
    fn read_aggregation_caps_keys_and_uses_one_overflow_bucket() {
        let audit = InstanceAudit::default();
        for index in 0..300 {
            audit.record(read_call(client(&format!("agent-{index}")), 120, 1));
        }

        let snapshot = audit.snapshot(120);
        assert_eq!(snapshot.reads.len(), MAX_READ_AUDIT_KEYS + 1);
        let overflow = snapshot
            .reads
            .iter()
            .find(|bucket| bucket.overflow)
            .expect("overflow bucket");
        assert_eq!(overflow.calls, 300 - MAX_READ_AUDIT_KEYS as u64);
    }

    #[test]
    fn audit_error_records_only_stable_code_and_safe_details() {
        let audit = InstanceAudit::default();
        audit.record(ControlCallAudit {
            response_delivery_failed: false,
            operation: ControlOperationKind::SetRecordingEnabled,
            mutation: Some(ControlMutationAudit {
                operation: MutationAuditOperationKind::SetRecordingEnabled,
                prior_settings_revision: Some(4),
                target: None,
            }),
            outcome: ControlCallOutcome::Error {
                code: ControlErrorCode::SettingsRevisionConflict,
                retryable: true,
            },
            ..read_call(client("agent"), 120, 0)
        });

        let snapshot = audit.snapshot(120);
        assert_eq!(snapshot.mutations.len(), 1);
        assert_eq!(
            snapshot.mutations[0].result_code,
            Some(ControlErrorCode::SettingsRevisionConflict)
        );
        assert_eq!(
            serde_json::to_value(&snapshot.mutations[0]).expect("audit value")["result_code"],
            json!("settings_revision_conflict")
        );
    }
}
