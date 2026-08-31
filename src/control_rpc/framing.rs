use crate::control_rpc::protocol::{
    ControlError, ControlRequest, RequestEnvelope, strict_from_slice,
};
use serde::{Serialize, de::DeserializeOwned};
use std::{
    fmt,
    io::{self, Write},
    sync::Arc,
    time::Instant,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    sync::{OwnedSemaphorePermit, Semaphore},
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;

pub(crate) const REQUEST_MAX_BYTES: usize = 1024 * 1024;
pub(crate) const RESPONSE_MAX_BYTES: usize = 8 * 1024 * 1024;
pub(crate) const RESPONSE_SERIALIZATION_BUDGET_BYTES: usize = 32 * 1024 * 1024;
pub(crate) const ACTIVE_CALL_LIMIT: usize = 32;

#[derive(Clone, Debug)]
pub(crate) struct CallAdmission {
    permits: Arc<Semaphore>,
}

#[derive(Debug)]
pub(crate) struct CallLease {
    _permit: OwnedSemaphorePermit,
}

impl CallAdmission {
    pub(crate) fn new(limit: usize) -> Self {
        Self {
            permits: Arc::new(Semaphore::new(limit)),
        }
    }

    pub(crate) async fn acquire(&self) -> Result<Arc<CallLease>, ControlError> {
        let permit = Arc::clone(&self.permits)
            .acquire_owned()
            .await
            .map_err(|_| ControlError::instance_unavailable("private RPC admission is closed"))?;
        Ok(Arc::new(CallLease { _permit: permit }))
    }

    #[cfg(test)]
    pub(crate) fn try_acquire(&self) -> Result<Arc<CallLease>, ControlError> {
        let permit = Arc::clone(&self.permits)
            .try_acquire_owned()
            .map_err(|_| ControlError::instance_unavailable("private RPC call limit reached"))?;
        Ok(Arc::new(CallLease { _permit: permit }))
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ResponseSerializationBudget {
    permits: Arc<Semaphore>,
}

impl ResponseSerializationBudget {
    pub(crate) fn new(max_bytes: usize) -> Self {
        Self {
            permits: Arc::new(Semaphore::new(max_bytes)),
        }
    }

    async fn acquire(&self, bytes: usize) -> Result<OwnedSemaphorePermit, ControlError> {
        let bytes = u32::try_from(bytes).map_err(|_| {
            ControlError::instance_unavailable("private RPC response budget is invalid")
        })?;
        Arc::clone(&self.permits)
            .acquire_many_owned(bytes)
            .await
            .map_err(|_| {
                ControlError::instance_unavailable("private RPC response budget is closed")
            })
    }
}

#[derive(Debug)]
pub(crate) struct EncodedFrame {
    bytes: Vec<u8>,
    _response_lease: OwnedSemaphorePermit,
    _call_lease: Arc<CallLease>,
    #[cfg(test)]
    allocation_growth_count: usize,
}

impl EncodedFrame {
    pub(crate) fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    #[cfg(test)]
    pub(crate) fn allocation_growth_count(&self) -> usize {
        self.allocation_growth_count
    }
}

#[derive(Debug)]
pub(crate) struct ValidatedRequestFrame {
    pub(crate) request_id: String,
    pub(crate) deadline: Instant,
    pub(crate) request: Result<ControlRequest, ControlError>,
}

pub(crate) async fn read_json_frame<T, R>(
    reader: &mut R,
    max_bytes: usize,
) -> Result<T, ControlError>
where
    T: DeserializeOwned + Send + 'static,
    R: AsyncRead + Unpin,
{
    let payload = read_frame_payload(reader, max_bytes).await?;
    parse_json_payload(payload).await
}

async fn read_frame_payload<R>(reader: &mut R, max_bytes: usize) -> Result<Vec<u8>, ControlError>
where
    R: AsyncRead + Unpin,
{
    let mut prefix = [0_u8; 4];
    reader
        .read_exact(&mut prefix)
        .await
        .map_err(|_| ControlError::invalid_argument("private RPC frame prefix is truncated"))?;
    let declared = u32::from_be_bytes(prefix) as usize;
    if declared > max_bytes {
        return Err(ControlError::frame_too_large(max_bytes));
    }

    let mut payload = Vec::new();
    payload
        .try_reserve_exact(declared)
        .map_err(|_| ControlError::instance_unavailable("private RPC frame allocation failed"))?;
    payload.resize(declared, 0);
    reader
        .read_exact(&mut payload)
        .await
        .map_err(|_| ControlError::invalid_argument("private RPC frame payload is truncated"))?;
    Ok(payload)
}

async fn parse_json_payload<T>(payload: Vec<u8>) -> Result<T, ControlError>
where
    T: DeserializeOwned + Send + 'static,
{
    tokio::task::spawn_blocking(move || strict_from_slice::<T>(&payload))
        .await
        .map_err(|_| ControlError::instance_unavailable("private RPC parser worker failed"))?
}

pub(crate) async fn read_validated_request_frame_with_call_lease<R>(
    reader: &mut R,
    max_bytes: usize,
    lease: Arc<CallLease>,
    received_at: Instant,
) -> Result<ValidatedRequestFrame, ControlError>
where
    R: AsyncRead + Unpin,
{
    read_validated_request_frame_inner(reader, max_bytes, Some(lease), received_at).await
}

async fn read_validated_request_frame_inner<R>(
    reader: &mut R,
    max_bytes: usize,
    lease: Option<Arc<CallLease>>,
    received_at: Instant,
) -> Result<ValidatedRequestFrame, ControlError>
where
    R: AsyncRead + Unpin,
{
    let payload = read_frame_payload(reader, max_bytes).await?;
    tokio::task::spawn_blocking(move || {
        let _lease = lease;
        let envelope = strict_from_slice::<RequestEnvelope>(&payload)?;
        let request_id = envelope.request_id.clone();
        let deadline = envelope.clamped_deadline(received_at);
        let request = envelope.validate(received_at);
        Ok(ValidatedRequestFrame {
            request_id,
            deadline,
            request,
        })
    })
    .await
    .map_err(|_| ControlError::instance_unavailable("private RPC parser worker failed"))?
}

#[cfg(test)]
pub(crate) async fn run_blocking_with_call_lease<F, T>(
    lease: Arc<CallLease>,
    work: F,
) -> Result<T, ControlError>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(move || {
        let _lease = lease;
        work()
    })
    .await
    .map_err(|_| ControlError::instance_unavailable("private RPC blocking worker failed"))
}

type SerializationWorkerOutput = (Vec<u8>, OwnedSemaphorePermit, usize);

pub(crate) fn encode_json_frame<T>(
    value: &T,
    max_payload_bytes: usize,
) -> Result<Vec<u8>, ControlError>
where
    T: Serialize,
{
    let mut writer = CappedFrameWriter::new(max_payload_bytes)?;
    let serialization = serde_json::to_writer(&mut writer, value);
    if writer.overflowed {
        return Err(ControlError::frame_too_large(max_payload_bytes));
    }
    serialization.map_err(|error| ControlError::invalid_argument(error.to_string()))?;
    writer.finish().map(|(bytes, _)| bytes)
}

pub(crate) fn ensure_json_payload_within_limit<T>(
    value: &T,
    max_payload_bytes: usize,
) -> Result<(), ControlError>
where
    T: Serialize,
{
    let mut writer = CappedCountingWriter {
        payload_bytes: 0,
        max_payload_bytes,
        overflowed: false,
    };
    let serialization = serde_json::to_writer(&mut writer, value);
    if writer.overflowed {
        return Err(ControlError::frame_too_large(max_payload_bytes));
    }
    serialization.map_err(|error| ControlError::invalid_argument(error.to_string()))
}

#[cfg(test)]
pub(crate) async fn serialize_json_frame<T>(
    value: T,
    max_bytes: usize,
    budget: ResponseSerializationBudget,
    call_lease: Arc<CallLease>,
) -> Result<EncodedFrame, ControlError>
where
    T: Serialize + Send + 'static,
{
    let pessimistic_lease = budget.acquire(max_bytes).await?;
    let worker =
        spawn_serialization_worker(value, max_bytes, pessimistic_lease, Arc::clone(&call_lease));
    let output = worker.await.map_err(|_| {
        ControlError::instance_unavailable("private RPC serializer worker failed")
    })??;
    finish_serialized_frame(output, call_lease)
}

pub(crate) async fn serialize_json_frame_until<T>(
    value: T,
    max_bytes: usize,
    budget: ResponseSerializationBudget,
    call_lease: Arc<CallLease>,
    deadline: Instant,
    cancelled: CancellationToken,
) -> Result<EncodedFrame, ControlError>
where
    T: Serialize + Send + 'static,
{
    let deadline = tokio::time::Instant::from_std(deadline);
    let pessimistic_lease = tokio::select! {
        biased;
        _ = cancelled.cancelled() => {
            return Err(ControlError::instance_unavailable(
                "private RPC response serialization was cancelled",
            ));
        }
        _ = tokio::time::sleep_until(deadline) => {
            return Err(ControlError::instance_unavailable(
                "private RPC operation deadline elapsed",
            ));
        }
        lease = budget.acquire(max_bytes) => lease?,
    };
    let mut worker =
        spawn_serialization_worker(value, max_bytes, pessimistic_lease, Arc::clone(&call_lease));
    let output = tokio::select! {
        biased;
        _ = cancelled.cancelled() => {
            return Err(ControlError::instance_unavailable(
                "private RPC response serialization was cancelled",
            ));
        }
        _ = tokio::time::sleep_until(deadline) => {
            return Err(ControlError::instance_unavailable(
                "private RPC operation deadline elapsed",
            ));
        }
        output = &mut worker => {
            output.map_err(|_| {
                ControlError::instance_unavailable("private RPC serializer worker failed")
            })??
        }
    };
    finish_serialized_frame(output, call_lease)
}

fn spawn_serialization_worker<T>(
    value: T,
    max_bytes: usize,
    pessimistic_lease: OwnedSemaphorePermit,
    worker_call_lease: Arc<CallLease>,
) -> JoinHandle<Result<SerializationWorkerOutput, ControlError>>
where
    T: Serialize + Send + 'static,
{
    tokio::task::spawn_blocking(move || {
        let _worker_call_lease = worker_call_lease;
        let mut writer = CappedFrameWriter::new(max_bytes)?;
        let serialization = serde_json::to_writer(&mut writer, &value);
        if writer.overflowed {
            return Err(ControlError::frame_too_large(max_bytes));
        }
        serialization.map_err(|error| ControlError::invalid_argument(error.to_string()))?;
        let (serialized, allocation_growth_count) = writer.finish()?;
        Ok((serialized, pessimistic_lease, allocation_growth_count))
    })
}

fn finish_serialized_frame(
    output: SerializationWorkerOutput,
    call_lease: Arc<CallLease>,
) -> Result<EncodedFrame, ControlError> {
    let (serialized, mut pessimistic_lease, _allocation_growth_count) = output;
    let payload_allocation = serialized.capacity().saturating_sub(4);
    let actual_lease = pessimistic_lease.split(payload_allocation).ok_or_else(|| {
        ControlError::instance_unavailable("private RPC response lease accounting failed")
    })?;
    drop(pessimistic_lease);
    Ok(EncodedFrame {
        bytes: serialized,
        _response_lease: actual_lease,
        _call_lease: call_lease,
        #[cfg(test)]
        allocation_growth_count: _allocation_growth_count,
    })
}

struct CappedCountingWriter {
    payload_bytes: usize,
    max_payload_bytes: usize,
    overflowed: bool,
}

impl Write for CappedCountingWriter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        let Some(next_payload_bytes) = self.payload_bytes.checked_add(buffer.len()) else {
            self.overflowed = true;
            return Err(io::Error::new(io::ErrorKind::WriteZero, FrameLimitExceeded));
        };
        if next_payload_bytes > self.max_payload_bytes {
            self.overflowed = true;
            return Err(io::Error::new(io::ErrorKind::WriteZero, FrameLimitExceeded));
        }
        self.payload_bytes = next_payload_bytes;
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

struct CappedFrameWriter {
    bytes: Vec<u8>,
    max_payload_bytes: usize,
    overflowed: bool,
    allocation_growth_count: usize,
}

impl CappedFrameWriter {
    fn new(max_payload_bytes: usize) -> Result<Self, ControlError> {
        let mut bytes = Vec::new();
        bytes.try_reserve_exact(4).map_err(|_| {
            ControlError::instance_unavailable("private RPC frame allocation failed")
        })?;
        bytes.extend_from_slice(&[0_u8; 4]);
        Ok(Self {
            bytes,
            max_payload_bytes,
            overflowed: false,
            allocation_growth_count: 1,
        })
    }

    fn finish(mut self) -> Result<(Vec<u8>, usize), ControlError> {
        let payload_length = self.bytes.len().saturating_sub(4);
        let payload_length = u32::try_from(payload_length)
            .map_err(|_| ControlError::frame_too_large(self.max_payload_bytes))?;
        self.bytes[..4].copy_from_slice(&payload_length.to_be_bytes());
        Ok((self.bytes, self.allocation_growth_count))
    }

    fn reserve_for(&mut self, additional: usize) -> io::Result<()> {
        let required = self
            .bytes
            .len()
            .checked_add(additional)
            .ok_or_else(|| io::Error::new(io::ErrorKind::WriteZero, FrameLimitExceeded))?;
        if required <= self.bytes.capacity() {
            return Ok(());
        }
        let maximum = self
            .max_payload_bytes
            .checked_add(4)
            .ok_or_else(|| io::Error::new(io::ErrorKind::WriteZero, FrameLimitExceeded))?;
        let mut target = self.bytes.capacity().max(4);
        while target < required {
            target = target.saturating_mul(2).min(maximum);
            if target < required && target == maximum {
                return Err(io::Error::new(io::ErrorKind::WriteZero, FrameLimitExceeded));
            }
        }
        self.bytes
            .try_reserve_exact(target.saturating_sub(self.bytes.len()))
            .map_err(|_| io::Error::other("private RPC frame allocation failed"))?;
        self.allocation_growth_count = self.allocation_growth_count.saturating_add(1);
        Ok(())
    }
}

impl Write for CappedFrameWriter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        let payload_length = self.bytes.len().saturating_sub(4);
        let Some(next_payload_length) = payload_length.checked_add(buffer.len()) else {
            self.overflowed = true;
            return Err(io::Error::new(io::ErrorKind::WriteZero, FrameLimitExceeded));
        };
        if next_payload_length > self.max_payload_bytes {
            self.overflowed = true;
            return Err(io::Error::new(io::ErrorKind::WriteZero, FrameLimitExceeded));
        }
        self.reserve_for(buffer.len())?;
        self.bytes.extend_from_slice(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[derive(Debug)]
struct FrameLimitExceeded;

impl fmt::Display for FrameLimitExceeded {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("private RPC frame limit exceeded")
    }
}

impl std::error::Error for FrameLimitExceeded {}

#[cfg(test)]
mod tests;
