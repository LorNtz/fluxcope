use crate::{
    control_rpc::{
        framing::{REQUEST_MAX_BYTES, RESPONSE_MAX_BYTES, read_json_frame},
        protocol::{
            ControlError, ControlOperation, ControlResult, DeclaredClient, InstanceScope,
            RequestEnvelope, ResponseEnvelope,
        },
    },
    instance_registry::InstanceDescriptor,
};
use std::{
    sync::atomic::{AtomicU64, Ordering},
    time::Instant,
};
use tokio::{io::AsyncWriteExt, net::UnixStream};
use tokio_util::sync::CancellationToken;

static NEXT_REQUEST_ID: AtomicU64 = AtomicU64::new(1);

pub(crate) struct ControlRpcClient;

impl ControlRpcClient {
    pub(crate) async fn call(
        descriptor: &InstanceDescriptor,
        operation: ControlOperation,
        deadline: Instant,
        client: DeclaredClient,
        cancelled: CancellationToken,
    ) -> Result<ControlResult, ControlError> {
        let request_id = next_request_id();
        let remaining = deadline.saturating_duration_since(Instant::now());
        let deadline_ms = u64::try_from(remaining.as_millis()).unwrap_or(u64::MAX);
        let request = RequestEnvelope::new(
            request_id.clone(),
            descriptor.run_id().clone(),
            deadline_ms,
            client,
            operation,
        )?;
        let expected_scope = InstanceScope {
            proxy_endpoint: descriptor.proxy_endpoint(),
            run_id: descriptor.run_id().clone(),
        };
        let socket_path = descriptor.socket_path().to_path_buf();
        let call = async move {
            let payload = serde_json::to_vec(&request).map_err(|error| {
                ControlError::invalid_argument(format!(
                    "failed to serialize private RPC request: {error}"
                ))
            })?;
            if payload.len() > REQUEST_MAX_BYTES {
                return Err(ControlError::frame_too_large(REQUEST_MAX_BYTES));
            }
            let length = u32::try_from(payload.len())
                .map_err(|_| ControlError::frame_too_large(REQUEST_MAX_BYTES))?;
            let mut stream = UnixStream::connect(&socket_path)
                .await
                .map_err(|_| ControlError::instance_unavailable("private RPC connection failed"))?;
            stream.write_all(&length.to_be_bytes()).await.map_err(|_| {
                ControlError::instance_unavailable("private RPC request write failed")
            })?;
            stream.write_all(&payload).await.map_err(|_| {
                ControlError::instance_unavailable("private RPC request write failed")
            })?;
            let response =
                read_json_frame::<ResponseEnvelope, _>(&mut stream, RESPONSE_MAX_BYTES).await?;
            response.validate(&request_id, &expected_scope)
        };

        tokio::select! {
            biased;
            () = cancelled.cancelled() => {
                Err(ControlError::cancelled("private RPC call cancelled"))
            }
            result = tokio::time::timeout_at(tokio::time::Instant::from_std(deadline), call) => {
                result
                    .map_err(|_| ControlError::deadline_exceeded("private RPC deadline elapsed"))?
            }
        }
    }
}

fn next_request_id() -> String {
    let sequence = NEXT_REQUEST_ID.fetch_add(1, Ordering::Relaxed);
    format!("{}-{sequence}", std::process::id())
}

#[cfg(test)]
mod tests;
