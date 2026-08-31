use bytes::{Bytes, BytesMut};
use parking_lot::Mutex;
use std::{
    collections::{BTreeMap, HashMap},
    fmt,
    future::Future,
    io,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use super::gateway::RuntimeControlClient;
#[cfg(test)]
pub(super) use super::gateway::{RuntimeControlReceiver, RuntimeGateway};
use anyhow::{Result, anyhow};
use futures::future::BoxFuture;
use tokio::{
    net::UnixListener,
    sync::{OwnedSemaphorePermit, Semaphore, oneshot},
    task::{JoinHandle, JoinSet},
    time,
};
use tokio_util::sync::CancellationToken;

#[cfg(test)]
use crate::capture::{DecodePolicy, DecodeService, start_decode_service_with_admission};
use crate::{
    capture::{
        ActiveBodyWorkLease, BodySide, BodyStatus, BodyStreamState, BodyWorkAdmission,
        CaptureChange, CaptureChangeError, CaptureChangeFeed, CaptureChangeKind,
        CaptureChangeSubscription, CaptureSequence, CaptureSnapshot, CapturedBodyPreview,
        CapturedHeaders, ContentDecodePolicy, DecodedBytes, decode_content_bytes,
    },
    control::{
        CaptureMilestone, InstanceRuntimeSnapshot, RuntimeReply, RuntimeRequest,
        WaitForCaptureRequest, WaitForCaptureResult,
        body::{
            BodyContentRequest, BodyPage, BodyPageSource, BodyRange, BodyRepresentation,
            CaptureBodyMetadataReply, CaptureBodySnapshotReply, ExtractCaptureBodyRequest,
            ExtractCaptureBodyResult, MAX_TERMINAL_DECODED_CACHE_BYTES, SearchCaptureBodyRequest,
            SearchCaptureBodyResult, SelectionContentRequest, SelectionPage,
            SelectionResourceRequest, content_resource_base_uri, extract_capture_body,
            extract_selected_bytes, media_type, page_selected_representation, search_capture_body,
        },
        capture_query::{
            CAPTURE_SEARCH_BATCH_SIZE, CAPTURE_SEARCH_PAGE_JSON_BUDGET, CaptureDetail,
            CaptureQuery, CaptureSearchCursor, CaptureSearchPage, CompactCapture,
            CompiledCaptureQuery, match_capture_page_with_budget, normalize_capture_page_limit,
        },
        json_walk::{
            FindJsonPointersRequest, ProbeJsonPointerPatternRequest, find_json_pointers, probe_json,
        },
        normalize_wait_timeout_ms,
    },
    control_rpc::{
        client::ControlRpcClient,
        protocol::{
            ControlError, ControlErrorCode, ControlOperation, ControlResult, DeclaredClient,
            InstanceScope, MappingSettingsPayload,
        },
        server::{ControlCallContext, ControlRpcHandler, ControlRpcServer},
    },
    instance::{InstanceIdentity, RunId},
    instance_registry::{InstanceDescriptor, RegistryPublisher, RegistryScanner},
    runtime::settings::SettingsTransactionClient,
    settings::SettingsSession,
};

pub(super) struct CaptureSearchAdmission {
    permits: Arc<Semaphore>,
}

pub(super) struct ActiveCaptureSearch {
    permit: Option<OwnedSemaphorePermit>,
    query: Arc<CompiledCaptureQuery>,
}

pub(super) struct DetailMaterializationAdmission {
    permits: Arc<Semaphore>,
}

impl CaptureSearchAdmission {
    pub(super) fn new(limit: usize) -> Self {
        Self {
            permits: Arc::new(Semaphore::new(limit)),
        }
    }

    async fn acquire(
        &self,
        cancelled: &CancellationToken,
    ) -> Result<OwnedSemaphorePermit, ControlError> {
        tokio::select! {
            permit = Arc::clone(&self.permits).acquire_owned() => permit.map_err(|_| {
                ControlError::service_unavailable("capture search admission is closed")
            }),
            _ = cancelled.cancelled() => {
                Err(ControlError::cancelled("capture search cancelled before admission"))
            }
        }
    }

    pub(super) async fn admit(
        &self,
        query: CaptureQuery,
        cancelled: CancellationToken,
    ) -> Result<ActiveCaptureSearch, ControlError> {
        let permit = self.acquire(&cancelled).await?;
        let worker = tokio::task::spawn_blocking(move || {
            let compiled = CompiledCaptureQuery::compile(query);
            (permit, compiled)
        });
        let (permit, compiled) = tokio::select! {
            biased;
            _ = cancelled.cancelled() => {
                return Err(ControlError::cancelled("capture search cancelled"));
            }
            result = worker => {
                result.map_err(|_| ControlError::internal("capture search worker failed"))?
            }
        };
        Ok(ActiveCaptureSearch {
            permit: Some(permit),
            query: Arc::new(compiled?),
        })
    }
    async fn admit_compiled(
        &self,
        query: Arc<CompiledCaptureQuery>,
        cancelled: &CancellationToken,
    ) -> Result<ActiveCaptureSearch, ControlError> {
        Ok(ActiveCaptureSearch {
            permit: Some(self.acquire(cancelled).await?),
            query,
        })
    }

    #[cfg(test)]
    pub(super) fn available_permits_for_test(&self) -> usize {
        self.permits.available_permits()
    }
}

impl ActiveCaptureSearch {
    fn compiled_query(&self) -> Arc<CompiledCaptureQuery> {
        Arc::clone(&self.query)
    }
    pub(super) async fn run_blocking<T, F>(
        &mut self,
        cancelled: CancellationToken,
        work: F,
    ) -> Result<T, ControlError>
    where
        T: Send + 'static,
        F: FnOnce(Arc<CompiledCaptureQuery>) -> Result<T, ControlError> + Send + 'static,
    {
        let permit = self
            .permit
            .take()
            .ok_or_else(|| ControlError::internal("capture search permit is not available"))?;
        let query = Arc::clone(&self.query);
        let worker = tokio::task::spawn_blocking(move || {
            let result = work(query);
            (permit, result)
        });
        let (permit, result) = tokio::select! {
            biased;
            _ = cancelled.cancelled() => {
                return Err(ControlError::cancelled("capture search cancelled"));
            }
            result = worker => {
                result.map_err(|_| ControlError::internal("capture search worker failed"))?
            }
        };
        self.permit = Some(permit);
        result
    }
}

impl DetailMaterializationAdmission {
    pub(super) fn new(limit: usize) -> Self {
        Self {
            permits: Arc::new(Semaphore::new(limit)),
        }
    }

    pub(super) async fn run_blocking<T, F>(
        &self,
        cancelled: CancellationToken,
        work: F,
    ) -> Result<T, ControlError>
    where
        T: Send + 'static,
        F: FnOnce() -> T + Send + 'static,
    {
        let permit = tokio::select! {
            permit = Arc::clone(&self.permits).acquire_owned() => permit.map_err(|_| {
                ControlError::service_unavailable("capture detail admission is closed")
            })?,
            _ = cancelled.cancelled() => {
                return Err(ControlError::cancelled(
                    "capture detail cancelled before admission",
                ));
            }
        };
        let worker = tokio::task::spawn_blocking(move || {
            let value = work();
            (permit, value)
        });
        let (_permit, value) = tokio::select! {
            biased;
            _ = cancelled.cancelled() => {
                return Err(ControlError::cancelled("capture detail cancelled"));
            }
            result = worker => {
                result.map_err(|_| {
                    ControlError::internal("capture detail worker failed")
                })?
            }
        };
        Ok(value)
    }

    #[cfg(test)]
    pub(super) fn available_permits_for_test(&self) -> usize {
        self.permits.available_permits()
    }
}

#[derive(Clone, Copy)]
struct WaitSequenceState {
    revision: u64,
    pending: bool,
}

struct WaitBatchInspection {
    capture: Option<Box<CompactCapture>>,
}

enum WaitSnapshotInspection {
    Matched(Box<CompactCapture>),
    Pending,
    Ignore,
}

fn inspect_wait_snapshot(
    snapshot: &CaptureSnapshot,
    query: &CompiledCaptureQuery,
    milestone: CaptureMilestone,
    cancelled: &CancellationToken,
) -> Result<WaitSnapshotInspection, ControlError> {
    if !query.matches(snapshot, cancelled)? {
        return Ok(WaitSnapshotInspection::Ignore);
    }
    if milestone.is_satisfied_by(snapshot) {
        return Ok(WaitSnapshotInspection::Matched(Box::new(
            CompactCapture::from_snapshot(snapshot),
        )));
    }
    if CaptureMilestone::ExchangeTerminal.is_satisfied_by(snapshot) {
        return Ok(WaitSnapshotInspection::Ignore);
    }
    Ok(WaitSnapshotInspection::Pending)
}

fn inspect_wait_batch(
    snapshots: &[CaptureSnapshot],
    query: &CompiledCaptureQuery,
    milestone: CaptureMilestone,
    cancelled: &CancellationToken,
    states: &mut HashMap<CaptureSequence, WaitSequenceState>,
) -> Result<WaitBatchInspection, ControlError> {
    for snapshot in snapshots {
        let pending = match inspect_wait_snapshot(snapshot, query, milestone, cancelled)? {
            WaitSnapshotInspection::Matched(capture) => {
                return Ok(WaitBatchInspection {
                    capture: Some(capture),
                });
            }
            WaitSnapshotInspection::Pending => true,
            WaitSnapshotInspection::Ignore => false,
        };
        states.insert(
            snapshot.sequence,
            WaitSequenceState {
                revision: snapshot.revision,
                pending,
            },
        );
    }
    Ok(WaitBatchInspection { capture: None })
}

async fn wait_step<T>(
    future: impl Future<Output = Result<T, ControlError>>,
    deadline: tokio::time::Instant,
    cancelled: &CancellationToken,
) -> Result<Option<T>, ControlError> {
    tokio::select! {
        biased;
        _ = cancelled.cancelled() => {
            Err(ControlError::cancelled("capture wait cancelled"))
        }
        _ = time::sleep_until(deadline) => Ok(None),
        result = future => result.map(Some),
    }
}

fn unmatched_wait(instance: InstanceScope) -> ControlResult {
    ControlResult::WaitForCapture {
        instance,
        result: WaitForCaptureResult {
            matched: false,
            capture: None,
        },
    }
}

fn matched_wait(instance: InstanceScope, capture: Box<CompactCapture>) -> ControlResult {
    ControlResult::WaitForCapture {
        instance,
        result: WaitForCaptureResult {
            matched: true,
            capture: Some(capture),
        },
    }
}

fn capture_change_lost(change: CaptureChange, message: &'static str) -> ControlError {
    ControlError::new(
        ControlErrorCode::CaptureNotFound,
        message,
        true,
        serde_json::json!({
            "capture_id": change.sequence,
            "capture_revision": change.revision,
        }),
    )
}

fn feed_gap(error: CaptureChangeError) -> ControlError {
    let CaptureChangeError::Gap {
        expected_epoch,
        oldest_available_epoch,
    } = error;
    ControlError::new(
        ControlErrorCode::ServiceUnavailable,
        "capture change feed history was exceeded",
        true,
        serde_json::json!({
            "expected_epoch": expected_epoch,
            "oldest_available_epoch": oldest_available_epoch,
        }),
    )
}

pub(super) fn page_raw_body(
    preview: &CapturedBodyPreview,
    request: &BodyContentRequest,
    status: &BodyStatus,
    headers: &CapturedHeaders,
) -> Result<BodyPage, ControlError> {
    request.validate()?;
    let total_bytes = preview.len();
    let start = request.offset.min(total_bytes);
    let end = start.saturating_add(request.length).min(total_bytes);
    let mut content = BytesMut::with_capacity(end.saturating_sub(start));
    let mut chunk_start: usize = 0;
    for chunk in preview.chunks() {
        let chunk_end = chunk_start.saturating_add(chunk.len());
        if chunk_end > start && chunk_start < end {
            let local_start = start.saturating_sub(chunk_start);
            let local_end = end.saturating_sub(chunk_start).min(chunk.len());
            content.extend_from_slice(&chunk[local_start..local_end]);
        }
        if chunk_end >= end {
            break;
        }
        chunk_start = chunk_end;
    }
    Ok(body_page(
        content.freeze(),
        request,
        status,
        headers,
        start,
        total_bytes,
        Vec::new(),
        false,
    ))
}

pub(super) fn page_decoded_body(
    decoded: &DecodedBytes,
    request: &BodyContentRequest,
    status: &BodyStatus,
    headers: &CapturedHeaders,
) -> Result<BodyPage, ControlError> {
    request.validate()?;
    let total_bytes = decoded.bytes.len();
    let start = request.offset.min(total_bytes);
    let requested_end = start.saturating_add(request.length).min(total_bytes);
    let mut end = requested_end;
    if decoded.is_utf8() {
        if !is_utf8_boundary(&decoded.bytes, start) {
            let nearest_start = (start.saturating_sub(3)..start)
                .rev()
                .find(|offset| is_utf8_boundary(&decoded.bytes, *offset))
                .unwrap_or(0);
            let nearest_end = (start.saturating_add(1)..=start.saturating_add(3).min(total_bytes))
                .find(|offset| is_utf8_boundary(&decoded.bytes, *offset))
                .unwrap_or(total_bytes);
            return Err(ControlError::new(
                ControlErrorCode::InvalidArgument,
                "decoded text offset is not a UTF-8 character boundary",
                false,
                serde_json::json!({
                    "offset": request.offset,
                    "nearest_start": nearest_start,
                    "nearest_end": nearest_end,
                }),
            ));
        }
        while end > start && !is_utf8_boundary(&decoded.bytes, end) {
            end -= 1;
        }
    }
    Ok(body_page(
        Bytes::copy_from_slice(&decoded.bytes[start..end]),
        request,
        status,
        headers,
        start,
        total_bytes,
        decoded.encoding_chain.clone(),
        decoded.output_limited,
    ))
}

fn is_utf8_boundary(bytes: &[u8], index: usize) -> bool {
    index == bytes.len()
        || bytes
            .get(index)
            .is_some_and(|byte| byte & 0b1100_0000 != 0b1000_0000)
}

#[allow(clippy::too_many_arguments)]
fn body_page(
    content: Bytes,
    request: &BodyContentRequest,
    status: &BodyStatus,
    headers: &CapturedHeaders,
    start: usize,
    total_bytes: usize,
    decoded_encoding_chain: Vec<String>,
    decoded_output_limited: bool,
) -> BodyPage {
    let actual_length = content.len();
    let end = start.saturating_add(actual_length);
    BodyPage {
        content,
        media_type: media_type(headers),
        requested_range: BodyRange {
            offset: request.offset,
            length: request.length,
        },
        actual_range: BodyRange {
            offset: start,
            length: actual_length,
        },
        total_bytes,
        next_offset: (end < total_bytes).then_some(end),
        source: BodyPageSource {
            stream: status.stream,
            observed_bytes: status.observed_bytes,
            retained_bytes: status.retained_bytes,
            truncated: status.preview_limit.is_some(),
            truncation_reason: status.preview_limit,
            decoded_encoding_chain,
            decoded_output_limited,
        },
    }
}

pub(super) fn should_cache_decoded_body(status: &BodyStatus) -> bool {
    matches!(
        status.stream,
        BodyStreamState::Complete | BodyStreamState::Failed | BodyStreamState::Cancelled
    )
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(super) struct BodyCacheKey {
    pub(super) instance: InstanceScope,
    pub(super) capture_id: CaptureSequence,
    pub(super) capture_revision: u64,
    pub(super) side: BodySide,
    pub(super) representation: BodyRepresentation,
}

struct DecodedCacheEntry {
    decoded: DecodedBytes,
    used_at: u64,
}

pub(super) struct DecodedBodyCache {
    capacity_bytes: usize,
    bytes: usize,
    clock: u64,
    entries: HashMap<BodyCacheKey, DecodedCacheEntry>,
    lru: BTreeMap<u64, BodyCacheKey>,
}

impl DecodedBodyCache {
    pub(super) fn new(capacity_bytes: usize) -> Self {
        Self {
            capacity_bytes,
            bytes: 0,
            clock: 0,
            entries: HashMap::new(),
            lru: BTreeMap::new(),
        }
    }

    #[cfg(test)]
    pub(super) fn get(&mut self, key: &BodyCacheKey) -> Option<Bytes> {
        self.get_decoded(key).map(|decoded| decoded.bytes)
    }

    fn get_decoded(&mut self, key: &BodyCacheKey) -> Option<DecodedBytes> {
        let entry = self.entries.get_mut(key)?;
        self.lru.remove(&entry.used_at);
        self.clock = self
            .clock
            .checked_add(1)
            .expect("decoded cache clock exhausted");
        entry.used_at = self.clock;
        let decoded = entry.decoded.clone();
        self.lru.insert(self.clock, key.clone());
        Some(decoded)
    }

    #[cfg(test)]
    pub(super) fn insert(&mut self, key: BodyCacheKey, bytes: Bytes) {
        self.insert_decoded(key, DecodedBytes::new(bytes, Vec::new(), false));
    }

    fn insert_decoded(&mut self, key: BodyCacheKey, decoded: DecodedBytes) {
        self.remove(&key);
        if decoded.bytes.len() > self.capacity_bytes {
            return;
        }
        self.clock = self
            .clock
            .checked_add(1)
            .expect("decoded cache clock exhausted");
        self.bytes = self.bytes.saturating_add(decoded.bytes.len());
        self.lru.insert(self.clock, key.clone());
        self.entries.insert(
            key,
            DecodedCacheEntry {
                decoded,
                used_at: self.clock,
            },
        );
        while self.bytes > self.capacity_bytes {
            let Some((_, oldest)) = self.lru.pop_first() else {
                break;
            };
            if let Some(evicted) = self.entries.remove(&oldest) {
                self.bytes = self.bytes.saturating_sub(evicted.decoded.bytes.len());
            }
        }
    }

    #[cfg(test)]
    pub(super) fn apply_change(&mut self, change: CaptureChange) {
        self.apply_changes(std::iter::once(change));
    }

    fn apply_changes(&mut self, changes: impl IntoIterator<Item = CaptureChange>) {
        let mut invalidated = HashMap::<CaptureSequence, Option<u64>>::new();
        for change in changes {
            match change.kind {
                CaptureChangeKind::Clear => {
                    self.clear();
                    invalidated.clear();
                }
                CaptureChangeKind::RetentionEviction | CaptureChangeKind::ExplicitDelete => {
                    invalidated.insert(change.sequence, None);
                }
                CaptureChangeKind::Admitted | CaptureChangeKind::RecordUpdated => {
                    invalidated.insert(change.sequence, Some(change.revision));
                }
            }
        }
        if invalidated.is_empty() {
            return;
        }
        self.retain(|key| {
            invalidated
                .get(&key.capture_id)
                .is_none_or(|revision| revision.is_some_and(|value| key.capture_revision == value))
        });
    }

    pub(super) fn apply_feed_error(&mut self, _error: CaptureChangeError) {
        self.clear();
    }

    fn retain(&mut self, keep: impl Fn(&BodyCacheKey) -> bool) {
        let mut removed_lru = Vec::new();
        self.entries.retain(|key, entry| {
            if keep(key) {
                true
            } else {
                self.bytes = self.bytes.saturating_sub(entry.decoded.bytes.len());
                removed_lru.push(entry.used_at);
                false
            }
        });
        for used_at in removed_lru {
            self.lru.remove(&used_at);
        }
    }

    fn remove(&mut self, key: &BodyCacheKey) {
        if let Some(entry) = self.entries.remove(key) {
            self.bytes = self.bytes.saturating_sub(entry.decoded.bytes.len());
            self.lru.remove(&entry.used_at);
        }
    }

    fn clear(&mut self) {
        self.entries.clear();
        self.lru.clear();
        self.bytes = 0;
    }

    #[cfg(test)]
    pub(super) fn test_snapshot(&self) -> DecodedBodyCacheSnapshot {
        DecodedBodyCacheSnapshot {
            entries: self.entries.len(),
            bytes: self.bytes,
        }
    }
}

#[cfg(test)]
pub(super) struct RuntimeBodyServices {
    pub(super) admission: Arc<BodyWorkAdmission>,
    pub(super) decode: DecodeService,
}

#[cfg(test)]
impl RuntimeBodyServices {
    pub(super) fn start(policy: DecodePolicy, shutdown: CancellationToken) -> Self {
        let admission = Arc::new(BodyWorkAdmission::new());
        let decode = start_decode_service_with_admission(policy, shutdown, Arc::clone(&admission));
        Self { admission, decode }
    }

    pub(super) fn control_context(
        &self,
        runtime: RuntimeControlClient,
        capture_changes: CaptureChangeFeed,
    ) -> ControlServiceContext {
        ControlServiceContext {
            runtime,
            capture_changes,
            body_work: Arc::clone(&self.admission),
        }
    }
}
#[cfg(test)]
pub(super) struct DecodedBodyCacheSnapshot {
    pub(super) entries: usize,
    pub(super) bytes: usize,
}

#[derive(Clone)]
pub(super) struct BodyJobScheduler {
    runtime: RuntimeControlClient,
    admission: Arc<BodyWorkAdmission>,
    cache: Arc<Mutex<DecodedBodyCache>>,
    change_feed: CaptureChangeFeed,
    changes: Arc<Mutex<CaptureChangeSubscription>>,
}

struct BodyAdmissionPlan {
    key: BodyCacheKey,
    charge_bytes: usize,
    cached: Option<DecodedBytes>,
}

impl BodyAdmissionPlan {
    fn requires_readmission(&self, current: &Self) -> bool {
        self.key != current.key
            || self.charge_bytes != current.charge_bytes
            || self.cached.is_some() != current.cached.is_some()
    }
}

impl BodyJobScheduler {
    #[cfg(test)]
    pub(super) fn new(
        _instance: InstanceScope,
        runtime: RuntimeControlClient,
        change_feed: CaptureChangeFeed,
        admission: Arc<BodyWorkAdmission>,
    ) -> Self {
        Self::from_context(runtime, change_feed, admission)
    }

    fn from_context(
        runtime: RuntimeControlClient,
        change_feed: CaptureChangeFeed,
        admission: Arc<BodyWorkAdmission>,
    ) -> Self {
        let changes = change_feed.subscribe();
        Self {
            runtime,
            admission,
            cache: Arc::new(Mutex::new(DecodedBodyCache::new(
                MAX_TERMINAL_DECODED_CACHE_BYTES,
            ))),
            change_feed,
            changes: Arc::new(Mutex::new(changes)),
        }
    }

    async fn read(
        &self,
        request: BodyContentRequest,
        deadline: Instant,
        cancelled: CancellationToken,
    ) -> Result<ControlResult, ControlError> {
        request.validate()?;
        self.apply_pending_changes();
        let metadata = self.metadata(&request, cancelled.clone()).await?;
        validate_body_metadata(&request, &metadata)?;
        let mut plan = self.admission_plan(&request, &metadata);
        let admission_deadline = tokio::time::Instant::from_std(deadline);
        let mut queued = self.admission.try_admit_mcp_until(
            plan.charge_bytes,
            admission_deadline,
            &cancelled,
        )?;
        loop {
            let active = queued
                .acquire_active(admission_deadline, cancelled.clone())
                .await?;
            self.apply_pending_changes();
            let current = self.metadata(&request, cancelled.clone()).await?;
            validate_body_metadata(&request, &current)?;
            let current_plan = self.admission_plan(&request, &current);
            if plan.requires_readmission(&current_plan) {
                let charge_bytes = current_plan.charge_bytes;
                plan = current_plan;
                queued = active.retry_mcp_after_revalidation(
                    charge_bytes,
                    admission_deadline,
                    &cancelled,
                )?;
                continue;
            }

            if let Some(decoded) = current_plan.cached {
                drop(active);
                let page =
                    page_decoded_body(&decoded, &request, &current.status, &current.headers)?;
                return Ok(ControlResult::ReadCaptureBody {
                    instance: current.instance,
                    page: Box::new(page),
                });
            }

            let snapshot_epoch = self.change_feed.epoch();
            let snapshot = self.snapshot(&request, cancelled.clone()).await?;
            validate_body_snapshot(&request, &snapshot)?;
            let cache_epoch =
                (self.change_feed.epoch() == snapshot_epoch).then_some(snapshot_epoch);
            let instance = snapshot.instance.clone();
            let page = match request.representation {
                BodyRepresentation::Raw => page_raw_body(
                    &snapshot.preview,
                    &request,
                    &snapshot.status,
                    &snapshot.headers,
                )?,
                BodyRepresentation::Decoded => {
                    let decoded = decode_body_off_thread(
                        snapshot.preview.clone(),
                        snapshot.headers.clone(),
                        active,
                        deadline,
                        cancelled,
                    )
                    .await?;
                    if should_cache_decoded_body(&snapshot.status)
                        && let Some(cache_epoch) = cache_epoch
                    {
                        self.apply_pending_changes();
                        self.change_feed.run_if_epoch(cache_epoch, || {
                            self.cache
                                .lock()
                                .insert_decoded(current_plan.key, decoded.clone());
                        });
                    }
                    return Ok(ControlResult::ReadCaptureBody {
                        instance,
                        page: Box::new(page_decoded_body(
                            &decoded,
                            &request,
                            &snapshot.status,
                            &snapshot.headers,
                        )?),
                    });
                }
            };
            drop(active);
            return Ok(ControlResult::ReadCaptureBody {
                instance,
                page: Box::new(page),
            });
        }
    }

    async fn find_json_pointers(
        &self,
        request: FindJsonPointersRequest,
        deadline: Instant,
        cancelled: CancellationToken,
    ) -> Result<ControlResult, ControlError> {
        request.validate()?;
        let capture_revision = request.capture_revision;
        let field_name = request.field_name;
        let match_mode = request.match_mode;
        let limit = request.limit;
        let target = BodyContentRequest {
            capture_id: request.capture_id,
            capture_revision,
            side: request.side,
            representation: BodyRepresentation::Decoded,
            offset: 0,
            length: 1,
        };
        let (instance, result) = self
            .inspect_decoded_body(
                target,
                deadline,
                cancelled,
                false,
                move |decoded, status, _headers, _instance, cancelled| {
                    reject_limited_structured_input(decoded)?;
                    let mut result = find_json_pointers(
                        &decoded.bytes,
                        &field_name,
                        match_mode,
                        limit,
                        cancelled,
                    )?;
                    result.capture_revision = capture_revision;
                    result.source_truncated = status.preview_limit.is_some();
                    Ok(result)
                },
            )
            .await?;
        Ok(ControlResult::FindJsonPointers {
            instance,
            result: Box::new(result),
        })
    }

    async fn probe_json_pointer_pattern(
        &self,
        request: ProbeJsonPointerPatternRequest,
        deadline: Instant,
        cancelled: CancellationToken,
    ) -> Result<ControlResult, ControlError> {
        request.validate()?;
        let capture_revision = request.capture_revision;
        let pattern = request.pattern;
        let target = BodyContentRequest {
            capture_id: request.capture_id,
            capture_revision,
            side: request.side,
            representation: BodyRepresentation::Decoded,
            offset: 0,
            length: 1,
        };
        let (instance, result) = self
            .inspect_decoded_body(
                target,
                deadline,
                cancelled,
                false,
                move |decoded, status, _headers, _instance, cancelled| {
                    reject_limited_structured_input(decoded)?;
                    let mut result = probe_json(&decoded.bytes, &pattern, cancelled)?;
                    result.capture_revision = capture_revision;
                    result.source_truncated = status.preview_limit.is_some();
                    Ok(result)
                },
            )
            .await?;
        Ok(ControlResult::ProbeJsonPointerPattern {
            instance,
            result: Box::new(result),
        })
    }

    async fn search_capture_body(
        &self,
        request: SearchCaptureBodyRequest,
        deadline: Instant,
        cancelled: CancellationToken,
    ) -> Result<ControlResult, ControlError> {
        request.validate()?;
        let capture_id = request.capture_id;
        let capture_revision = request.capture_revision;
        let side = request.side;
        let query = request.query;
        let limit = request.limit;
        let context_bytes = request.context_bytes;
        let target = decoded_inspection_target(capture_id, capture_revision, side);
        let (instance, result) = self
            .inspect_decoded_body(
                target,
                deadline,
                cancelled,
                false,
                move |decoded, status, _headers, instance, cancelled| {
                    let decoded_uri = content_resource_base_uri(
                        instance,
                        capture_id,
                        capture_revision,
                        side,
                        BodyRepresentation::Decoded,
                    );
                    let raw_uri = content_resource_base_uri(
                        instance,
                        capture_id,
                        capture_revision,
                        side,
                        BodyRepresentation::Raw,
                    );
                    let search = search_capture_body(
                        &decoded.bytes,
                        &query,
                        limit,
                        context_bytes,
                        &decoded_uri,
                        &raw_uri,
                        cancelled,
                    )
                    .map_err(|error| {
                        body_operation_error(
                            error,
                            capture_revision,
                            status.preview_limit.is_some(),
                        )
                    })?;
                    Ok(SearchCaptureBodyResult {
                        capture_revision,
                        total_matches: search.total_matches,
                        omitted_matches: search.omitted_matches,
                        decoded_bytes_inspected: decoded.bytes.len(),
                        matches: search.matches,
                        stream: status.stream,
                        observed_bytes: status.observed_bytes,
                        retained_bytes: status.retained_bytes,
                        source_truncated: status.preview_limit.is_some(),
                        source_truncation_reason: status.preview_limit,
                        decoded_encoding_chain: decoded.encoding_chain.clone(),
                        decoded_output_limited: decoded.output_limited,
                    })
                },
            )
            .await?;
        Ok(ControlResult::SearchCaptureBody {
            instance,
            result: Box::new(result),
        })
    }

    async fn extract_capture_body(
        &self,
        request: ExtractCaptureBodyRequest,
        deadline: Instant,
        cancelled: CancellationToken,
    ) -> Result<ControlResult, ControlError> {
        request.validate()?;
        let capture_id = request.capture_id;
        let capture_revision = request.capture_revision;
        let side = request.side;
        let selector = request.selector;
        let selector_kind = selector.kind();
        let target = decoded_inspection_target(capture_id, capture_revision, side);
        let (instance, result) = self
            .inspect_decoded_body(
                target,
                deadline,
                cancelled,
                true,
                move |decoded, status, headers, instance, cancelled| {
                    reject_limited_structured_input(decoded)?;
                    reject_truncated_structured_source(status)?;
                    let selection = SelectionResourceRequest {
                        proxy_endpoint: instance.proxy_endpoint,
                        run_id: instance.run_id.clone(),
                        capture_id,
                        capture_revision,
                        side,
                        selector: selector.clone(),
                        offset: 0,
                        length: crate::control::body::DEFAULT_BODY_PAGE_LENGTH,
                    };
                    let selection_uri = selection.base_uri()?;
                    let extracted = extract_capture_body(
                        &decoded.bytes,
                        media_type(headers).as_deref(),
                        &selector,
                        &selection_uri,
                        cancelled,
                    )
                    .map_err(|error| {
                        body_operation_error(
                            error,
                            capture_revision,
                            status.preview_limit.is_some(),
                        )
                    })?;
                    Ok(ExtractCaptureBodyResult {
                        capture_revision,
                        selector_kind,
                        selected_bytes: extracted.selected_bytes,
                        media_type: extracted.media_type,
                        inline: extracted.inline,
                        resource_uri: extracted.resource_uri,
                        stream: status.stream,
                        observed_bytes: status.observed_bytes,
                        retained_bytes: status.retained_bytes,
                        source_truncated: status.preview_limit.is_some(),
                        source_truncation_reason: status.preview_limit,
                        decoded_encoding_chain: decoded.encoding_chain.clone(),
                    })
                },
            )
            .await?;
        Ok(ControlResult::ExtractCaptureBody {
            instance,
            result: Box::new(result),
        })
    }

    async fn read_selected_body(
        &self,
        request: SelectionContentRequest,
        deadline: Instant,
        cancelled: CancellationToken,
    ) -> Result<ControlResult, ControlError> {
        request.validate()?;
        let capture_id = request.capture_id;
        let capture_revision = request.capture_revision;
        let side = request.side;
        let selector = request.selector;
        let offset = request.offset;
        let length = request.length;
        let target = decoded_inspection_target(capture_id, capture_revision, side);
        let (instance, page) = self
            .inspect_decoded_body(
                target,
                deadline,
                cancelled,
                true,
                move |decoded, status, headers, instance, cancelled| {
                    reject_limited_structured_input(decoded)?;
                    reject_truncated_structured_source(status)?;
                    let selection = SelectionResourceRequest {
                        proxy_endpoint: instance.proxy_endpoint,
                        run_id: instance.run_id.clone(),
                        capture_id,
                        capture_revision,
                        side,
                        selector: selector.clone(),
                        offset,
                        length,
                    };
                    let selection_uri = selection.base_uri()?;
                    let (selected, selected_media_type) = extract_selected_bytes(
                        &decoded.bytes,
                        media_type(headers).as_deref(),
                        &selector,
                        cancelled,
                    )
                    .map_err(|error| {
                        body_operation_error(
                            error,
                            capture_revision,
                            status.preview_limit.is_some(),
                        )
                    })?;
                    let window =
                        page_selected_representation(&selected, offset, length, &selection_uri)?;
                    Ok(SelectionPage {
                        content: window.content,
                        media_type: selected_media_type,
                        selected_bytes: window.total_bytes,
                        requested_range: window.requested_range,
                        actual_range: window.actual_range,
                        next_offset: window.next_offset,
                        next_uri: window.next_uri,
                        source: decoded_source(status, decoded),
                    })
                },
            )
            .await?;
        Ok(ControlResult::ReadSelectedBody {
            instance,
            page: Box::new(page),
        })
    }

    async fn inspect_decoded_body<R, F>(
        &self,
        request: BodyContentRequest,
        deadline: Instant,
        cancelled: CancellationToken,
        require_complete_source: bool,
        work: F,
    ) -> Result<(InstanceScope, R), ControlError>
    where
        R: Send + 'static,
        F: FnOnce(
                &DecodedBytes,
                &BodyStatus,
                &CapturedHeaders,
                &InstanceScope,
                &AtomicBool,
            ) -> Result<R, ControlError>
            + Send
            + 'static,
    {
        self.apply_pending_changes();
        let metadata = self.metadata(&request, cancelled.clone()).await?;
        validate_body_metadata(&request, &metadata)?;
        if require_complete_source {
            reject_truncated_structured_source(&metadata.status)
                .map_err(|error| body_operation_error(error, request.capture_revision, true))?;
        }
        let mut plan = self.admission_plan(&request, &metadata);
        let admission_deadline = tokio::time::Instant::from_std(deadline);
        let mut queued = self.admission.try_admit_mcp_until(
            plan.charge_bytes,
            admission_deadline,
            &cancelled,
        )?;
        let mut work = Some(work);
        loop {
            let active = queued
                .acquire_active(admission_deadline, cancelled.clone())
                .await?;
            self.apply_pending_changes();
            let current = self.metadata(&request, cancelled.clone()).await?;
            validate_body_metadata(&request, &current)?;
            if require_complete_source {
                reject_truncated_structured_source(&current.status)
                    .map_err(|error| body_operation_error(error, request.capture_revision, true))?;
            }
            let current_plan = self.admission_plan(&request, &current);
            if plan.requires_readmission(&current_plan) {
                let charge_bytes = current_plan.charge_bytes;
                plan = current_plan;
                queued = active.retry_mcp_after_revalidation(
                    charge_bytes,
                    admission_deadline,
                    &cancelled,
                )?;
                continue;
            }
            let work = work
                .take()
                .ok_or_else(|| ControlError::internal("decoded body work was already consumed"))?;
            if let Some(decoded) = current_plan.cached {
                let work_headers = current.headers.clone();
                let instance = current.instance;
                let work_instance = instance.clone();
                let result = inspect_cached_body_off_thread(
                    decoded,
                    current.status,
                    active,
                    deadline,
                    cancelled,
                    move |decoded, status, cancelled| {
                        work(decoded, status, &work_headers, &work_instance, cancelled)
                    },
                )
                .await?;
                return Ok((instance, result));
            }

            let snapshot_epoch = self.change_feed.epoch();
            let snapshot = self.snapshot(&request, cancelled.clone()).await?;
            validate_body_snapshot(&request, &snapshot)?;
            let cache_epoch =
                (self.change_feed.epoch() == snapshot_epoch).then_some(snapshot_epoch);
            let instance = snapshot.instance.clone();
            let work_instance = instance.clone();
            let work_headers = snapshot.headers.clone();
            let status = snapshot.status.clone();
            let (decoded, result) = decode_and_inspect_body_off_thread(
                snapshot.preview.clone(),
                snapshot.headers.clone(),
                status,
                active,
                deadline,
                cancelled,
                move |decoded, status, cancelled| {
                    work(decoded, status, &work_headers, &work_instance, cancelled)
                },
            )
            .await?;
            if should_cache_decoded_body(&snapshot.status)
                && let Some(cache_epoch) = cache_epoch
            {
                self.apply_pending_changes();
                self.change_feed.run_if_epoch(cache_epoch, || {
                    self.cache.lock().insert_decoded(current_plan.key, decoded);
                });
            }
            return Ok((instance, result));
        }
    }

    fn admission_plan(
        &self,
        request: &BodyContentRequest,
        metadata: &CaptureBodyMetadataReply,
    ) -> BodyAdmissionPlan {
        let key = BodyCacheKey {
            instance: metadata.instance.clone(),
            capture_id: request.capture_id,
            capture_revision: request.capture_revision,
            side: request.side,
            representation: request.representation,
        };
        let cached = (request.representation == BodyRepresentation::Decoded
            && should_cache_decoded_body(&metadata.status))
        .then(|| self.cache.lock().get_decoded(&key))
        .flatten();
        let charge_bytes = cached
            .as_ref()
            .map_or(metadata.retained_bytes, |decoded| decoded.bytes.len());
        BodyAdmissionPlan {
            key,
            charge_bytes,
            cached,
        }
    }

    async fn metadata(
        &self,
        request: &BodyContentRequest,
        cancelled: CancellationToken,
    ) -> Result<CaptureBodyMetadataReply, ControlError> {
        match self
            .runtime
            .request(
                RuntimeRequest::GetCaptureBodyMetadata {
                    capture_id: request.capture_id,
                    side: request.side,
                },
                cancelled,
            )
            .await?
        {
            RuntimeReply::CaptureBodyMetadata(reply) => Ok(*reply),
            _ => Err(ControlError::internal(
                "runtime returned an unexpected capture body metadata reply",
            )),
        }
    }

    async fn snapshot(
        &self,
        request: &BodyContentRequest,
        cancelled: CancellationToken,
    ) -> Result<CaptureBodySnapshotReply, ControlError> {
        match self
            .runtime
            .request(
                RuntimeRequest::GetCaptureBodySnapshot {
                    capture_id: request.capture_id,
                    expected_revision: request.capture_revision,
                    side: request.side,
                },
                cancelled,
            )
            .await?
        {
            RuntimeReply::CaptureBodySnapshot(reply) => Ok(*reply),
            _ => Err(ControlError::internal(
                "runtime returned an unexpected capture body snapshot reply",
            )),
        }
    }

    fn apply_pending_changes(&self) {
        let mut subscription = self.changes.lock();
        let mut changes = Vec::new();
        let mut feed_error = None;
        loop {
            match subscription.try_recv() {
                Ok(Some(change)) => changes.push(change),
                Ok(None) => break,
                Err(error) => {
                    feed_error = Some(error);
                    *subscription = self.change_feed.subscribe();
                    break;
                }
            }
        }
        drop(subscription);
        if let Some(error) = feed_error {
            self.cache.lock().apply_feed_error(error);
        } else if !changes.is_empty() {
            self.cache.lock().apply_changes(changes);
        }
    }

    #[cfg(test)]
    pub(super) fn test_admission(&self) -> &Arc<BodyWorkAdmission> {
        &self.admission
    }

    #[cfg(test)]
    pub(super) fn test_cache_capacity_bytes(&self) -> usize {
        self.cache.lock().capacity_bytes
    }
}

fn validate_body_metadata(
    request: &BodyContentRequest,
    reply: &CaptureBodyMetadataReply,
) -> Result<(), ControlError> {
    if reply.capture_id != request.capture_id || reply.side != request.side {
        return Err(ControlError::internal(
            "runtime returned mismatched capture body metadata",
        ));
    }
    validate_body_revision(request, reply.capture_revision)
}

fn validate_body_snapshot(
    request: &BodyContentRequest,
    reply: &CaptureBodySnapshotReply,
) -> Result<(), ControlError> {
    if reply.capture_id != request.capture_id || reply.side != request.side {
        return Err(ControlError::internal(
            "runtime returned mismatched capture body snapshot",
        ));
    }
    if reply.retained_bytes != reply.preview.len() {
        return Err(ControlError::internal(
            "runtime returned inconsistent capture body snapshot bytes",
        ));
    }
    validate_body_revision(request, reply.capture_revision)
}

fn validate_body_revision(
    request: &BodyContentRequest,
    current_revision: u64,
) -> Result<(), ControlError> {
    if current_revision == request.capture_revision {
        return Ok(());
    }
    Err(ControlError::new(
        ControlErrorCode::CaptureRevisionConflict,
        "capture revision changed",
        false,
        serde_json::json!({
            "capture_id": request.capture_id,
            "expected_revision": request.capture_revision,
            "current_revision": current_revision,
        }),
    ))
}

struct DecodeWorkerCancellation {
    flag: Arc<AtomicBool>,
    armed: bool,
}

impl DecodeWorkerCancellation {
    fn new(flag: Arc<AtomicBool>) -> Self {
        Self { flag, armed: true }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for DecodeWorkerCancellation {
    fn drop(&mut self) {
        if self.armed {
            self.flag.store(true, Ordering::Release);
        }
    }
}

async fn decode_body_off_thread(
    preview: CapturedBodyPreview,
    headers: CapturedHeaders,
    active: ActiveBodyWorkLease,
    deadline: Instant,
    cancelled: CancellationToken,
) -> Result<DecodedBytes, ControlError> {
    let cancellation_flag = Arc::new(AtomicBool::new(false));
    let mut cancel_worker_on_drop = DecodeWorkerCancellation::new(Arc::clone(&cancellation_flag));
    let worker_flag = Arc::clone(&cancellation_flag);
    let mut worker = tokio::task::spawn_blocking(move || {
        let _active = active;
        decode_content_bytes(
            &preview,
            &headers,
            &ContentDecodePolicy::default(),
            &worker_flag,
        )
    });
    tokio::select! {
        biased;
        _ = cancelled.cancelled() => {
            cancellation_flag.store(true, Ordering::Release);
            let _ = worker.await;
            Err(ControlError::cancelled("capture body decode cancelled"))
        }
        _ = tokio::time::sleep_until(tokio::time::Instant::from_std(deadline)) => {
            cancellation_flag.store(true, Ordering::Release);
            let _ = worker.await;
            Err(ControlError::deadline_exceeded("capture body decode deadline elapsed"))
        }
        result = &mut worker => {
            cancel_worker_on_drop.disarm();
            result
                .map_err(|_| ControlError::internal("capture body decode worker failed"))?
                .map_err(|error| {
                    ControlError::new(
                        error.code(),
                        "capture body decoding failed",
                        false,
                        error.details(),
                    )
                })
        }
    }
}
fn reject_limited_structured_input(decoded: &DecodedBytes) -> Result<(), ControlError> {
    if decoded.output_limited {
        return Err(ControlError::new(
            ControlErrorCode::JsonSizeLimit,
            "decoded capture JSON body exceeds the structured input limit",
            false,
            serde_json::json!({"maximum_bytes": crate::control::body::MAX_DECODED_CONTENT_BYTES}),
        ));
    }
    Ok(())
}
fn reject_truncated_structured_source(status: &BodyStatus) -> Result<(), ControlError> {
    if status.preview_limit.is_some() {
        return Err(ControlError::new(
            ControlErrorCode::ResourceLimit,
            "retained capture body is incomplete",
            false,
            serde_json::json!({
                "source_truncated": true,
                "truncation_reason": status.preview_limit,
            }),
        ));
    }
    Ok(())
}

fn decoded_inspection_target(
    capture_id: CaptureSequence,
    capture_revision: u64,
    side: BodySide,
) -> BodyContentRequest {
    BodyContentRequest {
        capture_id,
        capture_revision,
        side,
        representation: BodyRepresentation::Decoded,
        offset: 0,
        length: 1,
    }
}

fn decoded_source(status: &BodyStatus, decoded: &DecodedBytes) -> BodyPageSource {
    BodyPageSource {
        stream: status.stream,
        observed_bytes: status.observed_bytes,
        retained_bytes: status.retained_bytes,
        truncated: status.preview_limit.is_some(),
        truncation_reason: status.preview_limit,
        decoded_encoding_chain: decoded.encoding_chain.clone(),
        decoded_output_limited: decoded.output_limited,
    }
}

fn body_operation_error(
    mut error: ControlError,
    capture_revision: u64,
    source_truncated: bool,
) -> ControlError {
    let mut details = match error.details {
        serde_json::Value::Object(details) => details,
        _ => serde_json::Map::new(),
    };
    details.insert("capture_revision".to_owned(), capture_revision.into());
    details.insert("source_truncated".to_owned(), source_truncated.into());
    error.details = serde_json::Value::Object(details);
    error
}

async fn inspect_cached_body_off_thread<R, F>(
    decoded: DecodedBytes,
    status: BodyStatus,
    active: ActiveBodyWorkLease,
    deadline: Instant,
    cancelled: CancellationToken,
    work: F,
) -> Result<R, ControlError>
where
    R: Send + 'static,
    F: FnOnce(&DecodedBytes, &BodyStatus, &AtomicBool) -> Result<R, ControlError> + Send + 'static,
{
    let cancellation_flag = Arc::new(AtomicBool::new(false));
    let mut cancel_worker_on_drop = DecodeWorkerCancellation::new(Arc::clone(&cancellation_flag));
    let worker_flag = Arc::clone(&cancellation_flag);
    let mut worker = tokio::task::spawn_blocking(move || {
        let _active = active;
        work(&decoded, &status, &worker_flag)
    });
    tokio::select! {
        biased;
        _ = cancelled.cancelled() => {
            cancellation_flag.store(true, Ordering::Release);
            let _ = worker.await;
            Err(ControlError::cancelled("capture JSON traversal cancelled"))
        }
        _ = tokio::time::sleep_until(tokio::time::Instant::from_std(deadline)) => {
            cancellation_flag.store(true, Ordering::Release);
            let _ = worker.await;
            Err(ControlError::deadline_exceeded("capture JSON traversal deadline elapsed"))
        }
        result = &mut worker => {
            cancel_worker_on_drop.disarm();
            result.map_err(|_| ControlError::internal("capture JSON traversal worker failed"))?
        }
    }
}

async fn decode_and_inspect_body_off_thread<R, F>(
    preview: CapturedBodyPreview,
    headers: CapturedHeaders,
    status: BodyStatus,
    active: ActiveBodyWorkLease,
    deadline: Instant,
    cancelled: CancellationToken,
    work: F,
) -> Result<(DecodedBytes, R), ControlError>
where
    R: Send + 'static,
    F: FnOnce(&DecodedBytes, &BodyStatus, &AtomicBool) -> Result<R, ControlError> + Send + 'static,
{
    let cancellation_flag = Arc::new(AtomicBool::new(false));
    let mut cancel_worker_on_drop = DecodeWorkerCancellation::new(Arc::clone(&cancellation_flag));
    let worker_flag = Arc::clone(&cancellation_flag);
    let mut worker = tokio::task::spawn_blocking(move || {
        let _active = active;
        let decoded = decode_content_bytes(
            &preview,
            &headers,
            &ContentDecodePolicy::default(),
            &worker_flag,
        )
        .map_err(|error| {
            ControlError::new(
                error.code(),
                "capture body decoding failed",
                false,
                error.details(),
            )
        })?;
        let result = work(&decoded, &status, &worker_flag)?;
        Ok((decoded, result))
    });
    tokio::select! {
        biased;
        _ = cancelled.cancelled() => {
            cancellation_flag.store(true, Ordering::Release);
            let _ = worker.await;
            Err(ControlError::cancelled("capture JSON decode or traversal cancelled"))
        }
        _ = tokio::time::sleep_until(tokio::time::Instant::from_std(deadline)) => {
            cancellation_flag.store(true, Ordering::Release);
            let _ = worker.await;
            Err(ControlError::deadline_exceeded(
                "capture JSON decode or traversal deadline elapsed",
            ))
        }
        result = &mut worker => {
            cancel_worker_on_drop.disarm();
            result
                .map_err(|_| ControlError::internal("capture JSON body worker failed"))?
        }
    }
}

#[derive(Clone)]
pub(super) struct ControlServiceContext {
    pub(super) runtime: RuntimeControlClient,
    pub(super) capture_changes: CaptureChangeFeed,
    pub(super) body_work: Arc<BodyWorkAdmission>,
}

#[cfg(test)]
impl From<RuntimeControlClient> for ControlServiceContext {
    fn from(runtime: RuntimeControlClient) -> Self {
        Self {
            runtime,
            capture_changes: CaptureChangeFeed::new(),
            body_work: Arc::new(BodyWorkAdmission::default()),
        }
    }
}

#[derive(Clone)]
pub(super) struct RuntimeControlHandler {
    runtime: RuntimeControlClient,
    settings_transactions: Option<SettingsTransactionClient>,
    capture_changes: CaptureChangeFeed,
    capture_searches: Arc<CaptureSearchAdmission>,
    capture_details: Arc<DetailMaterializationAdmission>,
    body_jobs: BodyJobScheduler,
}

impl RuntimeControlHandler {
    pub(super) fn new(context: impl Into<ControlServiceContext>) -> Self {
        let context = context.into();
        let body_jobs = BodyJobScheduler::from_context(
            context.runtime.clone(),
            context.capture_changes.clone(),
            context.body_work,
        );
        Self {
            runtime: context.runtime,
            settings_transactions: None,
            capture_changes: context.capture_changes,
            capture_searches: Arc::new(CaptureSearchAdmission::new(4)),
            capture_details: Arc::new(DetailMaterializationAdmission::new(4)),
            body_jobs,
        }
    }

    pub(super) fn with_settings_transactions(
        mut self,
        settings_transactions: SettingsTransactionClient,
    ) -> Self {
        self.settings_transactions = Some(settings_transactions);
        self
    }

    async fn instance_snapshot(
        runtime: &RuntimeControlClient,
        cancelled: &CancellationToken,
    ) -> Result<InstanceRuntimeSnapshot, ControlError> {
        match runtime
            .request(RuntimeRequest::GetStatus, cancelled.clone())
            .await?
        {
            RuntimeReply::Instance(snapshot) => Ok(snapshot),
            _ => Err(ControlError::internal(
                "runtime returned an unexpected status reply",
            )),
        }
    }

    async fn search_captures(
        runtime: RuntimeControlClient,
        admission: Arc<CaptureSearchAdmission>,
        query: CaptureQuery,
        mut cursor: Option<CaptureSearchCursor>,
        limit: Option<usize>,
        cancelled: CancellationToken,
    ) -> Result<
        (
            InstanceRuntimeSnapshot,
            Vec<CompactCapture>,
            Option<CaptureSearchCursor>,
        ),
        ControlError,
    > {
        let limit = normalize_capture_page_limit(limit)?;
        let mut active = admission.admit(query, cancelled.clone()).await?;
        let instance = Self::instance_snapshot(&runtime, &cancelled).await?;
        let mut captures = Vec::with_capacity(limit);
        let mut serialized_bytes = 0;

        loop {
            if serialized_bytes == CAPTURE_SEARCH_PAGE_JSON_BUDGET {
                return Ok((instance, captures, cursor));
            }
            let batch = match runtime
                .request(
                    RuntimeRequest::GetCaptureSearchBatch {
                        cursor,
                        max_rows: CAPTURE_SEARCH_BATCH_SIZE,
                    },
                    cancelled.clone(),
                )
                .await?
            {
                RuntimeReply::CaptureSearchBatch(batch) => batch,
                _ => {
                    return Err(ControlError::internal(
                        "runtime returned an unexpected capture batch reply",
                    ));
                }
            };
            if batch.snapshots.is_empty() {
                return Ok((instance, captures, None));
            }
            let remaining_bytes = CAPTURE_SEARCH_PAGE_JSON_BUDGET.saturating_sub(serialized_bytes);
            let remaining = limit - captures.len();
            let batch_cursor = batch.next_cursor;
            let worker_cancelled = cancelled.clone();
            let page = active
                .run_blocking(cancelled.clone(), move |query| {
                    match_capture_page_with_budget(
                        &batch.snapshots,
                        &query,
                        None,
                        remaining,
                        CAPTURE_SEARCH_PAGE_JSON_BUDGET,
                        remaining_bytes,
                        &worker_cancelled,
                    )
                })
                .await?;
            let CaptureSearchPage {
                captures: matched,
                next_cursor: page_cursor,
                serialized_bytes: matched_bytes,
            } = page;
            serialized_bytes += matched_bytes;
            captures.extend(matched);
            if captures.len() == limit {
                return Ok((instance, captures, page_cursor.or(batch_cursor)));
            }
            if page_cursor.is_some() {
                return Ok((instance, captures, page_cursor));
            }
            let Some(next_cursor) = batch_cursor else {
                return Ok((instance, captures, None));
            };
            cursor = Some(next_cursor);
        }
    }

    async fn wait_for_capture(
        runtime: RuntimeControlClient,
        capture_changes: CaptureChangeFeed,
        admission: Arc<CaptureSearchAdmission>,
        request: WaitForCaptureRequest,
        cancelled: CancellationToken,
    ) -> Result<ControlResult, ControlError> {
        let WaitForCaptureRequest {
            query,
            milestone,
            timeout_ms,
        } = request;
        let timeout_ms = normalize_wait_timeout_ms(timeout_ms)?;
        let cancelled = cancelled.child_token();
        let _cancel_on_exit = cancelled.clone().drop_guard();
        let mut changes = capture_changes.subscribe();
        let active = admission.admit(query, cancelled.clone()).await?;
        let query = active.compiled_query();
        drop(active);
        let instance = Self::instance_snapshot(&runtime, &cancelled)
            .await?
            .instance;
        let deadline = tokio::time::Instant::now() + Duration::from_millis(timeout_ms);
        let mut sequences = HashMap::new();
        let mut cursor = None;

        loop {
            let admitted = admission.admit_compiled(Arc::clone(&query), &cancelled);
            let Some(mut active) = wait_step(admitted, deadline, &cancelled).await? else {
                return Ok(unmatched_wait(instance));
            };
            let batch_request = runtime.request(
                RuntimeRequest::GetCaptureSearchBatch {
                    cursor,
                    max_rows: CAPTURE_SEARCH_BATCH_SIZE,
                },
                cancelled.clone(),
            );
            let Some(batch) = wait_step(batch_request, deadline, &cancelled).await? else {
                return Ok(unmatched_wait(instance));
            };
            let batch = match batch {
                RuntimeReply::CaptureSearchBatch(batch) => batch,
                _ => {
                    return Err(ControlError::internal(
                        "runtime returned an unexpected capture batch reply",
                    ));
                }
            };
            if batch.snapshots.is_empty() {
                drop(active);
                break;
            }
            let next_cursor = batch.next_cursor;
            let worker_cancelled = cancelled.clone();
            let inspection = active.run_blocking(cancelled.clone(), move |query| {
                inspect_wait_batch(
                    &batch.snapshots,
                    &query,
                    milestone,
                    &worker_cancelled,
                    &mut sequences,
                )
                .map(|inspection| (sequences, inspection))
            });
            let Some((returned_sequences, inspection)) =
                wait_step(inspection, deadline, &cancelled).await?
            else {
                return Ok(unmatched_wait(instance));
            };
            sequences = returned_sequences;
            drop(active);
            if let Some(capture) = inspection.capture {
                return Ok(matched_wait(instance, capture));
            }
            let Some(next_cursor) = next_cursor else {
                break;
            };
            cursor = Some(next_cursor);
        }

        loop {
            let change = tokio::select! {
                biased;
                _ = cancelled.cancelled() => {
                    return Err(ControlError::cancelled("capture wait cancelled"));
                }
                _ = time::sleep_until(deadline) => {
                    return Ok(unmatched_wait(instance));
                }
                change = changes.recv() => change.map_err(feed_gap)?,
            };

            match change.kind {
                CaptureChangeKind::RetentionEviction
                | CaptureChangeKind::ExplicitDelete
                | CaptureChangeKind::Clear => {
                    if sequences
                        .remove(&change.sequence)
                        .is_some_and(|state| state.pending)
                    {
                        return Err(capture_change_lost(
                            change,
                            "matching capture was removed while waiting",
                        ));
                    }
                }
                CaptureChangeKind::Admitted | CaptureChangeKind::RecordUpdated => {
                    if sequences
                        .get(&change.sequence)
                        .is_some_and(|state| change.revision <= state.revision)
                    {
                        continue;
                    }
                    let materialized = runtime.request(
                        RuntimeRequest::GetCapture {
                            capture_id: change.sequence,
                            expected_revision: None,
                        },
                        cancelled.clone(),
                    );
                    let reply = match wait_step(materialized, deadline, &cancelled).await {
                        Ok(Some(reply)) => reply,
                        Ok(None) => return Ok(unmatched_wait(instance)),
                        Err(error) if error.code == ControlErrorCode::CaptureNotFound => {
                            return Err(capture_change_lost(
                                change,
                                "changed capture could not be materialized",
                            ));
                        }
                        Err(error) => return Err(error),
                    };
                    let crate::control::CaptureSnapshotReply {
                        instance: materialized_instance,
                        snapshot,
                    } = match reply {
                        RuntimeReply::CaptureSnapshot(snapshot) => *snapshot,
                        _ => {
                            return Err(ControlError::internal(
                                "runtime returned an unexpected capture snapshot reply",
                            ));
                        }
                    };
                    if materialized_instance != instance {
                        return Err(ControlError::new(
                            ControlErrorCode::InstanceGenerationConflict,
                            "capture wait materialized another instance generation",
                            false,
                            serde_json::json!({
                                "expected_identity": instance,
                                "received_identity": materialized_instance,
                            }),
                        ));
                    }
                    let snapshot_revision = snapshot.revision;

                    let admitted = admission.admit_compiled(Arc::clone(&query), &cancelled);
                    let Some(mut active) = wait_step(admitted, deadline, &cancelled).await? else {
                        return Ok(unmatched_wait(instance));
                    };
                    let worker_cancelled = cancelled.clone();
                    let inspection = active.run_blocking(cancelled.clone(), move |query| {
                        inspect_wait_snapshot(&snapshot, &query, milestone, &worker_cancelled)
                    });
                    let Some(inspection) = wait_step(inspection, deadline, &cancelled).await?
                    else {
                        return Ok(unmatched_wait(instance));
                    };
                    drop(active);
                    let pending = match inspection {
                        WaitSnapshotInspection::Matched(capture) => {
                            return Ok(matched_wait(instance, capture));
                        }
                        WaitSnapshotInspection::Pending => true,
                        WaitSnapshotInspection::Ignore => false,
                    };
                    sequences.insert(
                        change.sequence,
                        WaitSequenceState {
                            revision: snapshot_revision,
                            pending,
                        },
                    );
                }
            }
        }
    }
}

impl ControlRpcHandler for RuntimeControlHandler {
    fn handle(
        &self,
        _context: ControlCallContext,
        operation: ControlOperation,
        cancelled: CancellationToken,
    ) -> impl Future<Output = Result<ControlResult, ControlError>> + Send {
        let runtime = self.runtime.clone();
        let capture_searches = Arc::clone(&self.capture_searches);
        let capture_details = Arc::clone(&self.capture_details);
        let capture_changes = self.capture_changes.clone();
        let body_jobs = self.body_jobs.clone();
        let settings_transactions = self.settings_transactions.clone();
        async move {
            match operation {
                ControlOperation::DescribeInstance => {
                    let snapshot = match runtime
                        .request(RuntimeRequest::DescribeInstance, cancelled.clone())
                        .await?
                    {
                        RuntimeReply::Instance(snapshot) => snapshot,
                        _ => {
                            return Err(ControlError::internal(
                                "runtime returned an unexpected description reply",
                            ));
                        }
                    };
                    Ok(ControlResult::DescribeInstance {
                        instance: snapshot.instance,
                        config_mode: snapshot.config_mode,
                        persistence: snapshot.persistence,
                        recording_enabled: snapshot.recording_enabled,
                        retained_capture_count: snapshot.retained_capture_count,
                        settings_revision: snapshot.settings_revision,
                    })
                }
                ControlOperation::GetStatus => {
                    let snapshot = Self::instance_snapshot(&runtime, &cancelled).await?;
                    Ok(ControlResult::GetStatus {
                        instance: snapshot.instance,
                        config_mode: snapshot.config_mode,
                        persistence: snapshot.persistence,
                        recording_enabled: snapshot.recording_enabled,
                        retained_capture_count: snapshot.retained_capture_count,
                        settings_revision: snapshot.settings_revision,
                    })
                }
                ControlOperation::SetRecordingEnabled { enabled } => {
                    let update = match runtime
                        .request(
                            RuntimeRequest::SetRecordingEnabled { enabled },
                            cancelled.clone(),
                        )
                        .await?
                    {
                        RuntimeReply::RecordingUpdated(update) => update,
                        _ => {
                            return Err(ControlError::internal(
                                "runtime returned an unexpected recording reply",
                            ));
                        }
                    };
                    Ok(ControlResult::SetRecordingEnabled {
                        instance: update.instance,
                        previous: update.previous,
                        current: update.current,
                    })
                }
                ControlOperation::SearchCaptures {
                    query,
                    cursor,
                    limit,
                } => {
                    let (snapshot, captures, next_cursor) = Self::search_captures(
                        runtime,
                        capture_searches,
                        *query,
                        cursor,
                        limit,
                        cancelled,
                    )
                    .await?;
                    Ok(ControlResult::SearchCaptures {
                        instance: snapshot.instance,
                        captures,
                        next_cursor,
                    })
                }
                ControlOperation::GetCapture {
                    capture_id,
                    expected_revision,
                } => {
                    let snapshot = match runtime
                        .request(
                            RuntimeRequest::GetCapture {
                                capture_id,
                                expected_revision,
                            },
                            cancelled.clone(),
                        )
                        .await?
                    {
                        RuntimeReply::CaptureSnapshot(snapshot) => snapshot,
                        _ => {
                            return Err(ControlError::internal(
                                "runtime returned an unexpected capture snapshot reply",
                            ));
                        }
                    };
                    let crate::control::CaptureSnapshotReply { instance, snapshot } = *snapshot;
                    let capture = capture_details
                        .run_blocking(cancelled, move || CaptureDetail::from_snapshot(&snapshot))
                        .await?;
                    Ok(ControlResult::GetCapture {
                        instance,
                        capture: Box::new(capture),
                    })
                }
                ControlOperation::ReadCaptureBody(request) => {
                    body_jobs.read(*request, _context.deadline, cancelled).await
                }
                ControlOperation::SearchCaptureBody(request) => {
                    body_jobs
                        .search_capture_body(*request, _context.deadline, cancelled)
                        .await
                }
                ControlOperation::ExtractCaptureBody(request) => {
                    body_jobs
                        .extract_capture_body(*request, _context.deadline, cancelled)
                        .await
                }
                ControlOperation::ReadSelectedBody(request) => {
                    body_jobs
                        .read_selected_body(*request, _context.deadline, cancelled)
                        .await
                }
                ControlOperation::FindJsonPointers(request) => {
                    body_jobs
                        .find_json_pointers(*request, _context.deadline, cancelled)
                        .await
                }
                ControlOperation::ProbeJsonPointerPattern(request) => {
                    body_jobs
                        .probe_json_pointer_pattern(*request, _context.deadline, cancelled)
                        .await
                }
                ControlOperation::GetMappingSettings => {
                    let client = settings_transactions.as_ref().ok_or_else(|| {
                        ControlError::service_unavailable(
                            "settings transaction service is unavailable",
                        )
                    })?;
                    let identity = Self::instance_snapshot(&runtime, &cancelled)
                        .await?
                        .instance;
                    let result = client.get_mapping_settings(cancelled).await?;
                    Ok(ControlResult::GetMappingSettings {
                        instance: identity,
                        settings_revision: result.revision,
                        config_mode: result.config_mode,
                        persistence: result.persistence,
                        proxy: MappingSettingsPayload::from_snapshot(
                            result.settings,
                            result.worker_permit,
                        ),
                    })
                }
                ControlOperation::ValidateMappingSettings { proxy } => {
                    let client = settings_transactions.as_ref().ok_or_else(|| {
                        ControlError::service_unavailable(
                            "settings transaction service is unavailable",
                        )
                    })?;
                    let identity = Self::instance_snapshot(&runtime, &cancelled)
                        .await?
                        .instance;
                    let result = client.validate_mapping_settings(*proxy, cancelled).await?;
                    Ok(ControlResult::ValidateMappingSettings {
                        instance: identity,
                        settings_revision: result.revision,
                        config_mode: result.config_mode,
                        persistence: result.persistence,
                        validation: Box::new(result.validation),
                    })
                }
                ControlOperation::ExplainMapping {
                    url,
                    proposed_proxy,
                } => {
                    let client = settings_transactions.as_ref().ok_or_else(|| {
                        ControlError::service_unavailable(
                            "settings transaction service is unavailable",
                        )
                    })?;
                    let identity = Self::instance_snapshot(&runtime, &cancelled)
                        .await?
                        .instance;
                    let result = client
                        .explain_mapping(url, proposed_proxy.map(|proxy| *proxy), cancelled)
                        .await?;
                    Ok(ControlResult::ExplainMapping {
                        instance: identity,
                        settings_revision: result.revision,
                        config_mode: result.config_mode,
                        persistence: result.persistence,
                        explanation: Box::new(result.explanation),
                    })
                }
                ControlOperation::MutateMapping {
                    expected_revision,
                    mutation,
                } => {
                    let client = settings_transactions.as_ref().ok_or_else(|| {
                        ControlError::service_unavailable(
                            "settings transaction service is unavailable",
                        )
                    })?;
                    let identity = Self::instance_snapshot(&runtime, &cancelled)
                        .await?
                        .instance;
                    let result = client
                        .mutate_mapping(
                            *mutation,
                            expected_revision,
                            crate::runtime::settings::SettingsTransactionOrigin::Mcp,
                            cancelled,
                        )
                        .await?;
                    Ok(ControlResult::MutateMapping {
                        instance: identity,
                        settings_revision: result.revision,
                        config_mode: result.config_mode,
                        persistence: result.persistence,
                        outcome: result.outcome,
                        affected: result.affected,
                    })
                }
                ControlOperation::WaitForCapture(request) => {
                    Self::wait_for_capture(
                        runtime,
                        capture_changes,
                        capture_searches,
                        *request,
                        cancelled,
                    )
                    .await
                }
            }
        }
    }
}

pub(super) trait ExistingDescriptorProbe: Clone + Send + Sync + 'static {
    fn probe<'a>(
        &'a self,
        descriptor: &'a InstanceDescriptor,
        deadline: Instant,
        cancelled: CancellationToken,
    ) -> BoxFuture<'a, Result<InstanceScope, ControlError>>;
}

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct ControlRpcDescriptorProbe;

impl ExistingDescriptorProbe for ControlRpcDescriptorProbe {
    fn probe<'a>(
        &'a self,
        descriptor: &'a InstanceDescriptor,
        deadline: Instant,
        cancelled: CancellationToken,
    ) -> BoxFuture<'a, Result<InstanceScope, ControlError>> {
        Box::pin(async move {
            match ControlRpcClient::call(
                descriptor,
                ControlOperation::DescribeInstance,
                deadline,
                DeclaredClient {
                    name: "wirelens-runtime".to_owned(),
                    version: env!("CARGO_PKG_VERSION").to_owned(),
                },
                cancelled,
            )
            .await?
            {
                ControlResult::DescribeInstance { instance, .. } => Ok(instance),
                _ => Err(ControlError::invalid_argument(
                    "private descriptor probe returned an unexpected operation",
                )),
            }
        })
    }
}

#[derive(Debug)]
pub(super) enum PrivateControlStartupError {
    ControlBind(io::Error),
    ServiceStart(io::Error),
    Publication(io::Error),
    Probe(ControlError),
    LiveEndpointConflict { existing: RunId, proposed: RunId },
}

impl fmt::Display for PrivateControlStartupError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ControlBind(error) => {
                write!(formatter, "failed to bind private control: {error}")
            }
            Self::ServiceStart(error) => {
                write!(
                    formatter,
                    "failed to start private control service: {error}"
                )
            }
            Self::Publication(error) => {
                write!(formatter, "failed to publish instance descriptor: {error}")
            }
            Self::Probe(error) => write!(formatter, "private descriptor probe failed: {error:?}"),
            Self::LiveEndpointConflict { existing, proposed } => write!(
                formatter,
                "proxy endpoint is already owned by live run {existing}; proposed run is {proposed}"
            ),
        }
    }
}

impl std::error::Error for PrivateControlStartupError {}

pub(super) struct PrivateControlStartup {
    publisher: RegistryPublisher,
}

impl fmt::Debug for PrivateControlStartup {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PrivateControlStartup")
            .field("descriptor_path", &self.publisher.descriptor_path())
            .field("socket_path", &self.publisher.socket_path())
            .finish()
    }
}

impl PrivateControlStartup {
    pub(super) fn prepare(
        enabled: bool,
        wirelens_home: &std::path::Path,
        identity: InstanceIdentity,
        settings: &SettingsSession,
    ) -> std::result::Result<Option<Self>, PrivateControlStartupError> {
        if !enabled {
            return Ok(None);
        }
        RegistryPublisher::prepare(wirelens_home, identity, settings)
            .map(|publisher| Some(Self { publisher }))
            .map_err(PrivateControlStartupError::ControlBind)
    }

    #[cfg(test)]
    pub(super) fn descriptor_path(&self) -> &std::path::Path {
        self.publisher.descriptor_path()
    }

    #[cfg(test)]
    pub(super) fn socket_path(&self) -> &std::path::Path {
        self.publisher.socket_path()
    }

    pub(super) fn start(
        self,
        handler: RuntimeControlHandler,
        shutdown: CancellationToken,
    ) -> std::result::Result<RunningPrivateControl, PrivateControlStartupError> {
        self.start_with_factory(handler, shutdown, TokioControlServiceFactory)
    }

    pub(super) fn start_with_factory<F>(
        mut self,
        handler: RuntimeControlHandler,
        shutdown: CancellationToken,
        factory: F,
    ) -> std::result::Result<RunningPrivateControl, PrivateControlStartupError>
    where
        F: ControlServiceFactory,
    {
        let listener = self
            .publisher
            .take_listener()
            .map_err(PrivateControlStartupError::ServiceStart)?;
        let identity = self.publisher.identity().clone();
        let (task, ready) = factory
            .start(listener, identity, handler, shutdown.clone())
            .map_err(PrivateControlStartupError::ServiceStart)?;
        Ok(RunningPrivateControl {
            publisher: Some(self.publisher),
            task: Some(task),
            ready: Some(ready),
            shutdown,
        })
    }
}

pub(super) trait ControlServiceFactory {
    fn start(
        self,
        listener: std::os::unix::net::UnixListener,
        identity: InstanceIdentity,
        handler: RuntimeControlHandler,
        shutdown: CancellationToken,
    ) -> io::Result<(JoinHandle<Result<()>>, oneshot::Receiver<()>)>;
}

#[derive(Clone, Copy)]
struct TokioControlServiceFactory;

impl ControlServiceFactory for TokioControlServiceFactory {
    fn start(
        self,
        listener: std::os::unix::net::UnixListener,
        identity: InstanceIdentity,
        handler: RuntimeControlHandler,
        shutdown: CancellationToken,
    ) -> io::Result<(JoinHandle<Result<()>>, oneshot::Receiver<()>)> {
        listener.set_nonblocking(true)?;
        let listener = UnixListener::from_std(listener)?;
        let server = Arc::new(ControlRpcServer::new(identity, handler));
        let (ready, ready_rx) = oneshot::channel();
        let task = tokio::spawn(async move {
            let mut calls = JoinSet::new();
            let _ = ready.send(());
            loop {
                tokio::select! {
                    _ = shutdown.cancelled() => {
                        calls.shutdown().await;
                        return Ok(());
                    }
                    accepted = listener.accept() => {
                        let (stream, _) = accepted.map_err(|error| {
                            anyhow!("private control listener accept failed: {error}")
                        })?;
                        let server = Arc::clone(&server);
                        let call_shutdown = shutdown.child_token();
                        calls.spawn(async move {
                            let _ = server
                                .serve_connection_until(stream, call_shutdown)
                                .await;
                        });
                    }
                    completion = calls.join_next(), if !calls.is_empty() => {
                        if let Some(Err(error)) = completion {
                            return Err(anyhow!(
                                "private control connection task failed: {error}"
                            ));
                        }
                    }
                }
            }
        });
        Ok((task, ready_rx))
    }
}

#[cfg(test)]
pub(super) struct FailingControlServiceFactory {
    error: io::Error,
}

#[cfg(test)]
impl FailingControlServiceFactory {
    pub(super) fn new(error: io::Error) -> Self {
        Self { error }
    }
}

#[cfg(test)]
impl ControlServiceFactory for FailingControlServiceFactory {
    fn start(
        self,
        _listener: std::os::unix::net::UnixListener,
        _identity: InstanceIdentity,
        _handler: RuntimeControlHandler,
        _shutdown: CancellationToken,
    ) -> io::Result<(JoinHandle<Result<()>>, oneshot::Receiver<()>)> {
        Err(self.error)
    }
}

pub(super) struct RunningPrivateControl {
    publisher: Option<RegistryPublisher>,
    task: Option<JoinHandle<Result<()>>>,
    ready: Option<oneshot::Receiver<()>>,
    shutdown: CancellationToken,
}

impl fmt::Debug for RunningPrivateControl {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RunningPrivateControl")
            .field(
                "descriptor_path",
                &self
                    .publisher
                    .as_ref()
                    .map(RegistryPublisher::descriptor_path),
            )
            .finish()
    }
}

fn control_service_completion_error(
    phase: &str,
    completion: std::result::Result<Result<()>, tokio::task::JoinError>,
) -> PrivateControlStartupError {
    let message = match completion {
        Ok(Ok(())) => format!("private control service exited {phase}"),
        Ok(Err(error)) => format!("private control service failed {phase}: {error:#}"),
        Err(error) => format!("private control service task failed {phase}: {error}"),
    };
    PrivateControlStartupError::ServiceStart(io::Error::other(message))
}

fn cancelled_publication_error() -> PrivateControlStartupError {
    PrivateControlStartupError::Probe(ControlError::instance_unavailable(
        "descriptor publication was cancelled",
    ))
}

impl RunningPrivateControl {
    pub(super) async fn wait_until_ready(
        &mut self,
    ) -> std::result::Result<(), PrivateControlStartupError> {
        let Some(mut ready) = self.ready.take() else {
            return self.ensure_running().await;
        };
        let mut task = self.task.take().ok_or_else(|| {
            PrivateControlStartupError::ServiceStart(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "private control service task is unavailable",
            ))
        })?;
        tokio::select! {
            biased;
            completion = &mut task => {
                Err(control_service_completion_error("before readiness", completion))
            },
            result = &mut ready => {
                self.task = Some(task);
                result.map_err(|_| {
                    PrivateControlStartupError::ServiceStart(io::Error::new(
                        io::ErrorKind::BrokenPipe,
                        "private control service exited before readiness",
                    ))
                })
            },
        }
    }

    pub(super) async fn ensure_running(
        &mut self,
    ) -> std::result::Result<(), PrivateControlStartupError> {
        let Some(task) = self.task.as_ref() else {
            return Err(PrivateControlStartupError::ServiceStart(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "private control service task is unavailable",
            )));
        };
        if !task.is_finished() {
            return Ok(());
        }
        let task = self
            .task
            .take()
            .expect("finished private control task was just observed");
        Err(control_service_completion_error(
            "before supervision handoff",
            task.await,
        ))
    }

    pub(super) async fn publish_after_probe<P>(
        &mut self,
        probe: &P,
        deadline: Instant,
        cancelled: CancellationToken,
    ) -> std::result::Result<(), PrivateControlStartupError>
    where
        P: ExistingDescriptorProbe,
    {
        self.wait_until_ready().await?;
        self.ensure_running().await?;
        if cancelled.is_cancelled() {
            return Err(cancelled_publication_error());
        }
        let (wirelens_home, endpoint, proposed_run) = {
            let publisher = self.publisher.as_ref().ok_or_else(|| {
                PrivateControlStartupError::Publication(io::Error::new(
                    io::ErrorKind::NotConnected,
                    "private control publisher is unavailable",
                ))
            })?;
            (
                publisher.wirelens_home().to_path_buf(),
                publisher.identity().proxy_endpoint(),
                publisher.identity().run_id().clone(),
            )
        };
        let report = RegistryScanner::new(&wirelens_home)
            .and_then(|scanner| scanner.read_endpoint(endpoint))
            .map_err(PrivateControlStartupError::Publication)?;
        if let Some(existing) = report.candidates.first() {
            let mut task = self.task.take().ok_or_else(|| {
                PrivateControlStartupError::ServiceStart(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "private control service task is unavailable",
                ))
            })?;
            let probe_call = probe.probe(existing, deadline, cancelled.clone());
            tokio::pin!(probe_call);
            let probe_result = tokio::select! {
                biased;
                completion = &mut task => {
                    return Err(control_service_completion_error(
                        "during descriptor probe",
                        completion,
                    ));
                },
                result = &mut probe_call => result,
            };
            self.task = Some(task);
            self.ensure_running().await?;
            if cancelled.is_cancelled() {
                return Err(cancelled_publication_error());
            }
            match probe_result {
                Ok(scope) => {
                    return Err(PrivateControlStartupError::LiveEndpointConflict {
                        existing: scope.run_id,
                        proposed: proposed_run,
                    });
                }
                Err(error) if error.is_definitive_stale_connect() => {
                    self.ensure_running().await?;
                    if cancelled.is_cancelled() {
                        return Err(cancelled_publication_error());
                    }
                    let removed = self
                        .publisher
                        .as_mut()
                        .ok_or_else(|| {
                            PrivateControlStartupError::Publication(io::Error::new(
                                io::ErrorKind::NotConnected,
                                "private control publisher is unavailable",
                            ))
                        })?
                        .remove_stale_for_replacement_if(existing, || !cancelled.is_cancelled())
                        .map_err(PrivateControlStartupError::Publication)?;
                    if cancelled.is_cancelled() {
                        return Err(cancelled_publication_error());
                    }
                    if !removed {
                        return Err(PrivateControlStartupError::Publication(io::Error::new(
                            io::ErrorKind::WouldBlock,
                            "endpoint descriptor changed after its private probe",
                        )));
                    }
                    self.ensure_running().await?;
                }
                Err(error) if error.code == ControlErrorCode::InstanceGenerationConflict => {
                    return Err(PrivateControlStartupError::LiveEndpointConflict {
                        existing: existing.run_id().clone(),
                        proposed: proposed_run,
                    });
                }
                Err(error) => return Err(PrivateControlStartupError::Probe(error)),
            }
        }
        self.ensure_running().await?;
        if cancelled.is_cancelled() {
            return Err(cancelled_publication_error());
        }
        self.publisher
            .as_mut()
            .ok_or_else(|| {
                PrivateControlStartupError::Publication(io::Error::new(
                    io::ErrorKind::NotConnected,
                    "private control publisher is unavailable",
                ))
            })?
            .publish()
            .map_err(PrivateControlStartupError::Publication)?;
        tokio::task::yield_now().await;
        self.ensure_running().await
    }

    pub(super) fn into_supervised_parts(
        mut self,
    ) -> std::result::Result<(RegistryPublisher, JoinHandle<Result<()>>), PrivateControlStartupError>
    {
        let task = self.task.as_ref().ok_or_else(|| {
            PrivateControlStartupError::ServiceStart(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "private control service task is unavailable",
            ))
        })?;
        if task.is_finished() {
            return Err(PrivateControlStartupError::ServiceStart(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "private control service exited before supervision handoff",
            )));
        }
        Ok((
            self.publisher
                .take()
                .expect("running private control always owns its publisher"),
            self.task
                .take()
                .expect("running private control always owns its task"),
        ))
    }

    pub(super) async fn shutdown(
        mut self,
        grace: Duration,
    ) -> std::result::Result<(), PrivateControlStartupError> {
        self.shutdown.cancel();
        if let Some(mut task) = self.task.take()
            && time::timeout(grace, &mut task).await.is_err()
        {
            task.abort();
            let _ = task.await;
        }
        self.publisher.take();
        Ok(())
    }

    pub(super) async fn rollback(
        self,
        grace: Duration,
    ) -> std::result::Result<(), PrivateControlStartupError> {
        self.shutdown(grace).await
    }
}
#[cfg(test)]
mod task12_tests;

impl Drop for RunningPrivateControl {
    fn drop(&mut self) {
        self.shutdown.cancel();
        if let Some(task) = self.task.as_ref() {
            task.abort();
        }
    }
}

#[cfg(test)]
mod task8_tests;

#[cfg(test)]
mod task9_tests;

#[cfg(test)]
mod task10_tests;

#[cfg(test)]
mod tests {
    use super::{RuntimeControlHandler, RuntimeGateway};
    use crate::{
        control::{InstanceRuntimeSnapshot, RuntimeReply, RuntimeRequest},
        control_rpc::{
            protocol::{
                ControlError, ControlErrorCode, ControlOperation, ControlResult, DeclaredClient,
                InstanceScope,
            },
            server::{ControlCallContext, ControlRpcHandler},
        },
        instance::InstanceIdentity,
        settings::{ConfigMode, PersistenceMode},
    };
    use std::time::{Duration, Instant};
    use tokio_util::sync::CancellationToken;

    fn snapshot(identity: &InstanceIdentity) -> InstanceRuntimeSnapshot {
        InstanceRuntimeSnapshot {
            instance: InstanceScope {
                proxy_endpoint: identity.proxy_endpoint(),
                run_id: identity.run_id().clone(),
            },
            config_mode: ConfigMode::Temporary,
            persistence: PersistenceMode::Ephemeral,
            recording_enabled: false,
            retained_capture_count: 0,
            settings_revision: 0,
        }
    }

    fn context() -> ControlCallContext {
        ControlCallContext {
            request_id: "request-through-runtime".to_owned(),
            declared_client: DeclaredClient {
                name: "runtime-control-test".to_owned(),
                version: "1".to_owned(),
            },
            deadline: Instant::now() + Duration::from_secs(1),
        }
    }

    #[tokio::test]
    async fn gateway_rejects_after_shutdown() {
        let (client, receiver) = RuntimeGateway::channel(64);
        drop(receiver);

        let error = client
            .request(RuntimeRequest::DescribeInstance, CancellationToken::new())
            .await
            .expect_err("closed gateway");

        assert_eq!(error.code, ControlErrorCode::InstanceUnavailable);
    }

    #[tokio::test]
    async fn gateway_delivers_the_request_cancellation_token_and_exact_reply() {
        let identity =
            InstanceIdentity::new("127.0.0.1:19006".parse().expect("endpoint")).expect("identity");
        let expected = snapshot(&identity);
        let (client, mut receiver) = RuntimeGateway::channel(1);
        let cancelled = CancellationToken::new();
        let request_cancelled = cancelled.clone();
        let client_task = tokio::spawn(async move {
            client
                .request(RuntimeRequest::DescribeInstance, request_cancelled)
                .await
        });

        let command = receiver.recv().await.expect("admitted runtime command");
        assert_eq!(command.request, RuntimeRequest::DescribeInstance);
        assert!(!command.cancelled.is_cancelled());
        command
            .reply
            .send(Ok(RuntimeReply::Instance(expected.clone())))
            .expect("waiting runtime client");

        let actual = client_task
            .await
            .expect("client task")
            .expect("runtime reply");
        assert_eq!(actual.instance(), &expected);
    }

    #[tokio::test]
    async fn full_gateway_waits_without_dropping_the_next_command() {
        let identity =
            InstanceIdentity::new("127.0.0.1:19007".parse().expect("endpoint")).expect("identity");
        let expected = snapshot(&identity);
        let (client, mut receiver) = RuntimeGateway::channel(1);
        let first_client = client.clone();
        let first = tokio::spawn(async move {
            first_client
                .request(RuntimeRequest::DescribeInstance, CancellationToken::new())
                .await
        });
        tokio::task::yield_now().await;
        let second = tokio::spawn(async move {
            client
                .request(RuntimeRequest::DescribeInstance, CancellationToken::new())
                .await
        });
        tokio::task::yield_now().await;
        assert!(
            !second.is_finished(),
            "a full bounded queue must apply backpressure"
        );

        let first_command = receiver.recv().await.expect("first command");
        first_command
            .reply
            .send(Ok(RuntimeReply::Instance(expected.clone())))
            .expect("first reply receiver");
        let second_command = receiver.recv().await.expect("second command was retained");
        second_command
            .reply
            .send(Ok(RuntimeReply::Instance(expected.clone())))
            .expect("second reply receiver");

        assert_eq!(
            first
                .await
                .expect("first task")
                .expect("first reply")
                .instance(),
            &expected
        );
        assert_eq!(
            second
                .await
                .expect("second task")
                .expect("second reply")
                .instance(),
            &expected
        );
    }

    #[tokio::test]
    async fn cancellation_interrupts_backpressure_before_capacity_is_available() {
        let (client, mut receiver) = RuntimeGateway::channel(1);
        let admitted_client = client.clone();
        let admitted = tokio::spawn(async move {
            admitted_client
                .request(RuntimeRequest::DescribeInstance, CancellationToken::new())
                .await
        });
        tokio::task::yield_now().await;

        let cancelled = CancellationToken::new();
        let request_cancelled = cancelled.clone();
        let waiting = tokio::spawn(async move {
            client
                .request(RuntimeRequest::DescribeInstance, request_cancelled)
                .await
        });
        tokio::task::yield_now().await;
        assert!(
            !waiting.is_finished(),
            "second request must be waiting for capacity"
        );
        cancelled.cancel();

        let error = waiting
            .await
            .expect("waiting task")
            .expect_err("cancelled request");
        assert_eq!(error.code, ControlErrorCode::InstanceUnavailable);
        assert!(error.message.contains("cancel"));

        let command = receiver.recv().await.expect("only admitted command");
        command
            .reply
            .send(Err(ControlError::instance_unavailable("test shutdown")))
            .expect("admitted request still waits");
        admitted
            .await
            .expect("admitted task")
            .expect_err("test shutdown reply");
        assert!(
            receiver.try_recv().is_err(),
            "cancelled request was never enqueued"
        );
    }

    #[tokio::test]
    async fn cancellation_interrupts_waiting_for_a_runtime_reply() {
        let (client, mut receiver) = RuntimeGateway::channel(1);
        let cancelled = CancellationToken::new();
        let request_cancelled = cancelled.clone();
        let waiting = tokio::spawn(async move {
            client
                .request(RuntimeRequest::DescribeInstance, request_cancelled)
                .await
        });
        let command = receiver.recv().await.expect("admitted command");

        cancelled.cancel();
        let error = waiting
            .await
            .expect("waiting task")
            .expect_err("cancelled reply wait");

        assert_eq!(error.code, ControlErrorCode::InstanceUnavailable);
        assert!(
            command
                .reply
                .send(Ok(RuntimeReply::Instance(snapshot(
                    &InstanceIdentity::new("127.0.0.1:19008".parse().expect("endpoint"))
                        .expect("identity")
                ))))
                .is_err(),
            "cancelled client must drop its reply receiver"
        );
    }

    #[tokio::test]
    async fn closing_ingress_rejects_new_commands_but_preserves_an_admitted_reply() {
        let identity =
            InstanceIdentity::new("127.0.0.1:19009".parse().expect("endpoint")).expect("identity");
        let expected = snapshot(&identity);
        let (client, mut receiver) = RuntimeGateway::channel(1);
        let admitted_client = client.clone();
        let admitted = tokio::spawn(async move {
            admitted_client
                .request(RuntimeRequest::DescribeInstance, CancellationToken::new())
                .await
        });
        let command = receiver.recv().await.expect("admitted before close");
        receiver.close();

        let error = client
            .request(RuntimeRequest::DescribeInstance, CancellationToken::new())
            .await
            .expect_err("ordinary ingress is closed");
        assert_eq!(error.code, ControlErrorCode::InstanceUnavailable);
        command
            .reply
            .send(Ok(RuntimeReply::Instance(expected.clone())))
            .expect("admitted reply remains deliverable");
        assert_eq!(
            admitted
                .await
                .expect("admitted task")
                .expect("admitted reply")
                .instance(),
            &expected
        );
    }

    #[tokio::test]
    async fn dropped_runtime_reply_sender_is_reported_as_instance_unavailable() {
        let (client, mut receiver) = RuntimeGateway::channel(1);
        let waiting = tokio::spawn(async move {
            client
                .request(RuntimeRequest::DescribeInstance, CancellationToken::new())
                .await
        });
        let command = receiver.recv().await.expect("admitted command");
        drop(command.reply);

        let error = waiting
            .await
            .expect("waiting task")
            .expect_err("runtime stopped before replying");
        assert_eq!(error.code, ControlErrorCode::InstanceUnavailable);
    }

    #[tokio::test]
    async fn describe_handler_routes_through_the_gateway_and_returns_authoritative_scope() {
        let identity =
            InstanceIdentity::new("127.0.0.1:19010".parse().expect("endpoint")).expect("identity");
        let expected = snapshot(&identity);
        let (client, mut receiver) = RuntimeGateway::channel(1);
        let handler = RuntimeControlHandler::new(client);
        let handle_task = tokio::spawn(async move {
            handler
                .handle(
                    context(),
                    ControlOperation::DescribeInstance,
                    CancellationToken::new(),
                )
                .await
        });

        let command = receiver.recv().await.expect("runtime command");
        assert_eq!(command.request, RuntimeRequest::DescribeInstance);
        command
            .reply
            .send(Ok(RuntimeReply::Instance(expected.clone())))
            .expect("handler awaits reply");

        let result = handle_task
            .await
            .expect("handler task")
            .expect("describe result");
        assert_eq!(
            result,
            ControlResult::DescribeInstance {
                instance: expected.instance,
                config_mode: expected.config_mode,
                persistence: expected.persistence,
                recording_enabled: expected.recording_enabled,
                retained_capture_count: expected.retained_capture_count,
                settings_revision: expected.settings_revision,
            }
        );
    }

    #[tokio::test]
    async fn handler_disconnect_cancellation_reaches_the_admitted_runtime_command() {
        let (client, mut receiver) = RuntimeGateway::channel(1);
        let handler = RuntimeControlHandler::new(client);
        let cancelled = CancellationToken::new();
        let request_cancelled = cancelled.clone();
        let handle_task = tokio::spawn(async move {
            handler
                .handle(
                    context(),
                    ControlOperation::DescribeInstance,
                    request_cancelled,
                )
                .await
        });
        let command = receiver.recv().await.expect("admitted runtime command");

        cancelled.cancel();
        command.cancelled.cancelled().await;
        let error = handle_task
            .await
            .expect("handler task")
            .expect_err("disconnected handler");

        assert_eq!(error.code, ControlErrorCode::InstanceUnavailable);
        assert!(
            command
                .reply
                .send(Err(ControlError::instance_unavailable(
                    "late runtime reply"
                )))
                .is_err()
        );
    }

    #[tokio::test]
    async fn rpc_response_preserves_request_id_and_authoritative_runtime_scope() {
        use crate::control_rpc::{
            server::ControlRpcServer,
            test_support::{read_payload, request_json, write_payload},
        };
        use serde_json::Value;
        use tokio::net::UnixStream;

        let identity =
            InstanceIdentity::new("127.0.0.1:19015".parse().expect("endpoint")).expect("identity");
        let expected = snapshot(&identity);
        let (client, mut receiver) = RuntimeGateway::channel(1);
        let server = ControlRpcServer::new(identity.clone(), RuntimeControlHandler::new(client));
        let (server_stream, mut client_stream) = UnixStream::pair().expect("Unix stream pair");
        let server_task = tokio::spawn(async move { server.serve_connection(server_stream).await });

        write_payload(
            &mut client_stream,
            &request_json(identity.run_id().as_str(), 1_000),
        )
        .await;
        let command = receiver.recv().await.expect("runtime command");
        command
            .reply
            .send(Ok(RuntimeReply::Instance(expected.clone())))
            .expect("server awaits runtime reply");
        let response: Value =
            serde_json::from_slice(&read_payload(&mut client_stream).await).expect("response JSON");

        assert_eq!(response["request_id"], "request-1");
        assert_eq!(
            response["result"]["instance"]["proxy_endpoint"],
            identity.proxy_endpoint().to_string()
        );
        assert_eq!(
            response["result"]["instance"]["run_id"],
            identity.run_id().as_str()
        );
        assert_eq!(response["result"]["config_mode"], "temporary");
        assert_eq!(response["result"]["persistence"], "ephemeral");
        assert_eq!(response["result"]["recording_enabled"], false);
        assert_eq!(response["result"]["retained_capture_count"], 0);
        assert_eq!(response["result"]["settings_revision"], 0);
        server_task
            .await
            .expect("server task")
            .expect("serve describe request");
    }
}
