use crate::{
    control_rpc::{
        framing::{
            ACTIVE_CALL_LIMIT, REQUEST_MAX_BYTES, RESPONSE_MAX_BYTES, encode_json_frame,
            read_json_frame_cancellable,
        },
        protocol::{
            ControlError, ControlOperation, ControlResult, DeclaredClient, InstanceScope,
            LocalTransportCause, OutboundRequestEnvelope, ResponseEnvelope,
        },
    },
    instance_registry::InstanceDescriptor,
};
use std::{
    io,
    sync::{
        Arc, LazyLock,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Instant,
};
use tokio::{io::AsyncWriteExt, net::UnixStream, sync::Semaphore};
use tokio_util::sync::CancellationToken;

static NEXT_REQUEST_ID: AtomicU64 = AtomicU64::new(1);
static REQUEST_SERIALIZATION_ADMISSION: LazyLock<Arc<Semaphore>> =
    LazyLock::new(|| Arc::new(Semaphore::new(ACTIVE_CALL_LIMIT)));

pub(crate) struct ControlRpcClient;

struct CancelOnDrop(Arc<AtomicBool>);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

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
        let expected_kind = operation.kind();
        let request_run_id = descriptor.run_id().clone();
        let expected_scope = InstanceScope {
            proxy_endpoint: descriptor.proxy_endpoint(),
            run_id: descriptor.run_id().clone(),
        };
        let socket_path = descriptor.socket_path().to_path_buf();
        let parse_cancelled = Arc::new(AtomicBool::new(false));
        let call = async move {
            let _parse_cancellation = CancelOnDrop(parse_cancelled.clone());
            let serialization_permit = Arc::clone(&REQUEST_SERIALIZATION_ADMISSION)
                .acquire_owned()
                .await
                .map_err(|_| {
                    ControlError::service_unavailable(
                        "private RPC request serialization admission is closed",
                    )
                })?;
            let encoding_request_id = request_id.clone();
            let worker = tokio::task::spawn_blocking(move || {
                let _serialization_permit = serialization_permit;
                let request = OutboundRequestEnvelope::new(
                    &encoding_request_id,
                    &request_run_id,
                    deadline_ms,
                    &client,
                    &operation,
                );
                encode_json_frame(&request, REQUEST_MAX_BYTES)
            });
            let payload = worker.await.map_err(|_| {
                ControlError::service_unavailable("private RPC request serializer worker failed")
            })??;
            let mut stream = UnixStream::connect(&socket_path)
                .await
                .map_err(connect_failure)?;
            stream.write_all(&payload).await.map_err(|_| {
                ControlError::instance_unavailable("private RPC request write failed")
            })?;
            let response = read_json_frame_cancellable::<ResponseEnvelope, _>(
                &mut stream,
                RESPONSE_MAX_BYTES,
                parse_cancelled,
            )
            .await?;
            response.validate(&request_id, &expected_scope, expected_kind)
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

pub(crate) fn connect_failure(error: io::Error) -> ControlError {
    if matches!(
        error.kind(),
        io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
    ) {
        return ControlError::instance_unavailable("private RPC connection failed")
            .with_local_transport_cause(LocalTransportCause::DefinitiveStaleConnect);
    }
    if error.kind() == io::ErrorKind::OutOfMemory || matches!(error.raw_os_error(), Some(23 | 24)) {
        return ControlError::service_unavailable(
            "private RPC connection failed due to local resource exhaustion",
        );
    }
    ControlError::instance_unavailable("private RPC connection failed")
}

fn next_request_id() -> String {
    let sequence = NEXT_REQUEST_ID.fetch_add(1, Ordering::Relaxed);
    format!("{}-{sequence}", std::process::id())
}

#[cfg(test)]
mod tests;
