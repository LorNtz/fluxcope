use std::{
    ops::Deref,
    sync::{
        Arc,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use bytes::{Bytes, BytesMut};
use chrono::{DateTime, Utc};
use hyper::Method;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

#[cfg(any(test, feature = "benchmark"))]
use super::CapturedExchange;
use super::{CaptureChangeFeed, CaptureChangeKind, CaptureSequence};

#[derive(Clone, Copy, Debug, Deserialize, Hash, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BodySide {
    Request,
    Response,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BodyStreamState {
    Pending,
    Streaming,
    Complete,
    Failed,
    Cancelled,
}

impl BodyStreamState {
    fn is_terminal(self) -> bool {
        matches!(self, Self::Complete | Self::Failed | Self::Cancelled)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BodyPreviewLimit {
    PerBodyLimit,
    TotalMemoryLimit,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BodyStatus {
    pub stream: BodyStreamState,
    pub observed_bytes: u64,
    pub retained_bytes: usize,
    pub preview_limit: Option<BodyPreviewLimit>,
    pub error: Option<Arc<str>>,
}

impl Default for BodyStatus {
    fn default() -> Self {
        Self {
            stream: BodyStreamState::Pending,
            observed_bytes: 0,
            retained_bytes: 0,
            preview_limit: None,
            error: None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct RequestMetadata {
    pub method: Method,
    pub original_uri: String,
    pub effective_uri: String,
    pub local_path: Option<String>,
    pub headers: CapturedHeaders,
    _budget_lease: Arc<CaptureBudgetLease>,
}

impl RequestMetadata {
    pub(super) fn new(
        method: Method,
        original_uri: String,
        effective_uri: String,
        local_path: Option<String>,
        headers: Arc<[(String, String)]>,
        budget_lease: Arc<CaptureBudgetLease>,
    ) -> Self {
        Self {
            method,
            original_uri,
            effective_uri,
            local_path,
            headers: CapturedHeaders::new(headers, Arc::clone(&budget_lease)),
            _budget_lease: budget_lease,
        }
    }

    pub fn display_uri(&self) -> &str {
        &self.effective_uri
    }

    pub fn mapped_uri(&self) -> Option<&str> {
        (self.original_uri != self.effective_uri).then_some(self.effective_uri.as_str())
    }
}

#[derive(Clone, Debug)]
pub struct ResponseMetadata {
    pub status: u16,
    pub headers: CapturedHeaders,
    _budget_lease: Arc<CaptureBudgetLease>,
}

impl ResponseMetadata {
    pub(super) fn new(
        status: u16,
        headers: Arc<[(String, String)]>,
        budget_lease: Arc<CaptureBudgetLease>,
    ) -> Self {
        Self {
            status,
            headers: CapturedHeaders::new(headers, Arc::clone(&budget_lease)),
            _budget_lease: budget_lease,
        }
    }
}

#[derive(Clone, Debug)]
pub struct CapturedHeaders {
    values: Arc<[(String, String)]>,
    _budget_lease: Arc<CaptureBudgetLease>,
}

impl CapturedHeaders {
    fn new(values: Arc<[(String, String)]>, budget_lease: Arc<CaptureBudgetLease>) -> Self {
        Self {
            values,
            _budget_lease: budget_lease,
        }
    }

    fn empty(budget_lease: Arc<CaptureBudgetLease>) -> Self {
        Self::new(Arc::from([]), budget_lease)
    }

    pub(crate) fn unbudgeted(values: Arc<[(String, String)]>) -> Self {
        Self::new(values, CaptureBudgetLease::unbudgeted())
    }
}

impl Deref for CapturedHeaders {
    type Target = [(String, String)];

    fn deref(&self) -> &Self::Target {
        &self.values
    }
}

const BODY_CHUNK_BYTES: usize = 64 * 1024;
const BODY_CHUNK_NODE_OVERHEAD: usize =
    std::mem::size_of::<BodyChunkNode>() + 2 * std::mem::size_of::<usize>();

#[derive(Debug)]
struct BodyChunkNode {
    previous: Option<Arc<BodyChunkNode>>,
    bytes: Bytes,
    count: usize,
}

#[derive(Clone, Debug)]
pub struct CapturedBodyPreview {
    canonical_tail: Option<Arc<BodyChunkNode>>,
    canonical_chunks: usize,
    trailing: Bytes,
    len: usize,
    _budget_lease: Arc<CaptureBudgetLease>,
    #[cfg(test)]
    flatten_calls: Arc<AtomicUsize>,
}

impl CapturedBodyPreview {
    fn new(
        canonical_tail: Option<Arc<BodyChunkNode>>,
        trailing: Bytes,
        len: usize,
        budget_lease: Arc<CaptureBudgetLease>,
    ) -> Self {
        let canonical_chunks = canonical_tail.as_ref().map_or(0, |tail| tail.count);
        Self {
            canonical_tail,
            canonical_chunks,
            trailing,
            len,
            #[cfg(test)]
            flatten_calls: Arc::new(AtomicUsize::new(0)),
            _budget_lease: budget_lease,
        }
    }

    fn empty(budget_lease: Arc<CaptureBudgetLease>) -> Self {
        Self::new(None, Bytes::new(), 0, budget_lease)
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn chunks(&self) -> CapturedBodyChunks<'_> {
        let mut canonical = Vec::with_capacity(self.canonical_chunks);
        let mut node = self.canonical_tail.as_deref();
        while let Some(current) = node {
            canonical.push(current.bytes.as_ref());
            node = current.previous.as_deref();
        }
        canonical.reverse();
        CapturedBodyChunks {
            canonical: canonical.into_iter(),
            trailing: (!self.trailing.is_empty()).then_some(self.trailing.as_ref()),
        }
    }

    pub(crate) fn flatten(&self) -> Vec<u8> {
        #[cfg(test)]
        self.flatten_calls.fetch_add(1, Ordering::Relaxed);
        let mut flattened = Vec::with_capacity(self.len);
        for chunk in self.chunks() {
            flattened.extend_from_slice(chunk);
        }
        flattened
    }

    #[cfg(any(test, feature = "benchmark"))]
    pub(crate) fn unbudgeted(bytes: Bytes) -> Self {
        let len = bytes.len();
        Self::new(None, bytes, len, CaptureBudgetLease::unbudgeted())
    }

    #[cfg(test)]
    pub(crate) fn test_flatten_calls(&self) -> usize {
        self.flatten_calls.load(Ordering::Relaxed)
    }

    #[cfg(test)]
    pub(crate) fn test_canonical_chunk_ids(&self) -> Vec<usize> {
        let mut ids = Vec::with_capacity(self.canonical_chunks);
        let mut node = self.canonical_tail.as_deref();
        while let Some(current) = node {
            ids.push(current as *const BodyChunkNode as usize);
            node = current.previous.as_deref();
        }
        ids.reverse();
        ids
    }

    #[cfg(test)]
    pub(crate) fn test_trailing_copy_len(&self) -> usize {
        self.trailing.len()
    }

    #[cfg(test)]
    pub(crate) fn test_trailing_chunk_id(&self) -> Option<usize> {
        (!self.trailing.is_empty()).then(|| self.trailing.as_ptr() as usize)
    }
}

pub struct CapturedBodyChunks<'a> {
    canonical: std::vec::IntoIter<&'a [u8]>,
    trailing: Option<&'a [u8]>,
}

impl<'a> Iterator for CapturedBodyChunks<'a> {
    type Item = &'a [u8];

    fn next(&mut self) -> Option<Self::Item> {
        self.canonical.next().or_else(|| self.trailing.take())
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MetadataTruncation {
    pub request_target: bool,
    pub request_headers: bool,
    pub response_headers: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CaptureTiming {
    pub started_at: DateTime<Utc>,
    pub time_to_response: Option<Duration>,
    pub total_duration: Option<Duration>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptureSnapshotMode {
    MetadataOnly,
    WithBodyPreviews,
}

#[derive(Clone, Debug)]
pub struct BodySnapshot {
    pub status: BodyStatus,
    pub preview: CapturedBodyPreview,
}

#[derive(Clone, Debug)]
pub struct CaptureSnapshot {
    pub sequence: CaptureSequence,
    pub revision: u64,
    pub request: Arc<RequestMetadata>,
    pub response: Option<Arc<ResponseMetadata>>,
    pub request_body: BodySnapshot,
    pub response_body: BodySnapshot,
    pub metadata_truncation: MetadataTruncation,
    pub timing: CaptureTiming,
}

#[derive(Clone, Debug)]
pub struct CaptureSummary {
    pub sequence: CaptureSequence,
    pub request: Arc<RequestMetadata>,
    pub response: Option<Arc<ResponseMetadata>>,
    pub request_body: BodyStatus,
    pub response_body: BodyStatus,
    pub metadata_truncation: MetadataTruncation,
    pub revision: u64,
}

impl CaptureSummary {
    pub fn exchange_is_terminal(&self) -> bool {
        self.request_body.stream.is_terminal() && self.response_body.stream.is_terminal()
    }

    pub fn original_uri(&self) -> &str {
        &self.request.original_uri
    }
}

pub struct CaptureRecord {
    sequence: CaptureSequence,
    request: Arc<RequestMetadata>,
    started_at: DateTime<Utc>,
    started_mono: Instant,
    state: Mutex<RecordState>,
    revision: AtomicU64,
    retained_bytes: AtomicUsize,
    budget_lease: Arc<CaptureBudgetLease>,
    change_feed: CaptureChangeFeed,
}

struct RecordState {
    response: Option<Arc<ResponseMetadata>>,
    response_started: bool,
    response_at: Option<Instant>,
    request_terminal_at: Option<Instant>,
    response_terminal_at: Option<Instant>,
    request_body: BodyCapture,
    response_body: BodyCapture,
    metadata_truncation: MetadataTruncation,
    capture_disabled: bool,
    feed_bound: bool,
}

struct BodyCapture {
    status: BodyStatus,
    canonical_tail: Option<Arc<BodyChunkNode>>,
    trailing: BytesMut,
    sealed_trailing: Option<Bytes>,
}

impl BodyCapture {
    fn new() -> Self {
        Self {
            status: BodyStatus::default(),
            canonical_tail: None,
            trailing: BytesMut::new(),
            sealed_trailing: None,
        }
    }

    #[cfg(any(test, feature = "benchmark"))]
    fn completed(body: Option<String>) -> Self {
        let bytes = body.map_or_else(Bytes::new, Bytes::from);
        let retained_bytes = bytes.len();
        let full_bytes = retained_bytes / BODY_CHUNK_BYTES * BODY_CHUNK_BYTES;
        let mut canonical_tail = None;
        for start in (0..full_bytes).step_by(BODY_CHUNK_BYTES) {
            let count = canonical_tail
                .as_ref()
                .map_or(1, |tail: &Arc<BodyChunkNode>| tail.count + 1);
            canonical_tail = Some(Arc::new(BodyChunkNode {
                previous: canonical_tail,
                bytes: bytes.slice(start..start + BODY_CHUNK_BYTES),
                count,
            }));
        }
        let sealed_trailing = (full_bytes < retained_bytes).then(|| bytes.slice(full_bytes..));
        Self {
            status: BodyStatus {
                stream: BodyStreamState::Complete,
                observed_bytes: retained_bytes as u64,
                retained_bytes,
                preview_limit: None,
                error: None,
            },
            canonical_tail,
            trailing: BytesMut::new(),
            sealed_trailing,
        }
    }

    fn append(&mut self, bytes: &[u8]) {
        let mut remaining = bytes;
        while !remaining.is_empty() {
            let available = BODY_CHUNK_BYTES.saturating_sub(self.trailing.len());
            let take = available.min(remaining.len());
            self.trailing.extend_from_slice(&remaining[..take]);
            remaining = &remaining[take..];
            if self.trailing.len() == BODY_CHUNK_BYTES {
                let bytes = self.trailing.split_to(BODY_CHUNK_BYTES).freeze();
                let count = self
                    .canonical_tail
                    .as_ref()
                    .map_or(1, |tail| tail.count + 1);
                self.canonical_tail = Some(Arc::new(BodyChunkNode {
                    previous: self.canonical_tail.take(),
                    bytes,
                    count,
                }));
            }
        }
    }

    fn seal(&mut self) {
        if !self.trailing.is_empty() {
            let trailing_len = self.trailing.len();
            self.sealed_trailing = Some(self.trailing.split_to(trailing_len).freeze());
        }
    }

    fn preview(&self, budget_lease: Arc<CaptureBudgetLease>) -> CapturedBodyPreview {
        let trailing = self
            .sealed_trailing
            .clone()
            .unwrap_or_else(|| Bytes::copy_from_slice(self.trailing.as_ref()));
        CapturedBodyPreview::new(
            self.canonical_tail.as_ref().map(Arc::clone),
            trailing,
            self.status.retained_bytes,
            budget_lease,
        )
    }
}

impl CaptureRecord {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        sequence: CaptureSequence,
        request: RequestMetadata,
        metadata_truncation: MetadataTruncation,
        budget_lease: Arc<CaptureBudgetLease>,
        retained_bytes: usize,
        started_at: DateTime<Utc>,
        started_mono: Instant,
        change_feed: CaptureChangeFeed,
    ) -> Self {
        Self {
            sequence,
            request: Arc::new(request),
            started_at,
            started_mono,
            state: Mutex::new(RecordState {
                response: None,
                response_started: false,
                response_at: None,
                request_terminal_at: None,
                response_terminal_at: None,
                request_body: BodyCapture::new(),
                response_body: BodyCapture::new(),
                metadata_truncation,
                capture_disabled: false,
                feed_bound: false,
            }),
            revision: AtomicU64::new(0),
            retained_bytes: AtomicUsize::new(retained_bytes),
            budget_lease,
            change_feed,
        }
    }

    #[cfg(any(test, feature = "benchmark"))]
    pub(crate) fn from_completed(exchange: CapturedExchange) -> Arc<Self> {
        let metadata_bytes = exchange.retained_bytes();

        let CapturedExchange {
            sequence,
            method,
            uri,
            mapped_uri,
            local_path,
            status,
            req_headers,
            res_headers,
            req_body,
            res_body,
        } = exchange;
        let request_body_bytes = req_body.as_ref().map_or(0, String::len);
        let response_body_bytes = res_body.as_ref().map_or(0, String::len);
        let body_nodes = request_body_bytes
            .checked_div(BODY_CHUNK_BYTES)
            .unwrap_or(0)
            .saturating_add(
                response_body_bytes
                    .checked_div(BODY_CHUNK_BYTES)
                    .unwrap_or(0),
            );
        let retained_bytes =
            metadata_bytes.saturating_add(body_nodes.saturating_mul(BODY_CHUNK_NODE_OVERHEAD));
        let memory_budget = Arc::new(CaptureMemoryBudget::new(usize::MAX / 2));
        let budget_lease = CaptureBudgetLease::try_new(memory_budget, retained_bytes)
            .expect("completed capture budget should accept record");
        let effective_uri = mapped_uri.unwrap_or_else(|| uri.clone());
        let response = status.map(|status| {
            Arc::new(ResponseMetadata::new(
                status,
                res_headers.into(),
                Arc::clone(&budget_lease),
            ))
        });
        let started_at = Utc::now();
        let started_mono = Instant::now();
        Arc::new(Self {
            sequence,
            request: Arc::new(RequestMetadata::new(
                method,
                uri,
                effective_uri,
                local_path,
                req_headers.into(),
                Arc::clone(&budget_lease),
            )),
            started_at,
            started_mono,
            state: Mutex::new(RecordState {
                response_started: true,
                response,
                response_at: Some(started_mono),
                request_terminal_at: Some(started_mono),
                response_terminal_at: Some(started_mono),
                request_body: BodyCapture::completed(req_body),
                response_body: BodyCapture::completed(res_body),
                metadata_truncation: MetadataTruncation::default(),
                capture_disabled: false,
                feed_bound: false,
            }),
            revision: AtomicU64::new(0),
            retained_bytes: AtomicUsize::new(
                retained_bytes.max(request_body_bytes.saturating_add(response_body_bytes)),
            ),
            budget_lease,
            change_feed: CaptureChangeFeed::new(),
        })
    }

    pub fn sequence(&self) -> CaptureSequence {
        self.sequence
    }

    pub fn revision(&self) -> u64 {
        self.revision.load(Ordering::Acquire)
    }

    pub fn snapshot(&self, mode: CaptureSnapshotMode) -> CaptureSnapshot {
        let state = self.state.lock();
        self.snapshot_locked(&state, mode)
    }
    #[cfg(test)]
    pub(crate) fn snapshot_with_lock_held_test_hook(
        &self,
        mode: CaptureSnapshotMode,
        hook: impl FnOnce(),
    ) -> CaptureSnapshot {
        let state = self.state.lock();
        hook();
        self.snapshot_locked(&state, mode)
    }

    fn snapshot_locked(&self, state: &RecordState, mode: CaptureSnapshotMode) -> CaptureSnapshot {
        let preview = |body: &BodyCapture| match mode {
            CaptureSnapshotMode::MetadataOnly => {
                CapturedBodyPreview::empty(Arc::clone(&self.budget_lease))
            }
            CaptureSnapshotMode::WithBodyPreviews => body.preview(Arc::clone(&self.budget_lease)),
        };
        CaptureSnapshot {
            sequence: self.sequence,
            revision: self.revision.load(Ordering::Acquire),
            request: Arc::clone(&self.request),
            response: state.response.as_ref().map(Arc::clone),
            request_body: BodySnapshot {
                status: state.request_body.status.clone(),
                preview: preview(&state.request_body),
            },
            response_body: BodySnapshot {
                status: state.response_body.status.clone(),
                preview: preview(&state.response_body),
            },
            metadata_truncation: state.metadata_truncation,
            timing: CaptureTiming {
                started_at: self.started_at,
                time_to_response: state
                    .response_at
                    .map(|instant| instant.saturating_duration_since(self.started_mono)),
                total_duration: match (state.request_terminal_at, state.response_terminal_at) {
                    (Some(request), Some(response)) => Some(
                        request
                            .max(response)
                            .saturating_duration_since(self.started_mono),
                    ),
                    _ => None,
                },
            },
        }
    }

    pub fn summary(&self) -> CaptureSummary {
        let snapshot = self.snapshot(CaptureSnapshotMode::MetadataOnly);
        CaptureSummary {
            sequence: snapshot.sequence,
            request: snapshot.request,
            response: snapshot.response,
            request_body: snapshot.request_body.status,
            response_body: snapshot.response_body.status,
            metadata_truncation: snapshot.metadata_truncation,
            revision: snapshot.revision,
        }
    }

    pub fn body_preview(&self, side: BodySide) -> CapturedBodyPreview {
        let state = self.state.lock();
        body(&state, side).preview(Arc::clone(&self.budget_lease))
    }

    pub(crate) fn empty_headers(&self) -> CapturedHeaders {
        CapturedHeaders::empty(Arc::clone(&self.budget_lease))
    }

    pub(super) fn budget_lease(&self) -> Arc<CaptureBudgetLease> {
        Arc::clone(&self.budget_lease)
    }

    pub(crate) fn reserve_metadata(&self, requested: usize) -> usize {
        self.budget_lease.reserve_up_to(requested)
    }

    pub(crate) fn release_metadata(&self, released: usize) {
        self.budget_lease.release(released);
    }

    pub(crate) fn retained_bytes(&self) -> usize {
        self.retained_bytes.load(Ordering::Acquire)
    }

    pub(crate) fn bind_change_feed(&self) {
        let mut state = self.state.lock();
        if state.feed_bound {
            return;
        }
        self.change_feed.publish(
            self.sequence,
            self.revision.load(Ordering::Acquire),
            CaptureChangeKind::Admitted,
        );
        state.feed_bound = true;
    }

    pub(crate) fn remove_from_store(&self, kind: CaptureChangeKind) {
        let mut state = self.state.lock();
        state.capture_disabled = true;
        if state.feed_bound {
            self.change_feed
                .publish(self.sequence, self.revision.load(Ordering::Acquire), kind);
            state.feed_bound = false;
        }
    }

    pub(crate) fn set_response_at(
        &self,
        response: Option<ResponseMetadata>,
        response_headers_truncated: bool,
        reserved_bytes: usize,
        at: Instant,
    ) -> bool {
        let mut state = self.state.lock();
        if state.capture_disabled || state.response_started {
            return false;
        }
        state.response_started = true;
        state.response_at = Some(at);
        state.response = response.map(Arc::new);
        state.metadata_truncation.response_headers |= response_headers_truncated;
        state.response_body.status.stream = BodyStreamState::Streaming;
        self.retained_bytes
            .fetch_add(reserved_bytes, Ordering::AcqRel);
        let revision = self.revision.fetch_add(1, Ordering::AcqRel) + 1;
        if state.feed_bound {
            self.change_feed
                .publish(self.sequence, revision, CaptureChangeKind::RecordUpdated);
        }
        true
    }

    pub(crate) fn append_body(
        &self,
        side: BodySide,
        bytes: &[u8],
        per_body_limit: usize,
    ) -> AppendResult {
        let mut state = self.state.lock();
        let capture_disabled = state.capture_disabled;
        let body = body_mut(&mut state, side);
        if body.status.stream.is_terminal() || bytes.is_empty() {
            return AppendResult::default();
        }
        body.status.stream = BodyStreamState::Streaming;
        body.status.observed_bytes = body
            .status
            .observed_bytes
            .saturating_add(bytes.len() as u64);
        if capture_disabled {
            return AppendResult::default();
        }

        let existing_limit = body.status.preview_limit;
        let per_body_remaining = per_body_limit.saturating_sub(body.status.retained_bytes);
        let requested = if existing_limit.is_some() {
            0
        } else {
            bytes.len().min(per_body_remaining)
        };
        let (retained, charged) = self
            .budget_lease
            .reserve_body_prefix(requested, body.trailing.len());
        if retained > 0 {
            body.append(&bytes[..retained]);
            body.status.retained_bytes = body.status.retained_bytes.saturating_add(retained);
            self.retained_bytes.fetch_add(charged, Ordering::AcqRel);
        }

        let limit = existing_limit.or({
            if retained < requested {
                Some(BodyPreviewLimit::TotalMemoryLimit)
            } else if requested < bytes.len() {
                Some(BodyPreviewLimit::PerBodyLimit)
            } else {
                None
            }
        });
        let newly_limited = existing_limit.is_none().then_some(limit).flatten();
        if let Some(limit) = limit {
            body.status.preview_limit = Some(limit);
        }
        let revision = self.revision.fetch_add(1, Ordering::AcqRel) + 1;
        if state.feed_bound {
            self.change_feed
                .publish(self.sequence, revision, CaptureChangeKind::RecordUpdated);
        }

        AppendResult {
            changed: true,
            per_body_limited: newly_limited == Some(BodyPreviewLimit::PerBodyLimit),
            memory_limited: newly_limited == Some(BodyPreviewLimit::TotalMemoryLimit),
        }
    }

    pub(crate) fn finish_body_at(
        &self,
        side: BodySide,
        terminal: BodyTerminal,
        at: Instant,
    ) -> bool {
        let mut state = self.state.lock();
        let body = body_mut(&mut state, side);
        if body.status.stream.is_terminal() {
            return false;
        }
        match terminal {
            BodyTerminal::Complete => body.status.stream = BodyStreamState::Complete,
            BodyTerminal::Failed(error) => {
                body.status.stream = BodyStreamState::Failed;
                body.status.error = Some(error);
            }
            BodyTerminal::Cancelled => body.status.stream = BodyStreamState::Cancelled,
        }
        body.seal();
        match side {
            BodySide::Request => state.request_terminal_at = Some(at),
            BodySide::Response => state.response_terminal_at = Some(at),
        }
        let revision = self.revision.fetch_add(1, Ordering::AcqRel) + 1;
        if state.feed_bound {
            self.change_feed
                .publish(self.sequence, revision, CaptureChangeKind::RecordUpdated);
        }
        true
    }

    #[cfg(test)]
    pub(crate) fn test_body_chunk_strong_counts(&self, side: BodySide) -> Vec<usize> {
        let state = self.state.lock();
        let mut counts = Vec::new();
        let mut node = body(&state, side).canonical_tail.as_ref();
        while let Some(current) = node {
            counts.push(Arc::strong_count(current));
            node = current.previous.as_ref();
        }
        counts
    }
}

fn body(state: &RecordState, side: BodySide) -> &BodyCapture {
    match side {
        BodySide::Request => &state.request_body,
        BodySide::Response => &state.response_body,
    }
}

fn body_mut(state: &mut RecordState, side: BodySide) -> &mut BodyCapture {
    match side {
        BodySide::Request => &mut state.request_body,
        BodySide::Response => &mut state.response_body,
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct AppendResult {
    pub changed: bool,

    pub per_body_limited: bool,
    pub memory_limited: bool,
}

pub(crate) enum BodyTerminal {
    Complete,
    Failed(Arc<str>),
    Cancelled,
}

#[derive(Debug)]
pub(super) struct CaptureBudgetLease {
    budget: Arc<CaptureMemoryBudget>,
    reserved: AtomicUsize,
}

impl CaptureBudgetLease {
    pub fn try_new(budget: Arc<CaptureMemoryBudget>, reserved: usize) -> Option<Arc<Self>> {
        budget.reserve_exact(reserved).then(|| {
            Arc::new(Self {
                budget,
                reserved: AtomicUsize::new(reserved),
            })
        })
    }

    fn reserve_up_to(&self, requested: usize) -> usize {
        let reserved = self.budget.reserve_up_to(requested);
        self.reserved.fetch_add(reserved, Ordering::AcqRel);
        reserved
    }
    fn reserve_body_prefix(&self, requested: usize, trailing_len: usize) -> (usize, usize) {
        let (retained, charged) = self.budget.reserve_body_prefix(requested, trailing_len);
        self.reserved.fetch_add(charged, Ordering::AcqRel);
        (retained, charged)
    }

    pub(super) fn reserved_bytes(&self) -> usize {
        self.reserved.load(Ordering::Acquire)
    }

    fn release(&self, released: usize) {
        if released == 0 {
            return;
        }
        self.reserved.fetch_sub(released, Ordering::AcqRel);
        self.budget.release(released);
    }

    fn unbudgeted() -> Arc<Self> {
        Self::try_new(Arc::new(CaptureMemoryBudget::new(usize::MAX)), 0)
            .expect("zero-byte test lease should be admitted")
    }
}

impl Drop for CaptureBudgetLease {
    fn drop(&mut self) {
        self.budget.release(self.reserved.load(Ordering::Acquire));
    }
}

#[derive(Debug)]
pub(crate) struct CaptureMemoryBudget {
    limit: usize,
    used: AtomicUsize,
}

impl CaptureMemoryBudget {
    pub fn new(limit: usize) -> Self {
        Self {
            limit,
            used: AtomicUsize::new(0),
        }
    }
    #[cfg(test)]
    pub(crate) fn used(&self) -> usize {
        self.used.load(Ordering::Acquire)
    }

    pub fn reserve_exact(&self, requested: usize) -> bool {
        if requested == 0 {
            return true;
        }
        let mut used = self.used.load(Ordering::Acquire);
        loop {
            let Some(updated) = used.checked_add(requested) else {
                return false;
            };
            if updated > self.limit {
                return false;
            }
            match self.used.compare_exchange_weak(
                used,
                updated,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return true,
                Err(current) => used = current,
            }
        }
    }
    fn reserve_body_prefix(&self, requested: usize, trailing_len: usize) -> (usize, usize) {
        if requested == 0 {
            return (0, 0);
        }
        let mut used = self.used.load(Ordering::Acquire);
        loop {
            let available = self.limit.saturating_sub(used);
            let (retained, charged) = body_prefix_charge(requested, trailing_len, available);
            if retained == 0 {
                return (0, 0);
            }
            match self.used.compare_exchange_weak(
                used,
                used.saturating_add(charged),
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return (retained, charged),
                Err(current) => used = current,
            }
        }
    }

    pub fn reserve_up_to(&self, requested: usize) -> usize {
        let mut used = self.used.load(Ordering::Acquire);
        loop {
            let available = self.limit.saturating_sub(used);
            let reserved = requested.min(available);
            if reserved == 0 {
                return 0;
            }
            match self.used.compare_exchange_weak(
                used,
                used.saturating_add(reserved),
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return reserved,
                Err(current) => used = current,
            }
        }
    }

    pub(super) fn release(&self, released: usize) {
        self.used.fetch_sub(released, Ordering::AcqRel);
    }
}
fn body_prefix_charge(requested: usize, trailing_len: usize, available: usize) -> (usize, usize) {
    let max_bytes = requested.min(available);
    let max_nodes = trailing_len
        .saturating_add(max_bytes)
        .checked_div(BODY_CHUNK_BYTES)
        .unwrap_or(0);
    for nodes in (0..=max_nodes).rev() {
        let overhead = nodes.saturating_mul(BODY_CHUNK_NODE_OVERHEAD);
        if overhead > available {
            continue;
        }
        let lower = nodes
            .saturating_mul(BODY_CHUNK_BYTES)
            .saturating_sub(trailing_len);
        let upper = nodes
            .saturating_add(1)
            .saturating_mul(BODY_CHUNK_BYTES)
            .saturating_sub(trailing_len)
            .saturating_sub(1);
        let retained = requested.min(available.saturating_sub(overhead)).min(upper);
        if retained >= lower {
            return (retained, retained.saturating_add(overhead));
        }
    }
    (0, 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::{CaptureHandle, CapturePolicy, CapturePublisher, RequestCaptureInput};
    use hyper::HeaderMap;
    use tokio::sync::mpsc;

    use std::sync::atomic::AtomicBool;
    use std::thread;

    #[test]
    fn exact_reservation_is_all_or_nothing() {
        let budget = CaptureMemoryBudget::new(10);

        assert!(!budget.reserve_exact(11));
        assert!(budget.reserve_exact(10));
        assert_eq!(budget.reserve_up_to(1), 0);
    }

    const TEST_BODY_CHUNK_BYTES: usize = 64 * 1024;

    #[tokio::test]
    async fn snapshots_keep_revision_body_status_and_preview_from_one_state() {
        let (_publisher, handle, record) = live_capture(16 * TEST_BODY_CHUNK_BYTES).await;
        let append_completed = Arc::new(AtomicBool::new(false));
        let writer_completed = Arc::clone(&append_completed);
        let (start_tx, start_rx) = std::sync::mpsc::sync_channel(0);
        let (attempted_tx, attempted_rx) = std::sync::mpsc::sync_channel(0);
        let writer = thread::spawn(move || {
            start_rx.recv().expect("snapshot hook should start writer");
            attempted_tx
                .send(())
                .expect("snapshot hook should observe append attempt");
            handle.append(BodySide::Request, b"x");
            writer_completed.store(true, Ordering::Release);
        });

        let before_append =
            record.snapshot_with_lock_held_test_hook(CaptureSnapshotMode::WithBodyPreviews, || {
                start_tx
                    .send(())
                    .expect("writer should be waiting for snapshot hook");
                attempted_rx
                    .recv()
                    .expect("writer should signal immediately before append");
                assert!(
                    !append_completed.load(Ordering::Acquire),
                    "append cannot finish while snapshot holds RecordState"
                );
            });
        assert_eq!(before_append.revision, 0);
        assert_eq!(before_append.request_body.status.observed_bytes, 0);
        assert_eq!(before_append.request_body.status.retained_bytes, 0);
        assert!(before_append.request_body.preview.is_empty());

        writer.join().expect("capture writer should finish");
        assert!(append_completed.load(Ordering::Acquire));
        let after_append = record.snapshot(CaptureSnapshotMode::WithBodyPreviews);
        assert_eq!(after_append.revision, 1);
        assert_eq!(after_append.request_body.status.observed_bytes, 1);
        assert_eq!(after_append.request_body.status.retained_bytes, 1);
        assert_eq!(after_append.request_body.preview.flatten(), b"x");
    }

    #[tokio::test]
    async fn metadata_only_snapshot_does_not_retain_body_chunks() {
        let (_publisher, handle, record) = live_capture(4 * TEST_BODY_CHUNK_BYTES).await;
        handle.append(
            BodySide::Request,
            &vec![b'x'; 2 * TEST_BODY_CHUNK_BYTES + 7],
        );
        let strong_counts_before = record.test_body_chunk_strong_counts(BodySide::Request);

        let snapshot = record.snapshot(CaptureSnapshotMode::MetadataOnly);

        assert_eq!(
            snapshot.request_body.status.retained_bytes,
            2 * TEST_BODY_CHUNK_BYTES + 7
        );
        assert!(snapshot.request_body.preview.is_empty());
        assert_eq!(
            record.test_body_chunk_strong_counts(BodySide::Request),
            strong_counts_before
        );
    }

    #[tokio::test]
    async fn fragmented_live_previews_are_exact_immutable_and_share_full_chunks() {
        let (_publisher, handle, record) = live_capture(8 * TEST_BODY_CHUNK_BYTES).await;
        let input = (0..3 * TEST_BODY_CHUNK_BYTES + 137)
            .map(|index| (index % 251) as u8)
            .collect::<Vec<_>>();
        let early_len = TEST_BODY_CHUNK_BYTES + 123;
        for fragment in input[..early_len].chunks(31) {
            handle.append(BodySide::Request, fragment);
        }
        let early = record
            .snapshot(CaptureSnapshotMode::WithBodyPreviews)
            .request_body
            .preview;
        let same_state = record
            .snapshot(CaptureSnapshotMode::WithBodyPreviews)
            .request_body
            .preview;

        assert_eq!(early.flatten(), input[..early_len]);
        assert_eq!(early.test_canonical_chunk_ids().len(), 1);
        assert_eq!(early.test_trailing_copy_len(), 123);
        assert_eq!(
            early.test_canonical_chunk_ids(),
            same_state.test_canonical_chunk_ids()
        );
        assert_ne!(
            early.test_trailing_chunk_id(),
            same_state.test_trailing_chunk_id(),
            "a live trailing block is copied for each immutable snapshot"
        );

        for fragment in input[early_len..].chunks(47) {
            handle.append(BodySide::Request, fragment);
        }
        let final_preview = record
            .snapshot(CaptureSnapshotMode::WithBodyPreviews)
            .request_body
            .preview;

        assert_eq!(final_preview.flatten(), input);
        assert_eq!(early.flatten(), input[..early_len]);
        assert_eq!(final_preview.test_canonical_chunk_ids().len(), 3);
        assert_eq!(final_preview.test_trailing_copy_len(), 137);
        assert_eq!(
            final_preview.test_canonical_chunk_ids()[0],
            early.test_canonical_chunk_ids()[0],
            "canonical full chunks must be shared by later snapshots"
        );
        assert!(
            final_preview.test_trailing_copy_len() <= TEST_BODY_CHUNK_BYTES,
            "snapshot creation may copy at most one 64 KiB trailing block"
        );
    }

    #[tokio::test]
    async fn persistent_chunk_overhead_is_budgeted_and_snapshot_clone_holds_the_lease() {
        let (publisher, handle, record) = live_capture(4 * TEST_BODY_CHUNK_BYTES).await;
        handle.append(BodySide::Response, &vec![b'z'; TEST_BODY_CHUNK_BYTES]);
        let snapshot = record.snapshot(CaptureSnapshotMode::WithBodyPreviews);
        let preview = snapshot.response_body.preview.clone();
        let reserved = publisher.test_capture_budget_used();

        assert!(
            reserved > TEST_BODY_CHUNK_BYTES,
            "the fixed persistent chunk-node overhead must consume capture budget"
        );
        drop(snapshot);
        drop(handle);
        drop(record);
        assert_eq!(
            publisher.test_capture_budget_used(),
            reserved,
            "a cloned preview must retain the shared capture-budget lease"
        );
        drop(preview);
        assert_eq!(publisher.test_capture_budget_used(), 0);
    }

    async fn live_capture(limit: usize) -> (CapturePublisher, CaptureHandle, Arc<CaptureRecord>) {
        let (tx, mut rx) = mpsc::channel(1);
        let publisher = CapturePublisher::new(
            tx,
            CapturePolicy {
                request_preview_bytes: limit,
                response_preview_bytes: limit,
                total_retained_bytes: limit,
                ..CapturePolicy::default()
            },
        );
        let headers = HeaderMap::new();
        let handle = publisher
            .try_start(RequestCaptureInput {
                method: Method::POST,
                original_uri: "https://example.com/upload",
                effective_uri: "https://example.com/upload",
                local_path: None,
                headers: &headers,
            })
            .expect("capture admitted");
        let record = rx.recv().await.expect("capture published");
        (publisher, handle, record)
    }
}
