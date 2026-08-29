use crate::{
    control_rpc::{
        framing::{
            ACTIVE_CALL_LIMIT, CallAdmission, CallLease, REQUEST_MAX_BYTES, RESPONSE_MAX_BYTES,
            RESPONSE_SERIALIZATION_BUDGET_BYTES, ResponseSerializationBudget,
            read_request_frame_with_call_lease, serialize_json_frame,
        },
        protocol::{
            ControlError, ControlErrorCode, ControlOperation, ControlRequest, ControlResult,
            DeclaredClient, InstanceScope, ResponseEnvelope,
        },
    },
    instance::InstanceIdentity,
};
use std::{future::Future, sync::Arc, time::Instant};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::UnixStream,
};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug)]
pub(crate) struct ControlCallContext {
    pub(crate) request_id: String,
    pub(crate) declared_client: DeclaredClient,
    pub(crate) deadline: Instant,
}

pub(crate) trait ControlRpcHandler: Clone + Send + Sync + 'static {
    fn handle(
        &self,
        context: ControlCallContext,
        operation: ControlOperation,
        cancelled: CancellationToken,
    ) -> impl Future<Output = Result<ControlResult, ControlError>> + Send;
}

pub(crate) struct ControlRpcServer<H> {
    identity: InstanceIdentity,
    handler: H,
    admission: CallAdmission,
    response_budget: ResponseSerializationBudget,
}

enum DispatchOutcome {
    Response(Result<ControlResult, ControlError>),
    Disconnected,
}

impl<H> ControlRpcServer<H>
where
    H: ControlRpcHandler,
{
    pub(crate) fn new(identity: InstanceIdentity, handler: H) -> Self {
        Self {
            identity,
            handler,
            admission: CallAdmission::new(ACTIVE_CALL_LIMIT),
            response_budget: ResponseSerializationBudget::new(RESPONSE_SERIALIZATION_BUDGET_BYTES),
        }
    }

    pub(crate) async fn serve_connection(
        &self,
        mut stream: UnixStream,
    ) -> Result<(), ControlError> {
        let call_lease = self.admission.acquire().await?;
        validate_peer_identity(&stream)?;
        let envelope = read_request_frame_with_call_lease(
            &mut stream,
            REQUEST_MAX_BYTES,
            Arc::clone(&call_lease),
        )
        .await?;
        let request_id = envelope.request_id.clone();
        let request = match envelope.validate(Instant::now()) {
            Ok(request) => request,
            Err(error) => {
                return self
                    .write_response(
                        &mut stream,
                        ResponseEnvelope::error(request_id, error),
                        call_lease,
                    )
                    .await;
            }
        };

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
            return self
                .write_response(
                    &mut stream,
                    ResponseEnvelope::error(request.request_id, error),
                    call_lease,
                )
                .await;
        }

        let response_request_id = request.request_id.clone();
        let outcome = self.dispatch(&mut stream, request).await;
        let DispatchOutcome::Response(outcome) = outcome else {
            return Ok(());
        };
        let response = match outcome {
            Ok(result) => ResponseEnvelope::success(response_request_id, result),
            Err(error) => ResponseEnvelope::error(response_request_id, error),
        };
        self.write_response(&mut stream, response, call_lease).await
    }

    async fn dispatch(&self, stream: &mut UnixStream, request: ControlRequest) -> DispatchOutcome {
        let context = ControlCallContext {
            request_id: request.request_id,
            declared_client: request.client,
            deadline: request.deadline,
        };
        let cancelled = CancellationToken::new();
        let handler_cancelled = cancelled.clone();
        let handler = self
            .handler
            .handle(context, request.operation, handler_cancelled);
        tokio::pin!(handler);
        let mut disconnect_probe = [0_u8; 1];
        let deadline = tokio::time::Instant::from_std(request.deadline);

        tokio::select! {
            result = &mut handler => DispatchOutcome::Response(result),
            _read = stream.read(&mut disconnect_probe) => {
                cancelled.cancel();
                DispatchOutcome::Disconnected
            }
            _ = tokio::time::sleep_until(deadline) => {
                cancelled.cancel();
                DispatchOutcome::Response(Err(ControlError::instance_unavailable(
                    "private RPC operation deadline elapsed",
                )))
            }
        }
    }

    async fn write_response(
        &self,
        stream: &mut UnixStream,
        response: ResponseEnvelope,
        call_lease: Arc<CallLease>,
    ) -> Result<(), ControlError> {
        let frame = serialize_json_frame(
            response,
            RESPONSE_MAX_BYTES,
            self.response_budget.clone(),
            call_lease,
        )
        .await?;
        stream
            .write_all(frame.as_bytes())
            .await
            .map_err(|_| ControlError::instance_unavailable("private RPC response write failed"))?;
        stream.shutdown().await.map_err(|_| {
            ControlError::instance_unavailable("private RPC connection close failed")
        })?;
        Ok(())
    }
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
