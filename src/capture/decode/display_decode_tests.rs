use super::{
    DecodeClient, DecodeClientState, DecodeDisplayMode, DecodeJob, DecodeKey, DecodeMetrics,
    DecodePolicy, DecodeProgressProbe, decode_job, start_decode_service_with_admission,
    test_support::{gzip, headers, preview},
};
use crate::{
    capture::{BodySide, BodyWorkAdmission, CaptureSequence},
    control::body::MAX_CONTENT_ENCODING_LAYERS,
};
use std::{
    collections::{HashMap, HashSet},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize},
    },
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

#[test]
fn tui_decoder_uses_the_shared_content_encoding_layer_limit() {
    let plain = b"tui payload".to_vec();
    let mut encoded = plain;
    for _ in 0..=MAX_CONTENT_ENCODING_LAYERS {
        encoded = gzip(&encoded);
    }
    let content_encoding = std::iter::repeat_n("gzip", MAX_CONTENT_ENCODING_LAYERS + 1)
        .collect::<Vec<_>>()
        .join(", ");
    let key = DecodeKey {
        sequence: CaptureSequence::new(1),
        side: BodySide::Response,
        revision: 1,

        mode: DecodeDisplayMode::Response,
    };

    let result = decode_job(
        DecodeJob {
            key,
            input: preview(encoded),
            headers: headers(Some(&content_encoding), Some("text/plain")),
            body_work: None,
            cancellation: Arc::new(super::DecodeCancellation::new()),
        },
        &DecodePolicy::default(),
    );

    assert!(
        result.error.is_some(),
        "TUI decode must reject the same over-layer chain as MCP"
    );
}

#[tokio::test]
async fn tui_form_field_formatting_observes_bounded_cancellation_checkpoints() {
    let mut form = b"message=".to_vec();
    for _ in 0..(64 * 1_024) {
        form.extend_from_slice(b"%61");
    }
    assert_tui_form_formatting_cancelled(form).await;
}

#[tokio::test]
async fn tui_form_empty_segment_scan_observes_bounded_cancellation_checkpoints() {
    assert_tui_form_formatting_cancelled(vec![b'&'; 64 * 1_024]).await;
}

async fn assert_tui_form_formatting_cancelled(form: Vec<u8>) {
    let progress = Arc::new(DecodeProgressProbe::new());
    let policy = DecodePolicy {
        format_progress_probe: Some(Arc::clone(&progress)),
        ..DecodePolicy::default()
    };
    let cancelled = Arc::new(AtomicBool::new(false));
    let worker_cancelled = Arc::clone(&cancelled);

    let worker = tokio::task::spawn_blocking(move || {
        super::decode_job_cancellable(
            DecodeJob {
                key: DecodeKey {
                    sequence: CaptureSequence::new(2),
                    side: BodySide::Request,
                    revision: 1,
                    mode: DecodeDisplayMode::Request,
                },
                input: preview(form),
                headers: headers(None, Some("application/x-www-form-urlencoded")),
                body_work: None,
                cancellation: Arc::new(super::DecodeCancellation::new()),
            },
            &policy,
            &worker_cancelled,
        )
    });

    progress.wait_for_checkpoint().await;
    cancelled.store(true, std::sync::atomic::Ordering::Release);
    progress.release();

    let result = worker.await.expect("format worker");
    assert_eq!(result.error.as_deref(), Some("body decode was cancelled"));
}

#[tokio::test]
async fn selecting_a_new_tui_body_cancels_the_superseded_worker() {
    let shutdown = CancellationToken::new();
    let body_work = Arc::new(BodyWorkAdmission::new());
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
    let first_active = body_work
        .try_admit_tui(0)
        .expect("first queued lease")
        .acquire_active(deadline, CancellationToken::new())
        .await
        .expect("first active lease");
    let second_active = body_work
        .try_admit_tui(0)
        .expect("second queued lease")
        .acquire_active(deadline, CancellationToken::new())
        .await
        .expect("second active lease");
    let service =
        start_decode_service_with_admission(DecodePolicy::default(), shutdown.clone(), body_work);
    let first = DecodeKey {
        sequence: CaptureSequence::new(1),
        side: BodySide::Response,
        revision: 1,
        mode: DecodeDisplayMode::Response,
    };
    let second = DecodeKey {
        sequence: CaptureSequence::new(2),
        ..first
    };

    assert!(service.client.request(
        first,
        preview(vec![b'x'; 64 * 1_024]),
        headers(None, Some("text/plain")),
    ));
    let first_cancelled = tokio::time::timeout(std::time::Duration::from_secs(1), async {
        loop {
            if let Some(cancelled) = service
                .client
                .state
                .active_cancellations
                .lock()
                .get(&first)
                .cloned()
            {
                break cancelled;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("first decode becomes active");
    assert!(service.client.request(
        second,
        preview(b"new body".to_vec()),
        headers(None, Some("text/plain")),
    ));

    assert!(
        first_cancelled
            .flag
            .load(std::sync::atomic::Ordering::Acquire)
    );
    drop((first_active, second_active));
    shutdown.cancel();
    service.task.await.expect("service join").expect("service");
}

#[test]
fn rejected_shared_admission_releases_the_tui_local_byte_reservation() {
    let body_work = Arc::new(BodyWorkAdmission::new());
    let held = (0..crate::capture::body_work::QUEUED_BODY_WORK_LIMIT)
        .map(|_| body_work.try_admit_mcp(0).expect("fill shared queue"))
        .collect::<Vec<_>>();
    let (tx, _rx) = mpsc::channel(1);
    let state = Arc::new(DecodeClientState {
        pending: parking_lot::Mutex::new(HashSet::new()),
        desired: parking_lot::Mutex::new(None),
        active_cancellations: parking_lot::Mutex::new(HashMap::new()),
        queued_input_bytes: AtomicUsize::new(0),
    });
    let client = DecodeClient {
        tx,
        state: Arc::clone(&state),
        policy: Arc::new(DecodePolicy::default()),
        metrics: Arc::new(DecodeMetrics::default()),
        body_work,
    };
    let key = DecodeKey {
        sequence: CaptureSequence::new(1),
        side: BodySide::Response,
        revision: 1,
        mode: DecodeDisplayMode::Response,
    };

    assert!(!client.request(key, preview(b"body".to_vec()), headers(None, None)));
    assert_eq!(
        state
            .queued_input_bytes
            .load(std::sync::atomic::Ordering::Acquire),
        0
    );

    drop(held);
    assert!(client.request(key, preview(b"body".to_vec()), headers(None, None)));
}

#[test]
fn tui_local_queue_retains_the_thirty_two_mibibyte_backstop() {
    assert_eq!(
        DecodePolicy::default().max_queued_input_bytes,
        32 * 1_024 * 1_024
    );
}
