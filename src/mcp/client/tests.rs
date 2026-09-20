use super::*;
use rmcp::{model::ClientJsonRpcMessage, transport::Transport};

#[test]
fn structured_result_is_emitted_once_without_losing_additional_diagnostics() {
    let result: CallToolResult = serde_json::from_value(json!({
        "structuredContent": {"answer": 42},
        "content": [{"type": "text", "text": "{\"answer\":42}"}],
    }))
    .unwrap();
    assert_eq!(normalize(result).unwrap(), json!({"answer": 42}));
    let result: CallToolResult = serde_json::from_value(json!({
        "structuredContent": {"answer": 42},
        "content": [{"type": "text", "text": "additional warning"}],
    }))
    .unwrap();
    let value = normalize(result).unwrap();
    assert_eq!(value["result"], json!({"answer":42}));
    assert_eq!(value["content"][0]["text"], "additional warning");
}

#[test]
fn tool_errors_keep_domain_conflicts_and_do_not_duplicate_json() {
    let conflict = json!({"code": "settings_revision_conflict", "current_revision": "r2"});
    let result: CallToolResult = serde_json::from_value(json!({
        "isError": true,
        "structuredContent": conflict,
        "content": [
            {"type": "text", "text": conflict.to_string()},
            {"type": "text", "text": "the draft is still open"},
        ],
    }))
    .unwrap();
    let failure = normalize(result).unwrap_err().0;
    assert_eq!(failure["code"], "settings_revision_conflict");
    assert_eq!(failure["result"], conflict);
    assert_eq!(
        failure["content"],
        json!([{"type":"text", "text":"the draft is still open"}])
    );
    assert_eq!(failure["mutation_state"], "unknown");
}

#[test]
fn fallback_text_and_nontext_content_are_preserved_even_when_not_json() {
    let content = json!([
        {"type":"text", "text":"not JSON and not an error"},
        {"type":"image", "data":"aGVsbG8=", "mimeType":"image/png"},
    ]);
    let result = serde_json::from_value(json!({"content": content})).unwrap();
    assert_eq!(normalize(result).unwrap(), json!({"content": content}));
    let result = serde_json::from_value(json!({"content": content, "isError":true})).unwrap();
    let failure = normalize(result).unwrap_err().0;
    assert_eq!(failure["result"]["content"], content);
    assert_eq!(failure["code"], "tool_error");
}

#[tokio::test]
async fn malformed_transport_input_is_a_protocol_failure_not_a_silent_timeout() {
    let (mut server, client) = tokio::io::duplex(128);
    let (reader, writer) = tokio::io::split(client);
    let state = TransportState::default();
    let mut transport = ClientTransport::new(reader, writer, state.clone());
    server.write_all(b"{invalid JSON}\n").await.unwrap();
    assert!(transport.receive().await.is_none());
    let failure = state.take_failure().unwrap().0;
    assert_eq!(failure["code"], "protocol_error");
    assert_eq!(failure["mutation_state"], "not_started");
}

#[tokio::test]
async fn oversized_response_fails_explicitly_without_waiting_for_a_newline() {
    let (mut server, client) = tokio::io::duplex(16384);
    let writer_task =
        tokio::spawn(async move { server.write_all(&vec![b'x'; 8 * 1024 * 1024 + 1]).await });
    let (reader, writer) = tokio::io::split(client);
    let state = TransportState::default();
    let mut transport = ClientTransport::new(reader, writer, state.clone());
    assert!(transport.receive().await.is_none());
    assert_eq!(state.take_failure().unwrap().0["code"], "output_too_large");
    writer_task.abort();
}

#[tokio::test]
async fn disconnect_after_dispatch_reports_unknown_mutation_state() {
    let (mut server, client) = tokio::io::duplex(1024);
    let (reader, writer) = tokio::io::split(client);
    let state = TransportState::default();
    let mut transport = ClientTransport::new(reader, writer, state.clone());
    let request: ClientJsonRpcMessage = serde_json::from_value(json!({
        "jsonrpc":"2.0", "id":17, "method":"tools/call",
        "params":{"name":"unknown_mutability", "arguments":{}},
    }))
    .unwrap();
    transport.send(request).await.unwrap();
    use tokio::io::AsyncReadExt;
    let mut bytes = [0; 1024];
    assert!(server.read(&mut bytes).await.unwrap() > 0);
    drop(server);
    assert!(transport.receive().await.is_none());
    let failure = ClientFailure::service(ServiceError::TransportClosed, &state).0;
    assert_eq!(failure["code"], "disconnect");
    assert_eq!(failure["mutation_state"], "unknown");
}

#[cfg(unix)]
#[tokio::test]
async fn initialization_timeout_terminates_managed_child_before_returning() {
    let directory = tempfile::tempdir().unwrap();
    let marker = directory.path().join("child.pid");
    let mut command = Command::new("sh");
    command
        .arg("-c")
        .arg("echo $$ > \"$1\"; exec sleep 30")
        .arg("fixture")
        .arg(&marker);
    let failure = execute(
        command,
        McpClientCommand::Tools { name: None },
        None,
        Instant::now() + Duration::from_millis(20),
    )
    .await
    .unwrap_err()
    .0;
    assert_eq!(failure["code"], "timeout");
    assert_eq!(failure["mutation_state"], "not_started");
    let pid = std::fs::read_to_string(marker)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    let pid = rustix::process::Pid::from_raw(pid).unwrap();
    assert_eq!(
        rustix::process::test_kill_process(pid),
        Err(rustix::io::Errno::SRCH)
    );
}
