use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize, de};

use crate::{
    control::capture_query::{
        CaptureQuery, CaptureSearchCursor, CompactCapture, CompiledCaptureQuery,
        normalize_capture_page_limit,
    },
    control_rpc::protocol::{ControlError, ControlOperation, ControlResult},
};

use super::schema::InstanceSelector;

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct SetRecordingEnabledInput {
    #[serde(default)]
    pub(crate) instance: InstanceSelector,
    pub(crate) enabled: bool,
}

impl SetRecordingEnabledInput {
    pub(crate) fn validate(&self) -> Result<(), ControlError> {
        if self.instance.proxy_endpoint.is_none() || self.instance.run_id.is_none() {
            return Err(ControlError::invalid_argument(
                "set_recording_enabled requires instance.proxy_endpoint and instance.run_id",
            ));
        }
        Ok(())
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

#[derive(Clone, Debug, Default, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SearchCapturesInput {
    #[schemars(default)]
    pub(crate) instance: InstanceSelector,
    #[schemars(default)]
    pub(crate) query: CaptureQuery,
    pub(crate) cursor: Option<CaptureSearchCursor>,
    pub(crate) limit: Option<usize>,
}

#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RawSearchCapturesInput {
    instance: InstanceSelector,
    query: CaptureQuery,
    cursor: Option<CaptureSearchCursor>,
    limit: Option<usize>,
}

impl Default for RawSearchCapturesInput {
    fn default() -> Self {
        Self {
            instance: InstanceSelector::default(),
            query: CaptureQuery::default(),
            cursor: None,
            limit: None,
        }
    }
}

impl<'de> Deserialize<'de> for SearchCapturesInput {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = RawSearchCapturesInput::deserialize(deserializer)?;
        CompiledCaptureQuery::compile(raw.query.clone())
            .map_err(|error| de::Error::custom(error.message()))?;
        normalize_capture_page_limit(raw.limit)
            .map_err(|error| de::Error::custom(error.message()))?;
        Ok(Self {
            instance: raw.instance,
            query: raw.query,
            cursor: raw.cursor,
            limit: raw.limit,
        })
    }
}

impl SearchCapturesInput {
    pub(crate) fn operation(&self) -> ControlOperation {
        ControlOperation::SearchCaptures {
            query: self.query.clone(),
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

#[cfg(test)]
mod tests;
