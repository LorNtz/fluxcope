use std::{io, sync::Arc, time::Instant};

use anyhow::{Context, Result, anyhow};
use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture, Event, EventStream, KeyCode, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use futures::StreamExt;
use tokio::{sync::mpsc, time};
use tokio_util::sync::CancellationToken;

use super::{
    policy::RenderPolicy,
    save_settings_draft,
    services::{ServiceKind, ServiceSupervisor},
};
use crate::{
    app::App,
    capture::CapturedExchange,
    logging::{LogRecord, LoggingMetrics, LoggingMetricsSnapshot, LoggingStatus},
    mapping::MappingStore,
    settings::SettingsManager,
    ui::RootView,
};

pub(super) struct AppRuntime {
    app: App,
    ui: RootView,
    capture_rx: mpsc::Receiver<CapturedExchange>,
    log_rx: mpsc::Receiver<LogRecord>,
    logging_status_rx: mpsc::Receiver<LoggingStatus>,
    logging_metrics: Arc<LoggingMetrics>,
    last_logging_metrics: LoggingMetricsSnapshot,
    tui: Tui,
    settings: SettingsManager,
    mapping_store: MappingStore,
    policy: RenderPolicy,
    services: ServiceSupervisor,
    shutdown: CancellationToken,
}

impl AppRuntime {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        app: App,
        capture_rx: mpsc::Receiver<CapturedExchange>,
        log_rx: mpsc::Receiver<LogRecord>,
        logging_status_rx: mpsc::Receiver<LoggingStatus>,
        logging_metrics: Arc<LoggingMetrics>,
        tui: Tui,
        settings: SettingsManager,
        mapping_store: MappingStore,
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
            tui,
            settings,
            mapping_store,
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
        self.tui
            .draw(&mut self.ui, &mut self.app)
            .context("failed to draw initial terminal frame")?;
        let mut dirty = false;

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
                capture = self.capture_rx.recv() => {
                    if let Some(capture) = capture {
                        self.app.add_request(capture);
                        self.drain_runtime_events();
                        dirty = true;
                    }
                }
                record = self.log_rx.recv() => {
                    if let Some(record) = record {
                        self.app.append_log(record);
                        self.drain_runtime_events();
                        dirty = true;
                    }
                }
                status = self.logging_status_rx.recv() => {
                    if let Some(LoggingStatus::Degraded(message)) = status {
                        self.app.append_log(LogRecord::system(format!("ERROR - [wirelens::logging] {message}")));
                        dirty = true;
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
                            self.app.append_log(LogRecord::system(format!("WARN - [wirelens::runtime] {message}")));
                            dirty = true;
                        }
                        ServiceKind::Logger => {
                            let message = match completion.result {
                                Ok(()) => "logging service stopped".to_string(),
                                Err(error) => format!("logging service failed: {error:#}"),
                            };
                            self.app.append_log(LogRecord::system(format!("ERROR - [wirelens::runtime] {message}")));
                            dirty = true;
                        }
                    }
                }
                _ = metrics_tick.tick() => {
                    dirty |= self.refresh_metrics();
                }
                _ = frame_tick.tick() => {
                    if dirty {
                        self.tui.draw(&mut self.ui, &mut self.app)
                            .context("failed to draw terminal frame")?;
                        dirty = false;
                    }
                }
            }
        }
    }

    fn handle_terminal_event(&mut self, event: Event) -> bool {
        match event {
            Event::Key(key)
                if key.modifiers == KeyModifiers::CONTROL
                    && matches!(key.code, KeyCode::Char('c') | KeyCode::Char('C')) =>
            {
                true
            }
            Event::Key(key) => self.app.handle_key_event(key),
            Event::Mouse(mouse) => {
                self.ui.handle_mouse(mouse, &mut self.app);
                false
            }
            Event::Resize(_, _) | Event::FocusGained | Event::FocusLost | Event::Paste(_) => false,
        }
    }

    fn drain_runtime_events(&mut self) {
        let started = Instant::now();
        for _ in 1..self.policy.max_events_per_turn {
            if started.elapsed() >= self.policy.event_budget {
                break;
            }

            if let Ok(capture) = self.capture_rx.try_recv() {
                self.app.add_request(capture);
                continue;
            }
            if let Ok(record) = self.log_rx.try_recv() {
                self.app.append_log(record);
                continue;
            }
            if let Ok(LoggingStatus::Degraded(message)) = self.logging_status_rx.try_recv() {
                self.app.append_log(LogRecord::system(format!(
                    "ERROR - [wirelens::logging] {message}"
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

        match save_settings_draft(&mut self.settings, &self.mapping_store, draft) {
            Ok(saved) => {
                self.app.finish_settings_save(saved);
                log::info!("Settings saved");
            }
            Err(error) => self.app.fail_settings_save(error.to_string()),
        }
        true
    }

    fn refresh_metrics(&mut self) -> bool {
        let snapshot = self.logging_metrics.snapshot();
        if snapshot == self.last_logging_metrics {
            return false;
        }
        self.last_logging_metrics = snapshot;
        if snapshot == LoggingMetricsSnapshot::default() {
            return false;
        }

        self.app.append_log(LogRecord::system(format!(
            "WARN - [wirelens::metrics] log drops producer={} tui={} truncated={}",
            snapshot.producer_dropped, snapshot.tui_dropped, snapshot.records_truncated
        )));
        true
    }
}

pub(super) struct Tui {
    terminal: ratatui::Terminal<ratatui::backend::CrosstermBackend<io::Stdout>>,
}

impl Tui {
    pub fn enter() -> io::Result<Self> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        if let Err(error) = execute!(stdout, EnterAlternateScreen, EnableMouseCapture) {
            let _ = disable_raw_mode();
            return Err(error);
        }
        let backend = ratatui::backend::CrosstermBackend::new(stdout);
        match ratatui::Terminal::new(backend) {
            Ok(terminal) => Ok(Self { terminal }),
            Err(error) => {
                let _ = disable_raw_mode();
                Err(error)
            }
        }
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
            DisableMouseCapture
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
    }
}
