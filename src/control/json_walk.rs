use std::{
    fmt,
    str::FromStr,
    sync::atomic::{AtomicBool, Ordering},
};

use json_event_parser::{JsonEvent, LowLevelJsonParser};
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use unicode_casefold::UnicodeCaseFold;

use crate::{
    capture::{BodySide, CaptureSequence},
    control::body::MAX_DECODED_CONTENT_BYTES,
    control_rpc::protocol::{ControlError, ControlErrorCode},
};

pub(crate) const MAX_JSON_INPUT_BYTES: usize = MAX_DECODED_CONTENT_BYTES;
pub(crate) const MAX_JSON_DEPTH: usize = 512;
pub(crate) const MAX_JSON_EXAMPLES: usize = 20;
pub(crate) const MAX_JSON_HINTS: usize = 20;
pub(crate) const MAX_JSON_RESULT_BYTES: usize = 64 * 1_024;
const MAX_JSON_POINTER_BYTES: usize = 32 * 1_024;
const MAX_JSON_PATTERN_BYTES: usize = 4 * 1_024;
const MAX_JSON_FIELD_NAME_BYTES: usize = 4 * 1_024;
const RETAINED_RESULT_WIRE_BUDGET: usize = 48 * 1_024;

#[derive(Clone, Debug, Eq, Hash, JsonSchema, PartialEq)]
#[schemars(transparent)]
pub(crate) struct JsonPointer(String);

impl JsonPointer {
    pub(crate) fn parse(raw: &str) -> Result<Self, ControlError> {
        validate_pointer_text(raw, false)?;
        Ok(Self(raw.to_owned()))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }

    #[cfg(test)]
    pub(crate) fn segments(&self) -> impl Iterator<Item = String> + '_ {
        pointer_raw_segments(self.as_str()).map(|segment| {
            decode_segment(segment, false)
                .expect("a validated JSON pointer always has decodable segments")
        })
    }
}

impl fmt::Display for JsonPointer {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for JsonPointer {
    type Err = ControlError;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        Self::parse(raw)
    }
}

impl Serialize for JsonPointer {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for JsonPointer {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        Self::parse(&raw).map_err(|error| serde::de::Error::custom(error.message()))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum PatternSegment {
    Wildcard,
    Exact(String),
}

#[derive(Clone, Debug, Eq, JsonSchema, PartialEq)]
#[schemars(transparent)]
pub(crate) struct JsonPointerPattern {
    raw: String,
    #[schemars(skip)]
    segments: Box<[PatternSegment]>,
}

impl JsonPointerPattern {
    pub(crate) fn parse(raw: &str) -> Result<Self, ControlError> {
        if raw.len() > MAX_JSON_PATTERN_BYTES {
            return Err(invalid_json_argument(
                "JSON pointer pattern exceeds the maximum length",
                serde_json::json!({
                    "field": "pattern",
                    "maximum_bytes": MAX_JSON_PATTERN_BYTES,
                    "received_bytes": raw.len(),
                }),
            ));
        }
        validate_pointer_text(raw, true)?;
        let mut segments = Vec::new();
        for segment in pointer_raw_segments(raw) {
            if segment == "**" {
                return Err(invalid_json_argument(
                    "recursive JSON pointer wildcards are not supported",
                    serde_json::json!({"field": "pattern", "segment": "**"}),
                ));
            }
            if segment == "*" {
                segments.push(PatternSegment::Wildcard);
            } else {
                let decoded = decode_segment(segment, true)?;
                segments.push(PatternSegment::Exact(escape_pointer_segment(&decoded)));
            }
        }
        Ok(Self {
            raw: raw.to_owned(),
            segments: segments.into_boxed_slice(),
        })
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.raw
    }

    fn len(&self) -> usize {
        self.segments.len()
    }

    fn segment_matches(&self, index: usize, actual: &str) -> bool {
        self.segments
            .get(index)
            .is_some_and(|expected| pattern_segment_matches(expected, actual))
    }
}

impl fmt::Display for JsonPointerPattern {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for JsonPointerPattern {
    type Err = ControlError;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        Self::parse(raw)
    }
}

impl Serialize for JsonPointerPattern {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for JsonPointerPattern {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        Self::parse(&raw).map_err(|error| serde::de::Error::custom(error.message()))
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum FieldMatchMode {
    Exact,
    UnicodeCasefoldExact,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum JsonType {
    Object,
    Array,
    String,
    Number,
    Boolean,
    Null,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct JsonTypeCounts {
    pub(crate) object: usize,
    pub(crate) array: usize,
    pub(crate) string: usize,
    pub(crate) number: usize,
    pub(crate) boolean: usize,
    pub(crate) null: usize,
}

impl JsonTypeCounts {
    fn increment(&mut self, kind: JsonType) {
        match kind {
            JsonType::Object => self.object += 1,
            JsonType::Array => self.array += 1,
            JsonType::String => self.string += 1,
            JsonType::Number => self.number += 1,
            JsonType::Boolean => self.boolean += 1,
            JsonType::Null => self.null += 1,
        }
    }

    #[cfg(test)]
    pub(crate) fn count(self, kind: JsonType) -> usize {
        match kind {
            JsonType::Object => self.object,
            JsonType::Array => self.array,
            JsonType::String => self.string,
            JsonType::Number => self.number,
            JsonType::Boolean => self.boolean,
            JsonType::Null => self.null,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum JsonInspectionStatus {
    Matched,
    NoMatch,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct JsonFieldMatch {
    pub(crate) pointer: JsonPointer,
    pub(crate) value_type: JsonType,
    pub(crate) object_child_count: Option<usize>,
    pub(crate) array_length: Option<usize>,
    pub(crate) scalar_encoded_bytes: Option<usize>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FindJsonPointersRequest {
    pub(crate) capture_id: CaptureSequence,
    pub(crate) capture_revision: u64,
    #[schemars(with = "String")]
    pub(crate) side: BodySide,
    pub(crate) field_name: String,
    pub(crate) match_mode: FieldMatchMode,
    pub(crate) limit: usize,
}

impl FindJsonPointersRequest {
    pub(crate) fn validate(&self) -> Result<(), ControlError> {
        if self.field_name.len() > MAX_JSON_FIELD_NAME_BYTES {
            return Err(invalid_json_argument(
                "JSON field name exceeds the maximum length",
                serde_json::json!({
                    "field": "field_name",
                    "maximum_bytes": MAX_JSON_FIELD_NAME_BYTES,
                    "received_bytes": self.field_name.len(),
                }),
            ));
        }
        validate_result_limit(self.limit)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FindJsonPointersResult {
    pub(crate) status: JsonInspectionStatus,
    pub(crate) capture_revision: u64,
    pub(crate) matches: Vec<JsonFieldMatch>,
    pub(crate) total_matches: usize,
    pub(crate) omitted_matches: usize,
    pub(crate) inspected_bytes: usize,
    pub(crate) source_truncated: bool,
    pub(crate) truncated: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProbeJsonPointerPatternRequest {
    pub(crate) capture_id: CaptureSequence,
    pub(crate) capture_revision: u64,
    #[schemars(with = "String")]
    pub(crate) side: BodySide,
    pub(crate) pattern: JsonPointerPattern,
}

impl ProbeJsonPointerPatternRequest {
    pub(crate) fn validate(&self) -> Result<(), ControlError> {
        JsonPointerPattern::parse(self.pattern.as_str()).map(drop)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct JsonNextSegmentHint {
    pub(crate) segment: String,
    pub(crate) value_type: JsonType,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct JsonObjectKeySummary {
    pub(crate) pointer: JsonPointer,
    pub(crate) child_count: usize,
    pub(crate) keys: Vec<JsonNextSegmentHint>,
    pub(crate) omitted_keys: usize,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct JsonArrayLengthRange {
    pub(crate) minimum: usize,
    pub(crate) maximum: usize,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct JsonPointerMissHint {
    pub(crate) longest_matched_prefix: JsonPointer,
    pub(crate) next_segments: Vec<JsonNextSegmentHint>,
    pub(crate) omitted_next_segments: usize,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProbeJsonPointerPatternResult {
    pub(crate) status: JsonInspectionStatus,
    pub(crate) capture_revision: u64,
    pub(crate) match_count: usize,
    pub(crate) type_counts: JsonTypeCounts,
    pub(crate) examples: Vec<JsonPointer>,
    pub(crate) omitted_examples: usize,
    pub(crate) object_key_summaries: Vec<JsonObjectKeySummary>,
    pub(crate) omitted_object_summaries: usize,
    pub(crate) array_length_range: Option<JsonArrayLengthRange>,
    pub(crate) miss_hint: Option<JsonPointerMissHint>,
    pub(crate) inspected_bytes: usize,
    pub(crate) source_truncated: bool,
    pub(crate) truncated: bool,
}

#[derive(Clone, Copy)]
pub(crate) struct WalkPath<'a> {
    raw: &'a str,
    segment: Option<&'a str>,
    depth: usize,
}

impl WalkPath<'_> {
    fn into_owned(self) -> JsonPointer {
        JsonPointer(self.raw.to_owned())
    }
}

pub(crate) trait JsonWalkObserver {
    fn value_start(&mut self, _path: WalkPath<'_>) {}
    fn value_end(&mut self) {}
    fn scalar(
        &mut self,
        path: WalkPath<'_>,
        kind: JsonType,
        encoded_bytes: Option<usize>,
        _encoded_token: Option<&[u8]>,
    );
    fn object_start(&mut self, path: WalkPath<'_>);
    fn object_end(&mut self, path: WalkPath<'_>, child_count: usize);
    fn array_start(&mut self, path: WalkPath<'_>);
    fn array_end(&mut self, path: WalkPath<'_>, length: usize);
    fn object_key(&mut self, _path: WalkPath<'_>, _key: &str) {}
    fn wants_scalar_encoded_bytes(&self) -> bool {
        false
    }
    fn wants_scalar_token(&self) -> bool {
        false
    }
    fn should_descend(&self) -> bool;
}

#[derive(Clone, Copy)]
struct ValuePathState {
    restore_path_len: usize,
    segment_start: Option<usize>,
    depth: usize,
}

enum ContainerKind {
    Object {
        child_count: usize,
        pending_value: Option<ValuePathState>,
    },
    Array {
        length: usize,
    },
}

struct ContainerFrame {
    kind: ContainerKind,
    value: ValuePathState,
    observed: bool,
    descend: bool,
}

fn walk_json(
    input: &[u8],
    observer: &mut impl JsonWalkObserver,
    cancelled: &AtomicBool,
) -> Result<(), ControlError> {
    validate_json_input(input, cancelled)?;
    // SliceJsonParser does not expose consumed offsets. LowLevelJsonParser is the
    // same event parser with the consumed-byte count required for exact token spans.
    let mut parser = LowLevelJsonParser::new();
    let mut input_offset = 0usize;
    let mut path = String::new();
    let mut frames = Vec::<ContainerFrame>::new();
    let mut root_seen = false;
    loop {
        check_json_cancellation(cancelled)?;
        let event_call_start = input_offset;
        let parsed = parser.parse_next(&input[input_offset..], true);
        input_offset = input_offset
            .checked_add(parsed.consumed_bytes)
            .ok_or_else(|| ControlError::internal("JSON parser offset overflow"))?;
        let Some(event) = parsed.event else {
            if parsed.consumed_bytes == 0 {
                return Err(ControlError::internal(
                    "JSON parser made no progress without an event",
                ));
            }
            continue;
        };
        let event = event.map_err(safe_json_syntax_error)?;
        match event {
            JsonEvent::ObjectKey(key) => {
                let Some(frame) = frames.last_mut() else {
                    return Err(malformed_json("JSON object key appears outside an object"));
                };
                let ContainerKind::Object {
                    child_count,
                    pending_value,
                } = &mut frame.kind
                else {
                    return Err(malformed_json("JSON object key appears inside an array"));
                };
                if pending_value.is_some() {
                    return Err(malformed_json("JSON object key has no preceding value"));
                }
                let restore_path_len = path.len();
                let segment_start = append_pointer_segment(&mut path, &key, cancelled)?;
                *pending_value = Some(ValuePathState {
                    restore_path_len,
                    segment_start: Some(segment_start),
                    depth: frame.value.depth + 1,
                });
                *child_count += 1;
                if frame.descend {
                    observer.object_key(
                        walk_path(
                            &path,
                            ValuePathState {
                                restore_path_len,
                                segment_start: Some(segment_start),
                                depth: frame.value.depth + 1,
                            },
                        ),
                        &key,
                    );
                }
            }
            JsonEvent::StartObject => {
                let value = prepare_value_path(&mut path, &mut frames, &mut root_seen, cancelled)?;
                check_depth(frames.len() + 1)?;
                let parent_descends = frames.last().is_none_or(|frame| frame.descend);
                if parent_descends {
                    let view = walk_path(&path, value);
                    observer.value_start(view);
                    observer.object_start(view);
                }
                let descend = parent_descends && observer.should_descend();
                frames.push(ContainerFrame {
                    kind: ContainerKind::Object {
                        child_count: 0,
                        pending_value: None,
                    },
                    value,
                    descend,
                    observed: parent_descends,
                });
            }
            JsonEvent::StartArray => {
                let value = prepare_value_path(&mut path, &mut frames, &mut root_seen, cancelled)?;
                check_depth(frames.len() + 1)?;
                let parent_descends = frames.last().is_none_or(|frame| frame.descend);
                if parent_descends {
                    let view = walk_path(&path, value);
                    observer.value_start(view);
                    observer.array_start(view);
                }
                let descend = parent_descends && observer.should_descend();
                frames.push(ContainerFrame {
                    kind: ContainerKind::Array { length: 0 },
                    value,
                    observed: parent_descends,
                    descend,
                });
            }
            JsonEvent::EndObject => {
                let Some(frame) = frames.pop() else {
                    return Err(malformed_json("JSON object end has no matching start"));
                };
                let ContainerKind::Object {
                    child_count,
                    pending_value,
                } = frame.kind
                else {
                    return Err(malformed_json("JSON object end closes an array"));
                };
                if pending_value.is_some() {
                    return Err(malformed_json("JSON object key has no value"));
                }
                if frame.observed {
                    observer.object_end(walk_path(&path, frame.value), child_count);
                    observer.value_end();
                }
                path.truncate(frame.value.restore_path_len);
            }
            JsonEvent::EndArray => {
                let Some(frame) = frames.pop() else {
                    return Err(malformed_json("JSON array end has no matching start"));
                };
                let ContainerKind::Array { length } = frame.kind else {
                    return Err(malformed_json("JSON array end closes an object"));
                };
                if frame.observed {
                    observer.array_end(walk_path(&path, frame.value), length);
                    observer.value_end();
                }
                path.truncate(frame.value.restore_path_len);
            }
            JsonEvent::String(_) => scalar_event(
                input,
                event_call_start,
                input_offset,
                &mut path,
                &mut frames,
                &mut root_seen,
                observer,
                JsonType::String,
                cancelled,
            )?,
            JsonEvent::Number(_) => scalar_event(
                input,
                event_call_start,
                input_offset,
                &mut path,
                &mut frames,
                &mut root_seen,
                observer,
                JsonType::Number,
                cancelled,
            )?,
            JsonEvent::Boolean(_) => scalar_event(
                input,
                event_call_start,
                input_offset,
                &mut path,
                &mut frames,
                &mut root_seen,
                observer,
                JsonType::Boolean,
                cancelled,
            )?,
            JsonEvent::Null => scalar_event(
                input,
                event_call_start,
                input_offset,
                &mut path,
                &mut frames,
                &mut root_seen,
                observer,
                JsonType::Null,
                cancelled,
            )?,
            JsonEvent::Eof => {
                if !frames.is_empty() || !root_seen {
                    return Err(malformed_json("JSON document is incomplete"));
                }
                return Ok(());
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn scalar_event(
    input: &[u8],
    event_call_start: usize,
    event_end: usize,
    path: &mut String,
    frames: &mut [ContainerFrame],
    root_seen: &mut bool,
    observer: &mut impl JsonWalkObserver,
    kind: JsonType,
    cancelled: &AtomicBool,
) -> Result<(), ControlError> {
    let value = prepare_value_path(path, frames, root_seen, cancelled)?;
    if frames.last().is_none_or(|frame| frame.descend) {
        let view = walk_path(path, value);
        observer.value_start(view);
        let token_start = (observer.wants_scalar_encoded_bytes() || observer.wants_scalar_token())
            .then(|| source_token_start(input, event_call_start, event_end, cancelled))
            .transpose()?;
        let encoded_bytes = observer
            .wants_scalar_encoded_bytes()
            .then(|| event_end - token_start.expect("requested scalar token start"));
        let encoded_token = observer
            .wants_scalar_token()
            .then(|| &input[token_start.expect("requested scalar token start")..event_end]);
        observer.scalar(view, kind, encoded_bytes, encoded_token);
        observer.value_end();
    }
    path.truncate(value.restore_path_len);
    Ok(())
}

fn prepare_value_path(
    path: &mut String,
    frames: &mut [ContainerFrame],
    root_seen: &mut bool,
    cancelled: &AtomicBool,
) -> Result<ValuePathState, ControlError> {
    let Some(parent) = frames.last_mut() else {
        if *root_seen {
            return Err(malformed_json(
                "JSON document contains more than one root value",
            ));
        }
        *root_seen = true;
        return Ok(ValuePathState {
            restore_path_len: path.len(),
            segment_start: None,
            depth: 0,
        });
    };
    match &mut parent.kind {
        ContainerKind::Object { pending_value, .. } => pending_value
            .take()
            .ok_or_else(|| malformed_json("JSON object value has no key")),
        ContainerKind::Array { length } => {
            let restore_path_len = path.len();
            let index = length.to_string();
            let segment_start = append_pointer_segment(path, &index, cancelled)?;
            *length += 1;
            Ok(ValuePathState {
                restore_path_len,
                segment_start: Some(segment_start),
                depth: parent.value.depth + 1,
            })
        }
    }
}

fn walk_path(path: &str, value: ValuePathState) -> WalkPath<'_> {
    WalkPath {
        raw: path,
        segment: value.segment_start.map(|start| &path[start..]),
        depth: value.depth,
    }
}

fn safe_json_syntax_error(error: json_event_parser::JsonSyntaxError) -> ControlError {
    let location = error.location();
    ControlError::new(
        ControlErrorCode::MalformedJson,
        "capture body contains malformed JSON",
        false,
        serde_json::json!({
            "start": {
                "line": location.start.line,
                "column": location.start.column,
                "offset": location.start.offset,
            },
            "end": {
                "line": location.end.line,
                "column": location.end.column,
                "offset": location.end.offset,
            },
        }),
    )
}

fn source_token_start(
    input: &[u8],
    mut offset: usize,
    end: usize,
    cancelled: &AtomicBool,
) -> Result<usize, ControlError> {
    let mut scanned = 0usize;
    while offset < end && matches!(input[offset], b' ' | b'\n' | b'\r' | b'\t' | b':' | b',') {
        offset += 1;
        scanned += 1;
        if scanned.is_multiple_of(32 * 1_024) {
            check_json_cancellation(cancelled)?;
        }
    }
    check_json_cancellation(cancelled)?;
    Ok(offset)
}

fn validate_json_input(input: &[u8], cancelled: &AtomicBool) -> Result<(), ControlError> {
    check_json_cancellation(cancelled)?;
    if input.len() > MAX_JSON_INPUT_BYTES {
        return Err(ControlError::new(
            ControlErrorCode::JsonSizeLimit,
            "capture JSON body exceeds the decoded input limit",
            false,
            serde_json::json!({
                "maximum_bytes": MAX_JSON_INPUT_BYTES,
                "received_bytes": input.len(),
            }),
        ));
    }
    std::str::from_utf8(input).map_err(|_| {
        ControlError::new(
            ControlErrorCode::UndecodableBody,
            "capture body is not UTF-8 JSON text",
            false,
            serde_json::json!({}),
        )
    })?;
    Ok(())
}

fn check_json_cancellation(cancelled: &AtomicBool) -> Result<(), ControlError> {
    if cancelled.load(Ordering::Acquire) {
        Err(ControlError::cancelled("capture JSON traversal cancelled"))
    } else {
        Ok(())
    }
}

fn check_depth(depth: usize) -> Result<(), ControlError> {
    if depth <= MAX_JSON_DEPTH {
        return Ok(());
    }
    Err(ControlError::new(
        ControlErrorCode::JsonDepthLimit,
        "capture JSON nesting exceeds the traversal limit",
        false,
        serde_json::json!({"maximum_depth": MAX_JSON_DEPTH}),
    ))
}

fn append_pointer_segment(
    path: &mut String,
    segment: &str,
    cancelled: &AtomicBool,
) -> Result<usize, ControlError> {
    let restore = path.len();
    if restore == MAX_JSON_POINTER_BYTES {
        return Err(pointer_path_limit());
    }
    path.push('/');
    let segment_start = path.len();
    for (offset, character) in segment.char_indices() {
        if offset % (32 * 1_024) == 0 {
            check_json_cancellation(cancelled)?;
        }
        let mut utf8 = [0; 4];
        let encoded = match character {
            '~' => "~0",
            '/' => "~1",
            _ => character.encode_utf8(&mut utf8),
        };
        if path.len().saturating_add(encoded.len()) > MAX_JSON_POINTER_BYTES {
            path.truncate(restore);
            return Err(pointer_path_limit());
        }
        path.push_str(encoded);
    }
    Ok(segment_start)
}

fn pointer_path_limit() -> ControlError {
    ControlError::new(
        ControlErrorCode::ResourceLimit,
        "capture JSON pointer exceeds the traversal path limit",
        false,
        serde_json::json!({"maximum_pointer_bytes": MAX_JSON_POINTER_BYTES}),
    )
}

const FIELD_MATCH_WIRE_OVERHEAD: usize = 256;
const POINTER_EXAMPLE_WIRE_OVERHEAD: usize = 64;
const NEXT_HINT_WIRE_OVERHEAD: usize = 128;
const OBJECT_SUMMARY_WIRE_OVERHEAD: usize = 256;

#[derive(Default)]
struct WireBudget {
    used: usize,
}

impl WireBudget {
    fn reserve_string(&mut self, value: &str, structural_bytes: usize) -> bool {
        let bytes = json_string_wire_bytes(value).saturating_add(structural_bytes);
        let Some(next) = self.used.checked_add(bytes) else {
            return false;
        };
        if next > RETAINED_RESULT_WIRE_BUDGET {
            return false;
        }
        self.used = next;
        true
    }
}

struct FieldFinder<'a> {
    name: &'a str,
    mode: FieldMatchMode,
    limit: usize,
    next_value_matches: bool,
    matched_containers: Vec<bool>,
    matches: Vec<JsonFieldMatch>,
    total_matches: usize,
    budget: WireBudget,
    output_omitted: bool,
}

impl FieldFinder<'_> {
    fn key_matches(&self, key: &str) -> bool {
        match self.mode {
            FieldMatchMode::Exact => key == self.name,
            FieldMatchMode::UnicodeCasefoldExact => key.case_fold().eq(self.name.case_fold()),
        }
    }

    fn consume_pending_match(&mut self) -> bool {
        std::mem::take(&mut self.next_value_matches)
    }

    fn record(
        &mut self,
        path: WalkPath<'_>,
        value_type: JsonType,
        object_child_count: Option<usize>,
        array_length: Option<usize>,
        scalar_encoded_bytes: Option<usize>,
    ) {
        self.total_matches += 1;
        if self.matches.len() >= self.limit
            || !self
                .budget
                .reserve_string(path.raw, FIELD_MATCH_WIRE_OVERHEAD)
        {
            self.output_omitted = true;
            return;
        }
        self.matches.push(JsonFieldMatch {
            pointer: path.into_owned(),
            value_type,
            object_child_count,
            array_length,
            scalar_encoded_bytes,
        });
    }
}

impl JsonWalkObserver for FieldFinder<'_> {
    fn scalar(
        &mut self,
        path: WalkPath<'_>,
        kind: JsonType,
        encoded_bytes: Option<usize>,
        _encoded_token: Option<&[u8]>,
    ) {
        if self.consume_pending_match() {
            self.record(
                path,
                kind,
                None,
                None,
                Some(encoded_bytes.expect("matched scalar requests its source span")),
            );
        }
    }

    fn object_start(&mut self, _path: WalkPath<'_>) {
        let matched = self.consume_pending_match();
        self.matched_containers.push(matched);
    }

    fn object_end(&mut self, path: WalkPath<'_>, child_count: usize) {
        if self.matched_containers.pop().unwrap_or(false) {
            self.record(path, JsonType::Object, Some(child_count), None, None);
        }
    }

    fn array_start(&mut self, _path: WalkPath<'_>) {
        let matched = self.consume_pending_match();
        self.matched_containers.push(matched);
    }

    fn array_end(&mut self, path: WalkPath<'_>, length: usize) {
        if self.matched_containers.pop().unwrap_or(false) {
            self.record(path, JsonType::Array, None, Some(length), None);
        }
    }

    fn object_key(&mut self, _path: WalkPath<'_>, key: &str) {
        self.next_value_matches = self.key_matches(key);
    }

    fn wants_scalar_encoded_bytes(&self) -> bool {
        self.next_value_matches
    }

    fn should_descend(&self) -> bool {
        true
    }
}

pub(crate) fn find_json_pointers(
    input: &[u8],
    field_name: &str,
    mode: FieldMatchMode,
    limit: usize,
    cancelled: &AtomicBool,
) -> Result<FindJsonPointersResult, ControlError> {
    if field_name.len() > MAX_JSON_FIELD_NAME_BYTES {
        return Err(invalid_json_argument(
            "JSON field name exceeds the maximum length",
            serde_json::json!({"maximum_bytes": MAX_JSON_FIELD_NAME_BYTES}),
        ));
    }
    validate_result_limit(limit)?;
    let mut observer = FieldFinder {
        name: field_name,
        mode,
        limit,
        next_value_matches: false,
        matched_containers: Vec::new(),
        matches: Vec::with_capacity(limit),
        total_matches: 0,
        budget: WireBudget::default(),
        output_omitted: false,
    };
    walk_json(input, &mut observer, cancelled)?;
    let omitted_matches = observer.total_matches - observer.matches.len();
    let mut result = FindJsonPointersResult {
        status: if observer.total_matches == 0 {
            JsonInspectionStatus::NoMatch
        } else {
            JsonInspectionStatus::Matched
        },
        capture_revision: 0,
        matches: observer.matches,
        total_matches: observer.total_matches,
        omitted_matches,
        inspected_bytes: input.len(),
        source_truncated: false,
        truncated: observer.output_omitted || omitted_matches != 0,
    };
    enforce_find_result_size(&mut result)?;
    Ok(result)
}

#[derive(Clone, Copy)]
struct ObjectSummaryState {
    depth: usize,
    result_index: Option<usize>,
}

struct PatternProbe<'a> {
    pattern: &'a JsonPointerPattern,
    result: ProbeJsonPointerPatternResult,
    budget: WireBudget,
    prefix_stack: Vec<usize>,
    depth_stack: Vec<usize>,
    longest_prefix_segments: usize,
    longest_prefix_wire_bytes: usize,
    object_summary_stack: Vec<ObjectSummaryState>,
}

impl PatternProbe<'_> {
    fn current_prefix(&self) -> usize {
        self.prefix_stack.last().copied().unwrap_or(0)
    }

    fn is_match(&self, path: WalkPath<'_>) -> bool {
        path.depth == self.pattern.len() && self.current_prefix() == self.pattern.len()
    }

    fn terminal(&mut self, path: WalkPath<'_>, kind: JsonType) {
        self.record_child_shape(path, kind);
        self.consider_miss(path, kind);
        if !self.is_match(path) {
            return;
        }
        self.result.match_count += 1;
        self.result.type_counts.increment(kind);
        if self.result.examples.len() < MAX_JSON_EXAMPLES
            && self
                .budget
                .reserve_string(path.raw, POINTER_EXAMPLE_WIRE_OVERHEAD)
        {
            self.result.examples.push(path.into_owned());
        } else {
            self.result.omitted_examples += 1;
            self.result.truncated = true;
        }
    }

    fn consider_miss(&mut self, path: WalkPath<'_>, kind: JsonType) {
        let matching = self.current_prefix();
        if matching > self.longest_prefix_segments {
            self.longest_prefix_segments = matching;
            self.longest_prefix_wire_bytes = json_string_wire_bytes(path.raw);
            self.result.miss_hint = Some(JsonPointerMissHint {
                longest_matched_prefix: path.into_owned(),
                next_segments: Vec::new(),
                omitted_next_segments: 0,
            });
        }
        if matching != self.longest_prefix_segments || path.depth != matching + 1 {
            return;
        }
        let Some(raw_segment) = path.segment else {
            return;
        };
        let segment = decode_segment(raw_segment, false)
            .expect("walker paths always contain valid pointer escapes");
        let hint = self
            .result
            .miss_hint
            .get_or_insert_with(|| JsonPointerMissHint {
                longest_matched_prefix: JsonPointer(String::new()),
                next_segments: Vec::new(),
                omitted_next_segments: 0,
            });
        if hint
            .next_segments
            .iter()
            .any(|candidate| candidate.segment == segment && candidate.value_type == kind)
        {
            return;
        }
        if hint.next_segments.len() < MAX_JSON_HINTS
            && self
                .budget
                .reserve_string(&segment, NEXT_HINT_WIRE_OVERHEAD)
        {
            hint.next_segments.push(JsonNextSegmentHint {
                segment,
                value_type: kind,
            });
        } else {
            hint.omitted_next_segments += 1;
            self.result.truncated = true;
        }
    }

    fn record_child_shape(&mut self, path: WalkPath<'_>, kind: JsonType) {
        let Some(parent) = self.object_summary_stack.last().copied() else {
            return;
        };
        if parent.depth + 1 != path.depth {
            return;
        }
        let Some(index) = parent.result_index else {
            return;
        };
        let Some(raw_segment) = path.segment else {
            return;
        };
        let segment = decode_segment(raw_segment, false)
            .expect("walker paths always contain valid pointer escapes");
        let summary = &mut self.result.object_key_summaries[index];
        if summary.keys.len() < MAX_JSON_HINTS
            && self
                .budget
                .reserve_string(&segment, NEXT_HINT_WIRE_OVERHEAD)
        {
            summary.keys.push(JsonNextSegmentHint {
                segment,
                value_type: kind,
            });
        } else {
            summary.omitted_keys += 1;
            self.result.truncated = true;
        }
    }

    fn start_object_summary(&mut self, path: WalkPath<'_>) {
        if !self.is_match(path) {
            self.object_summary_stack.push(ObjectSummaryState {
                depth: path.depth,
                result_index: None,
            });
            return;
        }
        if self.result.object_key_summaries.len() >= MAX_JSON_EXAMPLES
            || !self
                .budget
                .reserve_string(path.raw, OBJECT_SUMMARY_WIRE_OVERHEAD)
        {
            self.result.omitted_object_summaries += 1;
            self.result.truncated = true;
            self.object_summary_stack.push(ObjectSummaryState {
                depth: path.depth,
                result_index: None,
            });
            return;
        }
        let index = self.result.object_key_summaries.len();
        self.result.object_key_summaries.push(JsonObjectKeySummary {
            pointer: path.into_owned(),
            child_count: 0,
            keys: Vec::new(),
            omitted_keys: 0,
        });
        self.object_summary_stack.push(ObjectSummaryState {
            depth: path.depth,
            result_index: Some(index),
        });
    }
}

impl JsonWalkObserver for PatternProbe<'_> {
    fn value_start(&mut self, path: WalkPath<'_>) {
        let parent_prefix = self.current_prefix();
        let prefix = if path.depth == 0 {
            0
        } else if parent_prefix == path.depth - 1
            && path
                .segment
                .is_some_and(|segment| self.pattern.segment_matches(parent_prefix, segment))
        {
            parent_prefix + 1
        } else {
            parent_prefix
        };
        self.prefix_stack.push(prefix);
        self.depth_stack.push(path.depth);
    }

    fn value_end(&mut self) {
        self.prefix_stack.pop();
        self.depth_stack.pop();
    }

    fn scalar(
        &mut self,
        path: WalkPath<'_>,
        kind: JsonType,
        _encoded_bytes: Option<usize>,
        _encoded_token: Option<&[u8]>,
    ) {
        self.terminal(path, kind);
    }

    fn object_start(&mut self, path: WalkPath<'_>) {
        self.terminal(path, JsonType::Object);
        self.start_object_summary(path);
    }

    fn object_end(&mut self, _path: WalkPath<'_>, child_count: usize) {
        if let Some(index) = self
            .object_summary_stack
            .pop()
            .and_then(|state| state.result_index)
        {
            self.result.object_key_summaries[index].child_count = child_count;
        }
    }

    fn array_start(&mut self, path: WalkPath<'_>) {
        self.terminal(path, JsonType::Array);
    }

    fn array_end(&mut self, path: WalkPath<'_>, length: usize) {
        if !self.is_match(path) {
            return;
        }
        self.result.array_length_range = Some(self.result.array_length_range.map_or(
            JsonArrayLengthRange {
                minimum: length,
                maximum: length,
            },
            |range| JsonArrayLengthRange {
                minimum: range.minimum.min(length),
                maximum: range.maximum.max(length),
            },
        ));
    }

    fn should_descend(&self) -> bool {
        let depth = self.depth_stack.last().copied().unwrap_or(0);
        (self.current_prefix() == depth && depth < self.pattern.len())
            || self
                .object_summary_stack
                .last()
                .is_some_and(|state| state.depth == depth && state.result_index.is_some())
    }
}

pub(crate) fn probe_json(
    input: &[u8],
    pattern: &JsonPointerPattern,
    cancelled: &AtomicBool,
) -> Result<ProbeJsonPointerPatternResult, ControlError> {
    let mut observer = PatternProbe {
        pattern,
        result: ProbeJsonPointerPatternResult {
            status: JsonInspectionStatus::NoMatch,
            capture_revision: 0,
            match_count: 0,
            type_counts: JsonTypeCounts::default(),
            examples: Vec::with_capacity(MAX_JSON_EXAMPLES),
            omitted_examples: 0,
            object_key_summaries: Vec::new(),
            omitted_object_summaries: 0,
            array_length_range: None,
            miss_hint: Some(JsonPointerMissHint {
                longest_matched_prefix: JsonPointer(String::new()),
                next_segments: Vec::new(),
                omitted_next_segments: 0,
            }),
            inspected_bytes: 0,
            source_truncated: false,
            truncated: false,
        },
        budget: WireBudget::default(),
        prefix_stack: Vec::with_capacity(MAX_JSON_DEPTH + 1),
        depth_stack: Vec::with_capacity(MAX_JSON_DEPTH + 1),
        longest_prefix_segments: 0,
        longest_prefix_wire_bytes: json_string_wire_bytes(""),
        object_summary_stack: Vec::new(),
    };
    walk_json(input, &mut observer, cancelled)?;
    observer.result.inspected_bytes = input.len();
    if observer.result.match_count != 0 {
        observer.result.status = JsonInspectionStatus::Matched;
        observer.result.miss_hint = None;
        observer.longest_prefix_wire_bytes = 0;
    }
    let charged_wire_bytes = observer
        .budget
        .used
        .saturating_add(observer.longest_prefix_wire_bytes);
    if charged_wire_bytes > MAX_JSON_RESULT_BYTES {
        observer.result.truncated = true;
    }
    enforce_probe_result_size(&mut observer.result)?;
    Ok(observer.result)
}

fn validate_result_limit(limit: usize) -> Result<(), ControlError> {
    if (1..=MAX_JSON_EXAMPLES).contains(&limit) {
        return Ok(());
    }
    Err(invalid_json_argument(
        "JSON result limit is out of range",
        serde_json::json!({
            "field": "limit",
            "minimum": 1,
            "maximum": MAX_JSON_EXAMPLES,
            "received": limit,
        }),
    ))
}

fn validate_pointer_text(raw: &str, pattern: bool) -> Result<(), ControlError> {
    if raw.len() > MAX_JSON_POINTER_BYTES {
        return Err(invalid_json_argument(
            "JSON pointer exceeds the maximum length",
            serde_json::json!({"maximum_bytes": MAX_JSON_POINTER_BYTES}),
        ));
    }
    if !raw.is_empty() && !raw.starts_with('/') {
        return Err(invalid_json_argument(
            "JSON pointer must be empty or start with '/'",
            serde_json::json!({}),
        ));
    }
    for segment in pointer_raw_segments(raw) {
        decode_segment(segment, pattern)?;
    }
    Ok(())
}

fn decode_segment(raw: &str, allow_literal_star: bool) -> Result<String, ControlError> {
    let mut decoded = String::with_capacity(raw.len());
    let mut characters = raw.chars();
    while let Some(character) = characters.next() {
        if character != '~' {
            decoded.push(character);
            continue;
        }
        match characters.next() {
            Some('0') => decoded.push('~'),
            Some('1') => decoded.push('/'),
            Some('2') if allow_literal_star => decoded.push('*'),
            _ => {
                return Err(invalid_json_argument(
                    "JSON pointer contains an invalid escape",
                    serde_json::json!({}),
                ));
            }
        }
    }
    Ok(decoded)
}

fn escape_pointer_segment(segment: &str) -> String {
    let mut escaped = String::with_capacity(segment.len());
    for character in segment.chars() {
        match character {
            '~' => escaped.push_str("~0"),
            '/' => escaped.push_str("~1"),
            _ => escaped.push(character),
        }
    }
    escaped
}

fn pointer_raw_segments(pointer: &str) -> impl DoubleEndedIterator<Item = &str> {
    pointer
        .strip_prefix('/')
        .into_iter()
        .flat_map(|remainder| remainder.split('/'))
}

fn pattern_segment_matches(expected: &PatternSegment, actual: &str) -> bool {
    match expected {
        PatternSegment::Wildcard => true,
        PatternSegment::Exact(expected) => expected == actual,
    }
}

fn json_string_wire_bytes(value: &str) -> usize {
    2 + value
        .chars()
        .map(|character| match character {
            '"' | '\\' | '\u{0008}' | '\u{000c}' | '\n' | '\r' | '\t' => 2,
            '\u{0000}'..='\u{001f}' => 6,
            _ => character.len_utf8(),
        })
        .sum::<usize>()
}

fn serialized_result_bytes(result: &impl Serialize) -> Result<usize, ControlError> {
    serde_json::to_vec(result)
        .map(|encoded| encoded.len())
        .map_err(|_| ControlError::internal("JSON probe result serialization failed"))
}

fn enforce_find_result_size(result: &mut FindJsonPointersResult) -> Result<(), ControlError> {
    while serialized_result_bytes(result)? > MAX_JSON_RESULT_BYTES {
        if result.matches.pop().is_none() {
            return Err(ControlError::internal(
                "bounded JSON field result metadata exceeds its wire limit",
            ));
        }
        result.omitted_matches += 1;
        result.truncated = true;
    }
    Ok(())
}

fn enforce_probe_result_size(
    result: &mut ProbeJsonPointerPatternResult,
) -> Result<(), ControlError> {
    while serialized_result_bytes(result)? > MAX_JSON_RESULT_BYTES {
        if result.examples.pop().is_some() {
            result.omitted_examples += 1;
        } else if let Some(summary) = result.object_key_summaries.last_mut()
            && summary.keys.pop().is_some()
        {
            summary.omitted_keys += 1;
        } else if result.object_key_summaries.pop().is_some() {
            result.omitted_object_summaries += 1;
        } else if let Some(hint) = &mut result.miss_hint
            && hint.next_segments.pop().is_some()
        {
            hint.omitted_next_segments += 1;
        } else if result.miss_hint.take().is_none() {
            return Err(ControlError::internal(
                "bounded JSON pattern result metadata exceeds its wire limit",
            ));
        }
        result.truncated = true;
    }
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct JsonSelection {
    pub(crate) bytes: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct JsonSelectionMiss {
    pub(crate) longest_valid_prefix: JsonPointer,
    pub(crate) next: Vec<JsonNextSegmentHint>,
    pub(crate) omitted_next: usize,
}

enum SelectedContainer {
    Object { first: bool },
    Array { first: bool },
}

struct JsonSelector<'a> {
    target: &'a JsonPointer,
    output: Vec<u8>,
    containers: Vec<SelectedContainer>,
    capturing: bool,
    selected: bool,
    error: Option<ControlError>,
    longest_prefix: JsonPointer,
    longest_depth: usize,
    next: Vec<JsonNextSegmentHint>,
    omitted_next: usize,
    cancelled: &'a AtomicBool,
}

impl JsonSelector<'_> {
    fn append(&mut self, bytes: &[u8]) {
        if self.error.is_some() {
            return;
        }
        for chunk in bytes.chunks(32 * 1_024) {
            if self.cancelled.load(Ordering::Relaxed) {
                self.error = Some(ControlError::cancelled("JSON extraction cancelled"));
                return;
            }
            if self.output.len().saturating_add(chunk.len()) > MAX_JSON_INPUT_BYTES {
                self.error = Some(ControlError::new(
                    ControlErrorCode::JsonSizeLimit,
                    "selected JSON representation exceeds the size limit",
                    false,
                    serde_json::json!({"maximum_bytes": MAX_JSON_INPUT_BYTES}),
                ));
                return;
            }
            self.output.extend_from_slice(chunk);
        }
    }

    fn begin_value(&mut self, path: WalkPath<'_>) {
        if !self.capturing && !self.selected && path.raw == self.target.as_str() {
            self.capturing = true;
            self.selected = true;
            return;
        }
        if self.capturing
            && let Some(SelectedContainer::Array { first }) = self.containers.last_mut()
        {
            let separator = if *first { b"" as &[u8] } else { b"," };
            *first = false;
            self.append(separator);
        }
    }

    fn note_value(&mut self, path: WalkPath<'_>, kind: JsonType) {
        let target = self.target.as_str();
        let is_prefix = path.raw.is_empty()
            || target == path.raw
            || target
                .strip_prefix(path.raw)
                .is_some_and(|suffix| suffix.starts_with('/'));
        if is_prefix && path.depth >= self.longest_depth {
            if path.depth > self.longest_depth || self.longest_prefix.as_str() != path.raw {
                self.longest_prefix = path.into_owned();
                self.longest_depth = path.depth;
                self.next.clear();
                self.omitted_next = 0;
            }
            return;
        }
        if path.depth != self.longest_depth.saturating_add(1)
            || parent_pointer(path.raw) != self.longest_prefix.as_str()
        {
            return;
        }
        let Some(segment) = path.segment else {
            return;
        };
        if self.next.len() >= MAX_JSON_HINTS {
            self.omitted_next += 1;
            return;
        }
        let segment = decode_segment(segment, false).unwrap_or_else(|_| segment.to_owned());
        self.next.push(JsonNextSegmentHint {
            segment,
            value_type: kind,
        });
    }

    fn finish_value(&mut self) {
        if self.capturing && self.containers.is_empty() {
            self.capturing = false;
        }
    }
}

impl JsonWalkObserver for JsonSelector<'_> {
    fn value_start(&mut self, path: WalkPath<'_>) {
        self.begin_value(path);
    }

    fn value_end(&mut self) {
        self.finish_value();
    }

    fn scalar(
        &mut self,
        path: WalkPath<'_>,
        kind: JsonType,
        _encoded_bytes: Option<usize>,
        encoded_token: Option<&[u8]>,
    ) {
        self.note_value(path, kind);
        if self.capturing {
            if let Some(token) = encoded_token {
                self.append(token);
            } else {
                self.error = Some(ControlError::internal(
                    "JSON extraction did not receive a scalar token",
                ));
            }
        }
    }

    fn object_start(&mut self, path: WalkPath<'_>) {
        self.note_value(path, JsonType::Object);
        if self.capturing {
            self.append(b"{");
            self.containers
                .push(SelectedContainer::Object { first: true });
        }
    }

    fn object_end(&mut self, _path: WalkPath<'_>, _child_count: usize) {
        if self.capturing
            && matches!(
                self.containers.last(),
                Some(SelectedContainer::Object { .. })
            )
        {
            self.append(b"}");
            self.containers.pop();
        }
    }

    fn array_start(&mut self, path: WalkPath<'_>) {
        self.note_value(path, JsonType::Array);
        if self.capturing {
            self.append(b"[");
            self.containers
                .push(SelectedContainer::Array { first: true });
        }
    }

    fn array_end(&mut self, _path: WalkPath<'_>, _length: usize) {
        if self.capturing
            && matches!(
                self.containers.last(),
                Some(SelectedContainer::Array { .. })
            )
        {
            self.append(b"]");
            self.containers.pop();
        }
    }

    fn object_key(&mut self, _path: WalkPath<'_>, key: &str) {
        if !self.capturing {
            return;
        }
        let Some(SelectedContainer::Object { first }) = self.containers.last_mut() else {
            return;
        };
        let separator = if *first { b"" as &[u8] } else { b"," };
        *first = false;
        self.append(separator);
        match serde_json::to_vec(key) {
            Ok(encoded) => self.append(&encoded),
            Err(_) => {
                self.error = Some(ControlError::internal(
                    "failed to encode selected JSON object key",
                ));
                return;
            }
        }
        self.append(b":");
    }

    fn wants_scalar_token(&self) -> bool {
        true
    }

    fn should_descend(&self) -> bool {
        self.error.is_none()
    }
}

pub(crate) fn extract_json_selection(
    input: &[u8],
    pointer: &JsonPointer,
    cancelled: &AtomicBool,
) -> Result<Result<JsonSelection, JsonSelectionMiss>, ControlError> {
    let mut selector = JsonSelector {
        target: pointer,
        output: Vec::new(),
        containers: Vec::with_capacity(16),
        capturing: false,
        selected: false,
        error: None,
        longest_prefix: JsonPointer(String::new()),
        longest_depth: 0,
        next: Vec::with_capacity(MAX_JSON_HINTS),
        omitted_next: 0,
        cancelled,
    };
    walk_json(input, &mut selector, cancelled)?;
    if let Some(error) = selector.error {
        return Err(error);
    }
    if selector.selected {
        return Ok(Ok(JsonSelection {
            bytes: selector.output,
        }));
    }
    Ok(Err(JsonSelectionMiss {
        longest_valid_prefix: selector.longest_prefix,
        next: selector.next,
        omitted_next: selector.omitted_next,
    }))
}

fn parent_pointer(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(parent, _)| parent)
}

fn malformed_json(message: &'static str) -> ControlError {
    ControlError::new(
        ControlErrorCode::MalformedJson,
        message,
        false,
        serde_json::json!({}),
    )
}

fn invalid_json_argument(message: impl Into<String>, details: serde_json::Value) -> ControlError {
    ControlError::new(ControlErrorCode::InvalidArgument, message, false, details)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicBool;

    use serde_json::json;

    use super::*;

    fn not_cancelled() -> AtomicBool {
        AtomicBool::new(false)
    }

    #[test]
    fn strict_pointer_parsing_round_trips_rfc_6901_escapes() {
        for (raw, segments) in [
            ("", vec![]),
            ("/", vec![""]),
            ("/a~1b/m~0n", vec!["a/b", "m~n"]),
            ("/0", vec!["0"]),
        ] {
            let pointer = JsonPointer::parse(raw).expect("valid pointer");
            assert_eq!(pointer.segments().collect::<Vec<_>>(), segments);
            assert_eq!(pointer.as_str(), raw);
        }
        for invalid in ["root", "/~", "/~2", "/~x"] {
            assert!(JsonPointer::parse(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn wildcard_literal_star_and_empty_key_have_distinct_semantics() {
        let json = br#"{"items":{"*":1,"a":2},"empty":{"":3}}"#;
        let wildcard = probe_json(
            json,
            &JsonPointerPattern::parse("/items/*").expect("pattern"),
            &not_cancelled(),
        )
        .expect("probe");
        assert_eq!(wildcard.match_count, 2);
        let literal = probe_json(
            json,
            &JsonPointerPattern::parse("/items/~2").expect("pattern"),
            &not_cancelled(),
        )
        .expect("probe");
        assert_eq!(
            literal.examples,
            vec![JsonPointer::parse("/items/*").expect("pointer")]
        );
        let empty = probe_json(
            json,
            &JsonPointerPattern::parse("/empty/").expect("pattern"),
            &not_cancelled(),
        )
        .expect("probe");
        assert_eq!(
            empty.examples,
            vec![JsonPointer::parse("/empty/").expect("pointer")]
        );
        assert!(JsonPointerPattern::parse("/**").is_err());
    }

    #[test]
    fn exact_pattern_probes_root_arrays_and_escaped_object_keys() {
        let json = br#"{"a/b":{"~":[true]},"zero":{"0":"array-like"}}"#;
        for (pattern, expected_type) in [
            ("", JsonType::Object),
            ("/a~1b/~0/0", JsonType::Boolean),
            ("/zero/0", JsonType::String),
        ] {
            let result = probe_json(
                json,
                &JsonPointerPattern::parse(pattern).expect("pattern"),
                &not_cancelled(),
            )
            .expect("probe");
            assert_eq!(result.match_count, 1);
            assert_eq!(result.type_counts.count(expected_type), 1);
        }
    }

    #[test]
    fn field_finder_returns_only_metadata_and_supports_unicode_casefold() {
        let json = br#"{"orders":[{"UserId":"secret-value"}],"STRASSE":{},"array":["UserId"]}"#;
        let exact = find_json_pointers(
            json,
            "userId",
            FieldMatchMode::UnicodeCasefoldExact,
            20,
            &not_cancelled(),
        )
        .expect("find");
        assert_eq!(exact.total_matches, 1);
        assert_eq!(exact.matches[0].pointer.as_str(), "/orders/0/UserId");
        assert_eq!(exact.matches[0].value_type, JsonType::String);
        assert!(exact.matches[0].scalar_encoded_bytes.is_some());
        let folded = find_json_pointers(
            json,
            "straße",
            FieldMatchMode::UnicodeCasefoldExact,
            20,
            &not_cancelled(),
        )
        .expect("find");
        assert_eq!(folded.matches[0].pointer.as_str(), "/STRASSE");
        assert_eq!(folded.matches[0].object_child_count, Some(0));
        let serialized = serde_json::to_string(&(exact, folded)).expect("json");
        assert!(!serialized.contains("secret-value"));
    }

    #[test]
    fn field_and_probe_outputs_are_bounded_while_totals_remain_exact() {
        let mut json = String::from("{");
        for index in 0..1_000 {
            if index != 0 {
                json.push(',');
            }
            json.push_str(&format!("\"k{index}\":{{\"needle\":{index}}}"));
        }
        json.push('}');
        let found = find_json_pointers(
            json.as_bytes(),
            "needle",
            FieldMatchMode::Exact,
            20,
            &not_cancelled(),
        )
        .expect("find");
        assert_eq!(found.total_matches, 1_000);
        assert_eq!(found.matches.len(), MAX_JSON_EXAMPLES);
        assert_eq!(found.omitted_matches, 1_000 - MAX_JSON_EXAMPLES);
        assert!(found.truncated);
        assert!(serde_json::to_vec(&found).expect("serialize").len() <= MAX_JSON_RESULT_BYTES);

        let probed = probe_json(
            json.as_bytes(),
            &JsonPointerPattern::parse("/*/needle").expect("pattern"),
            &not_cancelled(),
        )
        .expect("probe");
        assert_eq!(probed.match_count, 1_000);
        assert_eq!(probed.examples.len(), MAX_JSON_EXAMPLES);
        assert!(probed.truncated);
        assert!(serde_json::to_vec(&probed).expect("serialize").len() <= MAX_JSON_RESULT_BYTES);
    }

    #[test]
    fn matched_objects_arrays_and_misses_return_bounded_shape_metadata() {
        let json = br#"{"groups":[{"a":1,"b":true},{"c":[],"d":{}}]}"#;
        let groups = probe_json(
            json,
            &JsonPointerPattern::parse("/groups/*").expect("pattern"),
            &not_cancelled(),
        )
        .expect("probe");
        assert_eq!(groups.match_count, 2);
        assert_eq!(groups.object_key_summaries.len(), 2);
        assert!(
            groups
                .object_key_summaries
                .iter()
                .all(|summary| summary.keys.len() <= MAX_JSON_HINTS)
        );

        let arrays = probe_json(
            json,
            &JsonPointerPattern::parse("/groups/0/c").expect("pattern"),
            &not_cancelled(),
        )
        .expect("probe");
        assert_eq!(arrays.array_length_range, None);

        let miss = probe_json(
            json,
            &JsonPointerPattern::parse("/groups/0/missing").expect("pattern"),
            &not_cancelled(),
        )
        .expect("probe");
        assert_eq!(miss.status, JsonInspectionStatus::NoMatch);
        let hint = miss.miss_hint.expect("miss hint");
        assert_eq!(hint.longest_matched_prefix.as_str(), "/groups/0");
        assert!(hint.next_segments.len() <= MAX_JSON_HINTS);
        assert!(hint.next_segments.iter().any(|next| next.segment == "a"));
        assert!(!serde_json::to_string(&hint).expect("json").contains("true"));
    }

    #[test]
    fn malformed_binary_size_depth_and_cancellation_have_stable_codes() {
        let malformed = probe_json(
            br#"{"a":]"#,
            &JsonPointerPattern::parse("").expect("pattern"),
            &not_cancelled(),
        )
        .expect_err("malformed");
        assert_eq!(malformed.code, ControlErrorCode::MalformedJson);

        let binary = probe_json(
            &[0xff, 0xfe],
            &JsonPointerPattern::parse("").expect("pattern"),
            &not_cancelled(),
        )
        .expect_err("binary");
        assert_eq!(binary.code, ControlErrorCode::UndecodableBody);

        let oversized = vec![b' '; MAX_JSON_INPUT_BYTES + 1];
        let size = probe_json(
            &oversized,
            &JsonPointerPattern::parse("").expect("pattern"),
            &not_cancelled(),
        )
        .expect_err("size");
        assert_eq!(size.code, ControlErrorCode::JsonSizeLimit);

        let mut deep = vec![b'['; MAX_JSON_DEPTH + 1];
        deep.extend(std::iter::repeat_n(b']', MAX_JSON_DEPTH + 1));
        let depth = probe_json(
            &deep,
            &JsonPointerPattern::parse("").expect("pattern"),
            &not_cancelled(),
        )
        .expect_err("depth");
        assert_eq!(depth.code, ControlErrorCode::JsonDepthLimit);

        let cancelled = AtomicBool::new(true);
        let cancellation = probe_json(
            br#"{"a":1}"#,
            &JsonPointerPattern::parse("").expect("pattern"),
            &cancelled,
        )
        .expect_err("cancelled");
        assert_eq!(cancellation.code, ControlErrorCode::Cancelled);
    }

    #[test]
    fn result_serialization_never_contains_scalar_or_string_values() {
        let secret = "TASK11-NOT-IN-OUTPUT-98f9";
        let json = format!("{{\"password\":\"{secret}\",\"enabled\":true,\"count\":923847}} ");
        let find = find_json_pointers(
            json.as_bytes(),
            "password",
            FieldMatchMode::Exact,
            20,
            &not_cancelled(),
        )
        .expect("find");
        let probe = probe_json(
            json.as_bytes(),
            &JsonPointerPattern::parse("/*").expect("pattern"),
            &not_cancelled(),
        )
        .expect("probe");
        let output = serde_json::to_string(&json!({"find": find, "probe": probe})).expect("json");
        assert!(!output.contains(secret));
        assert!(!output.contains("923847"));
    }
    #[test]
    fn scalar_encoded_bytes_are_exact_source_token_spans() {
        let json = r#"{"s":"é\n","n":-1.20e+3,"b":false,"z":null}"#.as_bytes();
        for (field, expected) in [("s", 6), ("n", 8), ("b", 5), ("z", 4)] {
            let result =
                find_json_pointers(json, field, FieldMatchMode::Exact, 1, &not_cancelled())
                    .expect("find");
            assert_eq!(
                result.matches[0].scalar_encoded_bytes,
                Some(expected),
                "{field}"
            );
        }
    }

    #[test]
    fn exact_non_matching_pattern_handles_maximum_depth_without_observer_growth() {
        let mut json = vec![b'[', b'0', b']'];
        for _ in 1..MAX_JSON_DEPTH {
            let mut parent = Vec::with_capacity(json.len() + 2);
            parent.push(b'[');
            parent.extend_from_slice(&json);
            parent.push(b']');
            json = parent;
        }
        let result = probe_json(
            &json,
            &JsonPointerPattern::parse("/not-an-index").expect("pattern"),
            &not_cancelled(),
        )
        .expect("probe");
        assert_eq!(result.status, JsonInspectionStatus::NoMatch);
        assert!(result.examples.is_empty());
        assert!(result.object_key_summaries.is_empty());
    }

    #[test]
    fn object_summaries_include_only_direct_children() {
        let result = probe_json(
            br#"{"target":{"scalar":1,"nested":{"secret":2},"array":[3]}}"#,
            &JsonPointerPattern::parse("/target").expect("pattern"),
            &not_cancelled(),
        )
        .expect("probe");
        let summary = &result.object_key_summaries[0];
        assert_eq!(summary.child_count, 3);
        assert_eq!(
            summary
                .keys
                .iter()
                .map(|hint| hint.segment.as_str())
                .collect::<Vec<_>>(),
            ["scalar", "nested", "array"]
        );
        assert!(!summary.keys.iter().any(|hint| hint.segment == "secret"));
    }

    #[test]
    fn escaped_output_strings_are_charged_to_the_wire_budget() {
        let key = "\"".repeat(31 * 1_024);
        let mut object = serde_json::Map::new();
        object.insert(key, json!(1));
        let json = serde_json::to_vec(&object).expect("json");
        let result = probe_json(
            &json,
            &JsonPointerPattern::parse("/*").expect("pattern"),
            &not_cancelled(),
        )
        .expect("probe");
        assert_eq!(result.match_count, 1);
        assert_eq!(result.examples.len(), 0);
        assert_eq!(result.omitted_examples, 1);
        assert!(result.truncated);
        assert!(serde_json::to_vec(&result).expect("result").len() <= MAX_JSON_RESULT_BYTES);
    }

    #[test]
    fn miss_hint_churn_remains_bounded_after_json_escaping() {
        let mut object = serde_json::Map::new();
        for index in 0..MAX_JSON_HINTS {
            object.insert(format!("{}-{index}", "\"".repeat(2_000)), json!(index));
        }
        let json = serde_json::to_vec(&json!({"matched": object})).expect("json");
        let result = probe_json(
            &json,
            &JsonPointerPattern::parse("/matched/missing").expect("pattern"),
            &not_cancelled(),
        )
        .expect("probe");
        let hint = result.miss_hint.expect("hint");
        assert_eq!(hint.longest_matched_prefix.as_str(), "/matched");
        assert!(hint.next_segments.len() < MAX_JSON_HINTS);
        assert!(hint.omitted_next_segments > 0);
        assert!(result.truncated);
    }

    #[test]
    fn traversal_rejects_an_encoded_pointer_beyond_its_retained_limit() {
        let key = "a".repeat(MAX_JSON_POINTER_BYTES);
        let mut object = serde_json::Map::new();
        object.insert(key, serde_json::Value::Null);
        let json = serde_json::to_vec(&object).expect("json");
        let error = probe_json(
            &json,
            &JsonPointerPattern::parse("/*").expect("pattern"),
            &not_cancelled(),
        )
        .expect_err("path limit");
        assert_eq!(error.code, ControlErrorCode::ResourceLimit);
    }
}
