#![cfg(unix)]

use std::{io::Write as _, sync::Arc, time::Duration};

use bytes::Bytes;
use flate2::{Compression, write::GzEncoder};
use serde_json::json;
use tokio_util::sync::CancellationToken;

use super::{ControlServiceContext, RuntimeControlHandler, RuntimeGateway};
use crate::{
    capture::{
        BodySide, BodyStatus, BodyStreamState, BodyWorkAdmission, CaptureChangeFeed,
        CaptureSequence, CapturedBodyPreview, CapturedHeaders,
    },
    control::{
        RuntimeReply, RuntimeRequest,
        body::{
            CaptureBodyMetadataReply, CaptureBodySnapshotReply, ExtractSelector,
            MAX_DECODED_CONTENT_BYTES, SearchCaptureBodyRequest, SelectionContentRequest,
        },
        json_walk::JsonPointer,
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

fn scope() -> InstanceScope {
    let identity =
        InstanceIdentity::new("127.0.0.1:19112".parse().expect("endpoint")).expect("identity");
    InstanceScope {
        proxy_endpoint: identity.proxy_endpoint(),
        run_id: identity.run_id().clone(),
    }
}

fn context(name: &str) -> ControlCallContext {
    ControlCallContext {
        request_id: name.to_owned(),
        declared_client: DeclaredClient {
            name: "task12-runtime-test".to_owned(),
            version: "1".to_owned(),
        },
        deadline: std::time::Instant::now() + Duration::from_secs(30),
    }
}

fn body_status(bytes: usize) -> BodyStatus {
    BodyStatus {
        stream: BodyStreamState::Complete,
        observed_bytes: bytes as u64,
        retained_bytes: bytes,
        preview_limit: None,
        error: None,
    }
}

fn headers(content_type: &str) -> CapturedHeaders {
    CapturedHeaders::unbudgeted(vec![("Content-Type".to_owned(), content_type.to_owned())].into())
}

fn handler() -> (
    RuntimeControlHandler,
    super::RuntimeControlReceiver,
    Arc<BodyWorkAdmission>,
) {
    let (runtime, receiver) = RuntimeGateway::channel(16);
    let admission = Arc::new(BodyWorkAdmission::new());
    (
        RuntimeControlHandler::new(ControlServiceContext {
            runtime,
            capture_changes: CaptureChangeFeed::new(),
            body_work: Arc::clone(&admission),
            audit: crate::control::audit::InstanceAudit::default(),
            config_source: None,
        }),
        receiver,
        admission,
    )
}

async fn answer_body(
    receiver: &mut super::RuntimeControlReceiver,
    instance: &InstanceScope,
    body: Bytes,
    content_type: &str,
) {
    answer_body_with_headers(receiver, instance, body, headers(content_type)).await;
}

async fn answer_body_with_headers(
    receiver: &mut super::RuntimeControlReceiver,
    instance: &InstanceScope,
    body: Bytes,
    headers: CapturedHeaders,
) {
    let retained = body.len();
    for _ in 0..2 {
        let command = receiver.recv().await.expect("metadata command");
        assert!(matches!(
            command.request,
            RuntimeRequest::GetCaptureBodyMetadata {
                capture_id,
                side: BodySide::Response,
            } if capture_id == CaptureSequence::new(7)
        ));
        command
            .reply
            .send(Ok(RuntimeReply::CaptureBodyMetadata(Box::new(
                CaptureBodyMetadataReply {
                    instance: instance.clone(),
                    capture_id: CaptureSequence::new(7),
                    capture_revision: 3,
                    side: BodySide::Response,
                    status: body_status(retained),
                    headers: headers.clone(),
                    retained_bytes: retained,
                },
            ))))
            .expect("metadata reply");
    }
    let command = receiver.recv().await.expect("snapshot command");
    assert_eq!(
        command.request,
        RuntimeRequest::GetCaptureBodySnapshot {
            capture_id: CaptureSequence::new(7),
            expected_revision: 3,
            side: BodySide::Response,
        }
    );
    command
        .reply
        .send(Ok(RuntimeReply::CaptureBodySnapshot(Box::new(
            CaptureBodySnapshotReply {
                instance: instance.clone(),
                capture_id: CaptureSequence::new(7),
                capture_revision: 3,
                side: BodySide::Response,
                status: body_status(retained),
                headers,
                retained_bytes: retained,
                preview: CapturedBodyPreview::unbudgeted(body),
            },
        ))))
        .expect("snapshot reply");
}

#[tokio::test]
async fn search_keeps_runtime_authority_for_stale_revision() {
    let (handler, mut receiver, _) = handler();
    let call = tokio::spawn(async move {
        handler
            .handle(
                context("task12-stale"),
                ControlOperation::SearchCaptureBody(Box::new(SearchCaptureBodyRequest {
                    capture_id: CaptureSequence::new(7),
                    capture_revision: 3,
                    side: BodySide::Response,
                    query: "body".to_owned(),
                    limit: 10,
                    context_bytes: 160,
                })),
                CancellationToken::new(),
            )
            .await
    });
    let command = receiver.recv().await.expect("metadata command");
    command
        .reply
        .send(Err(ControlError::new(
            ControlErrorCode::CaptureRevisionConflict,
            "capture revision changed",
            false,
            json!({"capture_id": 7, "expected_revision": 3, "current_revision": 4}),
        )))
        .expect("stale reply");
    let error = call.await.expect("join").expect_err("stale revision");
    assert_eq!(error.code, ControlErrorCode::CaptureRevisionConflict);
    assert_eq!(error.details["current_revision"], json!(4));
}

#[tokio::test]
async fn selected_page_is_exact_utf8_and_builds_concrete_next_uri() {
    let (handler, mut receiver, admission) = handler();
    let operation = ControlOperation::ReadSelectedBody(Box::new(SelectionContentRequest {
        capture_id: CaptureSequence::new(7),
        capture_revision: 3,
        side: BodySide::Response,
        selector: ExtractSelector::JsonPointer {
            pointer: JsonPointer::parse("/value").expect("pointer"),
        },
        offset: 0,
        length: 4,
    }));
    let call = tokio::spawn(async move {
        handler
            .handle(context("task12-page"), operation, CancellationToken::new())
            .await
    });
    let instance = scope();
    answer_body(
        &mut receiver,
        &instance,
        Bytes::copy_from_slice(r#"{"value":"é雪xyz"}"#.as_bytes()),
        "application/json",
    )
    .await;
    let ControlResult::ReadSelectedBody { page, .. } =
        call.await.expect("join").expect("selected page")
    else {
        panic!("expected selected page result");
    };
    assert_eq!(page.content.as_ref(), "\"é".as_bytes());
    assert_eq!((page.actual_range.offset, page.actual_range.length), (0, 3));
    assert_eq!(page.selected_bytes, 10);
    let next = page.next_uri.as_deref().expect("next URI");
    assert!(next.contains("/extract/json-pointer?pointer=%2Fvalue&offset=3&length=4"));
    assert!(next.contains(&format!(
        "fluxcope://{}/runs/{}/",
        instance.proxy_endpoint, instance.run_id
    )));
    assert_eq!(admission.test_snapshot().active, 0);
}

#[tokio::test]
async fn cancellation_during_pinned_snapshot_releases_scheduler_admission() {
    let (handler, mut receiver, admission) = handler();
    let cancelled = CancellationToken::new();
    let worker_cancelled = cancelled.clone();
    let operation = ControlOperation::ReadSelectedBody(Box::new(SelectionContentRequest {
        capture_id: CaptureSequence::new(7),
        capture_revision: 3,
        side: BodySide::Response,
        selector: ExtractSelector::FormField {
            key: "target".to_owned(),
        },
        offset: 0,
        length: 8192,
    }));
    let call = tokio::spawn(async move {
        handler
            .handle(context("task12-cancel"), operation, worker_cancelled)
            .await
    });
    let instance = scope();
    for _ in 0..2 {
        let command = receiver.recv().await.expect("metadata command");
        command
            .reply
            .send(Ok(RuntimeReply::CaptureBodyMetadata(Box::new(
                CaptureBodyMetadataReply {
                    instance: instance.clone(),
                    capture_id: CaptureSequence::new(7),
                    capture_revision: 3,
                    side: BodySide::Response,
                    status: body_status(4),
                    headers: headers("application/x-www-form-urlencoded"),
                    retained_bytes: 4,
                },
            ))))
            .expect("metadata reply");
    }
    let pinned_snapshot = receiver.recv().await.expect("snapshot command");
    assert_eq!(admission.test_snapshot().active, 1);
    cancelled.cancel();
    let error = call
        .await
        .expect("join")
        .expect_err("cancelled pinned snapshot");
    assert_eq!(error.code, ControlErrorCode::InstanceUnavailable);
    assert!(error.message.contains("cancel"));
    drop(pinned_snapshot);
    assert_eq!(admission.test_snapshot().active, 0);
    assert_eq!(admission.test_snapshot().queued, 0);
}

#[tokio::test]
async fn source_preview_limit_rejects_extraction_before_snapshot_without_body_data() {
    let (handler, mut receiver, admission) = handler();
    let operation = ControlOperation::ReadSelectedBody(Box::new(SelectionContentRequest {
        capture_id: CaptureSequence::new(7),
        capture_revision: 3,
        side: BodySide::Response,
        selector: ExtractSelector::JsonPointer {
            pointer: JsonPointer::parse("/private").expect("pointer"),
        },
        offset: 0,
        length: 8192,
    }));
    let call = tokio::spawn(async move {
        handler
            .handle(
                context("task12-source-limit"),
                operation,
                CancellationToken::new(),
            )
            .await
    });
    let instance = scope();
    let command = receiver.recv().await.expect("metadata command");
    let mut limited = body_status(7);
    limited.preview_limit = Some(crate::capture::BodyPreviewLimit::PerBodyLimit);
    command
        .reply
        .send(Ok(RuntimeReply::CaptureBodyMetadata(Box::new(
            CaptureBodyMetadataReply {
                instance,
                capture_id: CaptureSequence::new(7),
                capture_revision: 3,
                side: BodySide::Response,
                status: limited,
                headers: headers("application/json"),
                retained_bytes: 7,
            },
        ))))
        .expect("metadata reply");
    let error = call.await.expect("join").expect_err("source preview limit");
    assert_eq!(error.code, ControlErrorCode::ResourceLimit);
    assert_eq!(error.details["capture_revision"], json!(3));
    assert_eq!(error.details["source_truncated"], json!(true));
    let wire = serde_json::to_string(&error).expect("safe source-limit error");
    assert!(!wire.contains("private"));
    assert_eq!(admission.test_snapshot().active, 0);
    assert_eq!(admission.test_snapshot().queued, 0);
    assert!(receiver.try_recv().is_err());
}

#[tokio::test]
async fn search_marks_prefix_totals_when_decoded_output_is_limited() {
    let (handler, mut receiver, admission) = handler();
    let operation = ControlOperation::SearchCaptureBody(Box::new(SearchCaptureBodyRequest {
        capture_id: CaptureSequence::new(7),
        capture_revision: 3,
        side: BodySide::Response,
        query: "needle".to_owned(),
        limit: 10,
        context_bytes: 0,
    }));
    let call = tokio::spawn(async move {
        handler
            .handle(
                context("task12-decoded-limit"),
                operation,
                CancellationToken::new(),
            )
            .await
    });
    let mut plain = vec![b'x'; MAX_DECODED_CONTENT_BYTES + 1];
    plain[..6].copy_from_slice(b"needle");
    let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
    encoder.write_all(&plain).expect("gzip input");
    let encoded = encoder.finish().expect("gzip finish");
    let encoded_headers = CapturedHeaders::unbudgeted(
        vec![
            ("Content-Type".to_owned(), "text/plain".to_owned()),
            ("Content-Encoding".to_owned(), "gzip".to_owned()),
        ]
        .into(),
    );
    answer_body_with_headers(
        &mut receiver,
        &scope(),
        Bytes::from(encoded),
        encoded_headers,
    )
    .await;
    let ControlResult::SearchCaptureBody { result, .. } =
        call.await.expect("join").expect("limited decoded search")
    else {
        panic!("expected search result");
    };
    assert!(result.decoded_output_limited);
    assert_eq!(result.decoded_bytes_inspected, MAX_DECODED_CONTENT_BYTES);
    assert_eq!(result.total_matches, 1);
    assert_eq!(admission.test_snapshot().active, 0);
}
