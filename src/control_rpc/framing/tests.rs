use super::{
    CallAdmission, REQUEST_MAX_BYTES, RESPONSE_MAX_BYTES, RESPONSE_SERIALIZATION_BUDGET_BYTES,
    ResponseSerializationBudget, read_json_frame, read_json_frame_cancellable,
    read_validated_request_frame_with_call_lease, run_blocking_with_call_lease,
    serialize_json_frame, serialize_json_frame_until,
};
use crate::control_rpc::{
    protocol::{ControlErrorCode, RequestEnvelope},
    test_support::{RUN_ID, framed, install_argument_parse_probe, request_json, request_value},
};
use serde::{Deserialize, Serialize, Serializer, ser::SerializeSeq};
use std::sync::{
    Arc, Condvar, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TinyPayload {
    value: u8,
}

#[tokio::test]
async fn length_prefix_and_payload_are_read_exactly() {
    let payload = br#"{"value":7}"#;
    let frame = framed(payload);
    let mut input = frame.as_slice();

    let decoded: TinyPayload = read_json_frame(&mut input, 128).await.expect("valid frame");

    assert_eq!(decoded.value, 7);
    assert!(input.is_empty());
}

#[tokio::test]
async fn truncated_prefix_and_payload_are_rejected() {
    let mut short_prefix = [0_u8, 0, 0].as_slice();
    let error = read_json_frame::<TinyPayload, _>(&mut short_prefix, 128)
        .await
        .expect_err("truncated prefix");
    assert_eq!(error.code, ControlErrorCode::InvalidArgument);

    let mut short_payload = [0_u8, 0, 0, 5, b'{', b'}'].as_slice();
    let error = read_json_frame::<TinyPayload, _>(&mut short_payload, 128)
        .await
        .expect_err("truncated payload");
    assert_eq!(error.code, ControlErrorCode::InvalidArgument);
}

#[tokio::test]
async fn oversized_length_is_rejected_before_payload_allocation() {
    let frame = ((REQUEST_MAX_BYTES + 1) as u32).to_be_bytes().to_vec();

    let error = read_json_frame::<RequestEnvelope, _>(&mut frame.as_slice(), REQUEST_MAX_BYTES)
        .await
        .expect_err("oversized frame");

    assert_eq!(error.code, ControlErrorCode::RpcFrameTooLarge);
}

#[tokio::test]
async fn malformed_utf8_and_trailing_json_are_rejected() {
    let invalid_utf8_frame = framed(&[0xff]);
    let mut invalid_utf8 = invalid_utf8_frame.as_slice();
    let error = read_json_frame::<TinyPayload, _>(&mut invalid_utf8, 128)
        .await
        .expect_err("invalid UTF-8");
    assert_eq!(error.code, ControlErrorCode::InvalidArgument);

    let trailing_frame = framed(br#"{"value":7}{}"#);
    let mut trailing = trailing_frame.as_slice();
    let error = read_json_frame::<TinyPayload, _>(&mut trailing, 128)
        .await
        .expect_err("trailing JSON");
    assert_eq!(error.code, ControlErrorCode::InvalidArgument);
}

#[tokio::test]
async fn cancelled_response_parse_returns_a_typed_cancellation() {
    let frame = framed(br#"{"value":7}"#);
    let mut input = frame.as_slice();
    let cancelled = Arc::new(AtomicBool::new(true));

    let error = read_json_frame_cancellable::<TinyPayload, _>(&mut input, 128, cancelled)
        .await
        .expect_err("cancelled response parse");

    assert_eq!(error.code, ControlErrorCode::Cancelled);
}

#[tokio::test]
async fn request_frame_cap_accepts_the_normal_wire_envelope() {
    let payload = request_json(crate::control_rpc::test_support::RUN_ID, 1_000);
    let frame = framed(&payload);
    let mut input = frame.as_slice();

    let envelope: RequestEnvelope = read_json_frame(&mut input, REQUEST_MAX_BYTES)
        .await
        .expect("bounded request frame");

    assert_eq!(
        envelope.protocol_version,
        crate::control_rpc::protocol::RPC_VERSION
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn typed_arguments_are_parsed_in_the_lease_owning_blocking_worker() {
    const PROBE_REQUEST_ID: &str = "argument-parse-probe";
    let runtime_thread = std::thread::current().id();
    let admission = CallAdmission::new(1);
    let lease = admission.acquire().await.expect("call permit");
    let mut probe = install_argument_parse_probe(PROBE_REQUEST_ID);
    let mut request = request_value(RUN_ID, 1_000);
    request["request_id"] = serde_json::json!(PROBE_REQUEST_ID);
    let payload = serde_json::to_vec(&request).expect("probe request JSON");
    let frame = framed(&payload);
    let task = tokio::spawn(async move {
        let mut input = frame.as_slice();
        read_validated_request_frame_with_call_lease(
            &mut input,
            REQUEST_MAX_BYTES,
            lease,
            Instant::now(),
        )
        .await
    });

    let parse_thread = tokio::time::timeout(Duration::from_secs(1), probe.entered())
        .await
        .expect("typed argument parser entered");
    assert_ne!(parse_thread, runtime_thread);
    task.abort();
    assert!(
        admission.try_acquire().is_err(),
        "the blocking typed parser must retain the call lease after outer cancellation"
    );

    probe.release();
    tokio::time::timeout(Duration::from_secs(1), admission.acquire())
        .await
        .expect("typed parser worker exits")
        .expect("call permit after parser exit");
}

#[derive(Clone)]
struct CountedSerialization {
    calls: Arc<AtomicUsize>,
}

impl Serialize for CountedSerialization {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.calls.fetch_add(1, Ordering::SeqCst);
        serializer.serialize_str("payload")
    }
}

#[tokio::test]
async fn response_is_serialized_once_into_its_final_prefixed_buffer() {
    let admission = CallAdmission::new(1);
    let budget = ResponseSerializationBudget::new(RESPONSE_SERIALIZATION_BUDGET_BYTES);
    let calls = Arc::new(AtomicUsize::new(0));
    let lease = admission.acquire().await.expect("call permit");

    let frame = serialize_json_frame(
        CountedSerialization {
            calls: Arc::clone(&calls),
        },
        RESPONSE_MAX_BYTES,
        budget,
        lease,
    )
    .await
    .expect("serialized response");

    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(&frame.as_bytes()[..4], &(9_u32.to_be_bytes()));
    assert_eq!(&frame.as_bytes()[4..], br#""payload""#);
}

#[tokio::test]
async fn encoded_frame_holds_its_actual_response_byte_lease_until_drop() {
    let admission = CallAdmission::new(2);
    let budget = ResponseSerializationBudget::new(RESPONSE_MAX_BYTES);
    let first = serialize_json_frame(
        "first",
        RESPONSE_MAX_BYTES,
        budget.clone(),
        admission.acquire().await.expect("first call permit"),
    )
    .await
    .expect("first encoded frame");
    let second_lease = admission.acquire().await.expect("second call permit");
    let second_budget = budget.clone();
    let mut second = tokio::spawn(async move {
        serialize_json_frame("second", RESPONSE_MAX_BYTES, second_budget, second_lease).await
    });

    assert!(
        tokio::time::timeout(Duration::from_millis(50), &mut second)
            .await
            .is_err(),
        "the retained first frame must keep its actual byte lease"
    );
    drop(first);
    second
        .await
        .expect("second serialization task")
        .expect("second frame after lease release");
}

#[tokio::test]
async fn response_larger_than_eight_mib_is_rejected_by_the_capped_writer() {
    let admission = CallAdmission::new(1);
    let budget = ResponseSerializationBudget::new(RESPONSE_SERIALIZATION_BUDGET_BYTES);
    let lease = admission.acquire().await.expect("call permit");
    let oversized = "x".repeat(RESPONSE_MAX_BYTES);

    let error = serialize_json_frame(oversized, RESPONSE_MAX_BYTES, budget, lease)
        .await
        .expect_err("response exceeds cap after JSON quoting");

    assert_eq!(error.code, ControlErrorCode::RpcFrameTooLarge);
}

#[tokio::test]
async fn response_budget_wait_obeys_the_same_operation_deadline_and_cancellation() {
    let admission = CallAdmission::new(3);
    let budget = ResponseSerializationBudget::new(RESPONSE_MAX_BYTES);
    let held = serialize_json_frame(
        "held",
        RESPONSE_MAX_BYTES,
        budget.clone(),
        admission.acquire().await.expect("held call permit"),
    )
    .await
    .expect("held frame");

    let deadline_error = serialize_json_frame_until(
        "deadline",
        RESPONSE_MAX_BYTES,
        budget.clone(),
        admission.acquire().await.expect("deadline call permit"),
        Instant::now() + Duration::from_millis(25),
        CancellationToken::new(),
    )
    .await
    .expect_err("budget wait deadline");
    assert_eq!(deadline_error.code, ControlErrorCode::DeadlineExceeded);

    let cancelled = CancellationToken::new();
    let cancellation = cancelled.clone();
    let cancelled_call = serialize_json_frame_until(
        "cancelled",
        RESPONSE_MAX_BYTES,
        budget,
        admission.acquire().await.expect("cancelled call permit"),
        Instant::now() + Duration::from_secs(1),
        cancelled,
    );
    tokio::pin!(cancelled_call);
    cancellation.cancel();
    let cancellation_error = cancelled_call.await.expect_err("budget wait cancellation");
    assert_eq!(cancellation_error.code, ControlErrorCode::Cancelled);
    drop(held);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelled_response_serialization_keeps_worker_leases_until_worker_exit() {
    let admission = CallAdmission::new(1);
    let budget = ResponseSerializationBudget::new(RESPONSE_SERIALIZATION_BUDGET_BYTES);
    let lease = admission.acquire().await.expect("call permit");
    let (started_tx, mut started_rx) = mpsc::channel(1);
    let release = SerializationGateRelease::new();
    let value = SerializationGate {
        started: started_tx,
        release: release.gate(),
    };
    let cancelled = CancellationToken::new();
    let cancel = cancelled.clone();
    let task = tokio::spawn(serialize_json_frame_until(
        value,
        RESPONSE_MAX_BYTES,
        budget,
        lease,
        Instant::now() + Duration::from_secs(30),
        cancelled,
    ));
    tokio::time::timeout(Duration::from_secs(30), started_rx.recv())
        .await
        .expect("serializer worker start stays bounded")
        .expect("serializer worker started");

    cancel.cancel();
    let error = task
        .await
        .expect("serialization task")
        .expect_err("serialization cancellation");
    assert_eq!(error.code, ControlErrorCode::Cancelled);
    assert!(
        admission.try_acquire().is_err(),
        "cancelled serializer worker must retain the call permit"
    );

    release.signal();
    tokio::time::timeout(Duration::from_secs(1), admission.acquire())
        .await
        .expect("serializer worker exits")
        .expect("call permit after serializer exit");
}

struct ManyTinyValues(usize);

impl Serialize for ManyTinyValues {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut sequence = serializer.serialize_seq(Some(self.0))?;
        for _ in 0..self.0 {
            sequence.serialize_element(&0_u8)?;
        }
        sequence.end()
    }
}

#[tokio::test]
async fn capped_writer_uses_bounded_geometric_growth_for_tiny_tokens() {
    let admission = CallAdmission::new(1);
    let frame = serialize_json_frame(
        ManyTinyValues(65_536),
        RESPONSE_MAX_BYTES,
        ResponseSerializationBudget::new(RESPONSE_SERIALIZATION_BUDGET_BYTES),
        admission.acquire().await.expect("call permit"),
    )
    .await
    .expect("bounded tiny-token serialization");

    assert!(
        frame.allocation_growth_count() <= 32,
        "bounded geometric growth must avoid per-token reallocations"
    );
}

#[derive(Clone)]
struct SerializationGate {
    started: mpsc::Sender<()>,
    release: Arc<(Mutex<bool>, Condvar)>,
}

struct SerializationGateRelease {
    gate: Arc<(Mutex<bool>, Condvar)>,
}

impl SerializationGateRelease {
    fn new() -> Self {
        Self {
            gate: Arc::new((Mutex::new(false), Condvar::new())),
        }
    }

    fn gate(&self) -> Arc<(Mutex<bool>, Condvar)> {
        Arc::clone(&self.gate)
    }

    fn signal(&self) {
        let (lock, wake) = &*self.gate;
        *lock.lock().expect("serializer gate") = true;
        wake.notify_all();
    }
}

impl Drop for SerializationGateRelease {
    fn drop(&mut self) {
        self.signal();
    }
}

impl Serialize for SerializationGate {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.started
            .blocking_send(())
            .expect("test receiver remains open");
        let (lock, wake) = &*self.release;
        let mut released = lock.lock().expect("gate lock");
        while !*released {
            released = wake.wait(released).expect("gate wait");
        }
        serializer.serialize_str("ok")
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn thirty_two_serializations_respect_the_thirty_two_mib_budget() {
    const CALLS: usize = 32;
    const MAXIMUM_SIMULTANEOUS_SERIALIZATIONS: usize =
        RESPONSE_SERIALIZATION_BUDGET_BYTES / RESPONSE_MAX_BYTES;

    let admission = CallAdmission::new(CALLS);
    let budget = ResponseSerializationBudget::new(RESPONSE_SERIALIZATION_BUDGET_BYTES);
    let (started_tx, mut started_rx) = mpsc::channel(CALLS);
    let release = Arc::new((Mutex::new(false), Condvar::new()));
    let mut tasks = Vec::with_capacity(CALLS);

    for _ in 0..CALLS {
        let lease = admission.acquire().await.expect("call permit");
        let budget = budget.clone();
        let value = SerializationGate {
            started: started_tx.clone(),
            release: Arc::clone(&release),
        };
        tasks.push(tokio::spawn(async move {
            serialize_json_frame(value, RESPONSE_MAX_BYTES, budget, lease).await
        }));
    }
    drop(started_tx);

    for _ in 0..MAXIMUM_SIMULTANEOUS_SERIALIZATIONS {
        started_rx.recv().await.expect("four serializers start");
    }
    assert!(
        tokio::time::timeout(Duration::from_millis(50), started_rx.recv())
            .await
            .is_err(),
        "a fifth pessimistic 8 MiB serialization must wait"
    );

    {
        let (lock, wake) = &*release;
        *lock.lock().expect("gate lock") = true;
        wake.notify_all();
    }

    for task in tasks {
        task.await
            .expect("serialization task")
            .expect("bounded response frame");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelled_outer_future_does_not_release_a_blocking_workers_call_permit() {
    let admission = CallAdmission::new(1);
    let lease = admission.acquire().await.expect("call permit");
    let (started_tx, mut started_rx) = mpsc::channel(1);
    let release = Arc::new((Mutex::new(false), Condvar::new()));
    let worker_release = Arc::clone(&release);

    let task = tokio::spawn(run_blocking_with_call_lease(lease, move || {
        started_tx
            .blocking_send(())
            .expect("test receiver remains open");
        let (lock, wake) = &*worker_release;
        let mut released = lock.lock().expect("worker gate lock");
        while !*released {
            released = wake.wait(released).expect("worker gate wait");
        }
    }));
    started_rx.recv().await.expect("worker started");

    task.abort();
    assert!(
        admission.try_acquire().is_err(),
        "aborting the server future must not release the worker-held permit"
    );

    {
        let (lock, wake) = &*release;
        *lock.lock().expect("worker gate lock") = true;
        wake.notify_all();
    }
    tokio::time::timeout(Duration::from_secs(1), admission.acquire())
        .await
        .expect("worker permit eventually releases")
        .expect("call permit after worker exit");
}
