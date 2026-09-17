mod preview;

use std::{fmt, sync::Arc};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::{
    sync::{OwnedSemaphorePermit, Semaphore, mpsc, oneshot},
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;

use super::gateway::RuntimeControlClient;
use crate::{
    app::validate_settings,
    control::{
        RuntimeReply, RuntimeRequest,
        settings::mapping::{MappingReadScope, MappingSettingsView},
        settings::{
            FinalizedSettingsTransaction, MappingExplanationReply, MappingSettingsResult,
            MappingValidationReply,
        },
    },
    control_rpc::protocol::{ControlError, ControlErrorCode},
    request_policy::{RequestPolicy, RequestPolicyDiagnosticSeverity},
    settings::{
        AppSettings, ConfigMode, PersistenceMode, ProxySettings, SettingsCommitter,
        mapping_ops::{
            MAX_MAPPING_DIAGNOSTICS, MappingMutation, MappingObjectRef, MutationEffect,
            apply_mapping_mutation_owned, explain_mapping_candidate, validate_mapping_candidate,
        },
    },
};

pub(crate) const SETTINGS_TRANSACTION_QUEUE_CAPACITY: usize = 8;
const MAPPING_WORKER_LIMIT: usize = 2;

#[derive(
    Clone, Copy, Debug, Deserialize, Eq, Hash, JsonSchema, Ord, PartialEq, PartialOrd, Serialize,
)]
#[serde(transparent)]
pub(crate) struct SettingsRevision(u64);

impl SettingsRevision {
    pub(crate) const INITIAL: Self = Self(1);

    #[cfg(test)]
    pub(crate) const fn new(value: u64) -> Self {
        Self(value)
    }

    pub(crate) const fn get(self) -> u64 {
        self.0
    }

    pub(crate) fn next(self) -> Self {
        Self(self.0.saturating_add(1))
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct SettingsTransactionToken(u64);

impl SettingsTransactionToken {
    pub(crate) const fn new(value: u64) -> Self {
        Self(value)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SettingsTransactionOrigin {
    Mcp,
    Tui,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SettingsTransactionOutcome {
    Committed,
    Unchanged,
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SettingsTransactionPhase {
    PreCommit,
    Committing,
    Committed,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SettingsTransactionResult {
    pub(crate) outcome: SettingsTransactionOutcome,
    pub(crate) revision: SettingsRevision,
    pub(crate) config_mode: ConfigMode,
    pub(crate) persistence: PersistenceMode,
    pub(crate) affected: MappingObjectRef,
    pub(crate) settings: Arc<AppSettings>,
}

#[derive(Clone)]
pub(crate) struct SettingsTransactionClient {
    transactions: mpsc::Sender<SettingsTransactionRequest>,
    runtime: RuntimeControlClient,
    mapping_workers: Arc<Semaphore>,
}

enum SettingsTransactionChange {
    Mapping(MappingMutation),
    Replace(Arc<AppSettings>),
}

struct SettingsTransactionRequest {
    work: SettingsTransactionWork,
    cancelled: CancellationToken,
    reply: oneshot::Sender<Result<SettingsTransactionResult, ControlError>>,
}

struct SettingsTransactionWork {
    change: SettingsTransactionChange,
    expected_revision: Option<SettingsRevision>,
    origin: SettingsTransactionOrigin,
}

impl SettingsTransactionClient {
    pub(crate) async fn mutate_mapping(
        &self,
        mutation: MappingMutation,
        expected_revision: SettingsRevision,
        origin: SettingsTransactionOrigin,
        cancelled: CancellationToken,
    ) -> Result<SettingsTransactionResult, ControlError> {
        self.submit(
            SettingsTransactionChange::Mapping(mutation),
            Some(expected_revision),
            origin,
            cancelled,
        )
        .await
    }

    pub(crate) async fn replace_from_tui(
        &self,
        settings: Arc<AppSettings>,
        cancelled: CancellationToken,
    ) -> Result<SettingsTransactionResult, ControlError> {
        self.submit(
            SettingsTransactionChange::Replace(settings),
            None,
            SettingsTransactionOrigin::Tui,
            cancelled,
        )
        .await
    }

    async fn submit(
        &self,
        change: SettingsTransactionChange,
        expected_revision: Option<SettingsRevision>,
        origin: SettingsTransactionOrigin,
        cancelled: CancellationToken,
    ) -> Result<SettingsTransactionResult, ControlError> {
        let permit = tokio::select! {
            permit = self.transactions.reserve() => permit.map_err(|_| {
                ControlError::service_unavailable("settings transaction service is unavailable")
            })?,
            _ = cancelled.cancelled() => return Err(ControlError::cancelled(
                "settings transaction cancelled before admission"
            )),
        };
        let (reply, response) = oneshot::channel();
        permit.send(SettingsTransactionRequest {
            work: SettingsTransactionWork {
                change,
                expected_revision,
                origin,
            },
            cancelled: cancelled.clone(),
            reply,
        });
        response.await.unwrap_or_else(|_| {
            Err(ControlError::service_unavailable(
                "settings transaction service stopped before terminal delivery",
            ))
        })
    }

    pub(crate) async fn get_mapping_settings(
        &self,
        scope: MappingReadScope,
        cancelled: CancellationToken,
    ) -> Result<MappingSettingsResult, ControlError> {
        let permit = acquire_worker_permit(Arc::clone(&self.mapping_workers), &cancelled).await?;
        let snapshot = match self
            .runtime
            .request(RuntimeRequest::GetMappingSettings, cancelled.clone())
            .await?
        {
            RuntimeReply::MappingSettings(snapshot) => snapshot,
            _ => {
                return Err(ControlError::internal(
                    "runtime returned an unexpected mapping snapshot",
                ));
            }
        };
        let worker_permit = Arc::new(permit);
        let retained_permit = Arc::clone(&worker_permit);
        let settings = snapshot.settings;
        let task = tokio::task::spawn_blocking(move || {
            let _permit = retained_permit;
            MappingSettingsView::scoped(
                settings.proxy.as_ref().unwrap_or(&ProxySettings::default()),
                scope,
            )
        });
        let mapping = tokio::select! {
            result = task => result.map_err(|error| ControlError::internal(format!("settings worker failed: {error}")))??,
            _ = cancelled.cancelled() => return Err(ControlError::cancelled("mapping settings read was cancelled")),
        };
        Ok(MappingSettingsResult {
            revision: snapshot.revision,
            config_mode: snapshot.config_mode,
            persistence: snapshot.persistence,
            mapping,
            worker_permit,
        })
    }

    pub(crate) async fn validate_mapping_settings(
        &self,
        proxy: ProxySettings,
        cancelled: CancellationToken,
    ) -> Result<MappingValidationReply, ControlError> {
        let permit = acquire_worker_permit(Arc::clone(&self.mapping_workers), &cancelled).await?;
        let snapshot = match self
            .runtime
            .request(RuntimeRequest::GetMappingSettings, cancelled.clone())
            .await?
        {
            RuntimeReply::MappingSettings(snapshot) => snapshot,
            _ => {
                return Err(ControlError::internal(
                    "runtime returned an unexpected mapping snapshot",
                ));
            }
        };
        let validation = run_worker_with_permit(permit, cancelled, move || {
            validate_mapping_candidate(Some(&proxy))
        })
        .await?;
        Ok(MappingValidationReply {
            revision: snapshot.revision,
            config_mode: snapshot.config_mode,
            persistence: snapshot.persistence,
            validation,
        })
    }

    pub(crate) async fn explain_mapping(
        &self,
        url: String,
        proposed_proxy: Option<ProxySettings>,
        cancelled: CancellationToken,
    ) -> Result<MappingExplanationReply, ControlError> {
        let permit = acquire_worker_permit(Arc::clone(&self.mapping_workers), &cancelled).await?;
        let snapshot = match self
            .runtime
            .request(RuntimeRequest::GetMappingSettings, cancelled.clone())
            .await?
        {
            RuntimeReply::MappingSettings(snapshot) => snapshot,
            _ => {
                return Err(ControlError::internal(
                    "runtime returned an unexpected mapping snapshot",
                ));
            }
        };
        let settings = Arc::clone(&snapshot.settings);
        let explanation = run_worker_with_permit(permit, cancelled, move || {
            explain_mapping_candidate(proposed_proxy.as_ref().or(settings.proxy.as_ref()), &url)
        })
        .await?;
        Ok(MappingExplanationReply {
            revision: snapshot.revision,
            config_mode: snapshot.config_mode,
            persistence: snapshot.persistence,
            explanation,
        })
    }

    #[cfg(test)]
    pub(crate) async fn run_mapping_read_for_test(
        &self,
        cancelled: CancellationToken,
        work: impl FnOnce() + Send + 'static,
    ) -> Result<(), ControlError> {
        run_bounded_worker(Arc::clone(&self.mapping_workers), cancelled, work).await
    }
}

pub(crate) trait SettingsTransactionObserver: Send + Sync {
    fn compile(&self) -> Result<(), ControlError>;
    fn before_finalize(&self);
}

pub(super) fn start_settings_transaction_service(
    runtime: RuntimeControlClient,
    committer: SettingsCommitter,
    shutdown: CancellationToken,
) -> (SettingsTransactionClient, JoinHandle<anyhow::Result<()>>) {
    start_settings_transaction_service_inner(runtime, committer, shutdown, None)
}

#[cfg(test)]
pub(super) fn start_settings_transaction_service_with_observer(
    runtime: RuntimeControlClient,
    committer: SettingsCommitter,
    shutdown: CancellationToken,
    observer: Arc<dyn SettingsTransactionObserver>,
) -> (SettingsTransactionClient, JoinHandle<anyhow::Result<()>>) {
    start_settings_transaction_service_inner(runtime, committer, shutdown, Some(observer))
}

fn start_settings_transaction_service_inner(
    runtime: RuntimeControlClient,
    committer: SettingsCommitter,
    shutdown: CancellationToken,
    observer: Option<Arc<dyn SettingsTransactionObserver>>,
) -> (SettingsTransactionClient, JoinHandle<anyhow::Result<()>>) {
    let (transactions, receiver) = mpsc::channel(SETTINGS_TRANSACTION_QUEUE_CAPACITY);
    let mapping_workers = Arc::new(Semaphore::new(MAPPING_WORKER_LIMIT));
    let client = SettingsTransactionClient {
        transactions,
        runtime: runtime.clone(),
        mapping_workers: Arc::clone(&mapping_workers),
    };
    let task = tokio::spawn(run_settings_transactions(
        receiver,
        runtime,
        committer,
        shutdown,
        observer,
        mapping_workers,
    ));
    (client, task)
}

async fn run_settings_transactions(
    mut receiver: mpsc::Receiver<SettingsTransactionRequest>,
    runtime: RuntimeControlClient,
    committer: SettingsCommitter,
    shutdown: CancellationToken,
    observer: Option<Arc<dyn SettingsTransactionObserver>>,
    mapping_workers: Arc<Semaphore>,
) -> anyhow::Result<()> {
    loop {
        let request = tokio::select! {
            request = receiver.recv() => request,
            _ = shutdown.cancelled() => {
                receiver.close();
                receiver.recv().await
            }
        };
        let Some(request) = request else { break };
        let SettingsTransactionRequest {
            work,
            cancelled: request_cancelled,
            reply,
        } = request;
        let cancelled = request_cancelled.child_token();
        let result = {
            let transaction = execute_transaction(
                &runtime,
                &committer,
                work,
                &cancelled,
                observer.as_ref(),
                &mapping_workers,
            );
            tokio::pin!(transaction);
            tokio::select! {
                result = &mut transaction => result,
                _ = shutdown.cancelled() => {
                    cancelled.cancel();
                    transaction.await
                }
            }
        };
        let _ = reply.send(result);
        if shutdown.is_cancelled() {
            while let Ok(queued) = receiver.try_recv() {
                let _ = queued.reply.send(Err(ControlError::cancelled(
                    "settings transaction cancelled during shutdown",
                )));
            }
            if receiver.is_empty() {
                break;
            }
        }
    }
    Ok(())
}

async fn execute_transaction(
    runtime: &RuntimeControlClient,
    committer: &SettingsCommitter,
    work: SettingsTransactionWork,
    cancelled: &CancellationToken,
    observer: Option<&Arc<dyn SettingsTransactionObserver>>,
    mapping_workers: &Arc<Semaphore>,
) -> Result<SettingsTransactionResult, ControlError> {
    let SettingsTransactionWork {
        change,
        expected_revision,
        origin,
    } = work;
    let begun = match runtime
        .request_admission(
            RuntimeRequest::BeginSettingsTransaction {
                expected_revision,
                origin,
            },
            cancelled.clone(),
        )
        .await?
    {
        RuntimeReply::SettingsTransactionBegun(begun) => begun,
        _ => {
            return Err(ControlError::internal(
                "runtime returned an unexpected settings admission reply",
            ));
        }
    };
    let terminal = runtime.reserve_terminal().await?;
    let admitted = execute_admitted_transaction(
        committer,
        change,
        &begun,
        cancelled,
        observer,
        mapping_workers,
    )
    .await;
    let commit = match admitted {
        Ok(commit) => commit,
        Err(error) => {
            runtime
                .request_with_terminal_permit(
                    terminal,
                    RuntimeRequest::AbortSettingsTransaction { token: begun.token },
                )
                .await?;
            return Err(error);
        }
    };
    let reply = runtime
        .request_with_terminal_permit(
            terminal,
            RuntimeRequest::FinalizeSettingsTransaction {
                token: begun.token,
                commit: Box::new(FinalizedSettingsTransaction {
                    settings: commit.settings.clone(),
                    policy: commit.policy,
                    origin,
                    outcome: commit.outcome,
                    affected: commit.affected.clone(),
                    persistence: commit.persistence,
                }),
            },
        )
        .await?;
    let RuntimeReply::SettingsTransactionFinalized(outcome) = reply else {
        return Err(ControlError::internal(
            "runtime returned an unexpected settings finalization reply",
        ));
    };
    if outcome != commit.outcome {
        return Err(ControlError::internal(
            "runtime returned a mismatched settings transaction outcome",
        ));
    }
    Ok(SettingsTransactionResult {
        outcome,
        revision: if outcome == SettingsTransactionOutcome::Committed {
            begun.revision.next()
        } else {
            begun.revision
        },
        config_mode: begun.config_mode,
        persistence: commit.persistence,
        affected: commit.affected,
        settings: commit.settings,
    })
}

struct AdmittedSettingsCommit {
    settings: Arc<AppSettings>,
    policy: Option<RequestPolicy>,
    outcome: SettingsTransactionOutcome,
    affected: MappingObjectRef,
    persistence: PersistenceMode,
}

struct PreparedSettingsCandidate {
    settings: Arc<AppSettings>,
    policy: Option<RequestPolicy>,
    effect: MutationEffect,
    affected: MappingObjectRef,
}

async fn execute_admitted_transaction(
    committer: &SettingsCommitter,
    change: SettingsTransactionChange,
    begun: &crate::control::settings::BeginSettingsTransactionReply,
    cancelled: &CancellationToken,
    observer: Option<&Arc<dyn SettingsTransactionObserver>>,
    mapping_workers: &Arc<Semaphore>,
) -> Result<AdmittedSettingsCommit, ControlError> {
    if cancelled.is_cancelled() {
        return Err(ControlError::cancelled(
            "settings transaction cancelled after admission",
        ));
    }

    let current = Arc::clone(&begun.settings);
    let compile_observer = observer.cloned();
    let prepared_candidate =
        run_bounded_worker(Arc::clone(mapping_workers), cancelled.clone(), move || {
            let (settings, effect, affected) = match change {
                SettingsTransactionChange::Mapping(mutation) => {
                    let (candidate, result) =
                        apply_mapping_mutation_owned(current.as_ref().clone(), mutation).map_err(
                            |error| {
                                ControlError::new(
                                    ControlErrorCode::InvalidArgument,
                                    error.to_string(),
                                    false,
                                    serde_json::json!({
                                        "stage": "validation",
                                        "location": error.location,
                                    }),
                                )
                            },
                        )?;
                    (
                        if result.effect == MutationEffect::Changed {
                            Arc::new(candidate)
                        } else {
                            Arc::clone(&current)
                        },
                        result.effect,
                        result.affected,
                    )
                }
                SettingsTransactionChange::Replace(settings) => {
                    validate_settings(&settings).map_err(|message| {
                        ControlError::new(
                            ControlErrorCode::InvalidArgument,
                            message,
                            false,
                            serde_json::json!({"stage": "validation"}),
                        )
                    })?;
                    let effect = if settings.as_ref() == current.as_ref() {
                        MutationEffect::Unchanged
                    } else {
                        MutationEffect::Changed
                    };
                    (
                        if effect == MutationEffect::Changed {
                            settings
                        } else {
                            Arc::clone(&current)
                        },
                        effect,
                        MappingObjectRef::Proxy,
                    )
                }
            };
            if effect == MutationEffect::Unchanged {
                return Ok(PreparedSettingsCandidate {
                    settings,
                    policy: None,
                    effect,
                    affected,
                });
            }
            if let Some(observer) = compile_observer {
                observer.compile()?;
            }
            let policy = preview::compile_candidate(&settings)?;
            Ok(PreparedSettingsCandidate {
                settings,
                policy: Some(policy),
                effect,
                affected,
            })
        })
        .await??;

    if prepared_candidate.effect == MutationEffect::Unchanged {
        return Ok(AdmittedSettingsCommit {
            settings: prepared_candidate.settings,
            policy: None,
            outcome: SettingsTransactionOutcome::Unchanged,
            affected: prepared_candidate.affected,
            persistence: begun.persistence,
        });
    }

    let prepare_candidate = Arc::clone(&prepared_candidate.settings);
    let prepare_committer = committer.clone();
    let mut prepared =
        run_bounded_worker(Arc::clone(mapping_workers), cancelled.clone(), move || {
            prepare_committer.prepare(prepare_candidate)
        })
        .await?
        .map_err(|error| persistence_error("prepare", error))?;
    let commit_permit = acquire_worker_permit(Arc::clone(mapping_workers), cancelled).await?;
    if cancelled.is_cancelled() {
        return Err(ControlError::cancelled(
            "settings transaction cancelled before commit",
        ));
    }
    prepared
        .begin_commit()
        .map_err(|error| persistence_error("begin_commit", error))?;
    let committed =
        run_worker_to_completion(commit_permit, move || prepared.commit_to_completion())
            .await?
            .map_err(|error| persistence_error("rename", error))?;
    if let Some(observer) = observer.cloned() {
        let _ = tokio::task::spawn_blocking(move || observer.before_finalize()).await;
    }
    Ok(AdmittedSettingsCommit {
        settings: committed.settings,
        policy: prepared_candidate.policy,
        outcome: SettingsTransactionOutcome::Committed,
        affected: prepared_candidate.affected,
        persistence: committed.persistence,
    })
}

async fn acquire_worker_permit(
    admission: Arc<Semaphore>,
    cancelled: &CancellationToken,
) -> Result<OwnedSemaphorePermit, ControlError> {
    tokio::select! {
        permit = admission.acquire_owned() => permit.map_err(|_| {
            ControlError::service_unavailable("settings worker admission is closed")
        }),
        _ = cancelled.cancelled() => {
            Err(ControlError::cancelled("settings worker cancelled before admission"))
        }
    }
}

async fn run_worker_with_permit<T: Send + 'static>(
    permit: OwnedSemaphorePermit,
    cancelled: CancellationToken,
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, ControlError> {
    let task = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        work()
    });
    tokio::select! {
        result = task => result.map_err(|error| ControlError::internal(format!("settings worker failed: {error}"))),
        _ = cancelled.cancelled() => Err(ControlError::cancelled("settings worker cancelled")),
    }
}

async fn run_bounded_worker<T: Send + 'static>(
    admission: Arc<Semaphore>,
    cancelled: CancellationToken,
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, ControlError> {
    let permit = acquire_worker_permit(admission, &cancelled).await?;
    run_worker_with_permit(permit, cancelled, work).await
}

async fn run_worker_to_completion<T: Send + 'static>(
    permit: OwnedSemaphorePermit,
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, ControlError> {
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        work()
    })
    .await
    .map_err(|error| ControlError::internal(format!("settings worker failed: {error}")))
}

fn persistence_error(operation: &'static str, error: std::io::Error) -> ControlError {
    ControlError::new(
        ControlErrorCode::InternalError,
        format!("settings persistence failed: {error}"),
        false,
        serde_json::json!({"stage": "persistence", "operation": operation}),
    )
}

impl fmt::Display for SettingsRevision {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

#[cfg(test)]
mod mapping_read_tests;
#[cfg(all(test, unix))]
mod test_support;
#[cfg(test)]
mod transaction_tests;
