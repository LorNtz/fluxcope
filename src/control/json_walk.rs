use std::{
    fmt,
    str::FromStr,
    sync::atomic::{AtomicBool, Ordering},
};

use json_event_parser::{JsonEvent, SliceJsonParser};
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
#[cfg(test)]
pub(crate) const MAX_JSON_RESULT_BYTES: usize = 64 * 1_024;
const MAX_JSON_POINTER_BYTES: usize = 64 * 1_024;
const MAX_JSON_PATTERN_BYTES: usize = 4 * 1_024;
const MAX_JSON_FIELD_NAME_BYTES: usize = 4 * 1_024;
const RETAINED_RESULT_TEXT_BUDGET: usize = 32 * 1_024;

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

    fn matches(&self, pointer: &JsonPointer) -> bool {
        let mut actual = pointer_raw_segments(pointer.as_str());
        for expected in &self.segments {
            let Some(segment) = actual.next() else {
                return false;
            };
            if !pattern_segment_matches(expected, segment) {
                return false;
            }
        }
        actual.next().is_none()
    }

    fn matching_prefix_len(&self, pointer: &JsonPointer) -> usize {
        self.segments
            .iter()
            .zip(pointer_raw_segments(pointer.as_str()))
            .take_while(|(expected, actual)| pattern_segment_matches(expected, actual))
            .count()
    }

    fn can_descend_from(&self, pointer: &JsonPointer) -> bool {
        let depth = pointer_raw_segments(pointer.as_str()).count();
        depth < self.segments.len() && self.matching_prefix_len(pointer) == depth
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

pub(crate) trait JsonWalkObserver {
    fn scalar(&mut self, path: &JsonPointer, kind: JsonType, encoded_bytes: usize);
    fn object_start(&mut self, path: &JsonPointer);
    fn object_end(&mut self, path: &JsonPointer, child_count: usize);
    fn array_start(&mut self, path: &JsonPointer);
    fn array_end(&mut self, path: &JsonPointer, length: usize);
    fn object_key(&mut self, _path: &JsonPointer, _key: &str) {}
    fn should_descend(&self, path: &JsonPointer) -> bool;
}

enum ContainerKind {
    Object {
        child_count: usize,
        pending_restore: Option<usize>,
    },
    Array {
        length: usize,
    },
}

struct ContainerFrame {
    kind: ContainerKind,
    restore_path_len: usize,
    observed: bool,
    descend: bool,
}

fn walk_json(
    input: &[u8],
    observer: &mut impl JsonWalkObserver,
    cancelled: &AtomicBool,
) -> Result<(), ControlError> {
    validate_json_input(input, cancelled)?;
    let mut parser = SliceJsonParser::new(input);
    let mut path = String::new();
    let mut frames = Vec::<ContainerFrame>::new();
    let mut root_seen = false;
    loop {
        check_json_cancellation(cancelled)?;
        let event = parser.parse_next().map_err(|error| {
            ControlError::new(
                ControlErrorCode::MalformedJson,
                "capture body contains malformed JSON",
                false,
                serde_json::json!({"parser_error": error.to_string()}),
            )
        })?;
        match event {
            JsonEvent::ObjectKey(key) => {
                let Some(frame) = frames.last_mut() else {
                    return Err(malformed_json("JSON object key appears outside an object"));
                };
                let ContainerKind::Object {
                    child_count,
                    pending_restore,
                } = &mut frame.kind
                else {
                    return Err(malformed_json("JSON object key appears inside an array"));
                };
                if pending_restore.is_some() {
                    return Err(malformed_json("JSON object key has no preceding value"));
                }
                let restore = path.len();
                append_pointer_segment(&mut path, &key)?;
                *pending_restore = Some(restore);
                *child_count += 1;
                if frame.descend {
                    observer.object_key(&JsonPointer(path.clone()), &key);
                }
            }
            JsonEvent::StartObject => {
                let restore = prepare_value_path(&mut path, &mut frames, &mut root_seen)?;
                check_depth(frames.len() + 1)?;
                let pointer = JsonPointer(path.clone());
                let parent_descends = frames.last().is_none_or(|frame| frame.descend);
                if parent_descends {
                    observer.object_start(&pointer);
                }
                let descend = parent_descends && observer.should_descend(&pointer);
                frames.push(ContainerFrame {
                    kind: ContainerKind::Object {
                        child_count: 0,
                        pending_restore: None,
                    },
                    restore_path_len: restore,
                    descend,
                    observed: parent_descends,
                });
            }
            JsonEvent::StartArray => {
                let restore = prepare_value_path(&mut path, &mut frames, &mut root_seen)?;
                check_depth(frames.len() + 1)?;
                let pointer = JsonPointer(path.clone());
                let parent_descends = frames.last().is_none_or(|frame| frame.descend);
                if parent_descends {
                    observer.array_start(&pointer);
                }
                let descend = parent_descends && observer.should_descend(&pointer);
                frames.push(ContainerFrame {
                    kind: ContainerKind::Array { length: 0 },
                    restore_path_len: restore,
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
                    pending_restore,
                } = frame.kind
                else {
                    return Err(malformed_json("JSON object end closes an array"));
                };
                if pending_restore.is_some() {
                    return Err(malformed_json("JSON object key has no value"));
                }
                if frame.observed {
                    observer.object_end(&JsonPointer(path.clone()), child_count);
                }
                path.truncate(frame.restore_path_len);
            }
            JsonEvent::EndArray => {
                let Some(frame) = frames.pop() else {
                    return Err(malformed_json("JSON array end has no matching start"));
                };
                let ContainerKind::Array { length } = frame.kind else {
                    return Err(malformed_json("JSON array end closes an object"));
                };
                if frame.observed {
                    observer.array_end(&JsonPointer(path.clone()), length);
                }
                path.truncate(frame.restore_path_len);
            }
            JsonEvent::String(value) => scalar_event(
                &mut path,
                &mut frames,
                &mut root_seen,
                observer,
                JsonType::String,
                encoded_string_bytes(&value),
            )?,
            JsonEvent::Number(value) => scalar_event(
                &mut path,
                &mut frames,
                &mut root_seen,
                observer,
                JsonType::Number,
                value.len(),
            )?,
            JsonEvent::Boolean(value) => scalar_event(
                &mut path,
                &mut frames,
                &mut root_seen,
                observer,
                JsonType::Boolean,
                if value { 4 } else { 5 },
            )?,
            JsonEvent::Null => scalar_event(
                &mut path,
                &mut frames,
                &mut root_seen,
                observer,
                JsonType::Null,
                4,
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

fn scalar_event(
    path: &mut String,
    frames: &mut [ContainerFrame],
    root_seen: &mut bool,
    observer: &mut impl JsonWalkObserver,
    kind: JsonType,
    encoded_bytes: usize,
) -> Result<(), ControlError> {
    let restore = prepare_value_path(path, frames, root_seen)?;
    if frames.last().is_none_or(|frame| frame.descend) {
        observer.scalar(&JsonPointer(path.clone()), kind, encoded_bytes);
    }
    path.truncate(restore);
    Ok(())
}

fn prepare_value_path(
    path: &mut String,
    frames: &mut [ContainerFrame],
    root_seen: &mut bool,
) -> Result<usize, ControlError> {
    let Some(parent) = frames.last_mut() else {
        if *root_seen {
            return Err(malformed_json(
                "JSON document contains more than one root value",
            ));
        }
        *root_seen = true;
        return Ok(path.len());
    };
    match &mut parent.kind {
        ContainerKind::Object {
            pending_restore, ..
        } => pending_restore
            .take()
            .ok_or_else(|| malformed_json("JSON object value has no key")),
        ContainerKind::Array { length } => {
            let restore = path.len();
            append_pointer_segment(path, &length.to_string())?;
            *length += 1;
            Ok(restore)
        }
    }
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

fn append_pointer_segment(path: &mut String, segment: &str) -> Result<(), ControlError> {
    let escaped_bytes = segment.bytes().fold(0usize, |size, byte| {
        size.saturating_add(if matches!(byte, b'~' | b'/') { 2 } else { 1 })
    });
    let next_len = path.len().saturating_add(1).saturating_add(escaped_bytes);
    if next_len > MAX_JSON_POINTER_BYTES {
        return Err(ControlError::new(
            ControlErrorCode::ResourceLimit,
            "capture JSON pointer exceeds the traversal path limit",
            false,
            serde_json::json!({"maximum_pointer_bytes": MAX_JSON_POINTER_BYTES}),
        ));
    }
    path.push('/');
    for character in segment.chars() {
        match character {
            '~' => path.push_str("~0"),
            '/' => path.push_str("~1"),
            _ => path.push(character),
        }
    }
    Ok(())
}

fn encoded_string_bytes(value: &str) -> usize {
    2 + value
        .chars()
        .map(|character| match character {
            '"' | '\\' | '\u{0008}' | '\u{000c}' | '\n' | '\r' | '\t' => 2,
            '\u{0000}'..='\u{001f}' => 6,
            _ => character.len_utf8(),
        })
        .sum::<usize>()
}

struct FieldFinder<'a> {
    name: &'a str,
    mode: FieldMatchMode,
    limit: usize,
    next_value_matches: bool,
    matched_containers: Vec<bool>,
    matches: Vec<JsonFieldMatch>,
    total_matches: usize,
    retained_text_bytes: usize,
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
        path: &JsonPointer,
        value_type: JsonType,
        object_child_count: Option<usize>,
        array_length: Option<usize>,
        scalar_encoded_bytes: Option<usize>,
    ) {
        self.total_matches += 1;
        if self.matches.len() >= self.limit
            || !reserve_text(&mut self.retained_text_bytes, path.as_str().len())
        {
            self.output_omitted = true;
            return;
        }
        self.matches.push(JsonFieldMatch {
            pointer: path.clone(),
            value_type,
            object_child_count,
            array_length,
            scalar_encoded_bytes,
        });
    }
}

impl JsonWalkObserver for FieldFinder<'_> {
    fn scalar(&mut self, path: &JsonPointer, kind: JsonType, encoded_bytes: usize) {
        if self.consume_pending_match() {
            self.record(path, kind, None, None, Some(encoded_bytes));
        }
    }

    fn object_start(&mut self, _path: &JsonPointer) {
        let matched = self.consume_pending_match();
        self.matched_containers.push(matched);
    }

    fn object_end(&mut self, path: &JsonPointer, child_count: usize) {
        if self.matched_containers.pop().unwrap_or(false) {
            self.record(path, JsonType::Object, Some(child_count), None, None);
        }
    }

    fn array_start(&mut self, _path: &JsonPointer) {
        let matched = self.consume_pending_match();
        self.matched_containers.push(matched);
    }

    fn array_end(&mut self, path: &JsonPointer, length: usize) {
        if self.matched_containers.pop().unwrap_or(false) {
            self.record(path, JsonType::Array, None, Some(length), None);
        }
    }

    fn object_key(&mut self, _path: &JsonPointer, key: &str) {
        self.next_value_matches = self.key_matches(key);
    }

    fn should_descend(&self, _path: &JsonPointer) -> bool {
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
        retained_text_bytes: 0,
        output_omitted: false,
    };
    walk_json(input, &mut observer, cancelled)?;
    let omitted_matches = observer.total_matches - observer.matches.len();
    Ok(FindJsonPointersResult {
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
    })
}

struct PatternProbe<'a> {
    pattern: &'a JsonPointerPattern,
    result: ProbeJsonPointerPatternResult,
    retained_text_bytes: usize,
    longest_prefix_segments: usize,
    object_summary_stack: Vec<Option<usize>>,
}

impl PatternProbe<'_> {
    fn terminal(&mut self, path: &JsonPointer, kind: JsonType) {
        self.record_child_shape(path, kind);
        self.consider_miss(path, kind);
        if !self.pattern.matches(path) {
            return;
        }
        self.result.match_count += 1;
        self.result.type_counts.increment(kind);
        if self.result.examples.len() < MAX_JSON_EXAMPLES
            && reserve_text(&mut self.retained_text_bytes, path.as_str().len())
        {
            self.result.examples.push(path.clone());
        } else {
            self.result.omitted_examples += 1;
            self.result.truncated = true;
        }
    }

    fn consider_miss(&mut self, path: &JsonPointer, kind: JsonType) {
        let matching = self.pattern.matching_prefix_len(path);
        if matching > self.longest_prefix_segments {
            self.longest_prefix_segments = matching;
            self.result.miss_hint = Some(JsonPointerMissHint {
                longest_matched_prefix: JsonPointer(
                    pointer_prefix(path.as_str(), matching).to_owned(),
                ),
                next_segments: Vec::new(),
                omitted_next_segments: 0,
            });
        }
        let depth = pointer_raw_segments(path.as_str()).count();
        if matching != self.longest_prefix_segments || depth != matching + 1 {
            return;
        }
        let prefix = pointer_prefix(path.as_str(), matching);
        let hint = self
            .result
            .miss_hint
            .get_or_insert_with(|| JsonPointerMissHint {
                longest_matched_prefix: JsonPointer(prefix.to_owned()),
                next_segments: Vec::new(),
                omitted_next_segments: 0,
            });
        let Some(raw_segment) = pointer_raw_segments(path.as_str()).nth(matching) else {
            return;
        };
        let segment = decode_segment(raw_segment, false)
            .expect("walker paths always contain valid pointer escapes");
        if hint
            .next_segments
            .iter()
            .any(|candidate| candidate.segment == segment && candidate.value_type == kind)
        {
            return;
        }
        if hint.next_segments.len() < MAX_JSON_HINTS
            && reserve_text(&mut self.retained_text_bytes, segment.len())
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

    fn record_child_shape(&mut self, path: &JsonPointer, kind: JsonType) {
        let parent = pointer_parent(path.as_str());
        let Some(summary) = self
            .result
            .object_key_summaries
            .iter_mut()
            .find(|summary| summary.pointer.as_str() == parent)
        else {
            return;
        };
        let Some(raw_segment) = pointer_raw_segments(path.as_str()).next_back() else {
            return;
        };
        let segment = decode_segment(raw_segment, false)
            .expect("walker paths always contain valid pointer escapes");
        if summary.keys.len() < MAX_JSON_HINTS
            && reserve_text(&mut self.retained_text_bytes, segment.len())
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

    fn start_object_summary(&mut self, path: &JsonPointer) {
        if !self.pattern.matches(path) {
            self.object_summary_stack.push(None);
            return;
        }
        if self.result.object_key_summaries.len() >= MAX_JSON_EXAMPLES
            || !reserve_text(&mut self.retained_text_bytes, path.as_str().len())
        {
            self.result.omitted_object_summaries += 1;
            self.result.truncated = true;
            self.object_summary_stack.push(None);
            return;
        }
        let index = self.result.object_key_summaries.len();
        self.result.object_key_summaries.push(JsonObjectKeySummary {
            pointer: path.clone(),
            child_count: 0,
            keys: Vec::new(),
            omitted_keys: 0,
        });
        self.object_summary_stack.push(Some(index));
    }
}

impl JsonWalkObserver for PatternProbe<'_> {
    fn scalar(&mut self, path: &JsonPointer, kind: JsonType, _encoded_bytes: usize) {
        self.terminal(path, kind);
    }

    fn object_start(&mut self, path: &JsonPointer) {
        self.terminal(path, JsonType::Object);
        self.start_object_summary(path);
    }

    fn object_end(&mut self, _path: &JsonPointer, child_count: usize) {
        if let Some(index) = self.object_summary_stack.pop().flatten() {
            self.result.object_key_summaries[index].child_count = child_count;
        }
    }

    fn array_start(&mut self, path: &JsonPointer) {
        self.terminal(path, JsonType::Array);
    }

    fn array_end(&mut self, path: &JsonPointer, length: usize) {
        if !self.pattern.matches(path) {
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

    fn should_descend(&self, path: &JsonPointer) -> bool {
        self.pattern.can_descend_from(path)
            || self
                .result
                .object_key_summaries
                .iter()
                .any(|summary| summary.pointer == *path)
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
        retained_text_bytes: 0,
        longest_prefix_segments: 0,
        object_summary_stack: Vec::new(),
    };
    walk_json(input, &mut observer, cancelled)?;
    observer.result.inspected_bytes = input.len();
    if observer.result.match_count != 0 {
        observer.result.status = JsonInspectionStatus::Matched;
        observer.result.miss_hint = None;
    }
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

fn pointer_parent(pointer: &str) -> &str {
    pointer.rsplit_once('/').map_or("", |(parent, _)| parent)
}

fn pointer_prefix(pointer: &str, segments: usize) -> &str {
    if segments == 0 {
        return "";
    }
    let mut slashes = 0;
    for (index, byte) in pointer.bytes().enumerate() {
        if byte == b'/' {
            slashes += 1;
            if slashes == segments + 1 {
                return &pointer[..index];
            }
        }
    }
    pointer
}

fn pattern_segment_matches(expected: &PatternSegment, actual: &str) -> bool {
    match expected {
        PatternSegment::Wildcard => true,
        PatternSegment::Exact(expected) => expected == actual,
    }
}

fn reserve_text(retained: &mut usize, bytes: usize) -> bool {
    let Some(next) = retained.checked_add(bytes) else {
        return false;
    };
    if next > RETAINED_RESULT_TEXT_BUDGET {
        return false;
    }
    *retained = next;
    true
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
}
