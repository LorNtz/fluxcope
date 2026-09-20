use super::*;
use crate::capture::{
    CapturePolicy, CapturePublisher, CaptureSnapshotMode, RequestCaptureInput, ResponseCaptureInput,
};

use hyper::{HeaderMap, Method, Response, body::Bytes};
use std::io;
use tokio::sync::mpsc;

#[tokio::test]
async fn tee_forwards_order_and_captures_prefix() {
    let (capture_tx, mut capture_rx) = mpsc::channel(1);
    let publisher = CapturePublisher::new(capture_tx, CapturePolicy::default());
    let headers = HeaderMap::new();
    let capture = publisher
        .try_start(RequestCaptureInput {
            method: Method::GET,
            original_uri: "https://example.com/",
            effective_uri: "https://example.com/",
            local_path: None,
            headers: &headers,
        })
        .expect("capture admitted");
    capture.set_response(ResponseCaptureInput {
        status: 200,
        headers: &headers,
    });
    capture.complete(BodySide::Request);
    let record = capture_rx.recv().await.expect("capture published");
    let shutdown = CancellationToken::new();
    let tasks = BodyTaskTracker::new(shutdown.clone());
    let (mut source_tx, source) = body_channel();
    let destination = tee_body(source, capture, BodySide::Response, &tasks);
    source_tx
        .send_data(Bytes::from_static(b"one"))
        .await
        .expect("first chunk sent");
    source_tx
        .send_data(Bytes::from_static(b"two"))
        .await
        .expect("second chunk sent");
    drop(source_tx);

    let forwarded = crate::capture::body_bytes(destination)
        .await
        .expect("destination body should complete");

    assert_eq!(forwarded.as_ref(), b"onetwo");
    assert_eq!(record.body_preview(BodySide::Response).flatten(), b"onetwo");
    shutdown.cancel();
    tasks
        .wait_for_shutdown(Duration::from_secs(1))
        .await
        .expect("body tasks should stop");
}

#[tokio::test]
async fn first_chunk_arrives_before_source_eof() {
    let (capture_tx, _capture_rx) = mpsc::channel(1);
    let publisher = CapturePublisher::new(capture_tx, CapturePolicy::default());
    let headers = HeaderMap::new();
    let capture = publisher
        .try_start(RequestCaptureInput {
            method: Method::GET,
            original_uri: "https://example.com/",
            effective_uri: "https://example.com/",
            local_path: None,
            headers: &headers,
        })
        .expect("capture admitted");
    let shutdown = CancellationToken::new();
    let tasks = BodyTaskTracker::new(shutdown.clone());
    let (mut source_tx, source) = body_channel();
    let mut destination = tee_body(source, capture, BodySide::Response, &tasks);
    source_tx
        .send_data(Bytes::from_static(b"first"))
        .await
        .expect("source chunk sent");

    let first = time::timeout(Duration::from_secs(1), destination.frame())
        .await
        .expect("first chunk should not wait for EOF")
        .expect("destination should yield data")
        .expect("destination chunk should succeed");

    assert_eq!(first.into_data().expect("data frame").as_ref(), b"first");
    drop(source_tx);
    drop(destination);
    shutdown.cancel();
}

#[tokio::test]
async fn source_error_remains_a_destination_body_error() {
    let (capture_tx, _capture_rx) = mpsc::channel(1);
    let publisher = CapturePublisher::new(capture_tx, CapturePolicy::default());
    let headers = HeaderMap::new();
    let capture = publisher
        .try_start(RequestCaptureInput {
            method: Method::GET,
            original_uri: "https://example.com/",
            effective_uri: "https://example.com/",
            local_path: None,
            headers: &headers,
        })
        .expect("capture admitted");
    let shutdown = CancellationToken::new();
    let tasks = BodyTaskTracker::new(shutdown.clone());
    let response = Response::builder()
        .body(Body::from_stream(futures::stream::iter(vec![Err::<
            Bytes,
            io::Error,
        >(
            io::Error::other("boom"),
        )])))
        .expect("test response should build");
    let destination = tee_body(response.into_body(), capture, BodySide::Response, &tasks);

    assert!(crate::capture::body_bytes(destination).await.is_err());
    shutdown.cancel();
}

#[tokio::test]
async fn preview_limit_never_truncates_forwarded_body() {
    let (capture_tx, mut capture_rx) = mpsc::channel(1);
    let publisher = CapturePublisher::new(
        capture_tx,
        CapturePolicy {
            response_preview_bytes: 3,
            ..CapturePolicy::default()
        },
    );
    let headers = HeaderMap::new();
    let capture = publisher
        .try_start(RequestCaptureInput {
            method: Method::GET,
            original_uri: "https://example.com/",
            effective_uri: "https://example.com/",
            local_path: None,
            headers: &headers,
        })
        .expect("capture admitted");
    capture.complete(BodySide::Request);
    capture.set_response(ResponseCaptureInput {
        status: 200,
        headers: &headers,
    });
    let record = capture_rx.recv().await.expect("capture published");
    let shutdown = CancellationToken::new();
    let tasks = BodyTaskTracker::new(shutdown.clone());
    let destination = tee_body(Body::from("abcdef"), capture, BodySide::Response, &tasks);

    let forwarded = crate::capture::body_bytes(destination)
        .await
        .expect("body forwards");
    assert_eq!(forwarded.as_ref(), b"abcdef");
    assert_eq!(record.body_preview(BodySide::Response).flatten(), b"abc");
    assert_eq!(
        record.summary().response_body.preview_limit,
        Some(crate::capture::BodyPreviewLimit::PerBodyLimit)
    );
    shutdown.cancel();
    tasks
        .wait_for_shutdown(Duration::from_secs(1))
        .await
        .expect("tasks stop");
}
#[tokio::test]
async fn thousands_of_tiny_fragments_preserve_forwarding_and_preview_byte_order() {
    const BLOCK_BYTES: usize = 64 * 1024;
    let expected = (0..4 * BLOCK_BYTES + 211)
        .map(|index| (index % 251) as u8)
        .collect::<Vec<_>>();
    let fragments = expected
        .chunks(47)
        .map(Bytes::copy_from_slice)
        .collect::<Vec<_>>();
    assert!(fragments.len() > 5_000);

    let (capture_tx, mut capture_rx) = mpsc::channel(1);
    let publisher = CapturePublisher::new(capture_tx, CapturePolicy::default());
    let headers = HeaderMap::new();
    let capture = publisher
        .try_start(RequestCaptureInput {
            method: Method::GET,
            original_uri: "https://example.com/fragmented",
            effective_uri: "https://example.com/fragmented",
            local_path: None,
            headers: &headers,
        })
        .expect("capture admitted");
    capture.complete(BodySide::Request);
    capture.set_response(ResponseCaptureInput {
        status: 200,
        headers: &headers,
    });
    let record = capture_rx.recv().await.expect("capture published");
    let shutdown = CancellationToken::new();
    let tasks = BodyTaskTracker::new(shutdown.clone());
    let source = Body::from_stream(futures::stream::iter(
        fragments.into_iter().map(Ok::<Bytes, io::Error>),
    ));
    let destination = tee_body(source, capture, BodySide::Response, &tasks);

    let forwarded = crate::capture::body_bytes(destination)
        .await
        .expect("all fragments should forward");
    let snapshot = record.snapshot(CaptureSnapshotMode::WithBodyPreviews);

    assert_eq!(forwarded.as_ref(), expected);
    assert_eq!(snapshot.response_body.preview.flatten(), expected);
    assert_eq!(snapshot.response_body.status.retained_bytes, expected.len());
    assert_eq!(
        snapshot.response_body.status.stream,
        crate::capture::BodyStreamState::Complete
    );
    shutdown.cancel();
    tasks
        .wait_for_shutdown(Duration::from_secs(1))
        .await
        .expect("body tasks should stop");
}

#[tokio::test]
async fn trailers_are_forwarded_unchanged() {
    let (capture_tx, _capture_rx) = mpsc::channel(1);
    let publisher = CapturePublisher::new(capture_tx, CapturePolicy::default());
    let headers = HeaderMap::new();
    let capture = publisher
        .try_start(RequestCaptureInput {
            method: Method::GET,
            original_uri: "https://example.com/",
            effective_uri: "https://example.com/",
            local_path: None,
            headers: &headers,
        })
        .expect("capture admitted");
    let shutdown = CancellationToken::new();
    let tasks = BodyTaskTracker::new(shutdown.clone());
    let (mut source_tx, source) = body_channel();
    let destination = tee_body(source, capture, BodySide::Response, &tasks);
    let mut trailers = HeaderMap::new();
    trailers.insert("x-checksum", "ok".parse().expect("header value"));
    source_tx
        .send_trailers(trailers.clone())
        .await
        .expect("trailers sent");
    drop(source_tx);

    let collected = destination.collect().await.expect("body and trailers read");
    assert_eq!(collected.trailers(), Some(&trailers));
    shutdown.cancel();
    tasks
        .wait_for_shutdown(Duration::from_secs(1))
        .await
        .expect("tasks stop");
}

#[tokio::test]
async fn runtime_shutdown_cancels_a_stalled_source_body() {
    let (capture_tx, _capture_rx) = mpsc::channel(1);
    let publisher = CapturePublisher::new(capture_tx, CapturePolicy::default());
    let headers = HeaderMap::new();
    let capture = publisher
        .try_start(RequestCaptureInput {
            method: Method::GET,
            original_uri: "https://example.com/",
            effective_uri: "https://example.com/",
            local_path: None,
            headers: &headers,
        })
        .expect("capture admitted");
    let shutdown = CancellationToken::new();
    let tasks = BodyTaskTracker::new(shutdown.clone());
    let (_source_tx, source) = body_channel();
    let destination = tee_body(source, capture, BodySide::Response, &tasks);
    shutdown.cancel();

    tasks
        .clone()
        .wait_for_shutdown(Duration::from_secs(1))
        .await
        .expect("tasks stop");
    assert!(crate::capture::body_bytes(destination).await.is_err());
}

#[tokio::test]
async fn runtime_shutdown_cancels_a_backpressured_destination() {
    let (capture_tx, _capture_rx) = mpsc::channel(1);
    let publisher = CapturePublisher::new(capture_tx, CapturePolicy::default());
    let headers = HeaderMap::new();
    let capture = publisher
        .try_start(RequestCaptureInput {
            method: Method::GET,
            original_uri: "https://example.com/",
            effective_uri: "https://example.com/",
            local_path: None,
            headers: &headers,
        })
        .expect("capture admitted");
    let shutdown = CancellationToken::new();
    let tasks = BodyTaskTracker::new(shutdown.clone());
    let source = Body::from_stream(futures::stream::repeat_with(|| {
        Ok::<Bytes, io::Error>(Bytes::from_static(b"chunk"))
    }));
    let destination = tee_body(source, capture, BodySide::Response, &tasks);
    tokio::task::yield_now().await;
    shutdown.cancel();

    tasks
        .clone()
        .wait_for_shutdown(Duration::from_secs(1))
        .await
        .expect("backpressured pump should stop");
    drop(destination);
}
