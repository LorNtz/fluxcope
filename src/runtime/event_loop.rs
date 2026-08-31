use std::{io, sync::Arc, time::Instant};

use anyhow::{Context, Result, anyhow};
use crossterm::{
    event::{
        DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
        Event, EventStream, KeyCode, KeyModifiers,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use futures::StreamExt;
use tokio::{
    sync::{mpsc, watch},
    time,
};
use tokio_util::sync::CancellationToken;

use super::{
    gateway::{RuntimeCommand, RuntimeControlReceiver},
    policy::RenderPolicy,
    services::{ServiceKind, ServiceSupervisor},
    settings::{
        SettingsRevision, SettingsTransactionClient, SettingsTransactionOrigin,
        SettingsTransactionToken,
    },
};
#[cfg(all(test, unix))]
use crate::settings::SettingsSession;
use crate::{
    app::{App, BodyDisplayPreparation},
    capture::{
        CaptureDirtySignal, CaptureMetrics, CaptureMetricsSnapshot, CaptureRecord, DecodeMetrics,
        DecodeMetricsSnapshot, DecodeResult,
    },
    logging::{LogRecord, LoggingMetrics, LoggingMetricsSnapshot, LoggingStatus},
    request_policy::RequestPolicyStore,
    request_search::{RequestSearchClient, RequestSearchDispatch, SearchJobOutcome},
    settings::{AppSettings, SettingsUiContext},
    ui::RootView,
};
#[cfg(unix)]
use crate::{
    capture::{BodySide, CaptureSequence, CaptureSnapshotMode, CapturedHeaders},
    control::{
        AppControlSummary, CaptureSnapshotReply, InstanceRuntimeSnapshot, RecordingUpdate,
        body::{CaptureBodyMetadataReply, CaptureBodySnapshotReply},
        capture_query::{CAPTURE_SEARCH_BATCH_SIZE, CaptureSearchBatch, cursor_before},
    },
    control_rpc::protocol::InstanceScope,
    instance::InstanceIdentity,
    instance_registry::RegistryPublisher,
};
use crate::{
    control::{
        RuntimeReply, RuntimeRequest,
        settings::{BeginSettingsTransactionReply, MappingSettingsSnapshot},
    },
    control_rpc::protocol::{ControlError, ControlErrorCode},
};
type PlatformControlReceiver = RuntimeControlReceiver;

enum PlatformControlEvent {
    Command(RuntimeCommand),
    Closed,
}

pub(super) struct AppRuntime {
    app: App,
    ui: RootView,
    capture_rx: mpsc::Receiver<Arc<CaptureRecord>>,
    log_rx: mpsc::Receiver<LogRecord>,
    logging_status_rx: mpsc::Receiver<LoggingStatus>,
    logging_metrics: Arc<LoggingMetrics>,
    last_logging_metrics: LoggingMetricsSnapshot,
    capture_dirty: Arc<CaptureDirtySignal>,
    capture_metrics: Arc<CaptureMetrics>,
    last_capture_metrics: CaptureMetricsSnapshot,
    last_capture_pressure: u64,
    decode_rx: mpsc::Receiver<DecodeResult>,
    decode_metrics: Arc<DecodeMetrics>,
    last_decode_metrics: DecodeMetricsSnapshot,
    request_search: RequestSearchClient,
    request_search_results: watch::Receiver<Option<Arc<SearchJobOutcome>>>,
    tui: Tui,
    settings: Arc<AppSettings>,
    settings_context: SettingsUiContext,
    settings_revision: SettingsRevision,
    pending_settings_transaction: Option<(SettingsTransactionToken, SettingsTransactionOrigin)>,
    next_settings_transaction_token: u64,
    settings_transactions: SettingsTransactionClient,
    settings_completion_rx: mpsc::Receiver<
        std::result::Result<super::settings::SettingsTransactionResult, ControlError>,
    >,
    settings_completion_tx:
        mpsc::Sender<std::result::Result<super::settings::SettingsTransactionResult, ControlError>>,
    request_policy_store: RequestPolicyStore,
    policy: RenderPolicy,
    services: ServiceSupervisor,
    shutdown: CancellationToken,
    control_rx: Option<PlatformControlReceiver>,
    #[cfg(unix)]
    identity: Option<InstanceIdentity>,
    #[cfg(unix)]
    control_publisher: Option<RegistryPublisher>,
}

impl AppRuntime {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        app: App,
        capture_rx: mpsc::Receiver<Arc<CaptureRecord>>,
        log_rx: mpsc::Receiver<LogRecord>,
        logging_status_rx: mpsc::Receiver<LoggingStatus>,
        logging_metrics: Arc<LoggingMetrics>,
        capture_dirty: Arc<CaptureDirtySignal>,
        capture_metrics: Arc<CaptureMetrics>,
        decode_rx: mpsc::Receiver<DecodeResult>,
        decode_metrics: Arc<DecodeMetrics>,
        request_search: RequestSearchClient,
        request_search_results: watch::Receiver<Option<Arc<SearchJobOutcome>>>,
        tui: Tui,
        settings: Arc<AppSettings>,
        settings_context: SettingsUiContext,
        settings_transactions: SettingsTransactionClient,
        control_rx: RuntimeControlReceiver,
        request_policy_store: RequestPolicyStore,
        policy: RenderPolicy,
        services: ServiceSupervisor,
        shutdown: CancellationToken,
    ) -> Self {
        let (settings_completion_tx, settings_completion_rx) = mpsc::channel(1);
        Self {
            app,
            ui: RootView::new(),
            capture_rx,
            log_rx,
            logging_status_rx,
            logging_metrics,
            last_logging_metrics: LoggingMetricsSnapshot::default(),
            capture_dirty,
            capture_metrics,
            last_capture_metrics: CaptureMetricsSnapshot::default(),
            last_capture_pressure: 0,
            decode_rx,
            decode_metrics,
            last_decode_metrics: DecodeMetricsSnapshot::default(),
            request_search,
            request_search_results,
            tui,
            settings,
            settings_context,
            settings_revision: SettingsRevision::INITIAL,
            pending_settings_transaction: None,
            next_settings_transaction_token: 1,
            settings_transactions,
            settings_completion_rx,
            settings_completion_tx,
            request_policy_store,
            policy,
            services,
            shutdown,
            control_rx: Some(control_rx),
            #[cfg(unix)]
            identity: None,
            #[cfg(unix)]
            control_publisher: None,
        }
    }

    #[cfg(unix)]
    pub(super) fn with_control(
        mut self,
        identity: InstanceIdentity,
        control_publisher: Option<RegistryPublisher>,
    ) -> Self {
        self.identity = Some(identity);
        self.control_publisher = control_publisher;
        self
    }

    #[cfg(all(test, unix))]
    fn test_with_control(
        identity: InstanceIdentity,
        app: App,
        settings: SettingsSession,
        control_rx: RuntimeControlReceiver,
    ) -> ControlExecutionHarness {
        ControlExecutionHarness {
            identity,
            app,
            settings,
            control_rx,
        }
    }

    pub async fn run(mut self) -> Result<()> {
        let result = self.run_loop().await;
        self.shutdown.cancel();
        self.drain_pending_settings_transaction().await;
        self.close_control_ingress();
        self.services.shutdown(self.policy.shutdown_grace).await;
        #[cfg(unix)]
        self.control_publisher.take();
        result
    }

    async fn run_loop(&mut self) -> Result<()> {
        let mut terminal_events = EventStream::new();
        let mut frame_tick = time::interval(self.policy.frame_interval);
        frame_tick.set_missed_tick_behavior(time::MissedTickBehavior::Skip);
        let mut metrics_tick = time::interval(self.policy.metrics_interval);
        metrics_tick.set_missed_tick_behavior(time::MissedTickBehavior::Skip);
        let mut live_body_tick = time::interval(self.policy.live_body_interval);
        live_body_tick.set_missed_tick_behavior(time::MissedTickBehavior::Skip);
        self.tui
            .draw(&mut self.ui, &mut self.app)
            .context("failed to draw initial terminal frame")?;
        let mut dirty = false;
        let mut captures_open = true;
        let mut logs_open = true;
        let mut logging_status_open = true;
        let mut decode_results_open = true;
        let mut request_search_results_open = true;

        loop {
            tokio::select! {
                event = terminal_events.next() => {
                    let Some(event) = event else {
                        return Err(anyhow!("terminal event stream closed"));
                    };
                    let event = event.context("failed to read terminal event")?;
                    if self.handle_terminal_event(event) {
                        return Ok(());
                    }
                    dirty = true;
                    dirty |= self.handle_settings_save_request();
                }
                completion = self.settings_completion_rx.recv() => {
                    if let Some(completion) = completion {
                        match completion {
                            Ok(result) => {
                                log::info!(
                                    "TUI settings transaction {:?} at revision {} ({:?})",
                                    result.outcome,
                                    result.revision,
                                    result.affected
                                );
                                self.app.finish_settings_save(
                                    Arc::clone(&result.settings),
                                    result.revision,
                                );
                            }
                            Err(error) => self.app.fail_settings_save(error.message().to_string()),
                        }
                        dirty = true;
                    }
                }
                control_event = receive_control_event(&mut self.control_rx) => {
                    match control_event {
                        PlatformControlEvent::Command(command) => {
                            dirty |= self.process_control_command(command);
                        }
                        PlatformControlEvent::Closed => self.control_rx = None,
                    }
                }
                capture = self.capture_rx.recv(), if captures_open => {
                    match capture {
                        Some(capture) => {
                            self.app.add_capture(capture);
                            self.drain_runtime_events();
                            dirty = true;
                        }
                        None => captures_open = false,
                    }
                }
                _ = self.capture_dirty.notified() => {
                    let pressure = self.capture_metrics.snapshot().memory_pressure;
                    if pressure > self.last_capture_pressure {
                        self.last_capture_pressure = pressure;
                        self.app.evict_oldest_capture();
                    }
                    dirty = true;
                }
                result = self.decode_rx.recv(), if decode_results_open => {
                    match result {
                        Some(result) => dirty |= self.app.apply_decode_result(result),
                        None => decode_results_open = false,
                    }
                }
                changed = self.request_search_results.changed(), if request_search_results_open => {
                    match changed {
                        Ok(()) => {
                            let outcome = self.request_search_results.borrow_and_update().clone();
                            if let Some(outcome) = outcome {
                                dirty |= self.app.apply_request_search_outcome(&outcome);
                            }
                        }
                        Err(_) => request_search_results_open = false,
                    }
                }
                record = self.log_rx.recv(), if logs_open => {
                    match record {
                        Some(record) => {
                            self.app.append_log(record);
                            self.drain_runtime_events();
                            dirty = true;
                        }
                        None => logs_open = false,
                    }
                }
                status = self.logging_status_rx.recv(), if logging_status_open => {
                    match status {
                        Some(LoggingStatus::Degraded(message)) => {
                            self.app.append_log(LogRecord::system(format!("ERROR - [fluxcope::logging] {message}")));
                            dirty = true;
                        }
                        None => logging_status_open = false,
                    }
                }
                completion = self.services.join_next() => {
                    let Some(completion) = completion else {
                        return Err(anyhow!("all runtime services exited unexpectedly"));
                    };
                    let completion = completion?;
                    if is_fatal_service(completion.kind) {
                        completion.result?;
                        return Err(anyhow!(
                            "{:?} service exited unexpectedly",
                            completion.kind
                        ));
                    }
                    match completion.kind {
                        ServiceKind::CertificateDownload => {
                            let message = match completion.result {
                                Ok(()) => "certificate download service stopped".to_string(),
                                Err(error) => format!("certificate download service disabled: {error:#}"),
                            };
                            self.app.append_log(LogRecord::system(format!("WARN - [fluxcope::runtime] {message}")));
                            dirty = true;
                        }
                        ServiceKind::Logger => {
                            let message = match completion.result {
                                Ok(()) => "logging service stopped".to_string(),
                                Err(error) => format!("logging service failed: {error:#}"),
                            };
                            self.app.append_log(LogRecord::system(format!("ERROR - [fluxcope::runtime] {message}")));
                            dirty = true;
                        }
                        ServiceKind::Proxy
                        | ServiceKind::ControlRpc
                        | ServiceKind::BodyPumps
                        | ServiceKind::Decoder
                        | ServiceKind::RequestSearch
                        | ServiceKind::SettingsTransactions => {
                            return Err(anyhow!("fatal service classification was inconsistent"));
                        }
                    }
                }
                _ = metrics_tick.tick() => {
                    dirty |= self.refresh_metrics();
                }
                _ = live_body_tick.tick() => {
                    dirty |= self.app.refresh_selected_live_body();
                }
                _ = frame_tick.tick() => {
                    if dirty {
                        let now = Instant::now();
                        let body = self.app.prepare_current_body_display(now);
                        render_dirty_frame(
                            &mut dirty,
                            body,
                            now,
                            self.policy.body_loading_grace,
                            || {
                                self.tui
                                    .draw(&mut self.ui, &mut self.app)
                                    .context("failed to draw terminal frame")
                            },
                        )?;
                    }
                }
            }

            if dirty {
                let _ = self.app.prepare_current_body_display(Instant::now());
            }
            self.dispatch_pending_request_search();
        }
    }

    fn handle_terminal_event(&mut self, event: Event) -> bool {
        match event {
            Event::Key(key) if is_runtime_quit_key(key) => true,
            Event::Key(key) => self.app.handle_key_event(key),
            Event::Mouse(mouse) => {
                self.ui.handle_mouse(mouse, &mut self.app);
                false
            }
            Event::Paste(pasted) => {
                self.app.handle_request_search_paste(&pasted);
                false
            }
            Event::Resize(_, _) | Event::FocusGained | Event::FocusLost => false,
        }
    }

    fn dispatch_pending_request_search(&mut self) {
        match self.app.take_request_search_dispatch() {
            Some(RequestSearchDispatch::Run(request)) => self.request_search.submit(request),
            Some(RequestSearchDispatch::Cancel) => self.request_search.cancel(),
            None => {}
        }
    }

    fn drain_runtime_events(&mut self) {
        let started = Instant::now();
        for _ in 1..self.policy.max_events_per_turn {
            if started.elapsed() >= self.policy.event_budget {
                break;
            }

            if let Ok(capture) = self.capture_rx.try_recv() {
                self.app.add_capture(capture);
                continue;
            }
            if let Ok(record) = self.log_rx.try_recv() {
                self.app.append_log(record);
                continue;
            }
            if let Ok(LoggingStatus::Degraded(message)) = self.logging_status_rx.try_recv() {
                self.app.append_log(LogRecord::system(format!(
                    "ERROR - [fluxcope::logging] {message}"
                )));
                continue;
            }
            break;
        }
    }

    fn handle_settings_save_request(&mut self) -> bool {
        let Some(draft) = self.app.take_settings_save_request() else {
            return false;
        };
        if self.app.settings_transaction_pending() {
            self.app.fail_settings_save(
                "settings transaction is pending; wait for it to finish".to_string(),
            );
            return true;
        }
        self.app.set_settings_transaction_pending(true);
        let client = self.settings_transactions.clone();
        let completion = self.settings_completion_tx.clone();
        let cancelled = self.shutdown.child_token();
        tokio::spawn(async move {
            let result = client.replace_from_tui(draft, cancelled).await;
            let _ = completion.send(result).await;
        });
        true
    }

    fn refresh_metrics(&mut self) -> bool {
        let logging = self.logging_metrics.snapshot();
        let capture = self.capture_metrics.snapshot();
        let decode = self.decode_metrics.snapshot();
        if logging == self.last_logging_metrics
            && capture == self.last_capture_metrics
            && decode == self.last_decode_metrics
        {
            return false;
        }
        self.last_logging_metrics = logging;
        self.last_capture_metrics = capture;
        self.last_decode_metrics = decode;
        let mut changed = false;
        if logging != LoggingMetricsSnapshot::default()
            || capture != CaptureMetricsSnapshot::default()
            || decode != DecodeMetricsSnapshot::default()
        {
            self.app.append_log(LogRecord::system(format!(
                "WARN - [fluxcope::metrics] capture_not_admitted={} capture_memory_pressure={} preview_body_limited={} preview_memory_limited={} metadata_truncated={} decode_rejected={} decode_superseded={} decode_limited={} decode_failed={} log_producer_dropped={} log_tui_dropped={} log_truncated={}",
                capture.exchanges_not_admitted,
                capture.memory_pressure,
                capture.previews_per_body_limited,
                capture.previews_memory_limited,
                capture.metadata_truncated,
                decode.rejected,
                decode.superseded,
                decode.output_limited,
                decode.failed,
                logging.producer_dropped,
                logging.tui_dropped,
                logging.records_truncated
            )));
            changed = true;
        }
        changed
    }
    fn execute_control(
        &mut self,
        request: RuntimeRequest,
    ) -> std::result::Result<RuntimeReply, ControlError> {
        match request {
            RuntimeRequest::GetMappingSettings => {
                Ok(RuntimeReply::MappingSettings(MappingSettingsSnapshot {
                    settings: Arc::clone(&self.settings),
                    revision: self.settings_revision,
                    config_mode: self.settings_context.config_mode,
                    persistence: self.settings_context.persistence,
                }))
            }
            RuntimeRequest::BeginSettingsTransaction {
                expected_revision,
                origin,
            } => {
                if let Some(expected) = expected_revision
                    && expected != self.settings_revision
                {
                    return Err(ControlError::new(
                        ControlErrorCode::SettingsRevisionConflict,
                        "settings revision does not match",
                        false,
                        serde_json::json!({
                            "expected_revision": expected,
                            "current_revision": self.settings_revision,
                        }),
                    ));
                }
                if origin == SettingsTransactionOrigin::Mcp && self.app.settings_popup.is_dirty() {
                    return Err(ControlError::new(
                        ControlErrorCode::TuiDraftConflict,
                        "mapping settings conflict with an unsaved TUI draft",
                        false,
                        serde_json::json!({"current_revision": self.settings_revision}),
                    ));
                }
                if origin == SettingsTransactionOrigin::Mcp
                    && self.app.settings_transaction_pending()
                {
                    return Err(ControlError::new(
                        ControlErrorCode::ServiceUnavailable,
                        "another settings transaction is pending",
                        true,
                        serde_json::json!({"stage": "settings_transaction_pending"}),
                    ));
                }
                if self.pending_settings_transaction.is_some() {
                    return Err(ControlError::new(
                        ControlErrorCode::ServiceUnavailable,
                        "another settings transaction is pending",
                        true,
                        serde_json::json!({"stage": "settings_transaction_pending"}),
                    ));
                }
                let token = SettingsTransactionToken::new(self.next_settings_transaction_token);
                self.next_settings_transaction_token =
                    self.next_settings_transaction_token.saturating_add(1);
                self.pending_settings_transaction = Some((token, origin));
                self.app.set_settings_transaction_pending(true);
                Ok(RuntimeReply::SettingsTransactionBegun(
                    BeginSettingsTransactionReply {
                        settings: Arc::clone(&self.settings),
                        revision: self.settings_revision,
                        token,
                        config_mode: self.settings_context.config_mode,
                        persistence: self.settings_context.persistence,
                    },
                ))
            }
            RuntimeRequest::FinalizeSettingsTransaction { token, commit } => {
                let Some((pending, origin)) = self.pending_settings_transaction else {
                    return Err(ControlError::internal(
                        "settings transaction finalization has no pending token",
                    ));
                };
                if pending != token || origin != commit.origin {
                    return Err(ControlError::internal(
                        "settings transaction finalization token does not match",
                    ));
                }
                if commit.outcome == super::settings::SettingsTransactionOutcome::Committed {
                    let policy = commit.policy.ok_or_else(|| {
                        ControlError::internal(
                            "committed settings transaction omitted compiled policy",
                        )
                    })?;
                    self.request_policy_store.replace(policy);
                    self.settings = Arc::clone(&commit.settings);
                    self.settings_revision = self.settings_revision.next();
                    self.app.apply_settings_transaction_commit_from_origin(
                        commit.settings,
                        self.settings_revision,
                        origin,
                    )?;
                } else {
                    self.app.set_settings_transaction_pending(false);
                }
                self.pending_settings_transaction = None;
                Ok(RuntimeReply::SettingsTransactionFinalized(commit.outcome))
            }
            RuntimeRequest::AbortSettingsTransaction { token } => {
                if self
                    .pending_settings_transaction
                    .is_some_and(|(pending, _)| pending == token)
                {
                    self.pending_settings_transaction = None;
                    self.app.set_settings_transaction_pending(false);
                    Ok(RuntimeReply::SettingsTransactionAborted)
                } else {
                    Err(ControlError::internal(
                        "settings transaction abort token does not match",
                    ))
                }
            }
            #[cfg(unix)]
            request => {
                let identity = self.identity.as_ref().ok_or_else(|| {
                    ControlError::instance_unavailable("runtime control is not enabled")
                })?;
                execute_control_request(identity, &mut self.app, self.settings_context, request)
            }
            #[cfg(not(unix))]
            _ => Err(ControlError::new(
                ControlErrorCode::UnsupportedPlatform,
                "private runtime control is unsupported on this platform",
                false,
                serde_json::json!({}),
            )),
        }
    }
    async fn drain_pending_settings_transaction(&mut self) {
        while self.pending_settings_transaction.is_some() {
            let Some(receiver) = self.control_rx.as_mut() else {
                break;
            };
            let Some(command) = receiver.recv().await else {
                break;
            };
            let _ = self.process_control_command(command);
        }
    }

    fn process_control_command(&mut self, command: RuntimeCommand) -> bool {
        if command.cancelled.is_cancelled() {
            return false;
        }
        let dirty = matches!(
            &command.request,
            RuntimeRequest::SetRecordingEnabled { .. }
                | RuntimeRequest::BeginSettingsTransaction { .. }
                | RuntimeRequest::FinalizeSettingsTransaction { .. }
                | RuntimeRequest::AbortSettingsTransaction { .. }
        );
        let result = self.execute_control(command.request);
        let _ = command.reply.send(result);
        dirty
    }

    fn close_control_ingress(&mut self) {
        let Some(receiver) = self.control_rx.as_mut() else {
            return;
        };
        receiver.close();
        while let Ok(command) = receiver.try_recv() {
            let _ = command.reply.send(Err(ControlError::instance_unavailable(
                "runtime command gateway is shutting down",
            )));
        }
    }
}

async fn receive_control_event(
    receiver: &mut Option<PlatformControlReceiver>,
) -> PlatformControlEvent {
    match receiver.as_mut() {
        Some(receiver) => receiver
            .recv()
            .await
            .map_or(PlatformControlEvent::Closed, PlatformControlEvent::Command),
        None => std::future::pending().await,
    }
}

#[cfg(unix)]
fn execute_control_request(
    identity: &InstanceIdentity,
    app: &mut App,
    context: SettingsUiContext,
    request: RuntimeRequest,
) -> std::result::Result<RuntimeReply, ControlError> {
    match request {
        RuntimeRequest::DescribeInstance | RuntimeRequest::GetStatus => {
            let AppControlSummary {
                recording_enabled,
                retained_capture_count,
                settings_revision,
            } = app.control_summary();
            Ok(RuntimeReply::Instance(InstanceRuntimeSnapshot {
                instance: InstanceScope {
                    proxy_endpoint: identity.proxy_endpoint(),
                    run_id: identity.run_id().clone(),
                },
                config_mode: context.config_mode,
                persistence: context.persistence,
                recording_enabled,
                retained_capture_count,
                settings_revision,
            }))
        }
        RuntimeRequest::SetRecordingEnabled { enabled } => {
            let previous = app.set_recording_enabled(enabled);
            Ok(RuntimeReply::RecordingUpdated(RecordingUpdate {
                instance: InstanceScope {
                    proxy_endpoint: identity.proxy_endpoint(),
                    run_id: identity.run_id().clone(),
                },
                previous,
                current: enabled,
            }))
        }
        RuntimeRequest::GetCaptureSearchBatch { cursor, max_rows } => {
            let max_rows = max_rows.min(CAPTURE_SEARCH_BATCH_SIZE);
            let mut snapshots = Vec::with_capacity(max_rows);
            let mut cursor = cursor
                .map(|cursor| cursor.sequence())
                .unwrap_or(CaptureSequence::new(u64::MAX));
            while snapshots.len() < max_rows {
                let Some(record) = app.capture_at_or_before(cursor) else {
                    break;
                };
                let sequence = record.sequence();
                snapshots.push(record.snapshot(CaptureSnapshotMode::MetadataOnly));
                let Some(older) = sequence.value().checked_sub(1) else {
                    break;
                };
                cursor = CaptureSequence::new(older);
            }
            let next_cursor = snapshots.last().and_then(|snapshot| {
                let cursor = cursor_before(snapshot.sequence)?;
                app.capture_at_or_before(cursor.sequence()).map(|_| cursor)
            });
            Ok(RuntimeReply::CaptureSearchBatch(CaptureSearchBatch {
                snapshots,
                next_cursor,
            }))
        }
        RuntimeRequest::GetCapture {
            capture_id,
            expected_revision,
        } => {
            let record = app.capture_record(capture_id).ok_or_else(|| {
                ControlError::new(
                    ControlErrorCode::CaptureNotFound,
                    "capture is not retained",
                    false,
                    serde_json::json!({"capture_id": capture_id}),
                )
            })?;
            let snapshot = record.snapshot(CaptureSnapshotMode::MetadataOnly);
            if let Some(expected_revision) = expected_revision
                && expected_revision != snapshot.revision
            {
                return Err(ControlError::new(
                    ControlErrorCode::CaptureRevisionConflict,
                    "capture revision changed",
                    false,
                    serde_json::json!({
                        "capture_id": capture_id,
                        "expected_revision": expected_revision,
                        "current_revision": snapshot.revision,
                    }),
                ));
            }
            Ok(RuntimeReply::CaptureSnapshot(Box::new(
                CaptureSnapshotReply {
                    instance: InstanceScope {
                        proxy_endpoint: identity.proxy_endpoint(),
                        run_id: identity.run_id().clone(),
                    },
                    snapshot,
                },
            )))
        }
        RuntimeRequest::GetCaptureBodyMetadata { capture_id, side } => {
            let record = retained_capture(app, capture_id)?;
            let snapshot = record.snapshot(CaptureSnapshotMode::MetadataOnly);
            validate_capture_revision(capture_id, None, snapshot.revision)?;
            let (body, headers) = body_snapshot_parts(&snapshot, side);
            Ok(RuntimeReply::CaptureBodyMetadata(Box::new(
                CaptureBodyMetadataReply {
                    instance: InstanceScope {
                        proxy_endpoint: identity.proxy_endpoint(),
                        run_id: identity.run_id().clone(),
                    },
                    capture_id,
                    capture_revision: snapshot.revision,
                    side,
                    status: body.status.clone(),
                    headers,
                    retained_bytes: body.status.retained_bytes,
                },
            )))
        }
        RuntimeRequest::GetCaptureBodySnapshot {
            capture_id,
            expected_revision,
            side,
        } => {
            let record = retained_capture(app, capture_id)?;
            let snapshot = record.snapshot(CaptureSnapshotMode::WithBodyPreviews);
            validate_capture_revision(capture_id, Some(expected_revision), snapshot.revision)?;
            let (body, headers) = body_snapshot_parts(&snapshot, side);
            Ok(RuntimeReply::CaptureBodySnapshot(Box::new(
                CaptureBodySnapshotReply {
                    instance: InstanceScope {
                        proxy_endpoint: identity.proxy_endpoint(),
                        run_id: identity.run_id().clone(),
                    },
                    capture_id,
                    capture_revision: snapshot.revision,
                    side,
                    status: body.status.clone(),
                    headers,
                    retained_bytes: body.status.retained_bytes,
                    preview: body.preview.clone(),
                },
            )))
        }
        #[cfg(test)]
        RuntimeRequest::UnsupportedForTest => Err(ControlError::invalid_argument(
            "unsupported runtime control operation",
        )),
        RuntimeRequest::GetMappingSettings
        | RuntimeRequest::BeginSettingsTransaction { .. }
        | RuntimeRequest::FinalizeSettingsTransaction { .. }
        | RuntimeRequest::AbortSettingsTransaction { .. } => Err(ControlError::internal(
            "settings transaction request bypassed AppRuntime authority",
        )),
    }
}

#[cfg(unix)]
fn retained_capture(
    app: &App,
    capture_id: CaptureSequence,
) -> std::result::Result<std::sync::Arc<CaptureRecord>, ControlError> {
    app.capture_record(capture_id).ok_or_else(|| {
        ControlError::new(
            ControlErrorCode::CaptureNotFound,
            "capture is not retained",
            false,
            serde_json::json!({"capture_id": capture_id}),
        )
    })
}

#[cfg(unix)]
fn validate_capture_revision(
    capture_id: CaptureSequence,
    expected_revision: Option<u64>,
    current_revision: u64,
) -> std::result::Result<(), ControlError> {
    if let Some(expected_revision) = expected_revision
        && expected_revision != current_revision
    {
        return Err(ControlError::new(
            ControlErrorCode::CaptureRevisionConflict,
            "capture revision changed",
            false,
            serde_json::json!({
                "capture_id": capture_id,
                "expected_revision": expected_revision,
                "current_revision": current_revision,
            }),
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn body_snapshot_parts(
    snapshot: &crate::capture::CaptureSnapshot,
    side: BodySide,
) -> (&crate::capture::BodySnapshot, CapturedHeaders) {
    match side {
        BodySide::Request => (&snapshot.request_body, snapshot.request.headers.clone()),
        BodySide::Response => (
            &snapshot.response_body,
            snapshot.response.as_ref().map_or_else(
                || CapturedHeaders::unbudgeted(std::sync::Arc::from([])),
                |response| response.headers.clone(),
            ),
        ),
    }
}

#[cfg(all(test, unix))]
pub(super) fn execute_control_request_for_test(
    identity: &InstanceIdentity,
    app: &mut App,
    settings: &SettingsSession,
    request: RuntimeRequest,
) -> std::result::Result<RuntimeReply, ControlError> {
    execute_control_request(identity, app, settings.ui_context(), request)
}

fn is_fatal_service(kind: ServiceKind) -> bool {
    kind.is_fatal()
}

#[cfg(all(test, unix))]
struct ControlExecutionHarness {
    identity: InstanceIdentity,
    app: App,
    settings: SettingsSession,
    control_rx: RuntimeControlReceiver,
}

#[cfg(all(test, unix))]
impl ControlExecutionHarness {
    fn execute_control(
        &mut self,
        request: RuntimeRequest,
    ) -> std::result::Result<RuntimeReply, ControlError> {
        execute_control_request(
            &self.identity,
            &mut self.app,
            self.settings.ui_context(),
            request,
        )
    }

    async fn process_next_control_command(&mut self) -> Result<bool> {
        let command = self
            .control_rx
            .recv()
            .await
            .ok_or_else(|| anyhow!("runtime command gateway closed"))?;
        if command.cancelled.is_cancelled() {
            return Ok(false);
        }
        let dirty = matches!(&command.request, RuntimeRequest::SetRecordingEnabled { .. });
        let result = self.execute_control(command.request);
        let _ = command.reply.send(result);
        Ok(dirty)
    }

    fn close_control_ingress(&mut self) {
        self.control_rx.close();
        while let Ok(command) = self.control_rx.try_recv() {
            let _ = command.reply.send(Err(ControlError::instance_unavailable(
                "runtime command gateway is shutting down",
            )));
        }
    }

    fn control_ingress_is_closed(&self) -> bool {
        self.control_rx.is_closed()
    }
}

fn is_runtime_quit_key(key: crossterm::event::KeyEvent) -> bool {
    key.modifiers == KeyModifiers::CONTROL
        && matches!(key.code, KeyCode::Char('c') | KeyCode::Char('C'))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FrameRenderOutcome {
    Deferred,
    Drawn,
}

fn render_dirty_frame<E>(
    dirty: &mut bool,
    preparation: BodyDisplayPreparation,
    now: Instant,
    loading_grace: std::time::Duration,
    draw: impl FnOnce() -> Result<(), E>,
) -> Result<FrameRenderOutcome, E> {
    if matches!(
        preparation,
        BodyDisplayPreparation::Pending { started_at }
            if now.saturating_duration_since(started_at) < loading_grace
    ) {
        return Ok(FrameRenderOutcome::Deferred);
    }

    draw()?;
    *dirty = false;
    Ok(FrameRenderOutcome::Drawn)
}

pub(super) struct Tui {
    terminal: ratatui::Terminal<ratatui::backend::CrosstermBackend<io::Stdout>>,
}

impl Tui {
    pub fn enter() -> io::Result<Self> {
        let backend = ratatui::backend::CrosstermBackend::new(io::stdout());
        let mut terminal = ratatui::Terminal::new(backend)?;
        enable_raw_mode()?;
        if let Err(error) = execute!(
            terminal.backend_mut(),
            EnterAlternateScreen,
            EnableMouseCapture,
            EnableBracketedPaste
        ) {
            let _ = execute!(
                terminal.backend_mut(),
                LeaveAlternateScreen,
                DisableMouseCapture,
                DisableBracketedPaste
            );
            let _ = disable_raw_mode();
            return Err(error);
        }
        Ok(Self { terminal })
    }

    fn draw(&mut self, ui: &mut RootView, app: &mut App) -> io::Result<()> {
        self.terminal
            .draw(|frame| ui.render(frame, app))
            .map(|_| ())
    }
}

impl Drop for Tui {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(
            self.terminal.backend_mut(),
            LeaveAlternateScreen,
            DisableMouseCapture,
            DisableBracketedPaste
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_policy_bounds_event_work() {
        let policy = RenderPolicy::default();

        assert_eq!(policy.max_events_per_turn, 256);
        assert_eq!(policy.event_budget, std::time::Duration::from_millis(2));
        assert!(policy.frame_interval >= std::time::Duration::from_millis(16));
        assert_eq!(
            policy.body_loading_grace,
            std::time::Duration::from_millis(100)
        );
    }

    #[test]
    fn fast_body_result_draws_ready_frame_without_intermediate_loading_draw() {
        let started_at = Instant::now();
        let grace = std::time::Duration::from_millis(100);
        let before_grace = started_at + std::time::Duration::from_millis(99);
        let mut dirty = true;
        let mut draws = 0;

        let deferred = render_dirty_frame(
            &mut dirty,
            BodyDisplayPreparation::Pending { started_at },
            before_grace,
            grace,
            || {
                draws += 1;
                Ok::<(), ()>(())
            },
        )
        .expect("deferral should not fail");
        assert_eq!(deferred, FrameRenderOutcome::Deferred);
        assert!(dirty);
        assert_eq!(draws, 0);

        let drawn = render_dirty_frame(
            &mut dirty,
            BodyDisplayPreparation::ReadyToRender,
            before_grace,
            grace,
            || {
                draws += 1;
                Ok::<(), ()>(())
            },
        )
        .expect("ready draw should not fail");
        assert_eq!(drawn, FrameRenderOutcome::Drawn);
        assert!(!dirty);
        assert_eq!(draws, 1);
    }

    #[test]
    fn slow_body_draws_loading_once_grace_expires() {
        let started_at = Instant::now();
        let grace = std::time::Duration::from_millis(100);
        let mut dirty = true;
        let mut draws = 0;

        let outcome = render_dirty_frame(
            &mut dirty,
            BodyDisplayPreparation::Pending { started_at },
            started_at + grace,
            grace,
            || {
                draws += 1;
                Ok::<(), ()>(())
            },
        )
        .expect("loading draw should not fail");

        assert_eq!(outcome, FrameRenderOutcome::Drawn);
        assert!(!dirty);
        assert_eq!(draws, 1);
    }

    #[test]
    fn ctrl_c_remains_the_runtime_quit_chord() {
        use crossterm::event::{KeyEvent, KeyEventKind, KeyEventState};

        for character in ['c', 'C'] {
            assert!(is_runtime_quit_key(KeyEvent {
                code: KeyCode::Char(character),
                modifiers: KeyModifiers::CONTROL,
                kind: KeyEventKind::Press,
                state: KeyEventState::NONE,
            }));
        }
        assert!(!is_runtime_quit_key(KeyEvent::new(
            KeyCode::Char('c'),
            KeyModifiers::NONE,
        )));
    }

    #[cfg(unix)]
    #[test]
    fn runtime_describe_returns_authoritative_identity_and_live_owned_state() {
        use crate::{
            app::App,
            control::{RuntimeReply, RuntimeRequest},
            instance::InstanceIdentity,
            recording::RecordingState,
            settings::{AppSettings, ConfigMode, PersistenceMode, SettingsSession, UiSettings},
        };

        let identity =
            InstanceIdentity::new("127.0.0.1:19011".parse().expect("endpoint")).expect("identity");
        let settings = SettingsSession::temporary(AppSettings::default());
        let app = App::with_recording(UiSettings::default(), RecordingState::new(true));
        let (_client, control_rx) = super::super::gateway::RuntimeGateway::channel(4);
        let mut runtime =
            AppRuntime::test_with_control(identity.clone(), app, settings, control_rx);

        let reply = runtime
            .execute_control(RuntimeRequest::DescribeInstance)
            .expect("describe runtime");
        let RuntimeReply::Instance(snapshot) = reply else {
            panic!("expected instance runtime reply");
        };

        assert_eq!(snapshot.instance.proxy_endpoint, identity.proxy_endpoint());
        assert_eq!(snapshot.instance.run_id, *identity.run_id());
        assert_eq!(snapshot.config_mode, ConfigMode::Temporary);
        assert_eq!(snapshot.persistence, PersistenceMode::Ephemeral);
        assert!(snapshot.recording_enabled);
        assert_eq!(snapshot.retained_capture_count, 0);
        assert_eq!(
            snapshot.settings_revision,
            super::super::settings::SettingsRevision::INITIAL.get()
        );
    }

    #[cfg(unix)]
    #[test]
    fn unsupported_runtime_operation_is_invalid_argument() {
        use crate::{
            app::App,
            control::RuntimeRequest,
            control_rpc::protocol::ControlErrorCode,
            instance::InstanceIdentity,
            recording::RecordingState,
            settings::{AppSettings, SettingsSession, UiSettings},
        };

        let identity =
            InstanceIdentity::new("127.0.0.1:19012".parse().expect("endpoint")).expect("identity");
        let settings = SettingsSession::temporary(AppSettings::default());
        let app = App::with_recording(UiSettings::default(), RecordingState::default());
        let (_client, control_rx) = super::super::gateway::RuntimeGateway::channel(4);
        let mut runtime = AppRuntime::test_with_control(identity, app, settings, control_rx);

        let error = runtime
            .execute_control(RuntimeRequest::UnsupportedForTest)
            .expect_err("unsupported control operation");

        assert_eq!(error.code, ControlErrorCode::InvalidArgument);
        assert!(!error.retryable);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn command_channel_describe_is_processed_without_marking_the_frame_dirty() {
        use crate::{
            app::App,
            control::RuntimeRequest,
            instance::InstanceIdentity,
            recording::RecordingState,
            settings::{AppSettings, SettingsSession, UiSettings},
        };
        use tokio_util::sync::CancellationToken;

        let identity =
            InstanceIdentity::new("127.0.0.1:19013".parse().expect("endpoint")).expect("identity");
        let expected_run_id = identity.run_id().clone();
        let settings = SettingsSession::temporary(AppSettings::default());
        let app = App::with_recording(UiSettings::default(), RecordingState::default());
        let (client, control_rx) = super::super::gateway::RuntimeGateway::channel(4);
        let mut runtime = AppRuntime::test_with_control(identity, app, settings, control_rx);
        let request = tokio::spawn(async move {
            client
                .request(RuntimeRequest::DescribeInstance, CancellationToken::new())
                .await
        });

        let visible_state_changed = runtime
            .process_next_control_command()
            .await
            .expect("process command");
        let reply = request
            .await
            .expect("request task")
            .expect("describe reply");

        assert!(!visible_state_changed);
        assert_eq!(reply.instance().instance.run_id, expected_run_id);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn runtime_shutdown_closes_command_ingress_before_service_cleanup() {
        use crate::{
            app::App,
            control::RuntimeRequest,
            control_rpc::protocol::ControlErrorCode,
            instance::InstanceIdentity,
            recording::RecordingState,
            settings::{AppSettings, SettingsSession, UiSettings},
        };
        use tokio_util::sync::CancellationToken;

        let identity =
            InstanceIdentity::new("127.0.0.1:19014".parse().expect("endpoint")).expect("identity");
        let settings = SettingsSession::temporary(AppSettings::default());
        let app = App::with_recording(UiSettings::default(), RecordingState::default());
        let (client, control_rx) = super::super::gateway::RuntimeGateway::channel(4);
        let mut runtime = AppRuntime::test_with_control(identity, app, settings, control_rx);

        runtime.close_control_ingress();
        let error = client
            .request(RuntimeRequest::DescribeInstance, CancellationToken::new())
            .await
            .expect_err("runtime no longer admits ordinary commands");

        assert_eq!(error.code, ControlErrorCode::InstanceUnavailable);
        assert!(runtime.control_ingress_is_closed());
    }

    #[test]
    fn fatal_service_policy_matches_runtime_contract() {
        assert!(is_fatal_service(ServiceKind::ControlRpc));
        assert!(is_fatal_service(ServiceKind::Proxy));
        assert!(!is_fatal_service(ServiceKind::CertificateDownload));
        assert!(!is_fatal_service(ServiceKind::Logger));
    }
}
