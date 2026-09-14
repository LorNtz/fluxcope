use std::{
    collections::{HashMap, HashSet},
    fs, io,
    path::PathBuf,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use tokio::{sync::Notify, task::JoinHandle};
use tokio_util::sync::CancellationToken;

use super::{
    SettingsRevision, SettingsTransactionClient, SettingsTransactionObserver,
    SettingsTransactionOrigin, SettingsTransactionPhase, SettingsTransactionResult,
    start_settings_transaction_service_with_observer,
};
use crate::{
    control::{RuntimeReply, RuntimeRequest, settings::BeginSettingsTransactionReply},
    control_rpc::protocol::{ControlError, ControlErrorCode},
    runtime::gateway::{RuntimeControlClient, RuntimeControlReceiver, RuntimeGateway},
    settings::{
        AppSettings, ConfigMode, PersistenceMode, ProxyPresetSettings, SettingsCommitObserver,
        SettingsCommitter, mapping_ops::MappingMutation,
    },
};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum FailurePoint {
    Compile,
    WriteTemporary,
    FsyncTemporary,
    PreCommit,
    Rename,
    Committed,
    Finalize,
    FinalizeSettingsSwap,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TerminalDelivery {
    Finalize,
    Abort,
}

#[derive(Clone)]
pub(crate) struct StageBarrier {
    inner: Arc<StageBarrierInner>,
}

struct StageBarrierInner {
    entered: AtomicBool,
    entered_notify: Notify,
    released: Mutex<bool>,
    released_notify: Condvar,
}

impl StageBarrier {
    fn new() -> Self {
        Self {
            inner: Arc::new(StageBarrierInner {
                entered: AtomicBool::new(false),
                entered_notify: Notify::new(),
                released: Mutex::new(false),
                released_notify: Condvar::new(),
            }),
        }
    }

    fn block(&self) {
        self.inner.entered.store(true, Ordering::Release);
        self.inner.entered_notify.notify_waiters();
        let mut released = self.inner.released.lock().expect("stage barrier lock");
        while !*released {
            released = self
                .inner
                .released_notify
                .wait(released)
                .expect("stage barrier wait");
        }
    }

    pub(crate) async fn entered(&self) {
        while !self.inner.entered.load(Ordering::Acquire) {
            self.inner.entered_notify.notified().await;
        }
    }

    pub(crate) fn release(&self) {
        *self.inner.released.lock().expect("stage barrier lock") = true;
        self.inner.released_notify.notify_all();
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SettingsRuntimeSnapshot {
    pub(crate) revision: SettingsRevision,
    pub(crate) config_mode: ConfigMode,
    pub(crate) persistence: PersistenceMode,
    pub(crate) settings: Arc<AppSettings>,
    pub(crate) persisted_bytes: Vec<u8>,
}

impl SettingsRuntimeSnapshot {
    pub(crate) fn active_preset(&self) -> Option<String> {
        self.settings
            .proxy
            .as_ref()
            .and_then(|proxy| proxy.active_preset.clone())
    }
}

#[derive(Default)]
struct HookState {
    barriers: HashMap<FailurePoint, StageBarrier>,
    failures: HashSet<FailurePoint>,
    phase: Option<SettingsTransactionPhase>,
    worker_jobs_started: usize,
    compile_count: usize,
    external_write_count: usize,
    policy_swap_count: usize,
    terminal_deliveries: Vec<TerminalDelivery>,
}

struct FixtureHooks {
    state: Mutex<HookState>,
    persistent: bool,
}

impl FixtureHooks {
    fn new(persistent: bool) -> Self {
        Self {
            state: Mutex::new(HookState::default()),
            persistent,
        }
    }

    fn block_next(&self, point: FailurePoint) -> StageBarrier {
        let barrier = StageBarrier::new();
        self.state
            .lock()
            .expect("hook state")
            .barriers
            .insert(point, barrier.clone());
        barrier
    }

    fn fail_next(&self, point: FailurePoint) {
        self.state
            .lock()
            .expect("hook state")
            .failures
            .insert(point);
    }

    fn hit(&self, point: FailurePoint) -> bool {
        let (failed, barrier) = {
            let mut state = self.state.lock().expect("hook state");
            let failed = state.failures.remove(&point);
            let barrier = state.barriers.remove(&point);
            (failed, barrier)
        };
        if let Some(barrier) = barrier {
            barrier.block();
        }
        failed
    }

    fn set_phase(&self, phase: Option<SettingsTransactionPhase>) {
        self.state.lock().expect("hook state").phase = phase;
    }

    fn terminal(&self, delivery: TerminalDelivery) {
        let mut state = self.state.lock().expect("hook state");
        state.terminal_deliveries.push(delivery);
        state.phase = None;
    }
}

impl SettingsTransactionObserver for FixtureHooks {
    fn compile(&self) -> Result<(), ControlError> {
        {
            let mut state = self.state.lock().expect("hook state");
            state.worker_jobs_started += 1;
            state.compile_count += 1;
        }
        if self.hit(FailurePoint::Compile) {
            return Err(ControlError::new(
                ControlErrorCode::InvalidArgument,
                "injected settings compilation failure",
                false,
                serde_json::json!({"stage": "compilation"}),
            ));
        }
        Ok(())
    }

    fn before_finalize(&self) {
        let _ = self.hit(FailurePoint::Finalize);
    }
}

impl SettingsCommitObserver for FixtureHooks {
    fn prepare(&self) -> io::Result<()> {
        self.state.lock().expect("hook state").worker_jobs_started += 1;
        if self.hit(FailurePoint::WriteTemporary) {
            return Err(io::Error::other("injected temporary write failure"));
        }
        if self.hit(FailurePoint::FsyncTemporary) {
            return Err(io::Error::other("injected temporary fsync failure"));
        }
        Ok(())
    }

    fn prepared(&self) {
        self.set_phase(Some(SettingsTransactionPhase::PreCommit));
        let _ = self.hit(FailurePoint::PreCommit);
    }

    fn rename(&self) -> io::Result<()> {
        self.set_phase(Some(SettingsTransactionPhase::Committing));
        if self.hit(FailurePoint::Rename) {
            return Err(io::Error::other("injected rename failure"));
        }
        Ok(())
    }

    fn committed(&self) {
        {
            let mut state = self.state.lock().expect("hook state");
            state.phase = Some(SettingsTransactionPhase::Committed);
            if self.persistent {
                state.external_write_count += 1;
            }
        }
        let _ = self.hit(FailurePoint::Committed);
    }
}

struct RuntimeState {
    settings: Arc<AppSettings>,
    revision: SettingsRevision,
    config_mode: ConfigMode,
    persistence: PersistenceMode,
    pending: Option<(super::SettingsTransactionToken, SettingsTransactionOrigin)>,
    next_token: u64,
    popup_visible: bool,
    popup_dirty: bool,
    popup_settings: AppSettings,
    active_policy_preset: Option<String>,
    turns: usize,
}

struct FixtureInner {
    client: SettingsTransactionClient,
    runtime: RuntimeControlClient,
    state: Arc<Mutex<RuntimeState>>,
    hooks: Arc<FixtureHooks>,
    shutdown: CancellationToken,
    shutdown_started: AtomicBool,
    shutdown_notify: Notify,
    service_task: Mutex<Option<JoinHandle<anyhow::Result<()>>>>,
    runtime_task: Mutex<Option<JoinHandle<()>>>,
    persisted_path: Option<PathBuf>,
    _directory: tempfile::TempDir,
}

pub(crate) struct SettingsRuntimeFixture {
    inner: Arc<FixtureInner>,
}

impl SettingsRuntimeFixture {
    pub(crate) async fn temporary() -> Self {
        Self::new(
            ConfigMode::Temporary,
            PersistenceMode::Ephemeral,
            AppSettings::default(),
        )
        .await
    }

    pub(crate) async fn read_only() -> Self {
        Self::new(
            ConfigMode::ReadOnlyFile,
            PersistenceMode::Ephemeral,
            AppSettings::default(),
        )
        .await
    }

    pub(crate) async fn persistent() -> Self {
        Self::new(
            ConfigMode::DefaultOwned,
            PersistenceMode::Persistent,
            AppSettings::default(),
        )
        .await
    }

    pub(crate) async fn persistent_with_preset(name: &str) -> Self {
        let mut settings = AppSettings::default();
        settings.proxy = Some(crate::settings::ProxySettings {
            active_preset: Some(name.to_string()),
            presets: vec![ProxyPresetSettings {
                name: name.to_string(),
                ..ProxyPresetSettings::default()
            }],
            ..crate::settings::ProxySettings::default()
        });
        Self::new(
            ConfigMode::DefaultOwned,
            PersistenceMode::Persistent,
            settings,
        )
        .await
    }

    pub(crate) async fn temporary_with_remote_presets() -> Self {
        let mut settings = AppSettings::default();
        settings.proxy = Some(crate::settings::ProxySettings {
            active_preset: Some("first".to_string()),
            presets: vec![
                ProxyPresetSettings {
                    name: "first".to_string(),
                    ..ProxyPresetSettings::default()
                },
                ProxyPresetSettings {
                    name: "second".to_string(),
                    ..ProxyPresetSettings::default()
                },
            ],
            ..crate::settings::ProxySettings::default()
        });
        Self::new(ConfigMode::Temporary, PersistenceMode::Ephemeral, settings).await
    }

    async fn new(
        config_mode: ConfigMode,
        persistence: PersistenceMode,
        settings: AppSettings,
    ) -> Self {
        let directory = tempfile::tempdir().expect("fixture directory");
        let persisted_path =
            (config_mode != ConfigMode::Temporary).then(|| directory.path().join("config.yml"));
        if let Some(path) = &persisted_path {
            fs::write(
                path,
                serde_yaml::to_string(&settings).expect("fixture YAML"),
            )
            .expect("fixture settings file");
        }
        let hooks = Arc::new(FixtureHooks::new(
            persistence == PersistenceMode::Persistent,
        ));
        let observer: Arc<dyn SettingsCommitObserver> = hooks.clone();
        let committer = SettingsCommitter::for_test(persistence, persisted_path.clone(), observer);
        let (runtime, receiver) = RuntimeGateway::channel(8);
        let shutdown = CancellationToken::new();
        let transaction_observer: Arc<dyn SettingsTransactionObserver> = hooks.clone();
        let (client, service_task) = start_settings_transaction_service_with_observer(
            runtime.clone(),
            committer,
            shutdown.child_token(),
            transaction_observer,
        );
        let settings = Arc::new(settings);
        let state = Arc::new(Mutex::new(RuntimeState {
            popup_settings: settings.as_ref().clone(),
            active_policy_preset: settings
                .proxy
                .as_ref()
                .and_then(|proxy| proxy.active_preset.clone()),
            settings,
            revision: SettingsRevision::INITIAL,
            config_mode,
            persistence,
            pending: None,
            next_token: 1,
            popup_visible: false,
            popup_dirty: false,
            turns: 0,
        }));
        let runtime_task = tokio::spawn(run_runtime_authority(
            receiver,
            Arc::clone(&state),
            Arc::clone(&hooks),
            shutdown.child_token(),
        ));
        Self {
            inner: Arc::new(FixtureInner {
                client,
                runtime,
                state,
                hooks,
                shutdown,
                shutdown_started: AtomicBool::new(false),
                shutdown_notify: Notify::new(),
                service_task: Mutex::new(Some(service_task)),
                runtime_task: Mutex::new(Some(runtime_task)),
                persisted_path,
                _directory: directory,
            }),
        }
    }

    pub(crate) fn client(&self) -> SettingsTransactionClient {
        self.inner.client.clone()
    }

    pub(crate) fn available_terminal_delivery_permits(&self) -> usize {
        self.inner.runtime.available_terminal_delivery_permits()
    }

    pub(crate) async fn mutate(
        &self,
        mutation: MappingMutation,
        revision: SettingsRevision,
        origin: SettingsTransactionOrigin,
        cancelled: CancellationToken,
    ) -> Result<SettingsTransactionResult, ControlError> {
        self.inner
            .client
            .mutate_mapping(mutation, revision, origin, cancelled)
            .await
    }

    pub(crate) fn snapshot(&self) -> SettingsRuntimeSnapshot {
        let state = self.inner.state.lock().expect("runtime state");
        SettingsRuntimeSnapshot {
            revision: state.revision,
            config_mode: state.config_mode,
            persistence: state.persistence,
            settings: Arc::clone(&state.settings),
            persisted_bytes: self.persisted_bytes(),
        }
    }

    pub(crate) fn preset(&self, name: &str) -> Option<ProxyPresetSettings> {
        self.inner
            .state
            .lock()
            .expect("runtime state")
            .settings
            .proxy
            .as_ref()?
            .presets
            .iter()
            .find(|preset| preset.name == name)
            .cloned()
    }

    pub(crate) fn preset_names(&self) -> Vec<String> {
        self.inner
            .state
            .lock()
            .expect("runtime state")
            .settings
            .proxy
            .as_ref()
            .map(|proxy| {
                proxy
                    .presets
                    .iter()
                    .map(|preset| preset.name.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    pub(crate) fn open_dirty_popup(&self) {
        let mut state = self.inner.state.lock().expect("runtime state");
        state.popup_visible = true;
        state.popup_dirty = true;
        let settings = state.settings.as_ref().clone();
        state.popup_settings = settings;
        state.popup_settings.server.port = state.popup_settings.server.port.saturating_add(1);
    }

    pub(crate) fn open_clean_popup(&self) {
        let mut state = self.inner.state.lock().expect("runtime state");
        state.popup_visible = true;
        state.popup_dirty = false;
        let settings = state.settings.as_ref().clone();
        state.popup_settings = settings;
    }

    pub(crate) fn popup_is_visible(&self) -> bool {
        self.inner
            .state
            .lock()
            .expect("runtime state")
            .popup_visible
    }
    pub(crate) fn popup_is_dirty(&self) -> bool {
        self.inner.state.lock().expect("runtime state").popup_dirty
    }
    pub(crate) fn popup_preset(&self, name: &str) -> Option<ProxyPresetSettings> {
        self.inner
            .state
            .lock()
            .expect("runtime state")
            .popup_settings
            .proxy
            .as_ref()?
            .presets
            .iter()
            .find(|preset| preset.name == name)
            .cloned()
    }

    pub(crate) fn try_tui_edit_server_port(&self, port: u16) -> Result<(), ControlError> {
        let mut state = self.inner.state.lock().expect("runtime state");
        if state.pending.is_some() {
            return Err(pending_error());
        }
        state.popup_settings.server.port = port;
        state.popup_dirty = true;
        Ok(())
    }

    pub(crate) fn try_tui_save(&self) -> Result<(), ControlError> {
        if self
            .inner
            .state
            .lock()
            .expect("runtime state")
            .pending
            .is_some()
        {
            Err(pending_error())
        } else {
            Ok(())
        }
    }

    pub(crate) fn block_next(&self, point: FailurePoint) -> StageBarrier {
        self.inner.hooks.block_next(point)
    }
    pub(crate) fn fail_next(&self, point: FailurePoint) {
        self.inner.hooks.fail_next(point);
    }
    pub(crate) fn persisted_bytes(&self) -> Vec<u8> {
        self.inner
            .persisted_path
            .as_ref()
            .map(|path| fs::read(path).expect("persisted settings"))
            .unwrap_or_default()
    }
    pub(crate) fn persisted_settings(&self) -> AppSettings {
        serde_yaml::from_slice(&self.persisted_bytes()).expect("persisted settings YAML")
    }
    pub(crate) fn worker_jobs_started(&self) -> usize {
        self.inner
            .hooks
            .state
            .lock()
            .expect("hook state")
            .worker_jobs_started
    }
    pub(crate) fn compile_count(&self) -> usize {
        self.inner
            .hooks
            .state
            .lock()
            .expect("hook state")
            .compile_count
    }
    pub(crate) fn external_write_count(&self) -> usize {
        self.inner
            .hooks
            .state
            .lock()
            .expect("hook state")
            .external_write_count
    }
    pub(crate) fn policy_swap_count(&self) -> usize {
        self.inner
            .hooks
            .state
            .lock()
            .expect("hook state")
            .policy_swap_count
    }
    pub(crate) fn terminal_deliveries(&self) -> Vec<TerminalDelivery> {
        self.inner
            .hooks
            .state
            .lock()
            .expect("hook state")
            .terminal_deliveries
            .clone()
    }
    pub(crate) fn transaction_phase(&self) -> Option<SettingsTransactionPhase> {
        self.inner.hooks.state.lock().expect("hook state").phase
    }
    pub(crate) fn has_pending_transaction(&self) -> bool {
        self.inner
            .state
            .lock()
            .expect("runtime state")
            .pending
            .is_some()
    }
    pub(crate) fn active_policy_preset(&self) -> Option<String> {
        self.inner
            .state
            .lock()
            .expect("runtime state")
            .active_policy_preset
            .clone()
    }
    pub(crate) fn runtime_turn_count(&self) -> usize {
        self.inner.state.lock().expect("runtime state").turns
    }

    pub(crate) async fn ping_runtime(&self) -> Result<(), ControlError> {
        self.inner
            .runtime
            .request(RuntimeRequest::GetMappingSettings, CancellationToken::new())
            .await
            .map(|_| ())
    }

    pub(crate) async fn get_mapping_settings(
        &self,
    ) -> Result<crate::control::settings::MappingSettingsResult, ControlError> {
        self.inner
            .client
            .get_mapping_settings(Default::default(), CancellationToken::new())
            .await
    }

    pub(crate) async fn validate_mapping_settings(
        &self,
        proxy: crate::settings::ProxySettings,
    ) -> Result<crate::settings::mapping_ops::MappingValidationResult, ControlError> {
        self.inner
            .client
            .validate_mapping_settings(proxy, CancellationToken::new())
            .await
            .map(|reply| reply.validation)
    }

    pub(crate) async fn explain_mapping(
        &self,
        url: &str,
        proxy: Option<crate::settings::ProxySettings>,
    ) -> Result<crate::settings::mapping_ops::MappingExplanation, ControlError> {
        self.inner
            .client
            .explain_mapping(url.to_string(), proxy, CancellationToken::new())
            .await
            .map(|reply| reply.explanation)
    }

    pub(crate) async fn shutdown(&self) {
        self.inner.shutdown_started.store(true, Ordering::Release);
        self.inner.shutdown_notify.notify_waiters();
        self.inner.shutdown.cancel();
        let service = self.inner.service_task.lock().expect("service task").take();
        if let Some(service) = service {
            let _ = service.await;
        }
        let runtime = self.inner.runtime_task.lock().expect("runtime task").take();
        if let Some(runtime) = runtime {
            let _ = runtime.await;
        }
    }

    pub(crate) async fn shutdown_started(&self) {
        while !self.inner.shutdown_started.load(Ordering::Acquire) {
            self.inner.shutdown_notify.notified().await;
        }
    }

    pub(crate) fn gateway_accepts_reserved_terminal_delivery(&self) -> bool {
        !self.gateway_is_closed()
    }
    pub(crate) fn gateway_is_closed(&self) -> bool {
        self.inner
            .runtime_task
            .lock()
            .expect("runtime task")
            .is_none()
    }
}

fn pending_error() -> ControlError {
    ControlError::new(
        ControlErrorCode::ServiceUnavailable,
        "another settings transaction is pending",
        true,
        serde_json::json!({"stage": "settings_transaction_pending"}),
    )
}

async fn run_runtime_authority(
    mut receiver: RuntimeControlReceiver,
    state: Arc<Mutex<RuntimeState>>,
    hooks: Arc<FixtureHooks>,
    shutdown: CancellationToken,
) {
    loop {
        let pending = state.lock().expect("runtime state").pending.is_some();
        let command = if pending {
            receiver.recv().await
        } else {
            tokio::select! {
                command = receiver.recv() => command,
                _ = shutdown.cancelled() => {
                    receiver.close();
                    break;
                }
            }
        };
        let Some(command) = command else { break };
        state.lock().expect("runtime state").turns += 1;
        let state = Arc::clone(&state);
        let hooks = Arc::clone(&hooks);
        let result = tokio::task::spawn_blocking(move || {
            execute_runtime_request(command.request, &state, &hooks)
        })
        .await
        .expect("runtime authority worker");
        let _ = command.reply.send(result);
    }
}

fn execute_runtime_request(
    request: RuntimeRequest,
    state: &Arc<Mutex<RuntimeState>>,
    hooks: &Arc<FixtureHooks>,
) -> Result<RuntimeReply, ControlError> {
    match request {
        RuntimeRequest::GetMappingSettings => {
            let state = state.lock().expect("runtime state");
            Ok(RuntimeReply::MappingSettings(
                crate::control::settings::MappingSettingsSnapshot {
                    settings: Arc::clone(&state.settings),
                    revision: state.revision,
                    config_mode: state.config_mode,
                    persistence: state.persistence,
                },
            ))
        }
        RuntimeRequest::PreviewMappingSnapshot { expected_revision } => {
            let state = state.lock().expect("runtime state");
            if expected_revision != state.revision {
                return Err(ControlError::new(
                    ControlErrorCode::SettingsRevisionConflict,
                    "settings revision does not match",
                    false,
                    serde_json::json!({"expected_revision": expected_revision, "current_revision": state.revision}),
                ));
            }
            if state.popup_dirty {
                return Err(ControlError::new(
                    ControlErrorCode::TuiDraftConflict,
                    "mapping settings conflict with an unsaved TUI draft",
                    false,
                    serde_json::json!({"current_revision": state.revision}),
                ));
            }
            if state.pending.is_some() {
                return Err(pending_error());
            }
            Ok(RuntimeReply::MappingSettings(
                crate::control::settings::MappingSettingsSnapshot {
                    settings: Arc::clone(&state.settings),
                    revision: state.revision,
                    config_mode: state.config_mode,
                    persistence: state.persistence,
                },
            ))
        }
        RuntimeRequest::BeginSettingsTransaction {
            expected_revision,
            origin,
        } => {
            let mut state = state.lock().expect("runtime state");
            if expected_revision.is_some_and(|expected| expected != state.revision) {
                return Err(ControlError::new(
                    ControlErrorCode::SettingsRevisionConflict,
                    "settings revision does not match",
                    false,
                    serde_json::json!({"expected_revision": expected_revision, "current_revision": state.revision}),
                ));
            }
            if origin == SettingsTransactionOrigin::Mcp && state.popup_dirty {
                return Err(ControlError::new(
                    ControlErrorCode::TuiDraftConflict,
                    "mapping settings conflict with an unsaved TUI draft",
                    false,
                    serde_json::json!({"current_revision": state.revision}),
                ));
            }
            if state.pending.is_some() {
                return Err(pending_error());
            }
            let token = super::SettingsTransactionToken::new(state.next_token);
            state.next_token = state.next_token.saturating_add(1);
            state.pending = Some((token, origin));
            Ok(RuntimeReply::SettingsTransactionBegun(
                BeginSettingsTransactionReply {
                    settings: Arc::clone(&state.settings),
                    revision: state.revision,
                    token,
                    config_mode: state.config_mode,
                    persistence: state.persistence,
                },
            ))
        }
        RuntimeRequest::FinalizeSettingsTransaction { token, commit } => {
            {
                let mut state = state.lock().expect("runtime state");
                if state.pending != Some((token, commit.origin)) {
                    return Err(ControlError::internal(
                        "settings finalization token mismatch",
                    ));
                }
                if commit.outcome == super::SettingsTransactionOutcome::Committed {
                    let policy = commit
                        .policy
                        .as_ref()
                        .ok_or_else(|| ControlError::internal("missing compiled policy"))?;
                    let _ = policy;
                    state.active_policy_preset = commit
                        .settings
                        .proxy
                        .as_ref()
                        .and_then(|proxy| proxy.active_preset.clone());
                    hooks.state.lock().expect("hook state").policy_swap_count += 1;
                }
            }
            let _ = hooks.hit(FailurePoint::FinalizeSettingsSwap);
            let mut state = state.lock().expect("runtime state");
            if commit.outcome == super::SettingsTransactionOutcome::Committed {
                state.settings = Arc::clone(&commit.settings);
                state.revision = state.revision.next();
                if state.popup_visible && !state.popup_dirty {
                    state.popup_settings = commit.settings.as_ref().clone();
                }
            }
            state.pending = None;
            hooks.terminal(TerminalDelivery::Finalize);
            Ok(RuntimeReply::SettingsTransactionFinalized(commit.outcome))
        }
        RuntimeRequest::AbortSettingsTransaction { token } => {
            let mut state = state.lock().expect("runtime state");
            if !state.pending.is_some_and(|(pending, _)| pending == token) {
                return Err(ControlError::internal("settings abort token mismatch"));
            }
            state.pending = None;
            hooks.terminal(TerminalDelivery::Abort);
            Ok(RuntimeReply::SettingsTransactionAborted)
        }
        _ => Err(ControlError::invalid_argument(
            "unsupported fixture runtime request",
        )),
    }
}
