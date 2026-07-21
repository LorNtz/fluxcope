use std::{
    ops::Deref,
    sync::{
        Arc,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
};

use hyper::{Method, body::Bytes};
use parking_lot::Mutex;

use super::CaptureSequence;
use super::CapturedExchange;

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub enum BodySide {
    Request,
    Response,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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

    #[cfg(test)]
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

#[derive(Clone, Debug)]
pub struct CapturedBodyPreview {
    bytes: Arc<Bytes>,
    _budget_lease: Arc<CaptureBudgetLease>,
}

impl CapturedBodyPreview {
    fn new(bytes: Arc<Bytes>, budget_lease: Arc<CaptureBudgetLease>) -> Self {
        Self {
            bytes,
            _budget_lease: budget_lease,
        }
    }

    #[cfg(test)]
    pub(crate) fn unbudgeted(bytes: Bytes) -> Self {
        Self::new(Arc::new(bytes), CaptureBudgetLease::unbudgeted())
    }
}

impl Deref for CapturedBodyPreview {
    type Target = Bytes;

    fn deref(&self) -> &Self::Target {
        &self.bytes
    }
}

impl AsRef<Bytes> for CapturedBodyPreview {
    fn as_ref(&self) -> &Bytes {
        &self.bytes
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MetadataTruncation {
    pub request_target: bool,
    pub request_headers: bool,
    pub response_headers: bool,
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
    state: Mutex<RecordState>,
    revision: AtomicU64,
    retained_bytes: AtomicUsize,
    budget_lease: Arc<CaptureBudgetLease>,
}

struct RecordState {
    response: Option<Arc<ResponseMetadata>>,
    response_started: bool,
    request_body: BodyCapture,
    response_body: BodyCapture,
    metadata_truncation: MetadataTruncation,
    capture_disabled: bool,
}

struct BodyCapture {
    status: BodyStatus,
    collecting: Vec<u8>,
    published: Arc<Bytes>,
}

impl BodyCapture {
    fn new() -> Self {
        Self {
            status: BodyStatus::default(),
            collecting: Vec::new(),
            published: Arc::new(Bytes::new()),
        }
    }

    fn completed(body: Option<String>) -> Self {
        let bytes = body.map_or_else(Bytes::new, Bytes::from);
        let retained_bytes = bytes.len();
        Self {
            status: BodyStatus {
                stream: BodyStreamState::Complete,
                observed_bytes: retained_bytes as u64,
                retained_bytes,
                preview_limit: None,
                error: None,
            },
            collecting: Vec::new(),
            published: Arc::new(bytes),
        }
    }

    fn publish(&mut self) {
        if !self.collecting.is_empty() {
            self.published = Arc::new(Bytes::from(std::mem::take(&mut self.collecting)));
        }
    }
}

impl CaptureRecord {
    pub(super) fn new(
        sequence: CaptureSequence,
        request: RequestMetadata,
        metadata_truncation: MetadataTruncation,
        budget_lease: Arc<CaptureBudgetLease>,
        retained_bytes: usize,
    ) -> Self {
        Self {
            sequence,
            request: Arc::new(request),
            state: Mutex::new(RecordState {
                response: None,
                response_started: false,
                request_body: BodyCapture::new(),
                response_body: BodyCapture::new(),
                metadata_truncation,
                capture_disabled: false,
            }),
            revision: AtomicU64::new(0),
            retained_bytes: AtomicUsize::new(retained_bytes),
            budget_lease,
        }
    }

    pub(crate) fn from_completed(exchange: CapturedExchange) -> Arc<Self> {
        let request_body_bytes = exchange.req_body.as_ref().map_or(0, String::len);
        let response_body_bytes = exchange.res_body.as_ref().map_or(0, String::len);
        let retained_bytes = exchange.retained_bytes();
        let memory_budget = Arc::new(CaptureMemoryBudget::new(usize::MAX / 2));
        let budget_lease = CaptureBudgetLease::try_new(memory_budget, retained_bytes)
            .expect("test capture budget should accept completed record");
        let effective_uri = exchange
            .mapped_uri
            .clone()
            .unwrap_or_else(|| exchange.uri.clone());
        let response = exchange.status.map(|status| {
            Arc::new(ResponseMetadata::new(
                status,
                exchange.res_headers.into(),
                Arc::clone(&budget_lease),
            ))
        });
        Arc::new(Self {
            sequence: exchange.sequence,
            request: Arc::new(RequestMetadata::new(
                exchange.method,
                exchange.uri,
                effective_uri,
                exchange.local_path,
                exchange.req_headers.into(),
                Arc::clone(&budget_lease),
            )),
            state: Mutex::new(RecordState {
                response_started: true,
                response,
                request_body: BodyCapture::completed(exchange.req_body),
                response_body: BodyCapture::completed(exchange.res_body),
                metadata_truncation: MetadataTruncation::default(),
                capture_disabled: false,
            }),
            revision: AtomicU64::new(0),
            retained_bytes: AtomicUsize::new(
                retained_bytes.max(request_body_bytes.saturating_add(response_body_bytes)),
            ),
            budget_lease,
        })
    }

    pub fn sequence(&self) -> CaptureSequence {
        self.sequence
    }

    pub fn summary(&self) -> CaptureSummary {
        let state = self.state.lock();
        CaptureSummary {
            sequence: self.sequence,
            request: Arc::clone(&self.request),
            response: state.response.as_ref().map(Arc::clone),
            request_body: state.request_body.status.clone(),
            response_body: state.response_body.status.clone(),
            metadata_truncation: state.metadata_truncation,
            revision: self.revision.load(Ordering::Acquire),
        }
    }

    pub fn body_preview(&self, side: BodySide) -> CapturedBodyPreview {
        let state = self.state.lock();
        CapturedBodyPreview::new(
            Arc::clone(&body(&state, side).published),
            Arc::clone(&self.budget_lease),
        )
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

    pub(crate) fn set_response(
        &self,
        response: Option<ResponseMetadata>,
        response_headers_truncated: bool,
        reserved_bytes: usize,
    ) -> bool {
        let mut state = self.state.lock();
        if state.capture_disabled || state.response_started {
            return false;
        }
        state.response_started = true;
        state.response = response.map(Arc::new);
        state.metadata_truncation.response_headers |= response_headers_truncated;
        state.response_body.status.stream = BodyStreamState::Streaming;
        self.retained_bytes
            .fetch_add(reserved_bytes, Ordering::AcqRel);
        self.revision.fetch_add(1, Ordering::AcqRel);
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
        if body.status.stream.is_terminal() {
            return AppendResult::default();
        }
        body.status.stream = BodyStreamState::Streaming;
        body.status.observed_bytes = body
            .status
            .observed_bytes
            .saturating_add(bytes.len() as u64);
        if capture_disabled || bytes.is_empty() {
            return AppendResult::default();
        }

        let per_body_remaining = per_body_limit.saturating_sub(body.status.retained_bytes);
        let requested = bytes.len().min(per_body_remaining);
        let reserved = self.budget_lease.reserve_up_to(requested);
        if reserved > 0 {
            body.collecting.extend_from_slice(&bytes[..reserved]);
            body.status.retained_bytes = body.status.retained_bytes.saturating_add(reserved);
            self.retained_bytes.fetch_add(reserved, Ordering::AcqRel);
        }

        let limit = if reserved < requested {
            Some(BodyPreviewLimit::TotalMemoryLimit)
        } else if requested < bytes.len() {
            Some(BodyPreviewLimit::PerBodyLimit)
        } else {
            None
        };
        let newly_limited = limit.filter(|limit| body.status.preview_limit != Some(*limit));
        if let Some(limit) = limit {
            body.status.preview_limit = Some(limit);
        }
        if newly_limited.is_some() {
            self.revision.fetch_add(1, Ordering::AcqRel);
        }

        AppendResult {
            per_body_limited: newly_limited == Some(BodyPreviewLimit::PerBodyLimit),
            memory_limited: newly_limited == Some(BodyPreviewLimit::TotalMemoryLimit),
        }
    }

    pub(crate) fn finish_body(&self, side: BodySide, terminal: BodyTerminal) {
        let mut state = self.state.lock();
        let body = body_mut(&mut state, side);
        if body.status.stream.is_terminal() {
            return;
        }
        match terminal {
            BodyTerminal::Complete => body.status.stream = BodyStreamState::Complete,
            BodyTerminal::Failed(error) => {
                body.status.stream = BodyStreamState::Failed;
                body.status.error = Some(error);
            }
            BodyTerminal::Cancelled => body.status.stream = BodyStreamState::Cancelled,
        }
        body.publish();
        self.revision.fetch_add(1, Ordering::AcqRel);
    }

    pub(crate) fn disable_capture(&self) {
        self.state.lock().capture_disabled = true;
        self.revision.fetch_add(1, Ordering::AcqRel);
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

    #[cfg(test)]
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_reservation_is_all_or_nothing() {
        let budget = CaptureMemoryBudget::new(10);

        assert!(!budget.reserve_exact(11));
        assert!(budget.reserve_exact(10));
        assert_eq!(budget.reserve_up_to(1), 0);
    }

    #[test]
    fn preview_clone_keeps_budget_reserved_after_owner_drops() {
        let budget = Arc::new(CaptureMemoryBudget::new(10));
        let lease = CaptureBudgetLease::try_new(Arc::clone(&budget), 10)
            .expect("initial reservation should fit");
        let preview = CapturedBodyPreview::new(
            Arc::new(Bytes::from_static(b"0123456789")),
            Arc::clone(&lease),
        );
        drop(lease);

        assert!(!budget.reserve_exact(1));
        drop(preview);
        assert!(budget.reserve_exact(10));
    }
}
