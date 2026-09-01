pub(crate) const MCP_LOCAL_ACCESS_WARNING: &str = "MCP access is unauthenticated within the current OS user account. Any process running as this user can launch the broker, read raw retained captures from MCP-enabled Wirelens instances, and change their live proxy mappings.";

pub(crate) mod audit;
pub(crate) mod body;
pub(crate) mod capture_query;
pub(crate) mod json_walk;
pub(crate) mod settings;

use crate::{
    capture::{BodyStreamState, CaptureSequence, CaptureSnapshot},
    control::capture_query::{
        CaptureQuery, CaptureSearchBatch, CaptureSearchCursor, CompactCapture,
    },
    control_rpc::protocol::{ControlError, InstanceScope},
    settings::{ConfigMode, PersistenceMode},
};
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct AppControlSummary {
    pub(crate) recording_enabled: bool,
    pub(crate) retained_capture_count: usize,
    pub(crate) settings_revision: u64,
    pub(crate) mapping: MappingRuntimeStatus,
    pub(crate) capture_store: CaptureStoreRuntimeStatus,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub(crate) struct MappingRuntimeStatus {
    pub(crate) configured: bool,
    pub(crate) enabled: bool,
    pub(crate) active_preset: Option<String>,
    pub(crate) map_remote_enabled: Option<bool>,
    pub(crate) map_local_enabled: Option<bool>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub(crate) struct CaptureStoreRuntimeStatus {
    pub(crate) revision: u64,
    pub(crate) retained_bytes: usize,
    pub(crate) maximum_retained_bytes: usize,
    pub(crate) maximum_retained_records: usize,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub(crate) struct CaptureRuntimeMetrics {
    pub(crate) exchanges_not_admitted: u64,
    pub(crate) memory_pressure: u64,
    pub(crate) previews_per_body_limited: u64,
    pub(crate) previews_memory_limited: u64,
    pub(crate) metadata_truncated: u64,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub(crate) struct DecodeRuntimeMetrics {
    pub(crate) rejected: u64,
    pub(crate) superseded: u64,
    pub(crate) output_limited: u64,
    pub(crate) failed: u64,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub(crate) struct LoggingRuntimeMetrics {
    pub(crate) producer_dropped: u64,
    pub(crate) tui_dropped: u64,
    pub(crate) records_truncated: u64,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub(crate) struct BodyWorkRuntimeStatus {
    pub(crate) active: usize,
    pub(crate) queued: usize,
    pub(crate) queued_bytes: usize,
    pub(crate) maximum_active: usize,
    pub(crate) maximum_queued: usize,
    pub(crate) maximum_queued_bytes: usize,
    pub(crate) rejected: u64,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub(crate) struct ControlRpcRuntimeStatus {
    pub(crate) active: usize,
    pub(crate) maximum_active: usize,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub(crate) struct SearchWorkRuntimeStatus {
    pub(crate) capture_searches_active: usize,
    pub(crate) maximum_capture_searches: usize,
    pub(crate) detail_materializations_active: usize,
    pub(crate) maximum_detail_materializations: usize,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
pub(crate) struct InstanceRuntimeMetrics {
    pub(crate) capture: CaptureRuntimeMetrics,
    pub(crate) decode: DecodeRuntimeMetrics,
    pub(crate) logging: LoggingRuntimeMetrics,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct InstanceRuntimeSnapshot {
    pub(crate) instance: InstanceScope,
    pub(crate) config_mode: ConfigMode,
    pub(crate) persistence: PersistenceMode,
    pub(crate) recording_enabled: bool,
    pub(crate) retained_capture_count: usize,
    pub(crate) mapping: MappingRuntimeStatus,
    pub(crate) capture_store: CaptureStoreRuntimeStatus,
    pub(crate) metrics: InstanceRuntimeMetrics,
    pub(crate) settings_revision: u64,
}
#[derive(Clone, Debug)]
pub(crate) struct RecordingUpdate {
    pub(crate) instance: InstanceScope,
    pub(crate) previous: bool,
    pub(crate) current: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct CaptureSnapshotReply {
    pub(crate) instance: InstanceScope,
    pub(crate) snapshot: CaptureSnapshot,
}
#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CaptureMilestone {
    RequestSeen,
    ResponseStarted,
    ExchangeTerminal,
}

impl CaptureMilestone {
    pub(crate) fn is_satisfied_by(self, snapshot: &CaptureSnapshot) -> bool {
        match self {
            Self::RequestSeen => true,
            Self::ResponseStarted => snapshot.timing.time_to_response.is_some(),
            Self::ExchangeTerminal => {
                matches!(
                    snapshot.request_body.status.stream,
                    BodyStreamState::Complete
                        | BodyStreamState::Failed
                        | BodyStreamState::Cancelled
                ) && matches!(
                    snapshot.response_body.status.stream,
                    BodyStreamState::Complete
                        | BodyStreamState::Failed
                        | BodyStreamState::Cancelled
                )
            }
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct WaitForCaptureRequest {
    pub(crate) query: CaptureQuery,
    pub(crate) milestone: CaptureMilestone,
    #[serde(default)]
    pub(crate) timeout_ms: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct WaitForCaptureResult {
    pub(crate) matched: bool,
    #[serde(deserialize_with = "deserialize_required_option")]
    #[schemars(required)]
    pub(crate) capture: Option<Box<CompactCapture>>,
}

fn deserialize_required_option<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::deserialize(deserializer)
}
pub(crate) const DEFAULT_WAIT_TIMEOUT_MS: u64 = 30_000;
pub(crate) const MAX_WAIT_TIMEOUT_MS: u64 = 300_000;

pub(crate) fn normalize_wait_timeout_ms(timeout_ms: Option<u64>) -> Result<u64, ControlError> {
    let timeout_ms = timeout_ms.unwrap_or(DEFAULT_WAIT_TIMEOUT_MS);
    if timeout_ms == 0 {
        return Err(ControlError::invalid_argument(
            "wait_for_capture timeout_ms must be greater than zero",
        ));
    }
    Ok(timeout_ms.min(MAX_WAIT_TIMEOUT_MS))
}

#[derive(Debug, PartialEq)]
pub(crate) enum RuntimeRequest {
    DescribeInstance,
    GetStatus,
    SetRecordingEnabled {
        enabled: bool,
    },
    GetCaptureSearchBatch {
        cursor: Option<CaptureSearchCursor>,
        max_rows: usize,
    },
    GetCapture {
        capture_id: CaptureSequence,
        expected_revision: Option<u64>,
    },
    GetCaptureBodyMetadata {
        capture_id: CaptureSequence,
        side: crate::capture::BodySide,
    },
    GetCaptureBodySnapshot {
        capture_id: CaptureSequence,
        expected_revision: u64,
        side: crate::capture::BodySide,
    },
    GetMappingSettings,
    BeginSettingsTransaction {
        expected_revision: Option<crate::runtime::settings::SettingsRevision>,
        origin: crate::runtime::settings::SettingsTransactionOrigin,
    },
    FinalizeSettingsTransaction {
        token: crate::runtime::settings::SettingsTransactionToken,
        commit: Box<settings::FinalizedSettingsTransaction>,
    },
    AbortSettingsTransaction {
        token: crate::runtime::settings::SettingsTransactionToken,
    },
    #[cfg(test)]
    UnsupportedForTest,
}

#[derive(Clone, Debug)]
pub(crate) enum RuntimeReply {
    Instance(Box<InstanceRuntimeSnapshot>),
    RecordingUpdated(RecordingUpdate),
    CaptureSearchBatch(CaptureSearchBatch),
    CaptureSnapshot(Box<CaptureSnapshotReply>),
    CaptureBodyMetadata(Box<body::CaptureBodyMetadataReply>),
    CaptureBodySnapshot(Box<body::CaptureBodySnapshotReply>),
    MappingSettings(settings::MappingSettingsSnapshot),
    SettingsTransactionBegun(settings::BeginSettingsTransactionReply),
    SettingsTransactionFinalized(crate::runtime::settings::SettingsTransactionOutcome),
    SettingsTransactionAborted,
}

#[cfg(test)]
impl RuntimeReply {
    pub(crate) fn instance(&self) -> &InstanceRuntimeSnapshot {
        match self {
            Self::Instance(instance) => instance,
            Self::RecordingUpdated(_)
            | Self::CaptureSearchBatch(_)
            | Self::CaptureSnapshot(_)
            | Self::CaptureBodyMetadata(_)
            | Self::CaptureBodySnapshot(_)
            | Self::MappingSettings(_)
            | Self::SettingsTransactionBegun(_)
            | Self::SettingsTransactionFinalized(_)
            | Self::SettingsTransactionAborted => {
                panic!("runtime reply does not contain an instance snapshot")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{InstanceRuntimeSnapshot, RuntimeReply, RuntimeRequest};
    use crate::{
        control_rpc::protocol::InstanceScope,
        instance::InstanceIdentity,
        settings::{ConfigMode, PersistenceMode},
    };

    fn snapshot() -> InstanceRuntimeSnapshot {
        let identity = InstanceIdentity::new("127.0.0.1:19005".parse().expect("endpoint"))
            .expect("instance identity");
        InstanceRuntimeSnapshot {
            instance: InstanceScope {
                proxy_endpoint: identity.proxy_endpoint(),
                run_id: identity.run_id().clone(),
            },
            config_mode: ConfigMode::Temporary,
            persistence: PersistenceMode::Ephemeral,
            recording_enabled: true,
            retained_capture_count: 3,
            settings_revision: 7,
            mapping: Default::default(),
            capture_store: Default::default(),
            metrics: Default::default(),
        }
    }

    #[test]
    fn command_domain_starts_with_describe_instance() {
        assert_eq!(
            RuntimeRequest::DescribeInstance,
            RuntimeRequest::DescribeInstance
        );
    }

    #[test]
    fn instance_reply_exposes_the_complete_compact_runtime_snapshot() {
        let expected = snapshot();
        let reply = RuntimeReply::Instance(Box::new(expected.clone()));
        let actual = reply.instance();

        assert_eq!(actual.instance, expected.instance);
        assert_eq!(actual.config_mode, ConfigMode::Temporary);
        assert_eq!(actual.persistence, PersistenceMode::Ephemeral);
        assert!(actual.recording_enabled);
        assert_eq!(actual.retained_capture_count, 3);
        assert_eq!(actual.settings_revision, 7);
    }
}
