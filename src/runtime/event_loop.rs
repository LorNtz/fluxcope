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
        }
    }

    pub async fn run(mut self) -> Result<()> {
        let result = self.run_loop().await;
        self.shutdown.cancel();
        self.services.shutdown(self.policy.shutdown_grace).await;
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
                    match completion.kind {
                        ServiceKind::Proxy => {
                            completion.result?;
                            return Err(anyhow!("proxy service exited unexpectedly"));
                        }
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
                        ServiceKind::BodyPumps => {
                            completion.result?;
                            return Err(anyhow!("body pump supervisor exited unexpectedly"));
                        }
                        ServiceKind::Decoder => {
                            completion.result?;
                            return Err(anyhow!("decode service exited unexpectedly"));
                        }
                        ServiceKind::RequestSearch => {
                            completion.result?;
                            return Err(anyhow!("request search service exited unexpectedly"));
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
}
