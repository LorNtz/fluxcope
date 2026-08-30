use std::{
    io,
    net::SocketAddr,
    str::FromStr,
    sync::atomic::{AtomicBool, Ordering},
};

use bytes::Bytes;
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use unicode_casefold::UnicodeCaseFold;
use url::{Url, form_urlencoded};

use crate::{
    capture::{
        BodyPreviewLimit, BodySide, BodyStatus, BodyStreamState, CaptureSequence,
        CapturedBodyPreview, CapturedHeaders,
    },
    control::json_walk::{JsonPointer, extract_json_selection},
    control_rpc::protocol::{ControlError, ControlErrorCode, InstanceScope},
    instance::RunId,
};
pub(crate) const MAX_CONTENT_ENCODING_LAYERS: usize = 8;

pub(crate) const DEFAULT_BODY_PAGE_LENGTH: usize = 8 * 1_024;
pub(crate) const MAX_BODY_PAGE_LENGTH: usize = 64 * 1_024;
pub(crate) const MAX_DECODED_CONTENT_BYTES: usize = 16 * 1_024 * 1_024;
pub(crate) const MAX_TERMINAL_DECODED_CACHE_BYTES: usize = 32 * 1_024 * 1_024;
pub(crate) const DEFAULT_BODY_SEARCH_LIMIT: usize = 10;
pub(crate) const MAX_BODY_SEARCH_LIMIT: usize = 50;
pub(crate) const DEFAULT_BODY_SEARCH_CONTEXT_BYTES: usize = 160;
pub(crate) const MAX_BODY_SEARCH_CONTEXT_BYTES: usize = 1_024;
pub(crate) const MAX_BODY_SEARCH_QUERY_BYTES: usize = 8 * 1_024;
pub(crate) const MAX_FORM_FIELD_KEY_BYTES: usize = 4 * 1_024;
pub(crate) const MAX_INLINE_SELECTION_BYTES: usize = 4 * 1_024;
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SearchCaptureBodyRequest {
    pub(crate) capture_id: CaptureSequence,
    pub(crate) capture_revision: u64,
    #[schemars(with = "String")]
    pub(crate) side: BodySide,
    #[schemars(length(min = 1, max = 8192))]
    pub(crate) query: String,
    #[schemars(range(min = 1, max = 50))]
    pub(crate) limit: usize,
    #[schemars(range(min = 0, max = 1024))]
    pub(crate) context_bytes: usize,
}

impl SearchCaptureBodyRequest {
    pub(crate) fn validate(&self) -> Result<(), ControlError> {
        if self.query.is_empty() {
            return Err(invalid_body_argument(
                "body search query must not be empty",
                serde_json::json!({"field": "query"}),
            ));
        }
        if self.query.len() > MAX_BODY_SEARCH_QUERY_BYTES {
            return Err(invalid_body_argument(
                "body search query exceeds the byte limit",
                serde_json::json!({
                    "field": "query",
                    "maximum_bytes": MAX_BODY_SEARCH_QUERY_BYTES,
                    "received_bytes": self.query.len(),
                }),
            ));
        }
        if !(1..=MAX_BODY_SEARCH_LIMIT).contains(&self.limit) {
            return Err(invalid_body_argument(
                "body search match limit is out of range",
                serde_json::json!({
                    "field": "limit",
                    "minimum": 1,
                    "maximum": MAX_BODY_SEARCH_LIMIT,
                    "received": self.limit,
                }),
            ));
        }
        if self.context_bytes > MAX_BODY_SEARCH_CONTEXT_BYTES {
            return Err(invalid_body_argument(
                "body search context is out of range",
                serde_json::json!({
                    "field": "context_bytes",
                    "minimum": 0,
                    "maximum": MAX_BODY_SEARCH_CONTEXT_BYTES,
                    "received": self.context_bytes,
                }),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum ExtractSelector {
    JsonPointer { pointer: JsonPointer },
    FormField { key: String },
}

impl ExtractSelector {
    pub(crate) fn validate(&self) -> Result<(), ControlError> {
        if let Self::FormField { key } = self
            && (key.is_empty() || key.len() > MAX_FORM_FIELD_KEY_BYTES)
        {
            return Err(invalid_body_argument(
                "form field key is out of range",
                serde_json::json!({
                    "field": "selector.key",
                    "minimum_bytes": 1,
                    "maximum_bytes": MAX_FORM_FIELD_KEY_BYTES,
                    "received_bytes": key.len(),
                }),
            ));
        }
        Ok(())
    }

    pub(crate) fn kind(&self) -> ExtractSelectorKind {
        match self {
            Self::JsonPointer { .. } => ExtractSelectorKind::JsonPointer,
            Self::FormField { .. } => ExtractSelectorKind::FormField,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ExtractSelectorKind {
    JsonPointer,
    FormField,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ExtractCaptureBodyRequest {
    pub(crate) capture_id: CaptureSequence,
    pub(crate) capture_revision: u64,
    #[schemars(with = "String")]
    pub(crate) side: BodySide,
    pub(crate) selector: ExtractSelector,
}

impl ExtractCaptureBodyRequest {
    pub(crate) fn validate(&self) -> Result<(), ControlError> {
        self.selector.validate()
    }
}

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

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
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
#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BodySearchMatch {
    pub(crate) original_range: BodyRange,
    pub(crate) matched: String,
    pub(crate) context_before: String,
    pub(crate) context_after: String,
    pub(crate) resource_uri: String,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SearchCaptureBodyResult {
    pub(crate) capture_revision: u64,
    pub(crate) total_matches: usize,
    pub(crate) omitted_matches: usize,
    pub(crate) decoded_bytes_inspected: usize,
    pub(crate) matches: Vec<BodySearchMatch>,
    #[schemars(with = "String")]
    pub(crate) stream: BodyStreamState,
    pub(crate) observed_bytes: u64,
    pub(crate) retained_bytes: usize,
    pub(crate) source_truncated: bool,
    #[schemars(with = "Option<String>")]
    pub(crate) source_truncation_reason: Option<BodyPreviewLimit>,
    pub(crate) decoded_encoding_chain: Vec<String>,
    pub(crate) decoded_output_limited: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct BodyTextSearch {
    pub(crate) total_matches: usize,
    pub(crate) omitted_matches: usize,
    pub(crate) matches: Vec<BodySearchMatch>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ExtractCaptureBodyResult {
    pub(crate) capture_revision: u64,
    pub(crate) selector_kind: ExtractSelectorKind,
    pub(crate) selected_bytes: usize,
    pub(crate) media_type: String,
    #[serde(deserialize_with = "deserialize_required_option")]
    #[schemars(required)]
    pub(crate) inline: Option<String>,
    #[serde(deserialize_with = "deserialize_required_option")]
    #[schemars(required)]
    pub(crate) resource_uri: Option<String>,
    #[schemars(with = "String")]
    pub(crate) stream: BodyStreamState,
    pub(crate) observed_bytes: u64,
    pub(crate) retained_bytes: usize,
    pub(crate) source_truncated: bool,
    #[schemars(with = "Option<String>")]
    pub(crate) source_truncation_reason: Option<BodyPreviewLimit>,
    pub(crate) decoded_encoding_chain: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ExtractedRepresentation {
    pub(crate) selected_bytes: usize,
    pub(crate) media_type: String,
    pub(crate) inline: Option<String>,
    pub(crate) resource_uri: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SelectionContentRequest {
    pub(crate) capture_id: CaptureSequence,
    pub(crate) capture_revision: u64,
    pub(crate) side: BodySide,
    pub(crate) selector: ExtractSelector,
    pub(crate) offset: usize,
    pub(crate) length: usize,
}

impl SelectionContentRequest {
    pub(crate) fn validate(&self) -> Result<(), ControlError> {
        self.selector.validate()?;
        BodyContentRequest {
            capture_id: self.capture_id,
            capture_revision: self.capture_revision,
            side: self.side,
            representation: BodyRepresentation::Decoded,
            offset: self.offset,
            length: self.length,
        }
        .validate()
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SelectionResourceRequest {
    pub(crate) proxy_endpoint: SocketAddr,
    pub(crate) run_id: RunId,
    pub(crate) capture_id: CaptureSequence,
    pub(crate) capture_revision: u64,
    pub(crate) side: BodySide,
    pub(crate) selector: ExtractSelector,
    pub(crate) offset: usize,
    pub(crate) length: usize,
}

impl SelectionResourceRequest {
    pub(crate) fn to_uri(&self) -> Result<String, ControlError> {
        self.content_request().validate()?;
        let capture_id = self.capture_id.value();
        let side = match self.side {
            BodySide::Request => "request",
            BodySide::Response => "response",
        };
        let (path_kind, selector_name, selector_value) = match &self.selector {
            ExtractSelector::JsonPointer { pointer } => {
                ("json-pointer", "pointer", pointer.as_str())
            }
            ExtractSelector::FormField { key } => ("form-field", "key", key.as_str()),
        };
        let query = form_urlencoded::Serializer::new(String::new())
            .append_pair(selector_name, selector_value)
            .append_pair("offset", &self.offset.to_string())
            .append_pair("length", &self.length.to_string())
            .finish();
        Ok(format!(
            "wirelens://{}/runs/{}/captures/{capture_id}/revisions/{}/bodies/{side}/extract/{path_kind}?{query}",
            self.proxy_endpoint, self.run_id, self.capture_revision
        ))
    }

    pub(crate) fn content_request(&self) -> SelectionContentRequest {
        SelectionContentRequest {
            capture_id: self.capture_id,
            capture_revision: self.capture_revision,
            side: self.side,
            selector: self.selector.clone(),
            offset: self.offset,
            length: self.length,
        }
    }

    pub(crate) fn base_uri(&self) -> Result<String, ControlError> {
        let uri = self.to_uri()?;
        uri.split_once("&offset=")
            .map(|(base, _)| base.to_owned())
            .ok_or_else(|| ControlError::internal("selection URI did not contain a page range"))
    }
}

pub(crate) fn content_resource_base_uri(
    instance: &InstanceScope,
    capture_id: CaptureSequence,
    capture_revision: u64,
    side: BodySide,
    representation: BodyRepresentation,
) -> String {
    let side = match side {
        BodySide::Request => "request",
        BodySide::Response => "response",
    };
    let representation = match representation {
        BodyRepresentation::Raw => "raw",
        BodyRepresentation::Decoded => "decoded",
    };
    format!(
        "wirelens://{}/runs/{}/captures/{}/revisions/{capture_revision}/bodies/{side}/content/{representation}",
        instance.proxy_endpoint,
        instance.run_id,
        capture_id.value(),
    )
}

pub(crate) fn parse_selection_resource_uri(
    raw: &str,
) -> Result<SelectionResourceRequest, ControlError> {
    let remainder = raw
        .strip_prefix("wirelens://")
        .ok_or_else(|| invalid_selection_uri("selection resource URI is malformed"))?;
    let (authority, path_and_query) = remainder
        .split_once('/')
        .ok_or_else(|| invalid_selection_uri("selection resource URI is malformed"))?;
    let raw_path = path_and_query
        .split_once('?')
        .map_or(path_and_query, |(path, _)| path);
    if raw_path.contains('%')
        || raw_path
            .split('/')
            .any(|segment| segment == "." || segment == "..")
    {
        return Err(invalid_selection_uri(
            "selection resource path is not canonical",
        ));
    }
    let url = Url::parse(raw)
        .map_err(|_| invalid_selection_uri("selection resource URI is malformed"))?;
    if url.scheme() != "wirelens"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(invalid_selection_uri(
            "selection resource URI is not canonical",
        ));
    }
    let proxy_endpoint = authority.parse::<SocketAddr>().map_err(|_| {
        invalid_selection_uri("selection resource authority must be a canonical IP endpoint")
    })?;
    if authority != proxy_endpoint.to_string() {
        return Err(invalid_selection_uri(
            "selection resource authority is not canonical",
        ));
    }
    let segments = url
        .path_segments()
        .ok_or_else(|| invalid_selection_uri("selection resource path is malformed"))?
        .collect::<Vec<_>>();
    if segments.len() != 10
        || segments[0] != "runs"
        || segments[2] != "captures"
        || segments[4] != "revisions"
        || segments[6] != "bodies"
        || segments[8] != "extract"
    {
        return Err(invalid_selection_uri(
            "selection resource path is not canonical",
        ));
    }
    let run_id = RunId::from_str(segments[1])
        .map_err(|_| invalid_selection_uri("selection resource run ID is invalid"))?;
    let capture_id = CaptureSequence::new(parse_canonical_decimal(segments[3], "capture ID")?);
    let capture_revision = parse_canonical_decimal(segments[5], "capture revision")?;
    let side = match segments[7] {
        "request" => BodySide::Request,
        "response" => BodySide::Response,
        _ => return Err(invalid_selection_uri("selection resource side is invalid")),
    };
    let (selector_name, selector_kind) = match segments[9] {
        "json-pointer" => ("pointer", ExtractSelectorKind::JsonPointer),
        "form-field" => ("key", ExtractSelectorKind::FormField),
        _ => {
            return Err(invalid_selection_uri(
                "selection resource selector kind is invalid",
            ));
        }
    };
    let query = url
        .query()
        .filter(|query| !query.is_empty())
        .ok_or_else(|| invalid_selection_uri("selection resource selector query is required"))?;
    let mut selector_value = None;
    let mut offset = None;
    let mut length = None;
    let mut stage = 0u8;
    for pair in query.split('&') {
        let (name, raw_value) = pair
            .split_once('=')
            .ok_or_else(|| invalid_selection_uri("selection resource query is malformed"))?;
        let value = decode_query_value(raw_value)?;
        match name {
            name if name == selector_name && selector_value.is_none() && stage == 0 => {
                selector_value = Some(value);
                stage = 1;
            }
            "offset" if offset.is_none() && stage <= 1 => {
                offset = Some(parse_canonical_usize(&value, "offset")?);
                stage = 2;
            }
            "length" if length.is_none() && stage <= 2 => {
                length = Some(parse_canonical_usize(&value, "length")?);
                stage = 3;
            }
            name if name == selector_name || name == "offset" || name == "length" => {
                return Err(invalid_selection_uri(
                    "selection resource query key is duplicated or out of order",
                ));
            }
            _ => {
                return Err(invalid_selection_uri(
                    "selection resource query key is unknown",
                ));
            }
        }
    }
    let selector_value = selector_value
        .ok_or_else(|| invalid_selection_uri("selection resource selector query is required"))?;
    let selector = match selector_kind {
        ExtractSelectorKind::JsonPointer => ExtractSelector::JsonPointer {
            pointer: JsonPointer::parse(&selector_value)?,
        },
        ExtractSelectorKind::FormField => ExtractSelector::FormField {
            key: selector_value,
        },
    };
    let request = SelectionResourceRequest {
        proxy_endpoint,
        run_id,
        capture_id,
        capture_revision,
        side,
        selector,
        offset: offset.unwrap_or(0),
        length: length.unwrap_or(DEFAULT_BODY_PAGE_LENGTH),
    };
    request.content_request().validate()?;
    Ok(request)
}

fn decode_query_value(raw: &str) -> Result<String, ControlError> {
    if raw.is_empty() {
        return Ok(String::new());
    }
    let decoded = form_urlencoded::parse(raw.as_bytes())
        .next()
        .map(|(value, _)| value.into_owned())
        .ok_or_else(|| invalid_selection_uri("selection resource query value is malformed"))?;
    let encoded = form_urlencoded::Serializer::new(String::new())
        .append_key_only(&decoded)
        .finish();
    if encoded != raw {
        return Err(invalid_selection_uri(
            "selection resource query value is not canonical",
        ));
    }
    Ok(decoded)
}

fn parse_canonical_decimal(raw: &str, field: &str) -> Result<u64, ControlError> {
    if raw.is_empty()
        || (raw.len() > 1 && raw.starts_with('0'))
        || !raw.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(invalid_selection_uri(format!(
            "selection resource {field} is not canonical"
        )));
    }
    raw.parse()
        .map_err(|_| invalid_selection_uri(format!("selection resource {field} is out of range")))
}

fn parse_canonical_usize(raw: &str, field: &str) -> Result<usize, ControlError> {
    usize::try_from(parse_canonical_decimal(raw, field)?)
        .map_err(|_| invalid_selection_uri(format!("selection resource {field} is out of range")))
}

fn invalid_selection_uri(message: impl Into<String>) -> ControlError {
    invalid_body_argument(message, serde_json::json!({}))
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SelectionPage {
    #[serde(with = "base64_bytes")]
    pub(crate) content: Bytes,
    pub(crate) media_type: String,
    pub(crate) selected_bytes: usize,
    pub(crate) requested_range: BodyRange,
    pub(crate) actual_range: BodyRange,
    pub(crate) next_offset: Option<usize>,
    pub(crate) next_uri: Option<String>,
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
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct OriginalBoundary {
    pub(crate) start: usize,
    pub(crate) end: usize,
}

pub(crate) struct FoldedMatcher<'a> {
    query: Box<[char]>,
    failure: Box<[usize]>,
    boundaries: Box<[OriginalBoundary]>,
    next_boundary: usize,
    seen: usize,
    matched: usize,
    units_since_cancellation: usize,
    cancelled: &'a AtomicBool,
}

impl<'a> FoldedMatcher<'a> {
    pub(crate) fn new(query: Vec<char>, cancelled: &'a AtomicBool) -> Result<Self, ControlError> {
        if query.is_empty() {
            return Err(invalid_body_argument(
                "body search query must not fold to an empty string",
                serde_json::json!({"field": "query"}),
            ));
        }
        let mut failure = vec![0usize; query.len()];
        let mut prefix = 0usize;
        for index in 1..query.len() {
            if index.is_multiple_of(8 * 1_024) {
                check_body_cancellation(cancelled, "body search cancelled")?;
            }
            while prefix > 0 && query[index] != query[prefix] {
                prefix = failure[prefix - 1];
            }
            if query[index] == query[prefix] {
                prefix += 1;
            }
            failure[index] = prefix;
        }
        let boundaries = vec![OriginalBoundary::default(); query.len()].into_boxed_slice();
        Ok(Self {
            query: query.into_boxed_slice(),
            failure: failure.into_boxed_slice(),
            boundaries,
            next_boundary: 0,
            seen: 0,
            matched: 0,
            units_since_cancellation: 0,
            cancelled,
        })
    }

    pub(crate) fn push(
        &mut self,
        value: char,
        boundary: OriginalBoundary,
    ) -> Result<Option<(usize, usize)>, ControlError> {
        self.units_since_cancellation += 1;
        if self.units_since_cancellation >= 8 * 1_024 {
            check_body_cancellation(self.cancelled, "body search cancelled")?;
            self.units_since_cancellation = 0;
        }
        self.boundaries[self.next_boundary] = boundary;
        self.next_boundary = (self.next_boundary + 1) % self.boundaries.len();
        self.seen = self.seen.saturating_add(1);
        while self.matched > 0 && value != self.query[self.matched] {
            self.matched = self.failure[self.matched - 1];
        }
        if value == self.query[self.matched] {
            self.matched += 1;
        }
        if self.matched != self.query.len() {
            return Ok(None);
        }
        let original_start = self.boundaries[self.next_boundary].start;
        self.matched = self.failure[self.matched - 1];
        Ok(Some((original_start, boundary.end)))
    }

    #[cfg(test)]
    pub(crate) fn retained_boundaries(&self) -> usize {
        self.seen.min(self.boundaries.len())
    }

    #[cfg(test)]
    pub(crate) fn boundary_capacity(&self) -> usize {
        self.boundaries.len()
    }
}

fn bounded_covering_window(text: &str, match_start: usize, match_end: usize) -> (usize, usize) {
    let desired_start = aligned_start(
        text,
        match_start.saturating_sub(DEFAULT_BODY_PAGE_LENGTH / 2),
    );
    let desired_end = aligned_end(
        text,
        match_end
            .saturating_add(DEFAULT_BODY_PAGE_LENGTH / 2)
            .min(text.len()),
    );
    if desired_end - desired_start <= MAX_BODY_PAGE_LENGTH {
        return (desired_start, desired_end);
    }
    let latest_start = match_end.saturating_sub(MAX_BODY_PAGE_LENGTH);
    let window_start = aligned_start(text, desired_start.max(latest_start));
    let window_end = aligned_end(
        text,
        window_start
            .saturating_add(MAX_BODY_PAGE_LENGTH)
            .min(text.len()),
    );
    (window_start, window_end)
}

pub(crate) fn search_capture_body(
    decoded: &[u8],
    query: &str,
    limit: usize,
    context_bytes: usize,
    decoded_uri: &str,
    raw_uri: &str,
    cancelled: &AtomicBool,
) -> Result<BodyTextSearch, ControlError> {
    if query.is_empty() {
        return Err(invalid_body_argument(
            "body search query must not be empty",
            serde_json::json!({"field": "query"}),
        ));
    }
    if query.len() > MAX_BODY_SEARCH_QUERY_BYTES {
        return Err(invalid_body_argument(
            "body search query exceeds the byte limit",
            serde_json::json!({
                "field": "query",
                "maximum_bytes": MAX_BODY_SEARCH_QUERY_BYTES,
                "received_bytes": query.len(),
            }),
        ));
    }
    if !(1..=MAX_BODY_SEARCH_LIMIT).contains(&limit) {
        return Err(invalid_body_argument(
            "body search match limit is out of range",
            serde_json::json!({
                "field": "limit",
                "minimum": 1,
                "maximum": MAX_BODY_SEARCH_LIMIT,
                "received": limit,
            }),
        ));
    }
    if context_bytes > MAX_BODY_SEARCH_CONTEXT_BYTES {
        return Err(invalid_body_argument(
            "body search context is out of range",
            serde_json::json!({
                "field": "context_bytes",
                "minimum": 0,
                "maximum": MAX_BODY_SEARCH_CONTEXT_BYTES,
                "received": context_bytes,
            }),
        ));
    }
    let text = std::str::from_utf8(decoded).map_err(|_| {
        ControlError::new(
            ControlErrorCode::BodyNotTextual,
            "decoded capture body is not textual UTF-8",
            false,
            serde_json::json!({"raw_content_uri": raw_uri}),
        )
    })?;
    check_body_cancellation(cancelled, "body search cancelled")?;
    let mut folded_query = Vec::with_capacity(query.len());
    let mut query_scanned = 0usize;
    for original in query.chars() {
        query_scanned += original.len_utf8();
        folded_query.extend(original.case_fold());
        if query_scanned >= 32 * 1_024 {
            check_body_cancellation(cancelled, "body search cancelled")?;
            query_scanned = 0;
        }
    }
    check_body_cancellation(cancelled, "body search cancelled")?;
    if folded_query.is_empty() {
        return Err(invalid_body_argument(
            "body search query must not fold to an empty string",
            serde_json::json!({"field": "query"}),
        ));
    }
    let mut matcher = FoldedMatcher::new(folded_query, cancelled)?;
    let mut total_matches = 0usize;
    let mut matches = Vec::with_capacity(limit);
    let mut scanned = 0usize;
    for (start, original) in text.char_indices() {
        scanned += original.len_utf8();
        for value in original.case_fold() {
            let boundary = OriginalBoundary {
                start,
                end: start + original.len_utf8(),
            };
            let Some((original_start, original_end)) = matcher.push(value, boundary)? else {
                continue;
            };
            total_matches += 1;
            if matches.len() >= limit {
                continue;
            }
            let context_start = aligned_start(text, original_start.saturating_sub(context_bytes));
            let context_end = aligned_end(
                text,
                original_end.saturating_add(context_bytes).min(text.len()),
            );
            let (window_start, window_end) =
                bounded_covering_window(text, original_start, original_end);
            matches.push(BodySearchMatch {
                original_range: BodyRange {
                    offset: original_start,
                    length: original_end - original_start,
                },
                matched: text[original_start..original_end].to_owned(),
                context_before: text[context_start..original_start].to_owned(),
                context_after: text[original_end..context_end].to_owned(),
                resource_uri: uri_with_range(decoded_uri, window_start, window_end - window_start),
            });
        }
        if scanned >= 32 * 1_024 {
            check_body_cancellation(cancelled, "body search cancelled")?;
            scanned = 0;
        }
    }
    check_body_cancellation(cancelled, "body search cancelled")?;
    Ok(BodyTextSearch {
        total_matches,
        omitted_matches: total_matches.saturating_sub(matches.len()),
        matches,
    })
}

pub(crate) fn extract_capture_body(
    decoded: &[u8],
    media_type: Option<&str>,
    selector: &ExtractSelector,
    selection_uri: &str,
    cancelled: &AtomicBool,
) -> Result<ExtractedRepresentation, ControlError> {
    let (bytes, selected_media_type) =
        extract_selected_bytes(decoded, media_type, selector, cancelled)?;
    let selected_bytes = bytes.len();
    let inline = if selected_bytes <= MAX_INLINE_SELECTION_BYTES {
        Some(String::from_utf8(bytes).map_err(|_| {
            ControlError::internal("selected body representation was not valid UTF-8")
        })?)
    } else {
        None
    };
    let resource_uri = inline
        .is_none()
        .then(|| uri_with_range(selection_uri, 0, DEFAULT_BODY_PAGE_LENGTH));
    Ok(ExtractedRepresentation {
        selected_bytes,
        media_type: selected_media_type,
        inline,
        resource_uri,
    })
}

pub(crate) fn extract_selected_bytes(
    decoded: &[u8],
    media_type: Option<&str>,
    selector: &ExtractSelector,
    cancelled: &AtomicBool,
) -> Result<(Vec<u8>, String), ControlError> {
    selector.validate()?;
    check_body_cancellation(cancelled, "body extraction cancelled")?;
    let selection = match selector {
        ExtractSelector::JsonPointer { pointer } => {
            let selection = extract_json_selection(decoded, pointer, cancelled)?;
            let selection = match selection {
                Ok(selection) => selection,
                Err(miss) => {
                    let mut details = serde_json::Map::new();
                    details.insert(
                        "longest_valid_prefix".to_owned(),
                        serde_json::json!(miss.longest_valid_prefix),
                    );
                    details.insert("next".to_owned(), serde_json::json!(miss.next));
                    if miss.omitted_next != 0 {
                        details.insert("omitted_next".to_owned(), miss.omitted_next.into());
                    }
                    return Err(ControlError::new(
                        ControlErrorCode::NotFound,
                        "JSON pointer does not select a value",
                        false,
                        serde_json::Value::Object(details),
                    ));
                }
            };
            (selection.bytes, "application/json".to_owned())
        }
        ExtractSelector::FormField { key } => {
            if !media_type.is_some_and(is_form_media_type) {
                return Err(invalid_body_argument(
                    "form-field extraction requires application/x-www-form-urlencoded",
                    serde_json::json!({"field": "selector.kind"}),
                ));
            }
            (
                extract_form_values(decoded, key, cancelled)?,
                "application/json".to_owned(),
            )
        }
    };
    check_body_cancellation(cancelled, "body extraction cancelled")?;
    Ok(selection)
}

pub(crate) struct CappedJsonArrayWriter {
    output: Vec<u8>,
    limit: usize,
    #[cfg(test)]
    growth_count: usize,
}

impl CappedJsonArrayWriter {
    fn new() -> Self {
        Self::with_limit(MAX_DECODED_CONTENT_BYTES)
    }

    pub(crate) fn with_limit(limit: usize) -> Self {
        let mut output = Vec::with_capacity(limit.min(MAX_INLINE_SELECTION_BYTES));
        output.push(b'[');
        Self {
            output,
            limit,
            #[cfg(test)]
            growth_count: 0,
        }
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.output.len()
    }

    #[cfg(test)]
    pub(crate) fn capacity(&self) -> usize {
        self.output.capacity()
    }

    #[cfg(test)]
    pub(crate) fn growth_count(&self) -> usize {
        self.growth_count
    }

    fn can_finish(&self) -> bool {
        self.output.len() < self.limit
    }

    pub(crate) fn finish(mut self) -> Result<Vec<u8>, ControlError> {
        if !self.can_finish() {
            return Err(form_output_limit_error());
        }
        self.output.push(b']');
        Ok(self.output)
    }
}

impl io::Write for CappedJsonArrayWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let Some(required) = self.output.len().checked_add(bytes.len()) else {
            return Err(io::Error::other(
                "selected form representation exceeds the size limit",
            ));
        };
        if required >= self.limit {
            return Err(io::Error::other(
                "selected form representation exceeds the size limit",
            ));
        }
        if self.output.capacity() < required {
            let target = self
                .output
                .capacity()
                .max(1)
                .saturating_mul(2)
                .max(required)
                .min(self.limit);
            self.output
                .try_reserve_exact(target - self.output.len())
                .map_err(io::Error::other)?;
            #[cfg(test)]
            {
                self.growth_count += 1;
            }
        }
        self.output.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn form_output_limit_error() -> ControlError {
    ControlError::new(
        ControlErrorCode::JsonSizeLimit,
        "selected form representation exceeds the size limit",
        false,
        serde_json::json!({"maximum_bytes": MAX_DECODED_CONTENT_BYTES}),
    )
}

fn write_form_json(
    writer: &mut CappedJsonArrayWriter,
    value: &str,
    separator: bool,
) -> Result<(), ControlError> {
    if separator {
        io::Write::write_all(writer, b",").map_err(|_| form_output_limit_error())?;
    }
    serde_json::to_writer(writer, value).map_err(|_| form_output_limit_error())?;
    Ok(())
}

pub(crate) fn find_form_delimiter_with<F>(
    input: &[u8],
    delimiter: u8,
    mut checkpoint: F,
) -> Result<usize, ControlError>
where
    F: FnMut() -> Result<(), ControlError>,
{
    let mut offset = 0usize;
    while offset < input.len() {
        checkpoint()?;
        let end = offset.saturating_add(32 * 1_024).min(input.len());
        if let Some(relative) = input[offset..end]
            .iter()
            .position(|byte| *byte == delimiter)
        {
            return Ok(offset + relative);
        }
        offset = end;
    }
    checkpoint()?;
    Ok(input.len())
}

#[derive(Default)]
struct FormComponentDecoder {
    bytes: Vec<u8>,
    output: String,
}

impl FormComponentDecoder {
    fn decode<'a>(
        &'a mut self,
        input: &[u8],
        limit: usize,
        cancelled: &AtomicBool,
    ) -> Result<Option<&'a str>, ControlError> {
        self.decode_with(input, limit, || {
            check_body_cancellation(cancelled, "form extraction cancelled")
        })
    }

    fn decode_with<'a, F>(
        &'a mut self,
        input: &[u8],
        limit: usize,
        mut checkpoint: F,
    ) -> Result<Option<&'a str>, ControlError>
    where
        F: FnMut() -> Result<(), ControlError>,
    {
        self.bytes.clear();
        self.output.clear();
        checkpoint()?;
        let mut offset = 0usize;
        let mut scanned = 0usize;
        while offset < input.len() {
            let (decoded, consumed) = decode_form_byte(&input[offset..]);
            self.bytes.push(decoded);
            offset += consumed;
            scanned += consumed;
            if scanned >= 32 * 1_024 {
                if !self.flush(false, limit) {
                    return Ok(None);
                }
                checkpoint()?;
                scanned = 0;
            }
        }
        if !self.flush(true, limit) {
            return Ok(None);
        }
        checkpoint()?;
        Ok(Some(&self.output))
    }

    fn flush(&mut self, final_chunk: bool, limit: usize) -> bool {
        let mut consumed = 0usize;
        while consumed < self.bytes.len() {
            match std::str::from_utf8(&self.bytes[consumed..]) {
                Ok(text) => {
                    if !append_bounded(&mut self.output, text, limit) {
                        return false;
                    }
                    consumed = self.bytes.len();
                }
                Err(error) => {
                    let valid_end = consumed + error.valid_up_to();
                    if valid_end > consumed {
                        // SAFETY: `valid_up_to` identifies this exact prefix as UTF-8.
                        let text = unsafe {
                            std::str::from_utf8_unchecked(&self.bytes[consumed..valid_end])
                        };
                        if !append_bounded(&mut self.output, text, limit) {
                            return false;
                        }
                    }
                    match error.error_len() {
                        Some(length) => {
                            if !append_bounded(&mut self.output, "\u{fffd}", limit) {
                                return false;
                            }
                            consumed = valid_end + length;
                        }
                        None if final_chunk => {
                            if !append_bounded(&mut self.output, "\u{fffd}", limit) {
                                return false;
                            }
                            consumed = self.bytes.len();
                        }
                        None => {
                            consumed = valid_end;
                            break;
                        }
                    }
                }
            }
        }
        if consumed != 0 {
            self.bytes.drain(..consumed);
        }
        true
    }
}

fn append_bounded(output: &mut String, text: &str, limit: usize) -> bool {
    if output
        .len()
        .checked_add(text.len())
        .is_none_or(|length| length > limit)
    {
        return false;
    }
    output.push_str(text);
    true
}

fn decode_form_byte(input: &[u8]) -> (u8, usize) {
    match input {
        [b'+', ..] => (b' ', 1),
        [b'%', high, low, ..] => match (hex_value(*high), hex_value(*low)) {
            (Some(high), Some(low)) => ((high << 4) | low, 3),
            _ => (b'%', 1),
        },
        [byte, ..] => (*byte, 1),
        [] => unreachable!("form decoder requires one input byte"),
    }
}

fn extract_form_values(
    input: &[u8],
    expected_key: &str,
    cancelled: &AtomicBool,
) -> Result<Vec<u8>, ControlError> {
    if input.len() > MAX_DECODED_CONTENT_BYTES {
        return Err(ControlError::new(
            ControlErrorCode::JsonSizeLimit,
            "decoded form body exceeds the structured input limit",
            false,
            serde_json::json!({"maximum_bytes": MAX_DECODED_CONTENT_BYTES}),
        ));
    }
    let checkpoint = || check_body_cancellation(cancelled, "form extraction cancelled");
    let mut decoder = FormComponentDecoder::default();
    let mut output = CappedJsonArrayWriter::new();
    let mut found = false;
    let mut offset = 0usize;
    while offset <= input.len() {
        let field_end = offset + find_form_delimiter_with(&input[offset..], b'&', checkpoint)?;
        let field = &input[offset..field_end];
        let equals = find_form_delimiter_with(field, b'=', checkpoint)?;
        let (raw_key, raw_value) = if equals == field.len() {
            (field, b"" as &[u8])
        } else {
            (&field[..equals], &field[equals + 1..])
        };
        let key_matches = decoder
            .decode(raw_key, expected_key.len(), cancelled)?
            .is_some_and(|decoded| decoded == expected_key);
        if key_matches {
            let decoded_value = decoder
                .decode(raw_value, MAX_DECODED_CONTENT_BYTES, cancelled)?
                .ok_or_else(form_output_limit_error)?;
            write_form_json(&mut output, decoded_value, found)?;
            found = true;
        }
        if field_end == input.len() {
            break;
        }
        offset = field_end + 1;
    }
    if !found {
        return Err(ControlError::new(
            ControlErrorCode::NotFound,
            "form field does not exist",
            false,
            serde_json::json!({}),
        ));
    }
    output.finish()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn is_form_media_type(value: &str) -> bool {
    value.split(';').next().is_some_and(|kind| {
        kind.trim()
            .eq_ignore_ascii_case("application/x-www-form-urlencoded")
    })
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SelectedPageWindow {
    pub(crate) content: Bytes,
    pub(crate) requested_range: BodyRange,
    pub(crate) actual_range: BodyRange,
    pub(crate) total_bytes: usize,
    pub(crate) next_offset: Option<usize>,
    pub(crate) next_uri: Option<String>,
}

pub(crate) fn page_selected_representation(
    selected: &[u8],
    offset: usize,
    length: usize,
    selection_uri: &str,
) -> Result<SelectedPageWindow, ControlError> {
    if length == 0 || length > MAX_BODY_PAGE_LENGTH {
        return Err(invalid_body_argument(
            "selection page length is out of range",
            serde_json::json!({
                "field": "length",
                "minimum": 1,
                "maximum": MAX_BODY_PAGE_LENGTH,
                "received": length,
            }),
        ));
    }
    let text = std::str::from_utf8(selected)
        .map_err(|_| ControlError::internal("selected representation was not valid UTF-8"))?;
    if offset > selected.len() {
        return Err(invalid_body_argument(
            "selection page offset exceeds the selected representation",
            serde_json::json!({
                "field": "offset",
                "maximum": selected.len(),
                "received": offset,
            }),
        ));
    }
    if !text.is_char_boundary(offset) {
        return Err(invalid_utf8_offset(text, offset));
    }
    let requested_end = offset.saturating_add(length).min(selected.len());
    let mut end = aligned_end(text, requested_end);
    if end == offset && end < selected.len() {
        end = (offset + 1..=selected.len())
            .find(|candidate| text.is_char_boundary(*candidate))
            .unwrap_or(selected.len());
    }
    let content = Bytes::copy_from_slice(&selected[offset..end]);
    let next_offset = (end < selected.len()).then_some(end);
    Ok(SelectedPageWindow {
        content,
        requested_range: BodyRange { offset, length },
        actual_range: BodyRange {
            offset,
            length: end - offset,
        },
        total_bytes: selected.len(),
        next_offset,
        next_uri: next_offset.map(|next| uri_with_range(selection_uri, next, length)),
    })
}

fn aligned_start(text: &str, mut offset: usize) -> usize {
    while offset < text.len() && !text.is_char_boundary(offset) {
        offset += 1;
    }
    offset
}

fn aligned_end(text: &str, mut offset: usize) -> usize {
    while offset > 0 && !text.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

fn invalid_utf8_offset(text: &str, offset: usize) -> ControlError {
    let nearest_floor = (0..offset)
        .rev()
        .find(|candidate| text.is_char_boundary(*candidate))
        .unwrap_or(0);
    let nearest_ceiling = (offset + 1..=text.len())
        .find(|candidate| text.is_char_boundary(*candidate))
        .unwrap_or(text.len());
    invalid_body_argument(
        "selection page offset is not a UTF-8 boundary",
        serde_json::json!({
            "field": "offset",
            "received": offset,
            "nearest_floor": nearest_floor,
            "nearest_ceiling": nearest_ceiling,
        }),
    )
}

fn uri_with_range(base: &str, offset: usize, length: usize) -> String {
    let separator = if base.contains('?') { '&' } else { '?' };
    format!("{base}{separator}offset={offset}&length={length}")
}

fn check_body_cancellation(
    cancelled: &AtomicBool,
    message: &'static str,
) -> Result<(), ControlError> {
    if cancelled.load(Ordering::Relaxed) {
        Err(ControlError::cancelled(message))
    } else {
        Ok(())
    }
}

fn invalid_body_argument(message: impl Into<String>, details: serde_json::Value) -> ControlError {
    ControlError::new(ControlErrorCode::InvalidArgument, message, false, details)
}

fn deserialize_required_option<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::deserialize(deserializer)
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
