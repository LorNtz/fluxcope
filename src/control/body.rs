use bytes::Bytes;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::{
    capture::{
        BodyPreviewLimit, BodySide, BodyStatus, BodyStreamState, CaptureSequence,
        CapturedBodyPreview, CapturedHeaders,
    },
    control_rpc::protocol::{ControlError, ControlErrorCode, InstanceScope},
};
pub(crate) const MAX_CONTENT_ENCODING_LAYERS: usize = 8;

pub(crate) const DEFAULT_BODY_PAGE_LENGTH: usize = 8 * 1_024;
pub(crate) const MAX_BODY_PAGE_LENGTH: usize = 64 * 1_024;
pub(crate) const MAX_DECODED_CONTENT_BYTES: usize = 16 * 1_024 * 1_024;
pub(crate) const MAX_TERMINAL_DECODED_CACHE_BYTES: usize = 32 * 1_024 * 1_024;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum BodyRepresentation {
    Raw,
    Decoded,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BodyContentRequest {
    pub(crate) capture_id: CaptureSequence,
    pub(crate) capture_revision: u64,
    pub(crate) side: BodySide,
    pub(crate) representation: BodyRepresentation,
    pub(crate) offset: usize,
    pub(crate) length: usize,
}

impl BodyContentRequest {
    pub(crate) fn validate(&self) -> Result<(), ControlError> {
        if self.length == 0 {
            return Err(ControlError::new(
                ControlErrorCode::InvalidArgument,
                "body page length must be at least one byte",
                false,
                serde_json::json!({"field": "length", "minimum": 1, "received": 0}),
            ));
        }
        if self.length > MAX_BODY_PAGE_LENGTH {
            return Err(ControlError::new(
                ControlErrorCode::InvalidArgument,
                "body page length exceeds the maximum",
                false,
                serde_json::json!({
                    "field": "length",
                    "maximum": MAX_BODY_PAGE_LENGTH,
                    "received": self.length,
                }),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BodyRange {
    pub(crate) offset: usize,
    pub(crate) length: usize,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BodyPageSource {
    pub(crate) stream: BodyStreamState,
    pub(crate) observed_bytes: u64,
    pub(crate) retained_bytes: usize,
    pub(crate) truncated: bool,
    pub(crate) truncation_reason: Option<BodyPreviewLimit>,
    pub(crate) decoded_encoding_chain: Vec<String>,
    pub(crate) decoded_output_limited: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BodyPage {
    #[serde(with = "base64_bytes")]
    pub(crate) content: Bytes,
    pub(crate) media_type: Option<String>,
    pub(crate) requested_range: BodyRange,
    pub(crate) actual_range: BodyRange,
    pub(crate) total_bytes: usize,
    pub(crate) next_offset: Option<usize>,
    pub(crate) source: BodyPageSource,
}

#[derive(Clone, Debug)]
pub(crate) struct CaptureBodyMetadataReply {
    pub(crate) instance: InstanceScope,
    pub(crate) capture_id: CaptureSequence,
    pub(crate) capture_revision: u64,
    pub(crate) side: BodySide,
    pub(crate) status: BodyStatus,
    pub(crate) headers: CapturedHeaders,
    pub(crate) retained_bytes: usize,
}

#[derive(Clone, Debug)]
pub(crate) struct CaptureBodySnapshotReply {
    pub(crate) instance: InstanceScope,
    pub(crate) capture_id: CaptureSequence,
    pub(crate) capture_revision: u64,
    pub(crate) side: BodySide,
    pub(crate) status: BodyStatus,
    pub(crate) headers: CapturedHeaders,
    pub(crate) retained_bytes: usize,
    pub(crate) preview: CapturedBodyPreview,
}

pub(crate) fn media_type(headers: &CapturedHeaders) -> Option<String> {
    headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("content-type"))
        .map(|(_, value)| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

mod base64_bytes {
    use super::*;
    use base64::{Engine as _, engine::general_purpose::STANDARD};

    pub(super) fn serialize<S>(bytes: &Bytes, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&STANDARD.encode(bytes))
    }

    pub(super) fn deserialize<'de, D>(deserializer: D) -> Result<Bytes, D::Error>
    where
        D: Deserializer<'de>,
    {
        let encoded = String::deserialize(deserializer)?;
        STANDARD
            .decode(encoded)
            .map(Bytes::from)
            .map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod task10_tests;
