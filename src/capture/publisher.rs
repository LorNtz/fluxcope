use std::sync::{
    Arc, Weak,
    atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering},
};

use hyper::{HeaderMap, Method};
use parking_lot::Mutex;
use tokio::sync::{Notify, OwnedSemaphorePermit, Semaphore, mpsc};

use super::{
    BodySide, CaptureRecord, CaptureSequence, MetadataTruncation, RequestMetadata,
    ResponseMetadata,
    model::{BodyTerminal, CaptureBudgetLease, CaptureMemoryBudget},
};

const REQUEST_TERMINAL: u8 = 0b01;
const RESPONSE_TERMINAL: u8 = 0b10;

#[derive(Clone, Debug)]
pub(crate) struct CapturePolicy {
    pub max_concurrent_exchanges: usize,
    pub queue_capacity: usize,
    pub request_target_bytes: usize,
    pub header_bytes_per_side: usize,
    pub request_preview_bytes: usize,
    pub response_preview_bytes: usize,
    pub total_retained_bytes: usize,
    pub retained_records: usize,
}

impl Default for CapturePolicy {
    fn default() -> Self {
        Self {
            max_concurrent_exchanges: 256,
            queue_capacity: 256,
            request_target_bytes: 16 * 1024,
            header_bytes_per_side: 256 * 1024,
            request_preview_bytes: 2 * 1024 * 1024,
            response_preview_bytes: 8 * 1024 * 1024,
            total_retained_bytes: 256 * 1024 * 1024,
            retained_records: 10_000,
        }
    }
}

pub(crate) struct RequestCaptureInput<'a> {
    pub method: Method,
    pub original_uri: &'a str,
    pub effective_uri: &'a str,
    pub local_path: Option<&'a str>,
    pub headers: &'a HeaderMap,
}

pub(crate) struct ResponseCaptureInput<'a> {
    pub status: u16,
    pub headers: &'a HeaderMap,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct CaptureMetricsSnapshot {
    pub exchanges_not_admitted: u64,
    pub memory_pressure: u64,
    pub previews_per_body_limited: u64,
    pub previews_memory_limited: u64,
    pub metadata_truncated: u64,
}

#[derive(Default)]
pub(crate) struct CaptureMetrics {
    exchanges_not_admitted: AtomicU64,
    memory_pressure: AtomicU64,
    previews_per_body_limited: AtomicU64,
    previews_memory_limited: AtomicU64,
    metadata_truncated: AtomicU64,
}

impl CaptureMetrics {
    pub fn snapshot(&self) -> CaptureMetricsSnapshot {
        CaptureMetricsSnapshot {
            exchanges_not_admitted: self.exchanges_not_admitted.load(Ordering::Relaxed),
            memory_pressure: self.memory_pressure.load(Ordering::Relaxed),
            previews_per_body_limited: self.previews_per_body_limited.load(Ordering::Relaxed),
            previews_memory_limited: self.previews_memory_limited.load(Ordering::Relaxed),
            metadata_truncated: self.metadata_truncated.load(Ordering::Relaxed),
        }
    }
}

#[derive(Default)]
pub(crate) struct CaptureDirtySignal {
    pending: AtomicBool,
    notify: Notify,
}

impl CaptureDirtySignal {
    pub fn mark(&self) {
        if !self.pending.swap(true, Ordering::AcqRel) {
            self.notify.notify_one();
        }
    }

    pub async fn notified(&self) {
        loop {
            let notified = self.notify.notified();
            if self.pending.swap(false, Ordering::AcqRel) {
                return;
            }
            notified.await;
        }
    }
}

#[derive(Clone)]
pub(crate) struct CapturePublisher {
    tx: mpsc::Sender<Arc<CaptureRecord>>,
    next_sequence: Arc<AtomicU64>,
    admission: Arc<Semaphore>,
    memory_budget: Arc<CaptureMemoryBudget>,
    policy: Arc<CapturePolicy>,
    metrics: Arc<CaptureMetrics>,
    dirty: Arc<CaptureDirtySignal>,
}

impl CapturePublisher {
    pub fn new(tx: mpsc::Sender<Arc<CaptureRecord>>, policy: CapturePolicy) -> Self {
        Self {
            tx,
            next_sequence: Arc::new(AtomicU64::new(0)),
            admission: Arc::new(Semaphore::new(policy.max_concurrent_exchanges)),
            memory_budget: Arc::new(CaptureMemoryBudget::new(policy.total_retained_bytes)),
            policy: Arc::new(policy),
            metrics: Arc::new(CaptureMetrics::default()),
            dirty: Arc::new(CaptureDirtySignal::default()),
        }
    }

    pub fn try_start(&self, input: RequestCaptureInput<'_>) -> Option<CaptureHandle> {
        let permit = match Arc::clone(&self.admission).try_acquire_owned() {
            Ok(permit) => permit,
            Err(_) => {
                self.metrics
                    .exchanges_not_admitted
                    .fetch_add(1, Ordering::Relaxed);
                self.dirty.mark();
                return None;
            }
        };
        let (metadata_bytes, truncation) = measure_request_metadata(&input, &self.policy);
        let Some(budget_lease) =
            CaptureBudgetLease::try_new(Arc::clone(&self.memory_budget), metadata_bytes)
        else {
            self.metrics
                .exchanges_not_admitted
                .fetch_add(1, Ordering::Relaxed);
            self.metrics.memory_pressure.fetch_add(1, Ordering::Relaxed);
            self.dirty.mark();
            return None;
        };
        let request = request_metadata(input, &self.policy, Arc::clone(&budget_lease));
        debug_assert_eq!(request_metadata_bytes(&request), metadata_bytes);
        debug_assert_eq!(metadata_bytes, budget_lease.reserved_bytes());
        if truncation.request_headers || truncation.request_target {
            self.metrics
                .metadata_truncated
                .fetch_add(1, Ordering::Relaxed);
        }

        let sequence = CaptureSequence::new(self.next_sequence.fetch_add(1, Ordering::Relaxed));
        let record = Arc::new(CaptureRecord::new(
            sequence,
            request,
            truncation,
            budget_lease,
            metadata_bytes,
        ));
        if self.tx.try_send(Arc::clone(&record)).is_err() {
            self.metrics
                .exchanges_not_admitted
                .fetch_add(1, Ordering::Relaxed);
            self.dirty.mark();
            return None;
        }

        self.dirty.mark();
        Some(CaptureHandle {
            record: Arc::downgrade(&record),
            lifecycle: Arc::new(ExchangeLifecycle::new(permit)),
            policy: Arc::clone(&self.policy),
            metrics: Arc::clone(&self.metrics),
            dirty: Arc::clone(&self.dirty),
        })
    }

    pub fn metrics(&self) -> Arc<CaptureMetrics> {
        Arc::clone(&self.metrics)
    }

    pub fn dirty_signal(&self) -> Arc<CaptureDirtySignal> {
        Arc::clone(&self.dirty)
    }
}

#[derive(Clone)]
pub(crate) struct CaptureHandle {
    record: Weak<CaptureRecord>,
    lifecycle: Arc<ExchangeLifecycle>,
    policy: Arc<CapturePolicy>,
    metrics: Arc<CaptureMetrics>,
    dirty: Arc<CaptureDirtySignal>,
}

impl CaptureHandle {
    pub fn set_response(&self, input: ResponseCaptureInput<'_>) {
        let Some(record) = self.record.upgrade() else {
            return;
        };
        let measured = measure_headers(input.headers, self.policy.header_bytes_per_side);
        let requested = 64_usize.saturating_add(measured.retained_bytes);
        let mut reserved = record.reserve_metadata(requested);
        let budget_truncated = reserved < requested;
        if measured.truncated || budget_truncated {
            self.metrics
                .metadata_truncated
                .fetch_add(1, Ordering::Relaxed);
        }
        if budget_truncated {
            self.metrics.memory_pressure.fetch_add(1, Ordering::Relaxed);
        }
        let (response, response_truncated) = if reserved < 64 {
            record.release_metadata(reserved);
            reserved = 0;
            (None, true)
        } else {
            let header_limit = reserved
                .saturating_sub(64)
                .min(self.policy.header_bytes_per_side);
            let (response, truncated) =
                response_metadata(input, header_limit, record.budget_lease());
            let actual = response_metadata_bytes(&response);
            if actual < reserved {
                record.release_metadata(reserved - actual);
                reserved = actual;
            }
            (Some(response), truncated)
        };
        if !record.set_response(
            response,
            measured.truncated || budget_truncated || response_truncated,
            reserved,
        ) {
            record.release_metadata(reserved);
            return;
        }
        self.dirty.mark();
    }

    pub fn append(&self, side: BodySide, bytes: &[u8]) {
        let Some(record) = self.record.upgrade() else {
            return;
        };
        let limit = match side {
            BodySide::Request => self.policy.request_preview_bytes,
            BodySide::Response => self.policy.response_preview_bytes,
        };
        let result = record.append_body(side, bytes, limit);
        if result.per_body_limited {
            self.metrics
                .previews_per_body_limited
                .fetch_add(1, Ordering::Relaxed);
            self.dirty.mark();
        }
        if result.memory_limited {
            self.metrics
                .previews_memory_limited
                .fetch_add(1, Ordering::Relaxed);
            self.metrics.memory_pressure.fetch_add(1, Ordering::Relaxed);
            self.dirty.mark();
        }
    }

    pub fn complete(&self, side: BodySide) {
        if let Some(record) = self.record.upgrade() {
            record.finish_body(side, BodyTerminal::Complete);
        }
        self.lifecycle.mark_terminal(side);
        self.dirty.mark();
    }

    pub fn fail(&self, side: BodySide, error: impl Into<Arc<str>>) {
        if let Some(record) = self.record.upgrade() {
            record.finish_body(side, BodyTerminal::Failed(error.into()));
        }
        self.lifecycle.mark_terminal(side);
        self.dirty.mark();
    }

    pub fn cancel(&self, side: BodySide) {
        if let Some(record) = self.record.upgrade() {
            record.finish_body(side, BodyTerminal::Cancelled);
        }
        self.lifecycle.mark_terminal(side);
        self.dirty.mark();
    }
}

struct ExchangeLifecycle {
    terminal_sides: AtomicU8,
    permit: Mutex<Option<OwnedSemaphorePermit>>,
}

impl ExchangeLifecycle {
    fn new(permit: OwnedSemaphorePermit) -> Self {
        Self {
            terminal_sides: AtomicU8::new(0),
            permit: Mutex::new(Some(permit)),
        }
    }

    fn mark_terminal(&self, side: BodySide) {
        let bit = match side {
            BodySide::Request => REQUEST_TERMINAL,
            BodySide::Response => RESPONSE_TERMINAL,
        };
        let terminal = self.terminal_sides.fetch_or(bit, Ordering::AcqRel) | bit;
        if terminal == REQUEST_TERMINAL | RESPONSE_TERMINAL {
            self.permit.lock().take();
        }
    }
}

fn request_metadata(
    input: RequestCaptureInput<'_>,
    policy: &CapturePolicy,
    budget_lease: Arc<CaptureBudgetLease>,
) -> RequestMetadata {
    let (original_uri, _) = truncate_utf8(input.original_uri, policy.request_target_bytes);
    let (effective_uri, _) = truncate_utf8(input.effective_uri, policy.request_target_bytes);
    let local_path = input.local_path.map(|path| {
        let (path, _) = truncate_utf8(path, policy.request_target_bytes);
        path
    });
    let (headers, _) = capture_headers(input.headers, policy.header_bytes_per_side);
    RequestMetadata::new(
        input.method,
        original_uri,
        effective_uri,
        local_path,
        headers.into(),
        budget_lease,
    )
}

fn response_metadata(
    input: ResponseCaptureInput<'_>,
    header_limit: usize,
    budget_lease: Arc<CaptureBudgetLease>,
) -> (ResponseMetadata, bool) {
    let (headers, truncated) = capture_headers(input.headers, header_limit);
    (
        ResponseMetadata::new(input.status, headers.into(), budget_lease),
        truncated,
    )
}

fn measure_request_metadata(
    input: &RequestCaptureInput<'_>,
    policy: &CapturePolicy,
) -> (usize, MetadataTruncation) {
    let (original_bytes, original_truncated) =
        truncated_utf8_len(input.original_uri, policy.request_target_bytes);
    let (effective_bytes, effective_truncated) =
        truncated_utf8_len(input.effective_uri, policy.request_target_bytes);
    let (local_bytes, local_truncated) = input.local_path.map_or((0, false), |path| {
        truncated_utf8_len(path, policy.request_target_bytes)
    });
    let headers = measure_headers(input.headers, policy.header_bytes_per_side);
    (
        256_usize
            .saturating_add(original_bytes)
            .saturating_add(effective_bytes)
            .saturating_add(local_bytes)
            .saturating_add(headers.retained_bytes),
        MetadataTruncation {
            request_target: original_truncated || effective_truncated || local_truncated,
            request_headers: headers.truncated,
            response_headers: false,
        },
    )
}

#[derive(Clone, Copy)]
struct HeaderMeasurement {
    retained_bytes: usize,
    truncated: bool,
}

fn measure_headers(headers: &HeaderMap, limit: usize) -> HeaderMeasurement {
    let mut retained_bytes = 0_usize;
    let mut truncated = false;
    for (name, value) in headers {
        let field_bytes = name
            .as_str()
            .len()
            .saturating_add(lossy_utf8_len(value.as_bytes()));
        if retained_bytes.saturating_add(field_bytes) > limit {
            truncated = true;
        } else {
            retained_bytes = retained_bytes.saturating_add(field_bytes);
        }
    }
    HeaderMeasurement {
        retained_bytes,
        truncated,
    }
}

fn capture_headers(headers: &HeaderMap, limit: usize) -> (Vec<(String, String)>, bool) {
    let mut captured = Vec::new();
    let mut retained = 0_usize;
    let mut truncated = false;
    for (name, value) in headers {
        let name = name.as_str();
        let value = String::from_utf8_lossy(value.as_bytes());
        let field_bytes = name.len().saturating_add(value.len());
        if retained.saturating_add(field_bytes) > limit {
            truncated = true;
            continue;
        }
        retained = retained.saturating_add(field_bytes);
        captured.push((name.to_string(), value.into_owned()));
    }
    (captured, truncated)
}

fn truncate_utf8(value: &str, limit: usize) -> (String, bool) {
    let (boundary, truncated) = truncated_utf8_len(value, limit);
    (value[..boundary].to_string(), truncated)
}

fn truncated_utf8_len(value: &str, limit: usize) -> (usize, bool) {
    if value.len() <= limit {
        return (value.len(), false);
    }
    let mut boundary = limit.min(value.len());
    while boundary > 0 && !value.is_char_boundary(boundary) {
        boundary -= 1;
    }
    (boundary, true)
}

fn lossy_utf8_len(mut bytes: &[u8]) -> usize {
    let mut length = 0_usize;
    loop {
        match std::str::from_utf8(bytes) {
            Ok(_) => return length.saturating_add(bytes.len()),
            Err(error) => {
                length = length
                    .saturating_add(error.valid_up_to())
                    .saturating_add('�'.len_utf8());
                let invalid = &bytes[error.valid_up_to()..];
                let Some(error_len) = error.error_len() else {
                    return length;
                };
                bytes = &invalid[error_len..];
            }
        }
    }
}

fn request_metadata_bytes(request: &RequestMetadata) -> usize {
    256_usize
        .saturating_add(request.original_uri.len())
        .saturating_add(request.effective_uri.len())
        .saturating_add(request.local_path.as_ref().map_or(0, String::len))
        .saturating_add(headers_bytes(&request.headers))
}

fn response_metadata_bytes(response: &ResponseMetadata) -> usize {
    64_usize.saturating_add(headers_bytes(&response.headers))
}

fn headers_bytes(headers: &[(String, String)]) -> usize {
    headers.iter().fold(0_usize, |total, (name, value)| {
        total.saturating_add(name.len()).saturating_add(value.len())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::BodyPreviewLimit;

    #[tokio::test]
    async fn admission_and_memory_limits_drop_without_waiting() {
        let (tx, mut rx) = mpsc::channel(1);
        let policy = CapturePolicy {
            max_concurrent_exchanges: 1,
            queue_capacity: 1,
            total_retained_bytes: 1_024,
            ..CapturePolicy::default()
        };
        let publisher = CapturePublisher::new(tx, policy);
        let headers = HeaderMap::new();
        let input = || RequestCaptureInput {
            method: Method::GET,
            original_uri: "https://example.com/",
            effective_uri: "https://example.com/",
            local_path: None,
            headers: &headers,
        };

        let first = publisher
            .try_start(input())
            .expect("first capture admitted");
        assert!(publisher.try_start(input()).is_none());
        let record = rx.recv().await.expect("record should be published");
        first.complete(BodySide::Request);
        first.complete(BodySide::Response);
        drop(record);

        assert_eq!(publisher.metrics().snapshot().exchanges_not_admitted, 1);
    }

    #[tokio::test]
    async fn preview_is_prefix_only_and_status_is_limited() {
        let (tx, mut rx) = mpsc::channel(1);
        let policy = CapturePolicy {
            request_preview_bytes: 3,
            ..CapturePolicy::default()
        };
        let publisher = CapturePublisher::new(tx, policy);
        let headers = HeaderMap::new();
        let handle = publisher
            .try_start(RequestCaptureInput {
                method: Method::POST,
                original_uri: "https://example.com/",
                effective_uri: "https://example.com/",
                local_path: None,
                headers: &headers,
            })
            .expect("capture should be admitted");
        let record = rx.recv().await.expect("record should be published");

        handle.append(BodySide::Request, b"abcdef");
        handle.complete(BodySide::Request);

        assert_eq!(
            record.body_preview(BodySide::Request).as_ref().as_ref(),
            b"abc"
        );
        assert_eq!(
            record.summary().request_body.preview_limit,
            Some(BodyPreviewLimit::PerBodyLimit)
        );
        assert_eq!(record.summary().request_body.observed_bytes, 6);
    }

    #[tokio::test]
    async fn response_metadata_pressure_is_bounded_and_visible() {
        let (tx, mut rx) = mpsc::channel(1);
        let publisher = CapturePublisher::new(
            tx,
            CapturePolicy {
                total_retained_bytes: 300,
                ..CapturePolicy::default()
            },
        );
        let headers = HeaderMap::new();
        let handle = publisher
            .try_start(RequestCaptureInput {
                method: Method::GET,
                original_uri: "https://example.com/",
                effective_uri: "https://example.com/",
                local_path: None,
                headers: &headers,
            })
            .expect("request metadata should fit");
        let record = rx.recv().await.expect("capture should be published");

        handle.set_response(ResponseCaptureInput {
            status: 200,
            headers: &headers,
        });

        let summary = record.summary();
        assert!(summary.response.is_none());
        assert!(summary.metadata_truncation.response_headers);
        assert_eq!(
            summary.response_body.stream,
            crate::capture::BodyStreamState::Streaming
        );
        assert_eq!(publisher.metrics().snapshot().memory_pressure, 1);
    }

    #[tokio::test]
    async fn dirty_signal_coalesces_multiple_marks() {
        let signal = CaptureDirtySignal::default();
        signal.mark();
        signal.mark();

        signal.notified().await;
        assert!(!signal.pending.load(Ordering::Acquire));
    }
}
