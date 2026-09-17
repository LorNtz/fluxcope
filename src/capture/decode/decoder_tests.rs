use super::{
    CancellationReader, ContentDecodePolicy, DecodeClient, DecodeClientState, DecodeContentError,
    DecodeDisplayMode, DecodeJob, DecodeKey, DecodeMetrics, DecodePolicy, DecodeProgressProbe,
    decode_content_bytes, decode_job, start_decode_service_with_admission,
};
use crate::{
    capture::{BodySide, BodyWorkAdmission, CaptureSequence, CapturedBodyPreview, CapturedHeaders},
    control::body::{MAX_CONTENT_ENCODING_LAYERS, MAX_DECODED_CONTENT_BYTES},
    control_rpc::protocol::ControlErrorCode,
};
use flate2::{
    Compression,
    write::{DeflateEncoder, GzEncoder, ZlibEncoder},
};
use hyper::body::Bytes;
use serde_json::json;
use std::{
    collections::{HashMap, HashSet},
    io::{Read, Write},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize},
    },
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

fn preview(bytes: Vec<u8>) -> CapturedBodyPreview {
    CapturedBodyPreview::unbudgeted(Bytes::from(bytes))
}

fn headers(encoding: Option<&str>, content_type: Option<&str>) -> CapturedHeaders {
    let mut values = Vec::new();
    if let Some(encoding) = encoding {
        values.push(("Content-Encoding".to_owned(), encoding.to_owned()));
    }
    if let Some(content_type) = content_type {
        values.push(("Content-Type".to_owned(), content_type.to_owned()));
    }
    CapturedHeaders::unbudgeted(values.into())
}

fn decode(
    input: Vec<u8>,
    encoding: Option<&str>,
) -> Result<super::DecodedBytes, DecodeContentError> {
    decode_content_bytes(
        &preview(input),
        &headers(encoding, None),
        &ContentDecodePolicy::default(),
        &AtomicBool::new(false),
    )
}

fn gzip(input: &[u8]) -> Vec<u8> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(input).expect("gzip input");
    encoder.finish().expect("gzip finish")
}

fn zlib(input: &[u8]) -> Vec<u8> {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(input).expect("zlib input");
    encoder.finish().expect("zlib finish")
}

fn raw_deflate(input: &[u8]) -> Vec<u8> {
    let mut encoder = DeflateEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(input).expect("raw deflate input");
    encoder.finish().expect("raw deflate finish")
}

fn brotli(input: &[u8]) -> Vec<u8> {
    let mut output = Vec::new();
    {
        let mut encoder = brotli::CompressorWriter::new(&mut output, 4_096, 5, 22);
        encoder.write_all(input).expect("brotli input");
    }
    output
}

#[test]
fn content_decoder_supports_every_v1_codec_and_normalizes_the_chain() {
    let plain = b"{\"message\":\"exact\"}\tform=a%2Bb\n";
    let zstd = zstd::stream::encode_all(plain.as_slice(), 1).expect("zstd encode");
    for (label, encoded, content_encoding, expected_chain) in [
        ("identity", plain.to_vec(), "identity", vec![]),
        ("gzip", gzip(plain), "gzip", vec!["gzip"]),
        ("x-gzip", gzip(plain), "x-gzip", vec!["gzip"]),
        ("zlib deflate", zlib(plain), "deflate", vec!["deflate"]),
        (
            "raw deflate",
            raw_deflate(plain),
            "deflate",
            vec!["deflate"],
        ),
        ("brotli", brotli(plain), "br", vec!["br"]),
        ("zstd", zstd, "zstd", vec!["zstd"]),
    ] {
        let decoded = decode(encoded, Some(content_encoding)).expect(label);
        assert_eq!(decoded.bytes.as_ref(), plain.as_slice(), "{label}");
        assert_eq!(decoded.encoding_chain, expected_chain, "{label}");
        assert!(!decoded.output_limited, "{label}");
    }

    let stacked = brotli(&gzip(plain));
    let decoded = decode(stacked, Some(" GZip , BR ")).expect("stacked encoding");
    assert_eq!(decoded.bytes.as_ref(), plain.as_slice());
    assert_eq!(decoded.encoding_chain, ["gzip", "br"]);
}

#[test]
fn identity_decoder_preserves_binary_json_form_and_tabs_without_display_formatting() {
    let exact = vec![
        0xff, 0x00, b'{', b' ', b'"', b'x', b'"', b':', b'1', b'}', b'\t',
    ];
    let decoded = decode(exact.clone(), None).expect("identity binary");
    assert_eq!(decoded.bytes, exact);
    assert!(decoded.encoding_chain.is_empty());
    assert!(!decoded.is_utf8());
    assert!(!decoded.output_limited);
}

#[test]
fn malformed_and_unsupported_encodings_are_typed_and_do_not_fall_back_to_source_bytes() {
    let malformed =
        decode(b"not a gzip stream".to_vec(), Some("gzip")).expect_err("malformed gzip");
    assert_eq!(malformed.code(), ControlErrorCode::UnsupportedBodyEncoding);
    assert_eq!(
        malformed.details(),
        json!({"encoding": "gzip", "reason": "malformed"})
    );

    let unsupported =
        decode(b"source".to_vec(), Some("compress")).expect_err("unsupported content encoding");
    assert_eq!(
        unsupported.code(),
        ControlErrorCode::UnsupportedBodyEncoding
    );
    assert_eq!(
        unsupported.details(),
        json!({"encoding": "compress", "reason": "unsupported"})
    );
}

#[test]
fn more_than_eight_non_identity_layers_are_rejected_before_codec_work() {
    let encoding = std::iter::repeat_n("gzip", MAX_CONTENT_ENCODING_LAYERS + 1)
        .collect::<Vec<_>>()
        .join(", ");
    let error = decode(b"not decoded".to_vec(), Some(&encoding)).expect_err("over-layer chain");
    assert_eq!(error.code(), ControlErrorCode::UnsupportedBodyEncoding);
    assert_eq!(
        error.details(),
        json!({
            "layers": MAX_CONTENT_ENCODING_LAYERS + 1,
            "maximum_layers": MAX_CONTENT_ENCODING_LAYERS
        })
    );

    let identities = format!("identity, identity, {}", "gzip, ".repeat(7) + "gzip");
    let error = decode(b"not decoded".to_vec(), Some(&identities))
        .expect_err("eight codec layers are attempted, not rejected as nine");
    assert_eq!(
        error.details(),
        json!({"encoding": "gzip", "reason": "malformed"})
    );
}

#[test]
fn every_layer_enforces_the_sixteen_mib_decoded_output_cap() {
    let source = vec![b'x'; MAX_DECODED_CONTENT_BYTES + 32_768];
    let decoded = decode(gzip(&source), Some("gzip")).expect("bounded gzip decode");
    assert_eq!(decoded.bytes.len(), MAX_DECODED_CONTENT_BYTES);
    assert!(decoded.bytes.iter().all(|byte| *byte == b'x'));
    assert!(decoded.output_limited);
    let stacked = brotli(&gzip(&source));
    let decoded = decode(stacked, Some("gzip, br")).expect("bounded final stacked decode");
    assert_eq!(decoded.bytes.len(), MAX_DECODED_CONTENT_BYTES);
    assert!(decoded.output_limited);

    let mut oversized_intermediate = gzip(b"small final body");
    oversized_intermediate.resize(MAX_DECODED_CONTENT_BYTES + 32_768, 0);
    let stacked = brotli(&oversized_intermediate);
    let error =
        decode(stacked, Some("gzip, br")).expect_err("intermediate layer limit is not content");
    assert_eq!(error.code(), ControlErrorCode::ResourceLimit);
    assert_eq!(
        error.details(),
        json!({"maximum_output_bytes": MAX_DECODED_CONTENT_BYTES})
    );
}

#[test]
fn cancellation_before_work_returns_the_typed_cancelled_failure() {
    let cancelled = AtomicBool::new(true);
    let error = decode_content_bytes(
        &preview(gzip(&vec![b'x'; 64 * 1_024])),
        &headers(Some("gzip"), None),
        &ContentDecodePolicy::default(),
        &cancelled,
    )
    .expect_err("cancelled decode");
    assert_eq!(error, DecodeContentError::Cancelled);
}

#[tokio::test]
async fn decompression_checks_cancellation_at_bounded_progress_checkpoints() {
    let probe = Arc::new(DecodeProgressProbe::new());
    let policy = ContentDecodePolicy::default().with_test_progress_probe(Arc::clone(&probe));
    let cancelled = Arc::new(AtomicBool::new(false));
    let worker_cancelled = Arc::clone(&cancelled);
    let input = preview(gzip(&vec![b'x'; 256 * 1_024]));
    let input_headers = headers(Some("gzip"), None);
    let worker = tokio::task::spawn_blocking(move || {
        decode_content_bytes(&input, &input_headers, &policy, &worker_cancelled)
    });

    probe.wait_for_checkpoint().await;
    cancelled.store(true, std::sync::atomic::Ordering::Release);
    probe.release();

    let error = worker
        .await
        .expect("decode worker join")
        .expect_err("mid-stream cancellation");
    assert_eq!(error, DecodeContentError::Cancelled);
    assert!(probe.bytes_since_previous_checkpoint() <= 32 * 1_024);
}

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
#[test]
fn codec_input_reader_checks_cancellation_at_thirty_two_kibibyte_boundaries() {
    let cancelled = AtomicBool::new(false);
    let source = vec![b'x'; 64 * 1_024];
    let mut reader = CancellationReader::new(source.as_slice(), &cancelled);
    let mut buffer = vec![0_u8; 64 * 1_024];

    assert_eq!(
        reader.read(&mut buffer).expect("first bounded read"),
        32 * 1_024
    );
    cancelled.store(true, std::sync::atomic::Ordering::Release);
    let error = reader.read(&mut buffer).expect_err("cancelled codec input");
    assert_eq!(error.kind(), std::io::ErrorKind::Interrupted);
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
fn identity_source_flattening_uses_bounded_progress_checkpoints() {
    let probe = Arc::new(DecodeProgressProbe::new());
    probe.release();
    let policy = ContentDecodePolicy::default().with_test_progress_probe(Arc::clone(&probe));
    let input = preview(vec![b'x'; 256 * 1_024]);

    let decoded = decode_content_bytes(
        &input,
        &headers(None, None),
        &policy,
        &AtomicBool::new(false),
    )
    .expect("identity content");

    assert_eq!(decoded.bytes.len(), 256 * 1_024);
    assert!(
        (1..=32 * 1_024).contains(&probe.bytes_since_previous_checkpoint()),
        "source copies must expose bounded cancellation checkpoints"
    );
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
