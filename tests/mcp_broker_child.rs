#![cfg(unix)]

use std::{path::Path, process::Stdio, time::Duration};

use anyhow::{Context, Result, anyhow};
use rmcp::{
    ServiceExt,
    model::{
        CallToolRequestParams, ClientCapabilities, ClientInfo, Implementation, ProtocolVersion,
    },
    transport::TokioChildProcess,
};
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::{
    io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::{Child, Command},
};

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_wirelens")
}

fn isolated_home() -> Result<TempDir> {
    tempfile::tempdir().map_err(Into::into)
}

fn broker_command(home: &Path) -> Command {
    let mut command = Command::new(binary());
    command
        .arg("mcp")
        .env("HOME", home)
        .env_remove("RUST_LOG")
        .kill_on_drop(true);
    command
}

#[tokio::test]
async fn mcp_child_negotiates_earlier_protocol_and_exposes_only_stateless_tools() -> Result<()> {
    let home = isolated_home()?;
    let client_info = ClientInfo::new(
        ClientCapabilities::default(),
        Implementation::new("wirelens-child-contract", "7.6.5"),
    )
    .with_protocol_version(ProtocolVersion::V_2024_11_05);
    let transport = TokioChildProcess::new(broker_command(home.path()))?;
    let client = tokio::time::timeout(Duration::from_secs(3), client_info.serve(transport))
        .await
        .context("broker initialize timed out")??;

    let server_info = client.peer_info().expect("initialized server info");
    assert_eq!(server_info.protocol_version, ProtocolVersion::V_2024_11_05);
    let tools_capability = server_info
        .capabilities
        .tools
        .as_ref()
        .expect("tools capability");
    assert_eq!(tools_capability.list_changed, None);
    assert!(server_info.capabilities.resources.is_none());
    assert!(server_info.capabilities.prompts.is_none());
    assert!(server_info.capabilities.logging.is_none());
    assert!(server_info.capabilities.completions.is_none());
    assert!(server_info.capabilities.experimental.is_none());
    assert!(server_info.capabilities.extensions.is_none());

    let mut tools = client.list_all_tools().await?;
    tools.sort_by(|left, right| left.name.cmp(&right.name));
    assert_eq!(
        tools
            .iter()
            .map(|tool| tool.name.as_ref())
            .collect::<Vec<_>>(),
        vec!["get_broker_status", "get_status", "list_instances"]
    );
    for tool in &tools {
        let annotations = tool.annotations.as_ref().expect("tool annotations");
        assert_eq!(annotations.read_only_hint, Some(true));
        assert_eq!(annotations.destructive_hint, Some(false));
        assert_eq!(annotations.idempotent_hint, Some(true));
        assert_eq!(annotations.open_world_hint, Some(false));
        assert_eq!(tool.input_schema.get("type"), Some(&json!("object")));
        assert_eq!(
            tool.input_schema.get("additionalProperties"),
            Some(&json!(false))
        );
    }
    let get_status = tools
        .iter()
        .find(|tool| tool.name == "get_status")
        .expect("get_status schema");
    assert!(get_status.input_schema["properties"]["instance"].is_object());
    for name in ["get_broker_status", "list_instances"] {
        let tool = tools
            .iter()
            .find(|tool| tool.name == name)
            .expect("tool schema");
        assert_eq!(tool.input_schema["properties"], json!({}));
    }

    let result = client
        .call_tool(CallToolRequestParams::new("list_instances"))
        .await?;
    assert_eq!(
        tool_json(&result)?,
        json!({
            "instances": [],
            "diagnostics": {
                "stale_count": 0,
                "rejected": [],
                "omitted": 0
            }
        })
    );

    client.cancel().await?;
    Ok(())
}

fn tool_json(result: &rmcp::model::CallToolResult) -> Result<Value> {
    if let Some(structured) = &result.structured_content {
        return Ok(structured.clone());
    }
    let encoded = serde_json::to_value(result)?;
    let text = encoded
        .pointer("/content/0/text")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("tool result did not contain structured JSON or JSON text"))?;
    serde_json::from_str(text).context("tool result text was not JSON")
}

#[tokio::test]
async fn mcp_child_stdout_contains_only_json_rpc_frames() -> Result<()> {
    let home = isolated_home()?;
    let mut command = broker_command(home.path());
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn()?;
    let mut stdin = child.stdin.take().context("child stdin")?;
    let stdout = child.stdout.take().context("child stdout")?;
    let mut stdout = BufReader::new(stdout);
    let stderr = child.stderr.take().context("child stderr")?;
    let stderr_task = tokio::spawn(async move {
        let mut stderr = stderr;
        let mut bytes = Vec::new();
        stderr.read_to_end(&mut bytes).await.map(|_| bytes)
    });
    let mut frames = Vec::new();

    send_json_line(
        &mut stdin,
        &json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name": "raw-stdio-contract", "version": "1.0.0"}
            }
        }),
    )
    .await?;
    let initialized = read_json_line(&mut stdout, &mut frames).await?;
    assert_eq!(initialized["id"], 1);
    assert_eq!(initialized["result"]["protocolVersion"], "2024-11-05");

    send_json_line(
        &mut stdin,
        &json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
    )
    .await?;
    send_json_line(
        &mut stdin,
        &json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}}),
    )
    .await?;
    let listed = read_json_line(&mut stdout, &mut frames).await?;
    assert_eq!(listed["id"], 2);
    assert!(listed["result"]["tools"].is_array());

    stdin.shutdown().await?;
    drop(stdin);
    let mut remainder = Vec::new();
    tokio::time::timeout(Duration::from_secs(3), stdout.read_to_end(&mut remainder))
        .await
        .context("stdout close timed out")??;
    if !remainder.is_empty() {
        frames.extend_from_slice(&remainder);
    }
    let status = wait_for_exit(&mut child).await?;
    let stderr = stderr_task.await.context("stderr task")??;
    assert!(
        status.success(),
        "broker failed: {}",
        String::from_utf8_lossy(&stderr)
    );

    let stdout_text = std::str::from_utf8(&frames).context("stdout was not UTF-8 MCP JSON")?;
    let lines = stdout_text.lines().collect::<Vec<_>>();
    assert!(!lines.is_empty());
    for line in lines {
        let frame: Value =
            serde_json::from_str(line).with_context(|| format!("non-MCP stdout line: {line:?}"))?;
        assert_eq!(frame["jsonrpc"], "2.0", "stdout frame was not JSON-RPC");
        assert!(frame.get("id").is_some() || frame.get("method").is_some());
    }
    Ok(())
}

async fn send_json_line(stdin: &mut tokio::process::ChildStdin, value: &Value) -> Result<()> {
    let mut encoded = serde_json::to_vec(value)?;
    encoded.push(b'\n');
    stdin.write_all(&encoded).await?;
    stdin.flush().await?;
    Ok(())
}

async fn read_json_line<R>(reader: &mut R, captured: &mut Vec<u8>) -> Result<Value>
where
    R: AsyncBufRead + Unpin,
{
    let mut line = String::new();
    tokio::time::timeout(Duration::from_secs(3), reader.read_line(&mut line))
        .await
        .context("MCP response timed out")??;
    if line.is_empty() {
        return Err(anyhow!("broker closed stdout before responding"));
    }
    captured.extend_from_slice(line.as_bytes());
    serde_json::from_str(&line).context("stdout response was not JSON")
}

async fn wait_for_exit(child: &mut Child) -> Result<std::process::ExitStatus> {
    tokio::time::timeout(Duration::from_secs(3), child.wait())
        .await
        .context("broker did not exit after stdin closed")?
        .context("wait for broker")
}
