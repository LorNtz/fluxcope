use super::ControlRpcClient;
use crate::control_rpc::{
    protocol::{
        ControlError, ControlErrorCode, ControlOperation, ControlResult, DeclaredClient,
        RPC_VERSION,
    },
    test_support::{
        ENDPOINT, OTHER_ENDPOINT, OTHER_RUN_ID, RUN_ID, describe_result, descriptor, read_payload,
        request_value, scope_value, success_response_value, write_payload,
    },
};
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    future::Future,
    time::{Duration, Instant},
};
use tempfile::TempDir;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::UnixListener,
};

fn declared_client() -> DeclaredClient {
    DeclaredClient {
        name: "test-client".to_owned(),
        version: "1.0".to_owned(),
    }
}

async fn call_against_fake<F, Fut>(respond: F) -> Result<ControlResult, ControlError>
where
    F: FnOnce(Value) -> Fut + Send + 'static,
    Fut: Future<Output = Value> + Send + 'static,
{
    let directory = TempDir::new().expect("temporary socket directory");
    let socket_path = directory.path().join("control.sock");
    let listener = UnixListener::bind(&socket_path).expect("bind fake control socket");
    let descriptor = descriptor(&socket_path);
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.expect("accept client");
        let payload = read_payload(&mut stream).await;
        let request: Value = serde_json::from_slice(&payload).expect("request JSON");
        let response = respond(request).await;
        let payload = serde_json::to_vec(&response).expect("response JSON");
        write_payload(&mut stream, &payload).await;
    });

    let result = ControlRpcClient::call(
        &descriptor,
        ControlOperation::DescribeInstance,
        Instant::now() + Duration::from_secs(1),
        declared_client(),
        tokio_util::sync::CancellationToken::new(),
    )
    .await;
    server.await.expect("fake server task");
    result
}

#[tokio::test]
async fn call_sends_the_exact_top_level_operation_and_arguments_shape() {
    let result = call_against_fake(|request| async move {
        let request_id = request["request_id"]
            .as_str()
            .expect("string request ID")
            .to_owned();
        let mut expected =
            request_value(RUN_ID, request["deadline_ms"].as_u64().expect("deadline"));
        expected["request_id"] = json!(request_id);

        assert_eq!(request, expected);
        assert_eq!(
            request
                .as_object()
                .expect("request object")
                .keys()
                .cloned()
                .collect::<BTreeSet<_>>(),
            [
                "arguments",
                "client",
                "deadline_ms",
                "operation",
                "protocol_version",
                "request_id",
                "run_id",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect()
        );
        success_response_value(&request_id, scope_value(ENDPOINT, RUN_ID))
    })
    .await
    .expect("successful RPC call");

    assert_eq!(result, describe_result());
}

#[tokio::test]
async fn client_closes_after_reading_one_response() {
    let directory = TempDir::new().expect("temporary socket directory");
    let socket_path = directory.path().join("control.sock");
    let listener = UnixListener::bind(&socket_path).expect("bind fake control socket");
    let descriptor = descriptor(&socket_path);
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.expect("accept client");
        let request: Value =
            serde_json::from_slice(&read_payload(&mut stream).await).expect("request JSON");
        let request_id = request["request_id"].as_str().expect("request ID");
        let response = success_response_value(request_id, scope_value(ENDPOINT, RUN_ID));
        write_payload(
            &mut stream,
            &serde_json::to_vec(&response).expect("response JSON"),
        )
        .await;

        let mut byte = [0_u8; 1];
        tokio::time::timeout(Duration::from_secs(1), stream.read(&mut byte))
            .await
            .expect("client closes promptly")
            .expect("read EOF")
    });

    ControlRpcClient::call(
        &descriptor,
        ControlOperation::DescribeInstance,
        Instant::now() + Duration::from_secs(1),
        declared_client(),
        tokio_util::sync::CancellationToken::new(),
    )
    .await
    .expect("successful RPC call");

    assert_eq!(server.await.expect("fake server task"), 0);
}

#[tokio::test]
async fn client_returns_a_typed_remote_error() {
    let error = call_against_fake(|request| async move {
        json!({
            "protocol_version": RPC_VERSION,
            "request_id": request["request_id"],
            "error": {
                "code": "instance_unavailable",
                "message": "runtime stopped",
                "retryable": true,
                "details": {"state": "closed"}
            }
        })
    })
    .await
    .expect_err("remote error");

    assert_eq!(error.code, ControlErrorCode::InstanceUnavailable);
    assert_eq!(error.message, "runtime stopped");
    assert!(error.retryable);
    assert_eq!(error.details, json!({"state": "closed"}));
}

#[tokio::test]
async fn client_rejects_mismatched_response_request_id() {
    let error = call_against_fake(|_request| async move {
        success_response_value("different-request", scope_value(ENDPOINT, RUN_ID))
    })
    .await
    .expect_err("mismatched request ID");

    assert_eq!(error.code, ControlErrorCode::InvalidArgument);
}

#[tokio::test]
async fn client_rejects_success_for_another_endpoint_or_run() {
    for scope in [
        scope_value(OTHER_ENDPOINT, RUN_ID),
        scope_value(ENDPOINT, OTHER_RUN_ID),
    ] {
        let error = call_against_fake(move |request| async move {
            success_response_value(request["request_id"].as_str().expect("request ID"), scope)
        })
        .await
        .expect_err("mismatched successful instance scope");

        assert_eq!(error.code, ControlErrorCode::InstanceGenerationConflict);
    }
}

#[tokio::test]
async fn one_outer_deadline_bounds_connect_write_and_read() {
    let directory = TempDir::new().expect("temporary socket directory");
    let socket_path = directory.path().join("control.sock");
    let listener = UnixListener::bind(&socket_path).expect("bind fake control socket");
    let descriptor = descriptor(&socket_path);
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.expect("accept client");
        let _request = read_payload(&mut stream).await;
        tokio::time::sleep(Duration::from_secs(5)).await;
        stream.shutdown().await.expect("close fake server");
    });
    let started = Instant::now();

    let error = ControlRpcClient::call(
        &descriptor,
        ControlOperation::DescribeInstance,
        started + Duration::from_millis(25),
        declared_client(),
        tokio_util::sync::CancellationToken::new(),
    )
    .await
    .expect_err("outer deadline");

    assert_eq!(error.code, ControlErrorCode::DeadlineExceeded);
    assert!(started.elapsed() < Duration::from_millis(500));
    server.abort();
}
