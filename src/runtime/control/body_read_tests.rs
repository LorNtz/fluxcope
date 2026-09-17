#![cfg(unix)]

use super::{
    BodyCacheKey, BodyJobScheduler, ControlServiceContext, DecodeWorkerCancellation,
    DecodedBodyCache, RuntimeBodyServices, RuntimeControlHandler, RuntimeGateway,
    page_decoded_body, page_raw_body, should_cache_decoded_body,
};
use crate::{
    capture::{
        BodySide, BodyStatus, BodyStreamState, BodyWorkAdmission, CaptureChange,
        CaptureChangeError, CaptureChangeFeed, CaptureChangeKind, CaptureRecord, CaptureSequence,
        CaptureSnapshotMode, CapturedBodyPreview, CapturedExchange, CapturedHeaders,
        ContentDecodePolicy, DecodeDisplayMode, DecodeKey, DecodePolicy, DecodedBytes,
        decode_content_bytes,
    },
    control::{
        RuntimeReply, RuntimeRequest,
        body::{
            BodyContentRequest, BodyRepresentation, CaptureBodyMetadataReply,
            CaptureBodySnapshotReply,
        },
    },
    control_rpc::{
        protocol::{
            ControlError, ControlErrorCode, ControlOperation, ControlResult, DeclaredClient,
            InstanceScope,
        },
        server::{ControlCallContext, ControlRpcHandler},
    },
    instance::InstanceIdentity,
};
use flate2::{
    Compression,
    write::{DeflateEncoder, GzEncoder, ZlibEncoder},
};
use hyper::{Method, body::Bytes};
use serde_json::json;
use std::{
    io::Write,
    sync::{Arc, atomic::AtomicBool},
    time::Duration,
};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

fn scope(port: u16) -> InstanceScope {
    let identity = InstanceIdentity::new(format!("127.0.0.1:{port}").parse().unwrap())
        .expect("instance identity");
    InstanceScope {
        proxy_endpoint: identity.proxy_endpoint(),
        run_id: identity.run_id().clone(),
    }
}

fn context(request_id: &str) -> ControlCallContext {
    ControlCallContext {
        request_id: request_id.to_owned(),
        declared_client: DeclaredClient {
            name: "task-10-runtime-test".to_owned(),
            version: "1".to_owned(),
        },
        deadline: std::time::Instant::now() + Duration::from_secs(30),
    }
}

fn status(stream: BodyStreamState, retained_bytes: usize) -> BodyStatus {
    BodyStatus {
        stream,
        observed_bytes: retained_bytes as u64,
        retained_bytes,
        preview_limit: None,
        error: None,
    }
}

fn headers(content_type: &str) -> CapturedHeaders {
    CapturedHeaders::unbudgeted(vec![("Content-Type".to_owned(), content_type.to_owned())].into())
}

fn request(representation: BodyRepresentation, offset: usize, length: usize) -> BodyContentRequest {
    BodyContentRequest {
        capture_id: CaptureSequence::new(7),
        capture_revision: 3,
        side: BodySide::Response,
        representation,
        offset,
        length,
    }
}

#[test]
fn raw_page_slices_across_preview_chunks_without_flattening_before_transport() {
    let retained = format!("{}abcdef{}", "x".repeat(64 * 1_024 - 3), "y".repeat(65_000));
    let snapshot = CaptureRecord::from_completed(CapturedExchange {
        sequence: CaptureSequence::new(7),
        method: Method::GET,
        uri: "https://example.test/chunked".to_owned(),
        mapped_uri: None,
        local_path: None,
        status: Some(200),
        req_headers: vec![],
        res_headers: vec![(
            "Content-Type".to_owned(),
            "  application/octet-stream ; charset=binary  ".to_owned(),
        )],
        req_body: None,
        res_body: Some(retained.clone()),
    })
    .snapshot(CaptureSnapshotMode::WithBodyPreviews);
    let preview = snapshot.response_body.preview;
    assert!(
        preview.chunks().count() > 1,
        "fixture must cross chunk nodes"
    );
    assert_eq!(preview.test_flatten_calls(), 0);

    let page = page_raw_body(
        &preview,
        &request(BodyRepresentation::Raw, 64 * 1_024 - 4, 8),
        &snapshot.response_body.status,
        &snapshot.response.as_ref().expect("response").headers,
    )
    .expect("raw page");
    assert_eq!(page.content.as_ref(), b"xabcdefy");
    assert_eq!(page.actual_range.offset, 64 * 1_024 - 4);
    assert_eq!(page.actual_range.length, 8);
    assert_eq!(page.next_offset, Some(64 * 1_024 + 4));
    assert_eq!(
        page.media_type.as_deref(),
        Some("application/octet-stream ; charset=binary")
    );
    assert_eq!(preview.test_flatten_calls(), 0);
}

#[test]
fn raw_page_is_byte_exact_and_clamps_offsets_beyond_the_retained_body() {
    let preview = CapturedBodyPreview::unbudgeted(Bytes::from_static(&[0xff, 0x00, 0x80, b'a']));
    let body_status = status(BodyStreamState::Complete, 4);
    let raw_headers = headers(" application/octet-stream ");
    let page = page_raw_body(
        &preview,
        &request(BodyRepresentation::Raw, 1, 2),
        &body_status,
        &raw_headers,
    )
    .expect("binary range");
    assert_eq!(page.content.as_ref(), &[0x00, 0x80]);
    assert_eq!(page.requested_range.offset, 1);
    assert_eq!(page.requested_range.length, 2);
    assert_eq!(page.actual_range.offset, 1);
    assert_eq!(page.actual_range.length, 2);
    assert_eq!(page.total_bytes, 4);
    assert_eq!(page.next_offset, Some(3));

    let beyond = page_raw_body(
        &preview,
        &request(BodyRepresentation::Raw, usize::MAX, 2),
        &body_status,
        &raw_headers,
    )
    .expect("offset beyond body");
    assert!(beyond.content.is_empty());
    assert_eq!(beyond.actual_range.offset, 4);
    assert_eq!(beyond.actual_range.length, 0);
    assert_eq!(beyond.total_bytes, 4);
    assert_eq!(beyond.next_offset, None);
}

#[test]
fn decoded_utf8_rejects_only_a_misaligned_start_and_reports_nearest_boundaries() {
    let decoded = DecodedBytes::new(
        Bytes::from_static("中a".as_bytes()),
        vec!["gzip".to_owned()],
        false,
    );
    let error = page_decoded_body(
        &decoded,
        &request(BodyRepresentation::Decoded, 1, 2),
        &status(BodyStreamState::Complete, 12),
        &headers("  text/plain; charset=utf-8  "),
    )
    .expect_err("misaligned UTF-8 start");
    assert_eq!(error.code, ControlErrorCode::InvalidArgument);
    assert_eq!(
        error.details,
        json!({"offset": 1, "nearest_start": 0, "nearest_end": 3})
    );
}

#[test]
fn decoded_utf8_treats_length_as_a_maximum_and_aligns_actual_end_down() {
    let decoded = DecodedBytes::new(Bytes::from_static("a中".as_bytes()), Vec::new(), false);
    let page = page_decoded_body(
        &decoded,
        &request(BodyRepresentation::Decoded, 0, 2),
        &status(BodyStreamState::Complete, 4),
        &headers(" text/plain; charset=utf-8 "),
    )
    .expect("aligned UTF-8 start");
    assert_eq!(page.content.as_ref(), b"a");
    assert_eq!(page.requested_range.length, 2);
    assert_eq!(page.actual_range.length, 1);
    assert_eq!(page.next_offset, Some(1));
    assert_eq!(
        page.media_type.as_deref(),
        Some("text/plain; charset=utf-8")
    );

    let final_page = page_decoded_body(
        &decoded,
        &request(BodyRepresentation::Decoded, 1, 64),
        &status(BodyStreamState::Complete, 4),
        &headers("text/plain"),
    )
    .expect("final aligned page");
    assert_eq!(final_page.content.as_ref(), "中".as_bytes());
    assert_eq!(final_page.actual_range.length, 3);
    assert_eq!(final_page.next_offset, None);
}

#[test]
fn decoded_utf8_short_page_advances_to_the_next_character_boundary() {
    let decoded = DecodedBytes::new(Bytes::from_static("中a".as_bytes()), Vec::new(), false);
    let page = page_decoded_body(
        &decoded,
        &request(BodyRepresentation::Decoded, 0, 1),
        &status(BodyStreamState::Complete, 4),
        &headers("text/plain"),
    )
    .expect("short decoded page");

    assert_eq!(page.content.as_ref(), "中".as_bytes());
    assert_eq!(page.requested_range.length, 1);
    assert_eq!(page.actual_range.length, 3);
    assert_eq!(page.next_offset, Some(3));
}

fn encode_with<W>(mut encoder: W, plain: &[u8]) -> Vec<u8>
where
    W: Write,
    W: RuntimeFinishEncoder,
{
    encoder.write_all(plain).expect("encoded body input");
    encoder.finish_bytes()
}

trait RuntimeFinishEncoder {
    fn finish_bytes(self) -> Vec<u8>;
}

impl RuntimeFinishEncoder for GzEncoder<Vec<u8>> {
    fn finish_bytes(self) -> Vec<u8> {
        self.finish().expect("gzip finish")
    }
}

impl RuntimeFinishEncoder for ZlibEncoder<Vec<u8>> {
    fn finish_bytes(self) -> Vec<u8> {
        self.finish().expect("zlib finish")
    }
}

impl RuntimeFinishEncoder for DeflateEncoder<Vec<u8>> {
    fn finish_bytes(self) -> Vec<u8> {
        self.finish().expect("deflate finish")
    }
}

#[test]
fn decoded_page_offsets_apply_after_every_content_encoding_codec() {
    let plain = b"prefix-middle-suffix";
    let gzip = encode_with(GzEncoder::new(Vec::new(), Compression::default()), plain);
    let zlib = encode_with(ZlibEncoder::new(Vec::new(), Compression::default()), plain);
    let raw_deflate = encode_with(
        DeflateEncoder::new(Vec::new(), Compression::default()),
        plain,
    );
    let mut br = Vec::new();
    {
        let mut encoder = brotli::CompressorWriter::new(&mut br, 4_096, 5, 22);
        encoder.write_all(plain).expect("brotli body input");
    }
    let zstd = zstd::stream::encode_all(plain.as_slice(), 1).expect("zstd body input");

    for (label, encoded, encoding) in [
        ("identity", plain.to_vec(), "identity"),
        ("gzip", gzip, "gzip"),
        ("zlib", zlib, "deflate"),
        ("raw deflate", raw_deflate, "deflate"),
        ("brotli", br, "br"),
        ("zstd", zstd, "zstd"),
    ] {
        let decoded = decode_content_bytes(
            &CapturedBodyPreview::unbudgeted(Bytes::from(encoded)),
            &CapturedHeaders::unbudgeted(
                vec![("Content-Encoding".to_owned(), encoding.to_owned())].into(),
            ),
            &ContentDecodePolicy::default(),
            &AtomicBool::new(false),
        )
        .unwrap_or_else(|_| panic!("{label} decode"));
        let page = page_decoded_body(
            &decoded,
            &request(BodyRepresentation::Decoded, 7, 6),
            &status(BodyStreamState::Complete, plain.len()),
            &headers("text/plain"),
        )
        .unwrap_or_else(|_| panic!("{label} decoded page"));
        assert_eq!(page.content.as_ref(), b"middle", "{label}");
        assert_eq!(page.actual_range.offset, 7, "{label}");
        assert_eq!(page.actual_range.length, 6, "{label}");
    }
}

#[test]
fn decoded_output_limit_does_not_report_source_capture_truncation() {
    let decoded = DecodedBytes::new(
        Bytes::from_static(b"bounded decoded content"),
        vec!["gzip".to_owned()],
        true,
    );
    let page = page_decoded_body(
        &decoded,
        &request(BodyRepresentation::Decoded, 0, 8),
        &status(BodyStreamState::Complete, 23),
        &headers("text/plain"),
    )
    .expect("decoded page");

    assert!(!page.source.truncated);
    assert_eq!(page.source.truncation_reason, None);
    assert!(page.source.decoded_output_limited);
}

#[test]
fn decoded_valid_utf8_aligns_ranges_even_without_a_textual_media_type() {
    let decoded = DecodedBytes::new(Bytes::from_static("中".as_bytes()), Vec::new(), false);
    let error = page_decoded_body(
        &decoded,
        &request(BodyRepresentation::Decoded, 1, 1),
        &status(BodyStreamState::Complete, 3),
        &headers("application/octet-stream"),
    )
    .expect_err("valid UTF-8 must use character boundaries");

    assert_eq!(error.code, ControlErrorCode::InvalidArgument);
    assert_eq!(
        error.details,
        json!({"offset": 1, "nearest_start": 0, "nearest_end": 3})
    );
}

#[test]
fn decoded_binary_ranges_remain_byte_exact_without_utf8_alignment() {
    let decoded = DecodedBytes::new(
        Bytes::from_static(&[0xff, 0x00, 0x80, b'a']),
        vec!["gzip".to_owned()],
        false,
    );
    let page = page_decoded_body(
        &decoded,
        &request(BodyRepresentation::Decoded, 1, 2),
        &status(BodyStreamState::Complete, 4),
        &headers("application/octet-stream"),
    )
    .expect("decoded binary range");
    assert_eq!(page.content.as_ref(), &[0x00, 0x80]);
    assert_eq!(page.actual_range.offset, 1);
    assert_eq!(page.actual_range.length, 2);
}

#[test]
fn decoded_page_owns_only_its_bounded_window() {
    let source = Bytes::from(vec![b'x'; 1024 * 1024]);
    let source_start = source.as_ptr() as usize;
    let source_end = source_start + source.len();
    let decoded = DecodedBytes::new(source, Vec::new(), false);

    let page = page_decoded_body(
        &decoded,
        &request(BodyRepresentation::Decoded, 128, 64),
        &status(BodyStreamState::Complete, decoded.bytes.len()),
        &headers("text/plain"),
    )
    .expect("decoded page");
    let page_start = page.content.as_ptr() as usize;

    assert_eq!(page.content.len(), 64);
    assert!(
        page_start < source_start || page_start >= source_end,
        "page bytes must not retain the full decoded allocation"
    );
}

#[test]
fn dropped_decode_future_cancels_its_blocking_worker_unless_completed() {
    let flag = Arc::new(AtomicBool::new(false));
    {
        let _guard = DecodeWorkerCancellation::new(Arc::clone(&flag));
    }
    assert!(flag.load(std::sync::atomic::Ordering::Acquire));

    flag.store(false, std::sync::atomic::Ordering::Release);
    {
        let mut guard = DecodeWorkerCancellation::new(Arc::clone(&flag));
        guard.disarm();
    }
    assert!(!flag.load(std::sync::atomic::Ordering::Acquire));
}

#[test]
fn decoded_cache_accepts_only_terminal_stream_states() {
    for (stream, expected) in [
        (BodyStreamState::Pending, false),
        (BodyStreamState::Streaming, false),
        (BodyStreamState::Complete, true),
        (BodyStreamState::Failed, true),
        (BodyStreamState::Cancelled, true),
    ] {
        assert_eq!(
            should_cache_decoded_body(&status(stream, 4)),
            expected,
            "{stream:?}"
        );
    }
}

fn key(instance: InstanceScope, revision: u64, side: BodySide) -> BodyCacheKey {
    BodyCacheKey {
        instance,
        capture_id: CaptureSequence::new(7),
        capture_revision: revision,
        side,
        representation: BodyRepresentation::Decoded,
    }
}

#[test]
fn decoded_cache_keys_exact_generation_revision_side_and_representation() {
    let mut cache = DecodedBodyCache::new(64);
    let first = key(scope(19010), 3, BodySide::Request);
    let other_generation = key(scope(19011), 3, BodySide::Request);
    let other_revision = key(scope(19010), 4, BodySide::Request);
    let other_side = key(scope(19010), 3, BodySide::Response);
    cache.insert(first.clone(), Bytes::from_static(b"first"));

    assert_eq!(cache.get(&first).as_deref(), Some(b"first".as_slice()));
    assert!(cache.get(&other_generation).is_none());
    assert!(cache.get(&other_revision).is_none());
    assert!(cache.get(&other_side).is_none());
}

#[test]
fn decoded_cache_preserves_utf8_validity_and_decode_metadata() {
    let mut cache = DecodedBodyCache::new(64);
    let cache_key = key(scope(19010), 3, BodySide::Response);
    cache.insert_decoded(
        cache_key.clone(),
        DecodedBytes::new(
            Bytes::from_static(&[0xff, 0x00]),
            vec!["gzip".to_owned()],
            true,
        ),
    );

    let cached = cache.get_decoded(&cache_key).expect("decoded cache hit");
    assert!(!cached.is_utf8());
    assert_eq!(cached.encoding_chain, ["gzip"]);
    assert!(cached.output_limited);
}

#[test]
fn decoded_cache_replacement_and_lru_eviction_are_byte_accounted() {
    let mut cache = DecodedBodyCache::new(9);
    let a = key(scope(19010), 1, BodySide::Request);
    let b = key(scope(19010), 2, BodySide::Request);
    let c = key(scope(19010), 3, BodySide::Request);
    cache.insert(a.clone(), Bytes::from_static(b"aaa"));
    cache.insert(b.clone(), Bytes::from_static(b"bbb"));
    cache.insert(c.clone(), Bytes::from_static(b"ccc"));
    assert_eq!(cache.test_snapshot().bytes, 9);
    assert_eq!(cache.get(&a).as_deref(), Some(b"aaa".as_slice()));

    cache.insert(c.clone(), Bytes::from_static(b"CCCC"));
    assert_eq!(cache.test_snapshot().bytes, 7);
    assert!(cache.get(&b).is_none(), "least-recently-used entry evicted");
    assert_eq!(cache.get(&a).as_deref(), Some(b"aaa".as_slice()));
    assert_eq!(cache.get(&c).as_deref(), Some(b"CCCC".as_slice()));
}

#[test]
fn capture_changes_purge_obsolete_cache_entries_and_a_feed_gap_invalidates_all() {
    let instance = scope(19010);
    let mut cache = DecodedBodyCache::new(64);
    let rev1 = key(instance.clone(), 1, BodySide::Request);
    let rev2 = key(instance.clone(), 2, BodySide::Request);
    let unrelated = BodyCacheKey {
        capture_id: CaptureSequence::new(8),
        ..rev1.clone()
    };
    for key in [rev1.clone(), rev2.clone(), unrelated.clone()] {
        cache.insert(key, Bytes::from_static(b"body"));
    }
    cache.apply_change(CaptureChange {
        epoch: 1,
        sequence: CaptureSequence::new(7),
        revision: 2,
        kind: CaptureChangeKind::RecordUpdated,
    });
    assert!(cache.get(&rev1).is_none());
    assert!(cache.get(&rev2).is_some());
    assert!(cache.get(&unrelated).is_some());

    for kind in [
        CaptureChangeKind::RetentionEviction,
        CaptureChangeKind::ExplicitDelete,
    ] {
        cache.insert(rev2.clone(), Bytes::from_static(b"body"));
        cache.apply_change(CaptureChange {
            epoch: 2,
            sequence: CaptureSequence::new(7),
            revision: 2,
            kind,
        });
        assert!(cache.get(&rev2).is_none(), "{kind:?}");
    }

    cache.insert(rev2, Bytes::from_static(b"body"));
    cache.apply_change(CaptureChange {
        epoch: 3,
        sequence: CaptureSequence::new(0),
        revision: 0,
        kind: CaptureChangeKind::Clear,
    });
    assert_eq!(cache.test_snapshot().entries, 0);

    cache.insert(unrelated, Bytes::from_static(b"body"));
    cache.apply_feed_error(CaptureChangeError::Gap {
        expected_epoch: 4,
        oldest_available_epoch: 9,
    });
    assert_eq!(cache.test_snapshot().entries, 0);
    assert_eq!(cache.test_snapshot().bytes, 0);
}

fn metadata_reply(instance: InstanceScope, stream: BodyStreamState) -> CaptureBodyMetadataReply {
    CaptureBodyMetadataReply {
        instance,
        capture_id: CaptureSequence::new(7),
        capture_revision: 3,
        side: BodySide::Response,
        status: status(stream, 4),
        headers: headers("text/plain"),
        retained_bytes: 4,
    }
}

fn snapshot_reply(instance: InstanceScope, stream: BodyStreamState) -> CaptureBodySnapshotReply {
    CaptureBodySnapshotReply {
        instance,
        capture_id: CaptureSequence::new(7),
        capture_revision: 3,
        side: BodySide::Response,
        status: status(stream, 4),
        headers: headers("text/plain"),
        retained_bytes: 4,
        preview: CapturedBodyPreview::unbudgeted(Bytes::from_static(b"body")),
    }
}

fn handler(
    feed: CaptureChangeFeed,
    admission: Arc<BodyWorkAdmission>,
) -> (RuntimeControlHandler, super::RuntimeControlReceiver) {
    let (runtime, receiver) = RuntimeGateway::channel(16);
    (
        RuntimeControlHandler::new(ControlServiceContext {
            runtime,
            capture_changes: feed,
            body_work: admission,
            audit: crate::control::audit::InstanceAudit::default(),
            config_source: None,
        }),
        receiver,
    )
}

fn spawn_read(
    handler: RuntimeControlHandler,
    representation: BodyRepresentation,
) -> tokio::task::JoinHandle<Result<ControlResult, ControlError>> {
    tokio::spawn(async move {
        handler
            .handle(
                context("task-10-read"),
                ControlOperation::ReadCaptureBody(Box::new(request(representation, 0, 8_192))),
                CancellationToken::new(),
            )
            .await
    })
}

#[tokio::test]
async fn change_published_during_decode_cannot_resurrect_a_stale_cache_entry() {
    let feed = CaptureChangeFeed::new();
    let admission = Arc::new(BodyWorkAdmission::new());
    let (handler, mut receiver) = handler(feed.clone(), admission);
    let read = spawn_read(handler.clone(), BodyRepresentation::Decoded);
    let instance = scope(19010);

    for _ in 0..2 {
        let command = receiver.recv().await.expect("metadata request");
        command
            .reply
            .send(Ok(RuntimeReply::CaptureBodyMetadata(Box::new(
                metadata_reply(instance.clone(), BodyStreamState::Complete),
            ))))
            .expect("metadata reply");
    }
    let command = receiver.recv().await.expect("snapshot request");
    feed.publish(CaptureSequence::new(7), 4, CaptureChangeKind::RecordUpdated);
    command
        .reply
        .send(Ok(RuntimeReply::CaptureBodySnapshot(Box::new(
            snapshot_reply(instance, BodyStreamState::Complete),
        ))))
        .expect("snapshot reply");

    read.await
        .expect("read join")
        .expect("decoded body still serves its validated snapshot");
    assert_eq!(handler.body_jobs.cache.lock().test_snapshot().entries, 0);
}

#[tokio::test]
async fn stale_revision_and_missing_capture_are_returned_by_runtime_authority() {
    for (error_code, details) in [
        (
            ControlErrorCode::CaptureRevisionConflict,
            json!({"capture_id": 7, "expected_revision": 3, "current_revision": 4}),
        ),
        (ControlErrorCode::CaptureNotFound, json!({"capture_id": 7})),
    ] {
        let admission = Arc::new(BodyWorkAdmission::new());
        let (handler, mut receiver) = handler(CaptureChangeFeed::new(), admission);
        let read = spawn_read(handler, BodyRepresentation::Decoded);
        let command = receiver.recv().await.expect("metadata request");
        assert_eq!(
            command.request,
            RuntimeRequest::GetCaptureBodyMetadata {
                capture_id: CaptureSequence::new(7),
                side: BodySide::Response,
            }
        );
        command
            .reply
            .send(Err(ControlError::new(
                error_code,
                "runtime body authority rejected read",
                false,
                details.clone(),
            )))
            .expect("metadata error reply");
        let error = read.await.expect("read join").expect_err("body read error");
        assert_eq!(error.code, error_code);
        assert_eq!(error.details, details);
    }
}

#[tokio::test]
async fn retention_loss_after_queue_wait_never_serves_or_caches_the_old_snapshot() {
    let admission = Arc::new(BodyWorkAdmission::new());
    let first = admission
        .try_admit_mcp(1)
        .expect("first queued lease")
        .acquire_active(
            Instant::now() + Duration::from_secs(30),
            CancellationToken::new(),
        )
        .await
        .expect("first active lease");
    let second = admission
        .try_admit_mcp(1)
        .expect("second queued lease")
        .acquire_active(
            Instant::now() + Duration::from_secs(30),
            CancellationToken::new(),
        )
        .await
        .expect("second active lease");
    let (handler, mut receiver) = handler(CaptureChangeFeed::new(), Arc::clone(&admission));
    let read = spawn_read(handler, BodyRepresentation::Decoded);
    let metadata = receiver.recv().await.expect("metadata request");
    metadata
        .reply
        .send(Ok(RuntimeReply::CaptureBodyMetadata(Box::new(
            metadata_reply(scope(19010), BodyStreamState::Complete),
        ))))
        .expect("metadata reply");

    admission.test_wait_for_queued(1).await;
    assert_eq!(admission.test_snapshot().active, 2);
    assert_eq!(admission.test_snapshot().queued, 1);
    drop(first);

    let metadata = receiver.recv().await.expect("post-active revalidation");
    assert!(matches!(
        metadata.request,
        RuntimeRequest::GetCaptureBodyMetadata {
            capture_id,
            side: BodySide::Response
        } if capture_id == CaptureSequence::new(7)
    ));
    metadata
        .reply
        .send(Err(ControlError::new(
            ControlErrorCode::CaptureNotFound,
            "capture was evicted while body work was queued",
            false,
            json!({"capture_id": 7}),
        )))
        .expect("retention loss reply");
    let error = read.await.expect("read join").expect_err("retention loss");
    assert_eq!(error.code, ControlErrorCode::CaptureNotFound);
    assert_eq!(admission.test_snapshot().queued, 0);
    assert_eq!(admission.test_snapshot().queued_bytes, 0);
    drop(second);
    assert_eq!(admission.test_snapshot().active, 0);
}

async fn answer_one_decoded_read(
    receiver: &mut super::RuntimeControlReceiver,
    instance: &InstanceScope,
    stream: BodyStreamState,
) {
    for phase in ["pre-admission", "post-active"] {
        let metadata = receiver.recv().await.expect("metadata request");
        assert_eq!(
            metadata.request,
            RuntimeRequest::GetCaptureBodyMetadata {
                capture_id: CaptureSequence::new(7),
                side: BodySide::Response,
            }
        );
        metadata
            .reply
            .send(Ok(RuntimeReply::CaptureBodyMetadata(Box::new(
                metadata_reply(instance.clone(), stream),
            ))))
            .unwrap_or_else(|_| panic!("{phase} metadata reply"));
    }
    let snapshot = receiver.recv().await.expect("snapshot request");
    assert_eq!(
        snapshot.request,
        RuntimeRequest::GetCaptureBodySnapshot {
            capture_id: CaptureSequence::new(7),
            expected_revision: 3,
            side: BodySide::Response,
        }
    );
    snapshot
        .reply
        .send(Ok(RuntimeReply::CaptureBodySnapshot(Box::new(
            snapshot_reply(instance.clone(), stream),
        ))))
        .expect("snapshot reply");
}

#[tokio::test]
async fn live_decoded_reads_never_use_the_terminal_cache() {
    let admission = Arc::new(BodyWorkAdmission::new());
    let (handler, mut receiver) = handler(CaptureChangeFeed::new(), admission);
    let instance = scope(19010);
    for _ in 0..2 {
        let read = spawn_read(handler.clone(), BodyRepresentation::Decoded);
        answer_one_decoded_read(&mut receiver, &instance, BodyStreamState::Streaming).await;
        let result = read.await.expect("read join").expect("live decoded read");
        let ControlResult::ReadCaptureBody { page, .. } = result else {
            panic!("expected body result");
        };
        assert_eq!(page.content.as_ref(), b"body");
    }
}

#[tokio::test]
async fn terminal_decoded_read_hits_cache_but_revalidates_metadata_before_serving() {
    let admission = Arc::new(BodyWorkAdmission::new());
    let (handler, mut receiver) = handler(CaptureChangeFeed::new(), admission);
    let instance = scope(19010);
    let first = spawn_read(handler.clone(), BodyRepresentation::Decoded);
    answer_one_decoded_read(&mut receiver, &instance, BodyStreamState::Complete).await;
    first
        .await
        .expect("first join")
        .expect("first terminal read");

    let second = spawn_read(handler, BodyRepresentation::Decoded);
    for phase in ["pre-admission", "post-active"] {
        let metadata = receiver.recv().await.expect("cached metadata revalidation");
        assert!(matches!(
            metadata.request,
            RuntimeRequest::GetCaptureBodyMetadata {
                capture_id,
                side: BodySide::Response
            } if capture_id == CaptureSequence::new(7)
        ));
        metadata
            .reply
            .send(Ok(RuntimeReply::CaptureBodyMetadata(Box::new(
                metadata_reply(instance.clone(), BodyStreamState::Complete),
            ))))
            .unwrap_or_else(|_| panic!("{phase} cached metadata reply"));
    }
    let result = second.await.expect("second join").expect("cached read");
    let ControlResult::ReadCaptureBody { page, .. } = result else {
        panic!("expected body result");
    };
    assert_eq!(page.content.as_ref(), b"body");
    assert!(
        receiver.try_recv().is_err(),
        "cache hit must not clone a body snapshot"
    );
}

#[tokio::test]
async fn terminal_cache_hits_charge_the_decoded_representation_while_queued() {
    let admission = Arc::new(BodyWorkAdmission::new());
    let first = admission
        .try_admit_mcp(0)
        .expect("first active queue")
        .acquire_active(
            Instant::now() + Duration::from_secs(30),
            CancellationToken::new(),
        )
        .await
        .expect("first active");
    let second = admission
        .try_admit_mcp(0)
        .expect("second active queue")
        .acquire_active(
            Instant::now() + Duration::from_secs(30),
            CancellationToken::new(),
        )
        .await
        .expect("second active");
    let instance = scope(19010);
    let (handler, mut receiver) = handler(CaptureChangeFeed::new(), Arc::clone(&admission));
    handler.body_jobs.cache.lock().insert_decoded(
        BodyCacheKey {
            instance: instance.clone(),
            capture_id: CaptureSequence::new(7),
            capture_revision: 3,
            side: BodySide::Response,
            representation: BodyRepresentation::Decoded,
        },
        DecodedBytes::new(Bytes::from_static(b"decoded"), Vec::new(), false),
    );

    let read = spawn_read(handler, BodyRepresentation::Decoded);
    let metadata = receiver.recv().await.expect("metadata request");
    let mut reply = metadata_reply(instance.clone(), BodyStreamState::Complete);
    reply.retained_bytes = 1;
    metadata
        .reply
        .send(Ok(RuntimeReply::CaptureBodyMetadata(Box::new(reply))))
        .expect("metadata reply");
    admission.test_wait_for_queued(1).await;
    assert_eq!(admission.test_snapshot().queued_bytes, b"decoded".len());

    drop(first);
    let metadata = receiver.recv().await.expect("post-active metadata request");
    let mut reply = metadata_reply(instance, BodyStreamState::Complete);
    reply.retained_bytes = 1;
    metadata
        .reply
        .send(Ok(RuntimeReply::CaptureBodyMetadata(Box::new(reply))))
        .expect("post-active metadata reply");
    read.await
        .expect("cached read join")
        .expect("cached decoded read");
    drop(second);
}

#[tokio::test]
async fn cache_availability_changes_trigger_re_admission_before_snapshot_work() {
    let admission = Arc::new(BodyWorkAdmission::new());
    let first = admission
        .try_admit_mcp(0)
        .expect("first active queue")
        .acquire_active(
            Instant::now() + Duration::from_secs(30),
            CancellationToken::new(),
        )
        .await
        .expect("first active");
    let second = admission
        .try_admit_mcp(0)
        .expect("second active queue")
        .acquire_active(
            Instant::now() + Duration::from_secs(30),
            CancellationToken::new(),
        )
        .await
        .expect("second active");
    let instance = scope(19010);
    let (handler, mut receiver) = handler(CaptureChangeFeed::new(), Arc::clone(&admission));
    let cache_key = key(instance.clone(), 3, BodySide::Response);
    handler.body_jobs.cache.lock().insert_decoded(
        cache_key.clone(),
        DecodedBytes::new(Bytes::from_static(b"decoded"), Vec::new(), false),
    );

    let read = spawn_read(handler.clone(), BodyRepresentation::Decoded);
    let metadata = receiver.recv().await.expect("initial metadata");
    metadata
        .reply
        .send(Ok(RuntimeReply::CaptureBodyMetadata(Box::new(
            metadata_reply(instance.clone(), BodyStreamState::Complete),
        ))))
        .expect("initial metadata reply");
    admission.test_wait_for_queued(1).await;
    handler.body_jobs.cache.lock().remove(&cache_key);

    drop(first);
    for phase in ["post-cache-loss", "post-readmission"] {
        let metadata = receiver.recv().await.expect(phase);
        metadata
            .reply
            .send(Ok(RuntimeReply::CaptureBodyMetadata(Box::new(
                metadata_reply(instance.clone(), BodyStreamState::Complete),
            ))))
            .unwrap_or_else(|_| panic!("{phase} metadata reply"));
    }
    let snapshot = receiver.recv().await.expect("snapshot after readmission");
    snapshot
        .reply
        .send(Ok(RuntimeReply::CaptureBodySnapshot(Box::new(
            snapshot_reply(instance, BodyStreamState::Complete),
        ))))
        .expect("snapshot reply");

    read.await
        .expect("read join")
        .expect("decoded miss after cache loss");
    drop(second);
}

#[test]
fn scheduler_owns_the_shared_admission_and_a_thirty_two_mib_cache() {
    let admission = Arc::new(BodyWorkAdmission::new());
    let (runtime, _receiver) = RuntimeGateway::channel(1);
    let scheduler = BodyJobScheduler::new(
        scope(19010),
        runtime,
        CaptureChangeFeed::new(),
        Arc::clone(&admission),
    );
    assert!(Arc::ptr_eq(&admission, scheduler.test_admission()));
    assert_eq!(scheduler.test_cache_capacity_bytes(), 32 * 1_024 * 1_024);
}

#[tokio::test]
async fn runtime_body_services_share_one_exact_admission_between_tui_and_mcp() {
    let shutdown = CancellationToken::new();
    let services = RuntimeBodyServices::start(DecodePolicy::default(), shutdown.clone());
    let (runtime, _receiver) = RuntimeGateway::channel(1);
    let context = services.control_context(runtime, CaptureChangeFeed::new());
    assert!(Arc::ptr_eq(
        services.decode.client.test_body_work_admission(),
        &context.body_work,
    ));
    assert!(Arc::ptr_eq(&services.admission, &context.body_work));

    let first = context
        .body_work
        .try_admit_mcp(1)
        .expect("first shared queue")
        .acquire_active(
            Instant::now() + Duration::from_secs(30),
            CancellationToken::new(),
        )
        .await
        .expect("first shared active");
    let second = context
        .body_work
        .try_admit_mcp(1)
        .expect("second shared queue")
        .acquire_active(
            Instant::now() + Duration::from_secs(30),
            CancellationToken::new(),
        )
        .await
        .expect("second shared active");
    let queued = (0..8)
        .map(|_| {
            context
                .body_work
                .try_admit_mcp(1)
                .expect("shared queue slot")
        })
        .collect::<Vec<_>>();
    let mcp_error = context
        .body_work
        .try_admit_mcp(1)
        .expect_err("MCP sees shared saturation");
    assert_eq!(mcp_error.code, ControlErrorCode::ResourceLimit);
    assert!(!services.decode.client.request(
        DecodeKey {
            sequence: CaptureSequence::new(99),
            side: BodySide::Response,
            revision: 1,
            mode: DecodeDisplayMode::Response,
        },
        CapturedBodyPreview::unbudgeted(Bytes::from_static(b"body")),
        CapturedHeaders::unbudgeted(Arc::from([])),
    ));
    drop((queued, first, second));

    shutdown.cancel();
    services
        .decode
        .task
        .await
        .expect("decode service join")
        .expect("decode service shutdown");
}
