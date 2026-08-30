use std::{
    collections::HashSet,
    fmt,
    future::Future,
    io,
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{Result, anyhow};
use futures::future::BoxFuture;
use tokio::{
    net::UnixListener,
    sync::{OwnedSemaphorePermit, Semaphore, mpsc, oneshot},
    task::{JoinHandle, JoinSet},
    time,
};
use tokio_util::sync::CancellationToken;

use crate::{
    capture::{
        CaptureChange, CaptureChangeError, CaptureChangeFeed, CaptureChangeKind, CaptureSequence,
        CaptureSnapshot,
    },
    control::{
        CaptureMilestone, InstanceRuntimeSnapshot, RuntimeReply, RuntimeRequest,
        WaitForCaptureRequest, WaitForCaptureResult,
        capture_query::{
            CAPTURE_SEARCH_BATCH_SIZE, CAPTURE_SEARCH_PAGE_JSON_BUDGET, CaptureDetail,
            CaptureQuery, CaptureSearchCursor, CaptureSearchPage, CompactCapture,
            CompiledCaptureQuery, match_capture_page_with_budget, normalize_capture_page_limit,
        },
        normalize_wait_timeout_ms,
    },
    control_rpc::{
        client::ControlRpcClient,
        protocol::{
            ControlError, ControlErrorCode, ControlOperation, ControlResult, DeclaredClient,
            InstanceScope,
        },
        server::{ControlCallContext, ControlRpcHandler, ControlRpcServer},
    },
    instance::{InstanceIdentity, RunId},
    instance_registry::{InstanceDescriptor, RegistryPublisher, RegistryScanner},
    settings::SettingsSession,
};

pub(super) struct RuntimeCommand {
    pub(super) request: RuntimeRequest,
    pub(super) cancelled: CancellationToken,
    pub(super) reply: oneshot::Sender<Result<RuntimeReply, ControlError>>,
}

#[derive(Clone)]
pub(super) struct RuntimeControlClient {
    commands: mpsc::Sender<RuntimeCommand>,
}

pub(super) struct RuntimeControlReceiver {
    commands: mpsc::Receiver<RuntimeCommand>,
}

pub(super) struct RuntimeGateway;

impl RuntimeGateway {
    pub(super) fn new(capacity: usize) -> (RuntimeControlClient, RuntimeControlReceiver) {
        let (commands, receiver) = mpsc::channel(capacity);
        (
            RuntimeControlClient { commands },
            RuntimeControlReceiver { commands: receiver },
        )
    }
}

impl RuntimeControlClient {
    pub(super) async fn request(
        &self,
        request: RuntimeRequest,
        cancelled: CancellationToken,
    ) -> Result<RuntimeReply, ControlError> {
        let permit = tokio::select! {
            permit = self.commands.reserve() => permit.map_err(|_| {
                ControlError::instance_unavailable("runtime command gateway is closed")
            })?,
            _ = cancelled.cancelled() => {
                return Err(ControlError::instance_unavailable(
                    "runtime command was cancelled before admission",
                ));
            }
        };
        let (reply, response) = oneshot::channel();
        permit.send(RuntimeCommand {
            request,
            cancelled: cancelled.clone(),
            reply,
        });
        tokio::select! {
            result = response => result.unwrap_or_else(|_| {
                Err(ControlError::instance_unavailable(
                    "runtime stopped before replying",
                ))
            }),
            _ = cancelled.cancelled() => Err(ControlError::instance_unavailable(
                "runtime command was cancelled",
            )),
        }
    }
}

impl RuntimeControlReceiver {
    pub(super) async fn recv(&mut self) -> Option<RuntimeCommand> {
        self.commands.recv().await
    }

    pub(super) fn try_recv(
        &mut self,
    ) -> std::result::Result<RuntimeCommand, mpsc::error::TryRecvError> {
        self.commands.try_recv()
    }

    pub(super) fn close(&mut self) {
        self.commands.close();
    }

    pub(super) fn is_closed(&self) -> bool {
        self.commands.is_closed()
    }
}

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

struct WaitBatchInspection {
    capture: Option<CompactCapture>,
    pending: Vec<CaptureSequence>,
}

enum WaitSnapshotInspection {
    Matched(CompactCapture),
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
        return Ok(WaitSnapshotInspection::Matched(
            CompactCapture::from_snapshot(snapshot),
        ));
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
) -> Result<WaitBatchInspection, ControlError> {
    let mut pending = Vec::new();
    for snapshot in snapshots {
        match inspect_wait_snapshot(snapshot, query, milestone, cancelled)? {
            WaitSnapshotInspection::Matched(capture) => {
                return Ok(WaitBatchInspection {
                    capture: Some(capture),
                    pending,
                });
            }
            WaitSnapshotInspection::Pending => pending.push(snapshot.sequence),
            WaitSnapshotInspection::Ignore => {}
        }
    }
    Ok(WaitBatchInspection {
        capture: None,
        pending,
    })
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

fn matched_wait(instance: InstanceScope, capture: CompactCapture) -> ControlResult {
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

#[derive(Clone)]
pub(super) struct ControlServiceContext {
    pub(super) runtime: RuntimeControlClient,
    pub(super) capture_changes: CaptureChangeFeed,
}

#[cfg(test)]
impl From<RuntimeControlClient> for ControlServiceContext {
    fn from(runtime: RuntimeControlClient) -> Self {
        Self {
            runtime,
            capture_changes: CaptureChangeFeed::new(),
        }
    }
}

#[derive(Clone)]
pub(super) struct RuntimeControlHandler {
    runtime: RuntimeControlClient,
    capture_changes: CaptureChangeFeed,
    capture_searches: Arc<CaptureSearchAdmission>,
    capture_details: Arc<DetailMaterializationAdmission>,
}

impl RuntimeControlHandler {
    pub(super) fn new(context: impl Into<ControlServiceContext>) -> Self {
        let context = context.into();
        Self {
            runtime: context.runtime,
            capture_changes: context.capture_changes,
            capture_searches: Arc::new(CaptureSearchAdmission::new(4)),
            capture_details: Arc::new(DetailMaterializationAdmission::new(4)),
        }
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
        let mut changes = capture_changes.subscribe();
        let active = admission.admit(query, cancelled.clone()).await?;
        let query = active.compiled_query();
        drop(active);
        let instance = Self::instance_snapshot(&runtime, &cancelled)
            .await?
            .instance;
        let deadline = tokio::time::Instant::now() + Duration::from_millis(timeout_ms);
        let mut pending = HashSet::new();
        let mut cursor = None;

        loop {
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
                break;
            }
            let next_cursor = batch.next_cursor;
            let admitted = admission.admit_compiled(Arc::clone(&query), &cancelled);
            let Some(mut active) = wait_step(admitted, deadline, &cancelled).await? else {
                return Ok(unmatched_wait(instance));
            };
            let worker_cancelled = cancelled.clone();
            let inspection = active.run_blocking(cancelled.clone(), move |query| {
                inspect_wait_batch(&batch.snapshots, &query, milestone, &worker_cancelled)
            });
            let Some(inspection) = wait_step(inspection, deadline, &cancelled).await? else {
                return Ok(unmatched_wait(instance));
            };
            drop(active);
            pending.extend(inspection.pending);
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
                    if pending.remove(&change.sequence) {
                        return Err(capture_change_lost(
                            change,
                            "matching capture was removed while waiting",
                        ));
                    }
                }
                CaptureChangeKind::Admitted | CaptureChangeKind::RecordUpdated => {
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
                    match inspection {
                        WaitSnapshotInspection::Matched(capture) => {
                            return Ok(matched_wait(instance, capture));
                        }
                        WaitSnapshotInspection::Pending => {
                            pending.insert(change.sequence);
                        }
                        WaitSnapshotInspection::Ignore => {
                            pending.remove(&change.sequence);
                        }
                    }
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

    pub(super) fn descriptor_path(&self) -> &std::path::Path {
        self.publisher.descriptor_path()
    }

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
        if let Some(mut task) = self.task.take() {
            if time::timeout(grace, &mut task).await.is_err() {
                task.abort();
                let _ = task.await;
            }
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
        let (client, receiver) = RuntimeGateway::new(64);
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
        let (client, mut receiver) = RuntimeGateway::new(1);
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
        let (client, mut receiver) = RuntimeGateway::new(1);
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
        let (client, mut receiver) = RuntimeGateway::new(1);
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
        let (client, mut receiver) = RuntimeGateway::new(1);
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
        let (client, mut receiver) = RuntimeGateway::new(1);
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
        let (client, mut receiver) = RuntimeGateway::new(1);
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
        let (client, mut receiver) = RuntimeGateway::new(1);
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
        let (client, mut receiver) = RuntimeGateway::new(1);
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
        let (client, mut receiver) = RuntimeGateway::new(1);
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
