pub(crate) mod capture_query;

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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct AppControlSummary {
    pub(crate) recording_enabled: bool,
    pub(crate) retained_capture_count: usize,
    pub(crate) settings_revision: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct InstanceRuntimeSnapshot {
    pub(crate) instance: InstanceScope,
    pub(crate) config_mode: ConfigMode,
    pub(crate) persistence: PersistenceMode,
    pub(crate) recording_enabled: bool,
    pub(crate) retained_capture_count: usize,
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
    pub(crate) capture: Option<CompactCapture>,
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
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
    #[cfg(test)]
    UnsupportedForTest,
}

#[derive(Clone, Debug)]
pub(crate) enum RuntimeReply {
    Instance(InstanceRuntimeSnapshot),
    RecordingUpdated(RecordingUpdate),
    CaptureSearchBatch(CaptureSearchBatch),
    CaptureSnapshot(Box<CaptureSnapshotReply>),
}

#[cfg(test)]
impl RuntimeReply {
    pub(crate) fn instance(&self) -> &InstanceRuntimeSnapshot {
        match self {
            Self::Instance(instance) => instance,
            Self::RecordingUpdated(_) | Self::CaptureSearchBatch(_) | Self::CaptureSnapshot(_) => {
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
        let reply = RuntimeReply::Instance(expected.clone());
        let actual = reply.instance();

        assert_eq!(actual.instance, expected.instance);
        assert_eq!(actual.config_mode, ConfigMode::Temporary);
        assert_eq!(actual.persistence, PersistenceMode::Ephemeral);
        assert!(actual.recording_enabled);
        assert_eq!(actual.retained_capture_count, 3);
        assert_eq!(actual.settings_revision, 7);
    }
}
