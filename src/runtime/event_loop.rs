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

#[cfg(unix)]
use super::control::{RuntimeCommand, RuntimeControlReceiver};
use super::{
    policy::RenderPolicy,
    save_settings_draft,
    services::{ServiceKind, ServiceSupervisor},
};
use crate::{
    app::{App, BodyDisplayPreparation},
    capture::{
        CaptureDirtySignal, CaptureMetrics, CaptureMetricsSnapshot, CaptureRecord, DecodeMetrics,
        DecodeMetricsSnapshot, DecodeResult,
    },
    logging::{LogRecord, LoggingMetrics, LoggingMetricsSnapshot, LoggingStatus},
    request_policy::RequestPolicyStore,
    request_search::{RequestSearchClient, RequestSearchDispatch, SearchJobOutcome},
    settings::SettingsSession,
    ui::RootView,
};
#[cfg(unix)]
use crate::{
    control::{AppControlSummary, InstanceRuntimeSnapshot, RuntimeReply, RuntimeRequest},
    control_rpc::protocol::{ControlError, InstanceScope},
    instance::InstanceIdentity,
    instance_registry::RegistryPublisher,
};
#[cfg(unix)]
type PlatformControlReceiver = RuntimeControlReceiver;

#[cfg(not(unix))]
struct PlatformControlReceiver;

enum PlatformControlEvent {
    #[cfg(unix)]
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
    settings: SettingsSession,
    request_policy_store: RequestPolicyStore,
    policy: RenderPolicy,
    services: ServiceSupervisor,
    shutdown: CancellationToken,
    #[cfg(unix)]
    identity: Option<InstanceIdentity>,
    control_rx: Option<PlatformControlReceiver>,
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
        settings: SettingsSession,
        request_policy_store: RequestPolicyStore,
        policy: RenderPolicy,
        services: ServiceSupervisor,
        shutdown: CancellationToken,
    ) -> Self {
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
            request_policy_store,
            policy,
            services,
            shutdown,
            #[cfg(unix)]
            identity: None,
            control_rx: None,
            #[cfg(unix)]
            control_publisher: None,
        }
    }

    #[cfg(unix)]
    pub(super) fn with_control(
        mut self,
        identity: InstanceIdentity,
        control_rx: Option<RuntimeControlReceiver>,
        control_publisher: Option<RegistryPublisher>,
    ) -> Self {
        self.identity = Some(identity);
        self.control_rx = control_rx;
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
        #[cfg(unix)]
        self.close_control_ingress();
        self.shutdown.cancel();
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
                control_event = receive_control_event(&mut self.control_rx) => {
                    #[cfg(unix)]
                    match control_event {
                        PlatformControlEvent::Command(command) => {
                            dirty |= self.process_control_command(command);
                        }
                        PlatformControlEvent::Closed => self.control_rx = None,
                    }
                    #[cfg(not(unix))]
                    let _ = control_event;
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
                        | ServiceKind::RequestSearch => {
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

        match save_settings_draft(&mut self.settings, &self.request_policy_store, draft) {
            Ok(saved) => {
                self.app.finish_settings_save(saved);
                log::info!("Settings saved");
            }
            Err(error) => self.app.fail_settings_save(error.to_string()),
        }
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
    #[cfg(unix)]
    fn execute_control(
        &mut self,
        request: RuntimeRequest,
    ) -> std::result::Result<RuntimeReply, ControlError> {
        let identity = self
            .identity
            .as_ref()
            .ok_or_else(|| ControlError::instance_unavailable("runtime control is not enabled"))?;
        execute_control_request(identity, &self.app, &self.settings, request)
    }

    #[cfg(unix)]
    fn process_control_command(&mut self, command: RuntimeCommand) -> bool {
        if command.cancelled.is_cancelled() {
            return false;
        }
        let result = self.execute_control(command.request);
        let _ = command.reply.send(result);
        false
    }

    #[cfg(unix)]
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

    #[cfg(unix)]
    fn control_ingress_is_closed(&self) -> bool {
        self.control_rx
            .as_ref()
            .map_or(true, RuntimeControlReceiver::is_closed)
    }

    #[cfg(all(test, unix))]
    async fn process_next_control_command(&mut self) -> Result<bool> {
        match receive_control_event(&mut self.control_rx).await {
            PlatformControlEvent::Command(command) => Ok(self.process_control_command(command)),
            PlatformControlEvent::Closed => Err(anyhow!("runtime command gateway closed")),
        }
    }
}

#[cfg(unix)]
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

#[cfg(not(unix))]
async fn receive_control_event(
    _receiver: &mut Option<PlatformControlReceiver>,
) -> PlatformControlEvent {
    std::future::pending().await
}

#[cfg(unix)]
fn execute_control_request(
    identity: &InstanceIdentity,
    app: &App,
    settings: &SettingsSession,
    request: RuntimeRequest,
) -> std::result::Result<RuntimeReply, ControlError> {
    match request {
        RuntimeRequest::DescribeInstance => {
            let AppControlSummary {
                recording_enabled,
                retained_capture_count,
                settings_revision,
            } = app.control_summary();
            let context = settings.ui_context();
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
        #[cfg(test)]
        RuntimeRequest::UnsupportedForTest => Err(ControlError::invalid_argument(
            "unsupported runtime control operation",
        )),
    }
}

fn is_fatal_service(kind: ServiceKind) -> bool {
    !matches!(kind, ServiceKind::CertificateDownload | ServiceKind::Logger)
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
        execute_control_request(&self.identity, &self.app, &self.settings, request)
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
        let result = self.execute_control(command.request);
        let _ = command.reply.send(result);
        Ok(false)
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
        let (_client, control_rx) = super::super::control::RuntimeGateway::new(4);
        let mut runtime =
            AppRuntime::test_with_control(identity.clone(), app, settings, control_rx);

        let reply = runtime
            .execute_control(RuntimeRequest::DescribeInstance)
            .expect("describe runtime");
        let RuntimeReply::Instance(snapshot) = reply;

        assert_eq!(snapshot.instance.proxy_endpoint, identity.proxy_endpoint());
        assert_eq!(snapshot.instance.run_id, *identity.run_id());
        assert_eq!(snapshot.config_mode, ConfigMode::Temporary);
        assert_eq!(snapshot.persistence, PersistenceMode::Ephemeral);
        assert!(snapshot.recording_enabled);
        assert_eq!(snapshot.retained_capture_count, 0);
        assert_eq!(snapshot.settings_revision, 0);
    }

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
        let (_client, control_rx) = super::super::control::RuntimeGateway::new(4);
        let mut runtime = AppRuntime::test_with_control(identity, app, settings, control_rx);

        let error = runtime
            .execute_control(RuntimeRequest::UnsupportedForTest)
            .expect_err("unsupported control operation");

        assert_eq!(error.code, ControlErrorCode::InvalidArgument);
        assert!(!error.retryable);
    }

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
        let (client, control_rx) = super::super::control::RuntimeGateway::new(4);
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
        let (client, control_rx) = super::super::control::RuntimeGateway::new(4);
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
    fn control_rpc_completion_is_fatal_to_the_proxy_instance() {
        assert!(is_fatal_service(ServiceKind::ControlRpc));
        assert!(is_fatal_service(ServiceKind::Proxy));
        assert!(!is_fatal_service(ServiceKind::CertificateDownload));
        assert!(!is_fatal_service(ServiceKind::Logger));
    }
}
