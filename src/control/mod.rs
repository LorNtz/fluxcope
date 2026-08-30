pub(crate) mod capture_query;

use crate::{
    capture::{CaptureSequence, CaptureSnapshot},
    control::capture_query::{CaptureSearchBatch, CaptureSearchCursor},
    control_rpc::protocol::InstanceScope,
    settings::{ConfigMode, PersistenceMode},
};
use serde::{Deserialize, Serialize};

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
