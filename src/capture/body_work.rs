use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, AtomicUsize, Ordering},
};

use serde_json::json;
use tokio::sync::{Notify, OwnedSemaphorePermit, Semaphore};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use crate::control_rpc::protocol::{ControlError, ControlErrorCode};

pub(crate) const ACTIVE_BODY_WORK_LIMIT: usize = 2;
pub(crate) const QUEUED_BODY_WORK_LIMIT: usize = 8;
pub(crate) const QUEUED_BODY_INPUT_LIMIT_BYTES: usize = 32 * 1_024 * 1_024;

#[derive(Default)]
struct QueueState {
    jobs: usize,
    bytes: usize,
}

#[derive(Clone)]
pub(crate) struct BodyWorkAdmission {
    inner: Arc<BodyWorkAdmissionInner>,
}

struct BodyWorkAdmissionInner {
    semaphore: Arc<Semaphore>,
    queue: Mutex<QueueState>,
    active: AtomicUsize,
    active_waiters: AtomicUsize,
    rejected: AtomicU64,
    changed: Notify,
}

impl Default for BodyWorkAdmission {
    fn default() -> Self {
        Self::new()
    }
}

impl BodyWorkAdmission {
    pub(crate) fn new() -> Self {
        Self {
            inner: Arc::new(BodyWorkAdmissionInner {
                semaphore: Arc::new(Semaphore::new(ACTIVE_BODY_WORK_LIMIT)),
                queue: Mutex::new(QueueState::default()),
                active: AtomicUsize::new(0),
                active_waiters: AtomicUsize::new(0),
                rejected: AtomicU64::new(0),
                changed: Notify::new(),
            }),
        }
    }

    pub(crate) fn try_admit_tui(&self, input_bytes: usize) -> Option<QueuedBodyWorkLease> {
        self.try_queue(input_bytes).ok()
    }

    #[cfg(test)]
    pub(crate) fn try_admit_mcp(
        &self,
        input_bytes: usize,
    ) -> Result<QueuedBodyWorkLease, ControlError> {
        self.try_queue(input_bytes)
    }

    pub(crate) fn try_admit_mcp_until(
        &self,
        input_bytes: usize,
        deadline: Instant,
        cancelled: &CancellationToken,
    ) -> Result<QueuedBodyWorkLease, ControlError> {
        if cancelled.is_cancelled() {
            return Err(cancelled_error());
        }
        if Instant::now() >= deadline {
            return Err(deadline_error());
        }
        self.try_queue(input_bytes)
    }

    fn try_queue(&self, input_bytes: usize) -> Result<QueuedBodyWorkLease, ControlError> {
        let mut queue = self
            .inner
            .queue
            .lock()
            .expect("body work queue lock poisoned");
        if queue.jobs >= QUEUED_BODY_WORK_LIMIT {
            self.inner.rejected.fetch_add(1, Ordering::Relaxed);
            return Err(ControlError::new(
                ControlErrorCode::ResourceLimit,
                "body work queue is full",
                true,
                json!({"maximum_queued_jobs": QUEUED_BODY_WORK_LIMIT}),
            ));
        }
        let Some(next_bytes) = queue.bytes.checked_add(input_bytes) else {
            self.inner.rejected.fetch_add(1, Ordering::Relaxed);
            return Err(queue_bytes_error(input_bytes, queue.bytes));
        };
        if next_bytes > QUEUED_BODY_INPUT_LIMIT_BYTES {
            self.inner.rejected.fetch_add(1, Ordering::Relaxed);
            return Err(queue_bytes_error(input_bytes, queue.bytes));
        }
        queue.jobs += 1;
        queue.bytes = next_bytes;
        drop(queue);
        self.inner.changed.notify_waiters();
        Ok(QueuedBodyWorkLease {
            admission: self.clone(),
            input_bytes,
            charged: true,
        })
    }

    fn release_queued(&self, input_bytes: usize) {
        let mut queue = self
            .inner
            .queue
            .lock()
            .expect("body work queue lock poisoned");
        queue.jobs = queue.jobs.saturating_sub(1);
        queue.bytes = queue.bytes.saturating_sub(input_bytes);
        drop(queue);
        self.inner.changed.notify_waiters();
    }

    pub(crate) fn snapshot(&self) -> BodyWorkAdmissionSnapshot {
        let queue = self
            .inner
            .queue
            .lock()
            .expect("body work queue lock poisoned");
        BodyWorkAdmissionSnapshot {
            active: self.inner.active.load(Ordering::Acquire),
            queued: queue.jobs,
            queued_bytes: queue.bytes,
            rejected: self.inner.rejected.load(Ordering::Relaxed),
        }
    }

    #[cfg(test)]
    pub(crate) fn test_snapshot(&self) -> BodyWorkAdmissionSnapshot {
        self.snapshot()
    }

    #[cfg(test)]
    pub(crate) async fn test_wait_for_active_waiters(&self, minimum: usize) {
        while self.inner.active_waiters.load(Ordering::Acquire) < minimum {
            self.inner.changed.notified().await;
        }
    }

    #[cfg(test)]
    pub(crate) async fn test_wait_for_queued(&self, minimum: usize) {
        loop {
            if self
                .inner
                .queue
                .lock()
                .expect("body work queue lock poisoned")
                .jobs
                >= minimum
            {
                return;
            }
            self.inner.changed.notified().await;
        }
    }
}

fn queue_bytes_error(input_bytes: usize, queued_bytes: usize) -> ControlError {
    ControlError::new(
        ControlErrorCode::ResourceLimit,
        "queued body input exceeds the memory budget",
        true,
        json!({
            "requested_input_bytes": input_bytes,
            "queued_input_bytes": queued_bytes,
            "maximum_queued_input_bytes": QUEUED_BODY_INPUT_LIMIT_BYTES,
        }),
    )
}

impl std::fmt::Debug for QueuedBodyWorkLease {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("QueuedBodyWorkLease")
            .field("input_bytes", &self.input_bytes)
            .finish_non_exhaustive()
    }
}

pub(crate) struct QueuedBodyWorkLease {
    admission: BodyWorkAdmission,
    input_bytes: usize,
    charged: bool,
}

impl QueuedBodyWorkLease {
    pub(crate) async fn acquire_active(
        mut self,
        deadline: Instant,
        cancelled: CancellationToken,
    ) -> Result<ActiveBodyWorkLease, ControlError> {
        self.admission
            .inner
            .active_waiters
            .fetch_add(1, Ordering::AcqRel);
        self.admission.inner.changed.notify_waiters();
        let semaphore = Arc::clone(&self.admission.inner.semaphore);
        let permit = tokio::select! {
            biased;
            _ = cancelled.cancelled() => Err(cancelled_error()),
            _ = tokio::time::sleep_until(deadline) => Err(deadline_error()),
            result = semaphore.acquire_owned() => result.map_err(|_| {
                ControlError::new(
                    ControlErrorCode::ServiceUnavailable,
                    "body work admission is closed",
                    true,
                    json!({}),
                )
            }),
        };
        self.admission
            .inner
            .active_waiters
            .fetch_sub(1, Ordering::AcqRel);
        let permit = permit?;
        self.admission.inner.active.fetch_add(1, Ordering::AcqRel);
        self.release_charge();
        Ok(ActiveBodyWorkLease {
            admission: self.admission.clone(),
            _permit: permit,
        })
    }

    fn release_charge(&mut self) {
        if self.charged {
            self.charged = false;
            self.admission.release_queued(self.input_bytes);
        }
    }
}

impl Drop for QueuedBodyWorkLease {
    fn drop(&mut self) {
        self.release_charge();
    }
}

impl std::fmt::Debug for ActiveBodyWorkLease {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ActiveBodyWorkLease")
            .finish_non_exhaustive()
    }
}

pub(crate) struct ActiveBodyWorkLease {
    admission: BodyWorkAdmission,
    _permit: OwnedSemaphorePermit,
}

impl ActiveBodyWorkLease {
    pub(crate) fn retry_mcp_after_revalidation(
        self,
        input_bytes: usize,
        deadline: Instant,
        cancelled: &CancellationToken,
    ) -> Result<QueuedBodyWorkLease, ControlError> {
        let admission = self.admission.clone();
        drop(self);
        admission.try_admit_mcp_until(input_bytes, deadline, cancelled)
    }
}

impl Drop for ActiveBodyWorkLease {
    fn drop(&mut self) {
        self.admission.inner.active.fetch_sub(1, Ordering::AcqRel);
        self.admission.inner.changed.notify_waiters();
    }
}

fn cancelled_error() -> ControlError {
    ControlError::cancelled("body work was cancelled")
}

fn deadline_error() -> ControlError {
    ControlError::deadline_exceeded("body work deadline elapsed")
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct BodyWorkAdmissionSnapshot {
    pub(crate) active: usize,
    pub(crate) queued: usize,
    pub(crate) queued_bytes: usize,
    pub(crate) rejected: u64,
}

#[cfg(test)]
mod admission_tests;
