use crate::{
    control::audit::MAX_AUDIT_TARGET_NAME_BYTES,
    control_rpc::{
        framing::{
            ACTIVE_CALL_LIMIT, CallAdmission, CallLease, REQUEST_MAX_BYTES, RESPONSE_MAX_BYTES,
            RESPONSE_SERIALIZATION_BUDGET_BYTES, ResponseSerializationBudget,
            read_validated_request_frame_with_call_lease, serialize_json_frame_until,
        },
        protocol::{
            ControlError, ControlErrorCode, ControlOperation, ControlOperationKind, ControlResult,
            DeclaredClient, InstanceScope, MutationAuditOperationKind, ResponseEnvelope,
        },
    },
    instance::InstanceIdentity,
    settings::{
        ConfigMode, PersistenceMode,
        mapping_ops::{MappingMutation, MappingObjectRef},
    },
};
use std::{
    future::Future,
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{
        UnixStream,
        unix::{OwnedReadHalf, OwnedWriteHalf},
    },
};
use tokio_util::sync::CancellationToken;

const REQUEST_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Debug)]
pub(crate) struct ControlCallContext {
    pub(crate) request_id: String,
    pub(crate) declared_client: DeclaredClient,
    pub(crate) deadline: Instant,
}

#[derive(Clone, Debug)]
pub(crate) struct BoundedAuditTarget {
    pub(crate) target: MappingObjectRef,
    pub(crate) truncated: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct ControlMutationAudit {
    pub(crate) operation: MutationAuditOperationKind,
    pub(crate) prior_settings_revision: Option<u64>,
    pub(crate) target: Option<BoundedAuditTarget>,
}

#[derive(Clone, Debug)]
pub(crate) enum ControlCallOutcome {
    Success {
        instance: Option<InstanceScope>,
        config_mode: Option<ConfigMode>,
        persistence: Option<PersistenceMode>,
        settings_revision: Option<u64>,
        affected: Option<BoundedAuditTarget>,
    },
    Error {
        code: ControlErrorCode,
        retryable: bool,
    },
}

#[derive(Clone, Debug)]
pub(crate) struct ControlCallAudit {
    pub(crate) client: DeclaredClient,
    pub(crate) operation: ControlOperationKind,
    pub(crate) mutation: Option<ControlMutationAudit>,
    pub(crate) outcome: ControlCallOutcome,
    pub(crate) duration: Duration,
    pub(crate) response_bytes: usize,
    pub(crate) response_delivery_failed: bool,
    pub(crate) completed_unix_seconds: u64,
}

pub(crate) trait ControlRpcHandler: Clone + Send + Sync + 'static {
    fn handle(
        &self,
        context: ControlCallContext,
        operation: ControlOperation,
        cancelled: CancellationToken,
    ) -> impl Future<Output = Result<ControlResult, ControlError>> + Send;

    fn record_call(&self, _call: ControlCallAudit) {}
}

pub(crate) struct ControlRpcServer<H> {
    identity: InstanceIdentity,
    handler: H,
    admission: CallAdmission,
    response_budget: ResponseSerializationBudget,
}

enum DispatchOutcome {
    Response(Box<Result<ControlResult, ControlError>>),
    Disconnected,
}

impl<H> ControlRpcServer<H>
where
    H: ControlRpcHandler,
{
    pub(crate) fn new(identity: InstanceIdentity, handler: H) -> Self {
        Self::with_response_budget(
            identity,
            handler,
            ResponseSerializationBudget::new(RESPONSE_SERIALIZATION_BUDGET_BYTES),
        )
    }

    fn with_response_budget(
        identity: InstanceIdentity,
        handler: H,
        response_budget: ResponseSerializationBudget,
    ) -> Self {
        Self {
            identity,
            handler,
            admission: CallAdmission::new(ACTIVE_CALL_LIMIT),
            response_budget,
        }
    }

    #[cfg(test)]
    pub(crate) fn new_with_response_budget(
        identity: InstanceIdentity,
        handler: H,
        response_budget: ResponseSerializationBudget,
    ) -> Self {
        Self::with_response_budget(identity, handler, response_budget)
    }

    #[cfg(test)]
    pub(crate) async fn serve_connection(&self, stream: UnixStream) -> Result<(), ControlError> {
        self.serve_connection_until(stream, CancellationToken::new())
            .await
    }

    pub(crate) async fn serve_connection_until(
        &self,
        stream: UnixStream,
        cancelled: CancellationToken,
    ) -> Result<(), ControlError> {
        let call_lease = tokio::select! {
            lease = self.admission.acquire() => lease?,
            _ = cancelled.cancelled() => return Ok(()),
        };
        validate_peer_identity(&stream)?;
        let (mut reader, mut writer) = stream.into_split();
        let handshake_deadline = tokio::time::Instant::now() + REQUEST_HANDSHAKE_TIMEOUT;
        let parsed = tokio::select! {
            biased;
            _ = cancelled.cancelled() => return Ok(()),
            _ = tokio::time::sleep_until(handshake_deadline) => {
                return Err(ControlError::deadline_exceeded(
                    "private RPC request handshake deadline elapsed",
                ));
            }
            parsed = read_validated_request_frame_with_call_lease(
                &mut reader,
                REQUEST_MAX_BYTES,
                Arc::clone(&call_lease),
                Instant::now(),
            ) => parsed?,
        };
        let request = match parsed.request {
            Ok(request) => request,
            Err(error) => {
                return self
                    .write_response(
                        &mut reader,
                        &mut writer,
                        ResponseEnvelope::error(parsed.request_id, error),
                        call_lease,
                        parsed.deadline,
                        cancelled,
                    )
                    .await
                    .map(|_| ());
            }
        };
        let deadline = request.deadline;
        let operation_kind = request.operation.kind();
        let mutation = mutation_audit(&request.operation);
        let mutation_must_reach_terminal = mutation.is_some();
        let context = ControlCallContext {
            request_id: request.request_id,
            declared_client: request.client,
            deadline,
        };
        let started = Instant::now();

        if request.run_id != *self.identity.run_id() {
            let authoritative = InstanceScope {
                proxy_endpoint: self.identity.proxy_endpoint(),
                run_id: self.identity.run_id().clone(),
            };
            let error = ControlError::new(
                ControlErrorCode::InstanceGenerationConflict,
                "private RPC request targets a stale instance generation",
                false,
                serde_json::json!({"authoritative_identity": authoritative}),
            );
            let written = self
                .write_response(
                    &mut reader,
                    &mut writer,
                    ResponseEnvelope::error(context.request_id.clone(), error),
                    call_lease,
                    deadline,
                    cancelled,
                )
                .await;
            let domain_outcome = ControlCallOutcome::Error {
                code: ControlErrorCode::InstanceGenerationConflict,
                retryable: false,
            };
            let (response_bytes, outcome) = audit_outcome_after_delivery(
                domain_outcome,
                mutation_must_reach_terminal,
                &written,
            );
            let response_delivery_failed = written.is_err();
            self.handler.record_call(ControlCallAudit {
                client: context.declared_client,
                operation: operation_kind,
                mutation,
                outcome,
                duration: started.elapsed(),
                response_delivery_failed,
                response_bytes,
                completed_unix_seconds: unix_seconds_now(),
            });
            return Ok(());
        }
        let outcome = self
            .dispatch(
                &mut reader,
                context.clone(),
                request.operation,
                cancelled.clone(),
                mutation_must_reach_terminal,
            )
            .await;
        let DispatchOutcome::Response(outcome) = outcome else {
            self.handler.record_call(ControlCallAudit {
                client: context.declared_client,
                operation: operation_kind,
                mutation,
                outcome: ControlCallOutcome::Error {
                    code: ControlErrorCode::Cancelled,
                    retryable: true,
                },
                duration: started.elapsed(),
                response_delivery_failed: false,
                response_bytes: 0,
                completed_unix_seconds: unix_seconds_now(),
            });
            return Ok(());
        };
        let mut outcome = *outcome;
        if let Ok(ControlResult::GetStatus { private_rpc, .. }) = &mut outcome {
            let (active, maximum_active) = self.admission.snapshot();
            private_rpc.active = active;
            private_rpc.maximum_active = maximum_active;
        }
        let audit_outcome = ControlCallOutcome::from_result(&outcome);
        let response = match outcome {
            Ok(result) => ResponseEnvelope::success(context.request_id.clone(), result),
            Err(error) => ResponseEnvelope::error(context.request_id.clone(), error),
        };
        let written = self
            .write_response(
                &mut reader,
                &mut writer,
                response,
                call_lease,
                deadline,
                cancelled,
            )
            .await;
        let response_delivery_failed = written.is_err();
        let (response_bytes, audit_outcome) =
            audit_outcome_after_delivery(audit_outcome, mutation.is_some(), &written);
        self.handler.record_call(ControlCallAudit {
            client: context.declared_client,
            operation: operation_kind,
            mutation,
            outcome: audit_outcome,
            duration: started.elapsed(),
            response_bytes,
            completed_unix_seconds: unix_seconds_now(),
            response_delivery_failed,
        });
        Ok(())
    }

    async fn dispatch(
        &self,
        reader: &mut OwnedReadHalf,
        context: ControlCallContext,
        operation: ControlOperation,
        cancelled: CancellationToken,
        mutation_must_reach_terminal: bool,
    ) -> DispatchOutcome {
        let deadline = tokio::time::Instant::from_std(context.deadline);
        let handler = self.handler.handle(context, operation, cancelled.clone());
        tokio::pin!(handler);
        let mut disconnect_probe = [0_u8; 1];

        tokio::select! {
            result = &mut handler => DispatchOutcome::Response(Box::new(result)),
            _read = reader.read(&mut disconnect_probe) => {
                cancelled.cancel();
                if mutation_must_reach_terminal {
                    DispatchOutcome::Response(Box::new(handler.await))
                } else {
                    DispatchOutcome::Disconnected
                }
            }
            _ = cancelled.cancelled() => {
                if mutation_must_reach_terminal {
                    DispatchOutcome::Response(Box::new(handler.await))
                } else {
                    DispatchOutcome::Disconnected
                }
            }
            _ = tokio::time::sleep_until(deadline) => {
                cancelled.cancel();
                if mutation_must_reach_terminal {
                    DispatchOutcome::Response(Box::new(handler.await))
                } else {
                    DispatchOutcome::Response(Box::new(Err(ControlError::deadline_exceeded(
                        "private RPC operation deadline elapsed",
                    ))))
                }
            }
        }
    }

    async fn write_response(
        &self,
        reader: &mut OwnedReadHalf,
        writer: &mut OwnedWriteHalf,
        response: ResponseEnvelope,
        call_lease: Arc<CallLease>,
        deadline: Instant,
        cancelled: CancellationToken,
    ) -> Result<usize, ControlError> {
        let serialization = serialize_json_frame_until(
            response,
            RESPONSE_MAX_BYTES,
            self.response_budget.clone(),
            call_lease,
            deadline,
            cancelled.clone(),
        );
        tokio::pin!(serialization);
        let mut disconnect_probe = [0_u8; 1];
        let frame = tokio::select! {
            result = &mut serialization => result?,
            _read = reader.read(&mut disconnect_probe) => {
                cancelled.cancel();
                return Err(ControlError::cancelled(
                    "private RPC client disconnected before response serialization completed",
                ));
            }
        };
        let response_bytes = frame.as_bytes().len();

        let write = writer.write_all(frame.as_bytes());
        tokio::pin!(write);
        let deadline = tokio::time::Instant::from_std(deadline);
        tokio::select! {
            result = &mut write => {
                result.map_err(|_| {
                    ControlError::instance_unavailable("private RPC response write failed")
                })?;
                Ok(response_bytes)
            }
            _read = reader.read(&mut disconnect_probe) => {
                cancelled.cancel();
                Err(ControlError::cancelled(
                    "private RPC client disconnected before response write completed",
                ))
            }
            _ = tokio::time::sleep_until(deadline) => {
                cancelled.cancel();
                Err(ControlError::deadline_exceeded(
                    "private RPC response deadline elapsed",
                ))
            }
        }
    }
}
fn audit_outcome_after_delivery(
    domain_outcome: ControlCallOutcome,
    is_mutation: bool,
    delivery: &Result<usize, ControlError>,
) -> (usize, ControlCallOutcome) {
    match delivery {
        Ok(bytes) => (*bytes, domain_outcome),
        Err(_) if is_mutation => (0, domain_outcome),
        Err(error) => (
            0,
            ControlCallOutcome::Error {
                code: error.code(),
                retryable: error.retryable(),
            },
        ),
    }
}

fn mutation_audit(operation: &ControlOperation) -> Option<ControlMutationAudit> {
    match operation {
        ControlOperation::SetRecordingEnabled { .. } => Some(ControlMutationAudit {
            operation: MutationAuditOperationKind::SetRecordingEnabled,
            prior_settings_revision: None,
            target: None,
        }),
        ControlOperation::MutateMapping {
            expected_revision,
            mutation,
        } => {
            let (target, truncated) = mutation.audit_target_bounded(MAX_AUDIT_TARGET_NAME_BYTES);
            Some(ControlMutationAudit {
                operation: mapping_mutation_audit_kind(mutation),
                prior_settings_revision: Some(expected_revision.get()),
                target: Some(BoundedAuditTarget { target, truncated }),
            })
        }
        _ => None,
    }
}

fn mapping_mutation_audit_kind(mutation: &MappingMutation) -> MutationAuditOperationKind {
    match mutation {
        MappingMutation::CreatePreset { .. } => MutationAuditOperationKind::CreatePreset,
        MappingMutation::RenamePreset { .. } => MutationAuditOperationKind::RenamePreset,
        MappingMutation::DeletePreset { .. } => MutationAuditOperationKind::DeletePreset,
        MappingMutation::SetActivePreset { .. } => MutationAuditOperationKind::SetActivePreset,
        MappingMutation::SetGlobalEnabled { .. } | MappingMutation::SetTableEnabled { .. } => {
            MutationAuditOperationKind::SetMappingGate
        }
        MappingMutation::AppendRemoteRule { .. }
        | MappingMutation::InsertRemoteRule { .. }
        | MappingMutation::AppendLocalRule { .. }
        | MappingMutation::InsertLocalRule { .. } => MutationAuditOperationKind::CreateMappingRule,
        MappingMutation::UpdateRemoteRule { .. } | MappingMutation::UpdateLocalRule { .. } => {
            MutationAuditOperationKind::UpdateMappingRule
        }
        MappingMutation::DeleteRule { .. } => MutationAuditOperationKind::DeleteMappingRule,
        MappingMutation::MoveRule { .. } => MutationAuditOperationKind::MoveMappingRule,
        MappingMutation::SetRuleEnabled { .. } => MutationAuditOperationKind::SetMappingRuleEnabled,
    }
}

impl ControlCallOutcome {
    fn from_result(result: &Result<ControlResult, ControlError>) -> Self {
        let Ok(result) = result else {
            let error = result.as_ref().expect_err("checked error result");
            return Self::Error {
                code: error.code(),
                retryable: error.retryable(),
            };
        };
        let instance = Some(result.instance_scope().clone());
        let (config_mode, persistence, settings_revision, affected) = match result {
            ControlResult::DescribeInstance {
                config_mode,
                persistence,
                settings_revision,
                ..
            }
            | ControlResult::GetStatus {
                config_mode,
                persistence,
                settings_revision,
                ..
            } => (
                Some(*config_mode),
                Some(*persistence),
                Some(*settings_revision),
                None,
            ),
            ControlResult::GetMappingSettings {
                config_mode,
                persistence,
                settings_revision,
                ..
            }
            | ControlResult::ValidateMappingSettings {
                config_mode,
                persistence,
                settings_revision,
                ..
            }
            | ControlResult::PreviewMappingMutation {
                config_mode,
                persistence,
                settings_revision,
                ..
            }
            | ControlResult::ExplainMapping {
                config_mode,
                persistence,
                settings_revision,
                ..
            } => (
                Some(*config_mode),
                Some(*persistence),
                Some(settings_revision.get()),
                None,
            ),
            ControlResult::MutateMapping {
                config_mode,
                persistence,
                settings_revision,
                affected,
                ..
            } => {
                let (target, truncated) = affected.bounded_clone(MAX_AUDIT_TARGET_NAME_BYTES);
                (
                    Some(*config_mode),
                    Some(*persistence),
                    Some(settings_revision.get()),
                    Some(BoundedAuditTarget { target, truncated }),
                )
            }
            _ => (None, None, None, None),
        };
        Self::Success {
            instance,
            config_mode,
            persistence,
            settings_revision,
            affected,
        }
    }
}

fn unix_seconds_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub(crate) fn validate_peer_identity(stream: &UnixStream) -> Result<(), ControlError> {
    let credentials = stream.peer_cred().map_err(|_| {
        ControlError::instance_unavailable("private RPC peer credentials unavailable")
    })?;
    validate_peer_uid(credentials.uid(), rustix::process::geteuid().as_raw())
}

pub(crate) fn validate_peer_uid(actual: u32, expected: u32) -> Result<(), ControlError> {
    if actual != expected {
        return Err(ControlError::instance_unavailable(
            "private RPC peer effective UID does not match",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod capture_wait_tests;
