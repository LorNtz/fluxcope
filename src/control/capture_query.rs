use std::time::Duration;

use chrono::{DateTime, Utc};
use globset::{GlobBuilder, GlobMatcher};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;
use unicode_casefold::UnicodeCaseFold;

use crate::{
    capture::{
        BodyPreviewLimit, BodyStatus, BodyStreamState, CaptureSequence, CaptureSnapshot,
        CaptureTiming, MetadataTruncation,
    },
    control_rpc::protocol::ControlError,
};

pub(crate) const CAPTURE_SEARCH_BATCH_SIZE: usize = 32;
pub(crate) const DEFAULT_CAPTURE_PAGE_LIMIT: usize = 20;
pub(crate) const MAX_CAPTURE_PAGE_LIMIT: usize = 100;
const SCAN_QUANTUM: usize = 4 * 1024;

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(
    tag = "mode",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub(crate) enum CaptureTextFilter {
    Substring(String),
    Glob(String),
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CaptureHeaderFilter {
    pub(crate) name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) value: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct CaptureStatusFilter {
    pub(crate) exact: Option<u16>,
    pub(crate) minimum: Option<u16>,
    pub(crate) maximum: Option<u16>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum MappingPath {
    Unmapped,
    RemoteOnly,
    LocalOnly,
    RemoteThenLocal,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CaptureLifecycle {
    Live,
    Complete,
    Failed,
    Cancelled,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct CaptureQuery {
    pub(crate) method: Option<String>,
    pub(crate) original_url: Option<CaptureTextFilter>,
    pub(crate) effective_url: Option<CaptureTextFilter>,
    pub(crate) status: Option<CaptureStatusFilter>,
    pub(crate) header: Option<CaptureHeaderFilter>,
    pub(crate) mapping_path: Option<MappingPath>,
    pub(crate) lifecycle: Option<CaptureLifecycle>,
    pub(crate) started_at_min: Option<String>,
    pub(crate) started_at_max: Option<String>,
    pub(crate) sequence_min: Option<u64>,
    pub(crate) sequence_max: Option<u64>,
    pub(crate) text: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CaptureSearchCursor {
    next_older_sequence: CaptureSequence,
}
impl CaptureSearchCursor {
    pub(crate) const fn new(next_older_sequence: CaptureSequence) -> Self {
        Self {
            next_older_sequence,
        }
    }
    pub(crate) const fn sequence(self) -> CaptureSequence {
        self.next_older_sequence
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CaptureBodyStreamState {
    Pending,
    Streaming,
    Complete,
    Failed,
    Cancelled,
}
impl From<BodyStreamState> for CaptureBodyStreamState {
    fn from(value: BodyStreamState) -> Self {
        match value {
            BodyStreamState::Pending => Self::Pending,
            BodyStreamState::Streaming => Self::Streaming,
            BodyStreamState::Complete => Self::Complete,
            BodyStreamState::Failed => Self::Failed,
            BodyStreamState::Cancelled => Self::Cancelled,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CaptureBodyTruncation {
    PerBodyLimit,
    TotalMemoryLimit,
}
impl From<BodyPreviewLimit> for CaptureBodyTruncation {
    fn from(value: BodyPreviewLimit) -> Self {
        match value {
            BodyPreviewLimit::PerBodyLimit => Self::PerBodyLimit,
            BodyPreviewLimit::TotalMemoryLimit => Self::TotalMemoryLimit,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CaptureBodyDescriptor {
    pub(crate) stream: CaptureBodyStreamState,
    pub(crate) observed_bytes: u64,
    pub(crate) retained_bytes: usize,
    pub(crate) truncated: bool,
    pub(crate) truncation: Option<CaptureBodyTruncation>,
    pub(crate) error: Option<String>,
}
impl From<&BodyStatus> for CaptureBodyDescriptor {
    fn from(status: &BodyStatus) -> Self {
        Self {
            stream: status.stream.into(),
            observed_bytes: status.observed_bytes,
            retained_bytes: status.retained_bytes,
            truncated: status.preview_limit.is_some(),
            truncation: status.preview_limit.map(Into::into),
            error: status.error.as_deref().map(str::to_owned),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CaptureTimingDescriptor {
    pub(crate) started_at: String,
    pub(crate) time_to_response_ms: Option<u64>,
    pub(crate) total_duration_ms: Option<u64>,
}
impl From<CaptureTiming> for CaptureTimingDescriptor {
    fn from(timing: CaptureTiming) -> Self {
        Self {
            started_at: timing.started_at.to_rfc3339(),
            time_to_response_ms: millis(timing.time_to_response),
            total_duration_ms: millis(timing.total_duration),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CaptureMetadataTruncation {
    pub(crate) request_target: bool,
    pub(crate) request_headers: bool,
    pub(crate) response_headers: bool,
}
impl From<MetadataTruncation> for CaptureMetadataTruncation {
    fn from(value: MetadataTruncation) -> Self {
        Self {
            request_target: value.request_target,
            request_headers: value.request_headers,
            response_headers: value.response_headers,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CaptureHeader {
    pub(crate) name: String,
    pub(crate) value: String,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CompactCapture {
    pub(crate) capture_sequence: CaptureSequence,
    pub(crate) capture_revision: u64,
    pub(crate) method: String,
    pub(crate) original_url: String,
    pub(crate) effective_url: String,
    pub(crate) mapping_path: MappingPath,
    pub(crate) status: Option<u16>,
    pub(crate) lifecycle: CaptureLifecycle,
    pub(crate) timing: CaptureTimingDescriptor,
    pub(crate) request_body: CaptureBodyDescriptor,
    pub(crate) response_body: CaptureBodyDescriptor,
    pub(crate) metadata_truncation: CaptureMetadataTruncation,
}
impl CompactCapture {
    pub(crate) fn from_snapshot(s: &CaptureSnapshot) -> Self {
        Self {
            capture_sequence: s.sequence,
            capture_revision: s.revision,
            method: s.request.method.as_str().to_owned(),
            original_url: s.request.original_uri.clone(),
            effective_url: s.request.effective_uri.clone(),
            mapping_path: classify_mapping(s),
            status: s.response.as_ref().map(|r| r.status),
            lifecycle: classify_lifecycle(s),
            timing: s.timing.into(),
            request_body: (&s.request_body.status).into(),
            response_body: (&s.response_body.status).into(),
            metadata_truncation: s.metadata_truncation.into(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CaptureDetail {
    pub(crate) capture_sequence: CaptureSequence,
    pub(crate) capture_revision: u64,
    pub(crate) method: String,
    pub(crate) original_url: String,
    pub(crate) effective_url: String,
    pub(crate) local_path: Option<String>,
    pub(crate) mapping_path: MappingPath,
    pub(crate) status: Option<u16>,
    pub(crate) lifecycle: CaptureLifecycle,
    pub(crate) timing: CaptureTimingDescriptor,
    pub(crate) request_headers: Vec<CaptureHeader>,
    pub(crate) response_headers: Vec<CaptureHeader>,
    pub(crate) request_body: CaptureBodyDescriptor,
    pub(crate) response_body: CaptureBodyDescriptor,
    pub(crate) metadata_truncation: CaptureMetadataTruncation,
}
impl CaptureDetail {
    pub(crate) fn from_snapshot(s: &CaptureSnapshot) -> Self {
        Self {
            capture_sequence: s.sequence,
            capture_revision: s.revision,
            method: s.request.method.as_str().to_owned(),
            original_url: s.request.original_uri.clone(),
            effective_url: s.request.effective_uri.clone(),
            local_path: s.request.local_path.clone(),
            mapping_path: classify_mapping(s),
            status: s.response.as_ref().map(|r| r.status),
            lifecycle: classify_lifecycle(s),
            timing: s.timing.into(),
            request_headers: headers(&s.request.headers),
            response_headers: s
                .response
                .as_ref()
                .map_or_else(Vec::new, |r| headers(&r.headers)),
            request_body: (&s.request_body.status).into(),
            response_body: (&s.response_body.status).into(),
            metadata_truncation: s.metadata_truncation.into(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CaptureSearchPage {
    pub(crate) captures: Vec<CompactCapture>,
    pub(crate) next_cursor: Option<CaptureSearchCursor>,
}
#[derive(Clone, Debug)]
pub(crate) struct CaptureSearchBatch {
    pub(crate) snapshots: Vec<CaptureSnapshot>,
    pub(crate) next_cursor: Option<CaptureSearchCursor>,
}

#[derive(Debug)]
enum CompiledTextFilter {
    Substring(String),
    Glob(GlobMatcher),
}
#[derive(Debug)]
pub(crate) struct CompiledCaptureQuery {
    method: Option<String>,
    original_url: Option<CompiledTextFilter>,
    effective_url: Option<CompiledTextFilter>,
    status: Option<CaptureStatusFilter>,
    header_name: Option<String>,
    header_value: Option<String>,
    mapping_path: Option<MappingPath>,
    lifecycle: Option<CaptureLifecycle>,
    started_at_min: Option<DateTime<Utc>>,
    started_at_max: Option<DateTime<Utc>>,
    sequence_min: Option<CaptureSequence>,
    sequence_max: Option<CaptureSequence>,
    text: Option<String>,
}
impl CompiledCaptureQuery {
    pub(crate) fn compile(q: CaptureQuery) -> Result<Self, ControlError> {
        validate_status(q.status)?;
        let started_at_min = parse_time("started_at_min", q.started_at_min.as_deref())?;
        let started_at_max = parse_time("started_at_max", q.started_at_max.as_deref())?;
        if started_at_min
            .zip(started_at_max)
            .is_some_and(|(a, b)| a > b)
        {
            return Err(invalid("started_at_min must not exceed started_at_max"));
        }
        if q.sequence_min
            .zip(q.sequence_max)
            .is_some_and(|(a, b)| a > b)
        {
            return Err(invalid("sequence_min must not exceed sequence_max"));
        }
        if q.header.as_ref().is_some_and(|h| h.name.is_empty()) {
            return Err(invalid("capture header name must not be empty"));
        }
        let (header_name, header_value) = q.header.map_or((None, None), |h| {
            (Some(fold(&h.name)), h.value.map(|v| fold(&v)))
        });
        Ok(Self {
            method: q.method.map(|v| fold(&v)),
            original_url: q.original_url.map(compile_text).transpose()?,
            effective_url: q.effective_url.map(compile_text).transpose()?,
            status: q.status,
            header_name,
            header_value,
            mapping_path: q.mapping_path,
            lifecycle: q.lifecycle,
            started_at_min,
            started_at_max,
            sequence_min: q.sequence_min.map(CaptureSequence::new),
            sequence_max: q.sequence_max.map(CaptureSequence::new),
            text: q.text.map(|v| fold(&v)),
        })
    }
    pub(crate) fn matches(
        &self,
        s: &CaptureSnapshot,
        c: &CancellationToken,
    ) -> Result<bool, ControlError> {
        self.matches_with_scan_hook(s, c, |_| {})
    }
    pub(crate) fn matches_with_scan_hook(
        &self,
        s: &CaptureSnapshot,
        c: &CancellationToken,
        mut hook: impl FnMut(usize),
    ) -> Result<bool, ControlError> {
        check(c)?;
        if self.sequence_min.is_some_and(|x| s.sequence < x)
            || self.sequence_max.is_some_and(|x| s.sequence > x)
            || self.started_at_min.is_some_and(|x| s.timing.started_at < x)
            || self.started_at_max.is_some_and(|x| s.timing.started_at > x)
            || self.mapping_path.is_some_and(|x| classify_mapping(s) != x)
            || self.lifecycle.is_some_and(|x| classify_lifecycle(s) != x)
        {
            return Ok(false);
        }
        if let Some(x) = self.method.as_deref()
            && fold_checked(s.request.method.as_str(), c, &mut hook)? != x
        {
            return Ok(false);
        }
        if let Some(x) = self.original_url.as_ref()
            && !x.is_match(&fold_checked(&s.request.original_uri, c, &mut hook)?)
        {
            return Ok(false);
        }
        if let Some(x) = self.effective_url.as_ref()
            && !x.is_match(&fold_checked(&s.request.effective_uri, c, &mut hook)?)
        {
            return Ok(false);
        }
        if let Some(x) = self.status {
            let Some(status) = s.response.as_ref().map(|r| r.status) else {
                return Ok(false);
            };
            if !status_match(x, status) {
                return Ok(false);
            }
        }
        if let Some(name) = self.header_name.as_deref()
            && !header_match(s, name, self.header_value.as_deref(), c, &mut hook)?
        {
            return Ok(false);
        }
        if let Some(text) = self.text.as_deref()
            && !text_match(s, text, c, &mut hook)?
        {
            return Ok(false);
        }
        check(c)?;
        Ok(true)
    }
}
impl CompiledTextFilter {
    fn is_match(&self, v: &str) -> bool {
        match self {
            Self::Substring(n) => v.contains(n),
            Self::Glob(g) => g.is_match(v),
        }
    }
}

pub(crate) fn classify_mapping(s: &CaptureSnapshot) -> MappingPath {
    match (
        s.request.original_uri != s.request.effective_uri,
        s.request.local_path.is_some(),
    ) {
        (false, false) => MappingPath::Unmapped,
        (true, false) => MappingPath::RemoteOnly,
        (false, true) => MappingPath::LocalOnly,
        (true, true) => MappingPath::RemoteThenLocal,
    }
}
pub(crate) fn classify_lifecycle(s: &CaptureSnapshot) -> CaptureLifecycle {
    let (a, b) = (s.request_body.status.stream, s.response_body.status.stream);
    if matches!(a, BodyStreamState::Failed) || matches!(b, BodyStreamState::Failed) {
        CaptureLifecycle::Failed
    } else if matches!(a, BodyStreamState::Cancelled) || matches!(b, BodyStreamState::Cancelled) {
        CaptureLifecycle::Cancelled
    } else if matches!(a, BodyStreamState::Complete) && matches!(b, BodyStreamState::Complete) {
        CaptureLifecycle::Complete
    } else {
        CaptureLifecycle::Live
    }
}
pub(crate) fn normalize_capture_page_limit(limit: Option<usize>) -> Result<usize, ControlError> {
    let n = limit.unwrap_or(DEFAULT_CAPTURE_PAGE_LIMIT);
    if !(1..=MAX_CAPTURE_PAGE_LIMIT).contains(&n) {
        Err(invalid(format!(
            "capture page limit must be between 1 and {MAX_CAPTURE_PAGE_LIMIT}"
        )))
    } else {
        Ok(n)
    }
}
pub(crate) fn match_capture_page(
    s: &[CaptureSnapshot],
    q: &CompiledCaptureQuery,
    cursor: Option<CaptureSearchCursor>,
    limit: usize,
    c: &CancellationToken,
) -> Result<CaptureSearchPage, ControlError> {
    let limit = normalize_capture_page_limit(Some(limit))?;
    let cursor = cursor.map(CaptureSearchCursor::sequence);
    let mut v = s
        .iter()
        .filter(|x| cursor.is_none_or(|n| x.sequence <= n))
        .collect::<Vec<_>>();
    v.sort_unstable_by_key(|x| std::cmp::Reverse(x.sequence));
    let mut captures = Vec::with_capacity(limit.min(v.len()));
    let mut last = None;
    let mut older = false;
    for x in v {
        check(c)?;
        if captures.len() == limit {
            older = true;
            break;
        }
        last = Some(x.sequence);
        if q.matches(x, c)? {
            captures.push(CompactCapture::from_snapshot(x));
        }
    }
    let next_cursor = if captures.len() == limit && older {
        last.and_then(cursor_before)
    } else {
        None
    };
    Ok(CaptureSearchPage {
        captures,
        next_cursor,
    })
}
pub(crate) fn cursor_before(s: CaptureSequence) -> Option<CaptureSearchCursor> {
    s.value()
        .checked_sub(1)
        .map(CaptureSequence::new)
        .map(CaptureSearchCursor::new)
}

fn headers(h: &[(String, String)]) -> Vec<CaptureHeader> {
    h.iter()
        .map(|(name, value)| CaptureHeader {
            name: name.clone(),
            value: value.clone(),
        })
        .collect()
}
fn compile_text(t: CaptureTextFilter) -> Result<CompiledTextFilter, ControlError> {
    match t {
        CaptureTextFilter::Substring(v) => Ok(CompiledTextFilter::Substring(fold(&v))),
        CaptureTextFilter::Glob(v) => {
            let g = GlobBuilder::new(&fold(&v))
                .literal_separator(false)
                .build()
                .map_err(|e| invalid(format!("invalid capture URL glob: {e}")))?;
            Ok(CompiledTextFilter::Glob(g.compile_matcher()))
        }
    }
}
fn validate_status(s: Option<CaptureStatusFilter>) -> Result<(), ControlError> {
    let Some(s) = s else { return Ok(()) };
    if s.exact.is_some() && (s.minimum.is_some() || s.maximum.is_some()) {
        return Err(invalid(
            "exact status cannot be combined with status bounds",
        ));
    }
    for x in [s.exact, s.minimum, s.maximum].into_iter().flatten() {
        if !(100..=599).contains(&x) {
            return Err(invalid("HTTP status filters must be between 100 and 599"));
        }
    }
    if s.minimum.zip(s.maximum).is_some_and(|(a, b)| a > b) {
        return Err(invalid("minimum status must not exceed maximum status"));
    }
    Ok(())
}
fn status_match(f: CaptureStatusFilter, s: u16) -> bool {
    f.exact.is_none_or(|x| s == x)
        && f.minimum.is_none_or(|x| s >= x)
        && f.maximum.is_none_or(|x| s <= x)
}
fn parse_time(name: &str, v: Option<&str>) -> Result<Option<DateTime<Utc>>, ControlError> {
    v.map(|x| {
        DateTime::parse_from_rfc3339(x)
            .map(|x| x.with_timezone(&Utc))
            .map_err(|_| invalid(format!("{name} must be an RFC 3339 timestamp")))
    })
    .transpose()
}
fn header_match(
    snapshot: &CaptureSnapshot,
    expected_name: &str,
    expected_value: Option<&str>,
    cancelled: &CancellationToken,
    hook: &mut impl FnMut(usize),
) -> Result<bool, ControlError> {
    for (name, value) in snapshot.request.headers.iter().chain(
        snapshot
            .response
            .iter()
            .flat_map(|response| response.headers.iter()),
    ) {
        check(cancelled)?;
        if fold_checked(name, cancelled, hook)? != expected_name {
            continue;
        }
        match expected_value {
            None => return Ok(true),
            Some(expected) if fold_checked(value, cancelled, hook)?.contains(expected) => {
                return Ok(true);
            }
            Some(_) => {}
        }
    }
    Ok(false)
}
fn text_match(
    s: &CaptureSnapshot,
    text: &str,
    c: &CancellationToken,
    h: &mut impl FnMut(usize),
) -> Result<bool, ControlError> {
    for v in [
        s.request.method.as_str(),
        s.request.original_uri.as_str(),
        s.request.effective_uri.as_str(),
    ] {
        if fold_checked(v, c, h)?.contains(text) {
            return Ok(true);
        }
    }
    if let Some(r) = s.response.as_ref()
        && fold_checked(&r.status.to_string(), c, h)?.contains(text)
    {
        return Ok(true);
    }
    for (n, v) in s
        .request
        .headers
        .iter()
        .chain(s.response.iter().flat_map(|r| r.headers.iter()))
    {
        check(c)?;
        if fold_checked(n, c, h)?.contains(text) || fold_checked(v, c, h)?.contains(text) {
            return Ok(true);
        }
    }
    Ok(false)
}
fn fold_checked(
    v: &str,
    c: &CancellationToken,
    h: &mut impl FnMut(usize),
) -> Result<String, ControlError> {
    let mut out = String::with_capacity(v.len());
    let mut start = 0;
    while start < v.len() {
        check(c)?;
        let mut end = (start + SCAN_QUANTUM).min(v.len());
        while end > start && !v.is_char_boundary(end) {
            end -= 1
        }
        if end == start {
            end = v[start..]
                .char_indices()
                .nth(1)
                .map_or(v.len(), |(i, _)| start + i)
        }
        out.extend(v[start..end].case_fold());
        h(end - start);
        check(c)?;
        start = end
    }
    Ok(out)
}
fn fold(v: &str) -> String {
    v.case_fold().collect()
}
fn check(c: &CancellationToken) -> Result<(), ControlError> {
    if c.is_cancelled() {
        Err(ControlError::cancelled("capture search cancelled"))
    } else {
        Ok(())
    }
}
fn invalid(message: impl Into<String>) -> ControlError {
    ControlError::invalid_argument(message)
}
fn millis(v: Option<Duration>) -> Option<u64> {
    v.map(|x| u64::try_from(x.as_millis()).unwrap_or(u64::MAX))
}

#[cfg(test)]
mod tests;
