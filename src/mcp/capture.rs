use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    control::{
        CaptureMilestone, WaitForCaptureRequest,
        WaitForCaptureResult as DomainWaitForCaptureResult,
        capture_query::{
            CaptureQuery, CaptureSearchCursor, CompactCapture, normalize_capture_page_limit,
        },
        normalize_wait_timeout_ms,
    },
    control_rpc::{
        framing::REQUEST_MAX_BYTES,
        protocol::{ControlError, ControlOperation, ControlResult},
    },
};

use super::schema::InstanceSelector;

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RequiredInstanceSelector {
    pub(crate) proxy_endpoint: std::net::SocketAddr,
    #[schemars(with = "String")]
    pub(crate) run_id: crate::instance::RunId,
}

impl RequiredInstanceSelector {
    pub(crate) fn selector(&self) -> InstanceSelector {
        InstanceSelector {
            proxy_endpoint: Some(self.proxy_endpoint),
            run_id: Some(self.run_id.clone()),
        }
    }
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SetRecordingEnabledInput {
    pub(crate) instance: RequiredInstanceSelector,
    pub(crate) enabled: bool,
}

impl SetRecordingEnabledInput {
    pub(crate) fn validate(&self) -> Result<(), ControlError> {
        validate_public_input_size(self)
    }

    pub(crate) fn operation(&self) -> ControlOperation {
        ControlOperation::SetRecordingEnabled {
            enabled: self.enabled,
        }
    }
}

#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SetRecordingEnabledResult {
    pub(crate) instance: InstanceSelector,
    pub(crate) previous: bool,
    pub(crate) current: bool,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct SearchCapturesInput {
    #[schemars(default)]
    pub(crate) instance: InstanceSelector,
    #[schemars(default)]
    pub(crate) query: CaptureQuery,
    pub(crate) cursor: Option<CaptureSearchCursor>,
    pub(crate) limit: Option<usize>,
}

impl SearchCapturesInput {
    pub(crate) fn validate(&self) -> Result<(), ControlError> {
        validate_public_input_size(self)?;
        self.query.validate()?;
        normalize_capture_page_limit(self.limit).map(drop)
    }

    pub(crate) fn operation(&self) -> ControlOperation {
        ControlOperation::SearchCaptures {
            query: Box::new(self.query.clone()),
            cursor: self.cursor,
            limit: self.limit,
        }
    }
}

#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SearchCapturesResult {
    pub(crate) instance: InstanceSelector,
    pub(crate) captures: Vec<CompactCapture>,
    pub(crate) next_cursor: Option<CaptureSearchCursor>,
}
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct WaitForCaptureInput {
    pub(crate) instance: RequiredInstanceSelector,
    #[serde(default)]
    #[schemars(default)]
    pub(crate) query: CaptureQuery,
    pub(crate) milestone: CaptureMilestone,
    #[schemars(range(min = 1, max = 300_000))]
    pub(crate) timeout_ms: Option<u64>,
}

impl WaitForCaptureInput {
    pub(crate) fn normalize(mut self) -> Result<Self, ControlError> {
        validate_public_input_size(&self)?;
        self.query.validate()?;
        self.timeout_ms = Some(normalize_wait_timeout_ms(self.timeout_ms)?);
        Ok(self)
    }

    pub(crate) fn selector(&self) -> InstanceSelector {
        self.instance.selector()
    }

    pub(crate) fn operation(&self) -> ControlOperation {
        ControlOperation::WaitForCapture(Box::new(WaitForCaptureRequest {
            query: self.query.clone(),
            milestone: self.milestone,
            timeout_ms: self.timeout_ms,
        }))
    }
}

#[derive(Clone, Debug, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct WaitForCaptureResult {
    pub(crate) instance: InstanceSelector,
    pub(crate) matched: bool,
    #[schemars(required)]
    pub(crate) capture: Option<CompactCapture>,
}

fn validate_public_input_size(input: &impl Serialize) -> Result<(), ControlError> {
    #[derive(Default)]
    struct ByteCounter(usize);
    impl std::io::Write for ByteCounter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0 = self.0.saturating_add(bytes.len());
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    let mut counter = ByteCounter::default();
    serde_json::to_writer(&mut counter, input)
        .map_err(|_| ControlError::invalid_argument("capture tool input is not serializable"))?;
    if counter.0 > REQUEST_MAX_BYTES.saturating_sub(4 * 1024) {
        Err(ControlError::new(
            crate::control_rpc::protocol::ControlErrorCode::InvalidArgument,
            "capture tool input exceeds the private request byte limit",
            false,
            serde_json::json!({
                "max_bytes": REQUEST_MAX_BYTES,
                "received_bytes": counter.0,
            }),
        ))
    } else {
        Ok(())
    }
}

pub(crate) fn recording_result(
    result: ControlResult,
) -> Result<SetRecordingEnabledResult, ControlError> {
    match result {
        ControlResult::SetRecordingEnabled {
            instance,
            previous,
            current,
        } => Ok(SetRecordingEnabledResult {
            instance: InstanceSelector {
                proxy_endpoint: Some(instance.proxy_endpoint),
                run_id: Some(instance.run_id),
            },
            previous,
            current,
        }),
        _ => Err(ControlError::internal(
            "private RPC returned an unexpected recording result",
        )),
    }
}

pub(crate) fn search_result(result: ControlResult) -> Result<SearchCapturesResult, ControlError> {
    match result {
        ControlResult::SearchCaptures {
            instance,
            captures,
            next_cursor,
        } => Ok(SearchCapturesResult {
            instance: InstanceSelector {
                proxy_endpoint: Some(instance.proxy_endpoint),
                run_id: Some(instance.run_id),
            },
            captures,
            next_cursor,
        }),
        _ => Err(ControlError::internal(
            "private RPC returned an unexpected capture search result",
        )),
    }
}

pub(crate) fn wait_result(result: ControlResult) -> Result<WaitForCaptureResult, ControlError> {
    match result {
        ControlResult::WaitForCapture {
            instance,
            result: DomainWaitForCaptureResult { matched, capture },
        } if matched == capture.is_some() => Ok(WaitForCaptureResult {
            instance: InstanceSelector {
                proxy_endpoint: Some(instance.proxy_endpoint),
                run_id: Some(instance.run_id),
            },
            matched,
            capture: capture.map(|capture| *capture),
        }),
        ControlResult::WaitForCapture { .. } => Err(ControlError::internal(
            "private RPC returned an inconsistent capture wait result",
        )),
        _ => Err(ControlError::internal(
            "private RPC returned an unexpected capture wait result",
        )),
    }
}

#[cfg(test)]
mod tests;
