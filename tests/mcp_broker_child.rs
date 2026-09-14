#![cfg(unix)]

use std::{path::Path, process::Stdio, time::Duration};

use anyhow::{Context, Result, anyhow};
use rmcp::{
    ServiceError, ServiceExt,
    model::{
        CallToolRequestParams, ClientCapabilities, ClientInfo, Implementation, ProtocolVersion,
        ReadResourceRequestParams,
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
async fn mcp_child_negotiates_earlier_protocol_and_exposes_task15_contract() -> Result<()> {
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
    let resources_capability = server_info
        .capabilities
        .resources
        .as_ref()
        .expect("resources capability");
    assert_eq!(resources_capability.subscribe, None);
    assert_eq!(resources_capability.list_changed, None);
    assert!(server_info.capabilities.prompts.is_some());
    assert!(server_info.capabilities.logging.is_none());
    assert!(server_info.capabilities.completions.is_none());
    assert!(server_info.capabilities.experimental.is_none());
    assert!(server_info.capabilities.extensions.is_none());

    let mut prompts = client.list_all_prompts().await?;
    prompts.sort_by(|left, right| left.name.cmp(&right.name));
    assert_eq!(
        prompts
            .iter()
            .map(|prompt| prompt.name.as_str())
            .collect::<Vec<_>>(),
        vec!["configure_mapping", "debug_http_flow"]
    );
    let mut tools = client.list_all_tools().await?;
    tools.sort_by(|left, right| left.name.cmp(&right.name));
    assert_eq!(
        tools
            .iter()
            .map(|tool| tool.name.as_ref())
            .collect::<Vec<_>>(),
        vec![
            "create_mapping_rule",
            "create_preset",
            "delete_mapping_rule",
            "delete_preset",
            "explain_mapping",
            "extract_capture_body",
            "find_json_pointers",
            "get_broker_status",
            "get_capture",
            "get_mapping_settings",
            "get_status",
            "list_instances",
            "move_mapping_rule",
            "preview_mapping_mutation",
            "probe_json_pointer_pattern",
            "rename_preset",
            "search_capture_body",
            "search_captures",
            "set_active_preset",
            "set_mapping_gate",
            "set_mapping_rule_enabled",
            "set_recording_enabled",
            "update_mapping_rule",
            "validate_mapping_settings",
            "wait_for_capture",
        ]
    );
    for tool in &tools {
        let annotations = tool.annotations.as_ref().expect("tool annotations");
        let expected_read_only = matches!(
            tool.name.as_ref(),
            "list_instances"
                | "get_broker_status"
                | "get_status"
                | "search_captures"
                | "get_capture"
                | "search_capture_body"
                | "extract_capture_body"
                | "find_json_pointers"
                | "probe_json_pointer_pattern"
                | "wait_for_capture"
                | "get_mapping_settings"
                | "validate_mapping_settings"
                | "explain_mapping"
                | "preview_mapping_mutation"
        );
        assert_eq!(annotations.read_only_hint, Some(expected_read_only));
        let expected_destructive =
            matches!(tool.name.as_ref(), "delete_preset" | "delete_mapping_rule");
        assert_eq!(annotations.destructive_hint, Some(expected_destructive));
        let expected_idempotent = expected_read_only && tool.name != "wait_for_capture"
            || matches!(
                tool.name.as_ref(),
                "set_recording_enabled"
                    | "set_active_preset"
                    | "set_mapping_gate"
                    | "set_mapping_rule_enabled"
            );
        assert_eq!(annotations.idempotent_hint, Some(expected_idempotent));
        assert_eq!(annotations.open_world_hint, Some(false));
        assert_eq!(tool.input_schema.get("type"), Some(&json!("object")));
        assert_eq!(
            tool.input_schema.get("additionalProperties"),
            Some(&json!(false))
        );
    }
    let broker_status = client
        .call_tool(CallToolRequestParams::new("get_broker_status"))
        .await
        .map_err(|error| anyhow!("get_broker_status failed: {error}"))?;
    let broker_status_text =
        serde_json::to_string(&broker_status).context("serialize broker status")?;
    assert!(broker_status_text.contains("local_unauthenticated_access"));
    let get_status = tools
        .iter()
        .find(|tool| tool.name == "get_status")
        .expect("get_status schema");
    assert!(get_status.input_schema["properties"]["instance"].is_object());
    let set_recording = tools
        .iter()
        .find(|tool| tool.name == "set_recording_enabled")
        .expect("set_recording_enabled schema");
    assert!(set_recording.input_schema["properties"]["instance"].is_object());
    assert!(set_recording.input_schema["properties"]["enabled"].is_object());
    for name in [
        "get_mapping_settings",
        "validate_mapping_settings",
        "explain_mapping",
    ] {
        let tool = tools
            .iter()
            .find(|tool| tool.name == name)
            .expect("mapping read schema");
        let required = tool
            .input_schema
            .get("required")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert!(
            !required.contains(&json!("instance")),
            "{name} is a snapshot read"
        );
        let instance = resolve_local_schema(
            tool.input_schema.as_ref(),
            &tool.input_schema["properties"]["instance"],
        );
        assert!(instance.get("required").is_none(), "{name}");
    }
    let validate_mapping = tools
        .iter()
        .find(|tool| tool.name == "validate_mapping_settings")
        .expect("validate mapping schema");
    assert!(
        validate_mapping.input_schema["required"]
            .as_array()
            .expect("validate required fields")
            .contains(&json!("proxy"))
    );
    let explain_mapping = tools
        .iter()
        .find(|tool| tool.name == "explain_mapping")
        .expect("explain mapping schema");
    assert!(
        explain_mapping.input_schema["required"]
            .as_array()
            .expect("explain required fields")
            .contains(&json!("url"))
    );
    for name in [
        "create_preset",
        "rename_preset",
        "delete_preset",
        "set_active_preset",
        "set_mapping_gate",
        "create_mapping_rule",
        "update_mapping_rule",
        "delete_mapping_rule",
        "move_mapping_rule",
        "set_mapping_rule_enabled",
    ] {
        let tool = tools
            .iter()
            .find(|tool| tool.name == name)
            .expect("mapping mutation schema");
        let required = tool.input_schema["required"]
            .as_array()
            .expect("mapping mutation required fields");
        assert!(required.contains(&json!("instance")), "{name}");
        assert!(
            required.contains(&json!("expected_settings_revision")),
            "{name}"
        );
        let instance = resolve_local_schema(
            tool.input_schema.as_ref(),
            &tool.input_schema["properties"]["instance"],
        );
        assert_eq!(
            instance["required"],
            json!(["proxy_endpoint", "run_id"]),
            "{name}"
        );
    }
    let create_preset = tools
        .iter()
        .find(|tool| tool.name == "create_preset")
        .expect("create preset schema");
    assert!(
        !create_preset.input_schema["required"]
            .as_array()
            .expect("create preset required fields")
            .contains(&json!("initial"))
    );
    let set_active = tools
        .iter()
        .find(|tool| tool.name == "set_active_preset")
        .expect("set active schema");
    assert!(
        set_active.input_schema["required"]
            .as_array()
            .expect("set active required fields")
            .contains(&json!("name"))
    );
    let create_rule = tools
        .iter()
        .find(|tool| tool.name == "create_mapping_rule")
        .expect("create rule schema");
    assert!(
        !create_rule.input_schema["required"]
            .as_array()
            .expect("create rule required fields")
            .contains(&json!("index"))
    );
    let search = tools
        .iter()
        .find(|tool| tool.name == "search_captures")
        .expect("search_captures schema");
    assert!(search.input_schema["properties"]["instance"].is_object());
    assert!(search.input_schema["properties"]["query"].is_object());
    assert!(search.input_schema["properties"]["cursor"].is_object());
    assert!(search.input_schema["properties"]["limit"].is_object());
    let find_json = tools
        .iter()
        .find(|tool| tool.name == "find_json_pointers")
        .expect("find_json_pointers schema");
    let probe_json = tools
        .iter()
        .find(|tool| tool.name == "probe_json_pointer_pattern")
        .expect("probe_json_pointer_pattern schema");
    for (tool, operation_field) in [(find_json, "field_name"), (probe_json, "pattern")] {
        let required = tool.input_schema["required"]
            .as_array()
            .expect("structured JSON tool required fields");
        for field in [
            "instance",
            "capture_id",
            "capture_revision",
            "side",
            operation_field,
        ] {
            assert!(
                required.contains(&json!(field)),
                "{} must require {field}",
                tool.name
            );
        }
        let instance = resolve_local_schema(
            tool.input_schema.as_ref(),
            &tool.input_schema["properties"]["instance"],
        );
        let instance_required = instance["required"]
            .as_array()
            .expect("structured JSON instance required fields");
        assert_eq!(instance_required.len(), 2);
        assert!(instance_required.contains(&json!("proxy_endpoint")));
        assert!(instance_required.contains(&json!("run_id")));
        assert_eq!(
            tool.input_schema["properties"]["side"]["type"],
            json!("string")
        );
        assert!(
            tool.description
                .as_deref()
                .expect("structured JSON tool description")
                .contains("without returning values")
        );
    }
    let match_mode = resolve_local_schema(
        find_json.input_schema.as_ref(),
        &find_json.input_schema["properties"]["match_mode"],
    );
    assert_eq!(
        match_mode["enum"],
        json!(["exact", "unicode_casefold_exact"])
    );
    assert_eq!(
        find_json.input_schema["properties"]["limit"]["minimum"],
        json!(1)
    );
    assert_eq!(
        find_json.input_schema["properties"]["limit"]["maximum"],
        json!(20)
    );
    assert!(
        !find_json.input_schema["required"]
            .as_array()
            .expect("find required fields")
            .contains(&json!("limit"))
    );
    let pattern = resolve_local_schema(
        probe_json.input_schema.as_ref(),
        &probe_json.input_schema["properties"]["pattern"],
    );
    assert_eq!(pattern["type"], json!("string"));
    let body_search = tools
        .iter()
        .find(|tool| tool.name == "search_capture_body")
        .expect("search_capture_body schema");
    let body_extract = tools
        .iter()
        .find(|tool| tool.name == "extract_capture_body")
        .expect("extract_capture_body schema");
    for tool in [body_search, body_extract] {
        let required = tool.input_schema["required"]
            .as_array()
            .expect("targeted body tool required fields");
        for field in ["instance", "capture_id", "capture_revision", "side"] {
            assert!(
                required.contains(&json!(field)),
                "{} requires {field}",
                tool.name
            );
        }
        let instance = resolve_local_schema(
            tool.input_schema.as_ref(),
            &tool.input_schema["properties"]["instance"],
        );
        let instance_required = instance["required"]
            .as_array()
            .expect("targeted body instance required fields");
        assert!(instance_required.contains(&json!("proxy_endpoint")));
        assert!(instance_required.contains(&json!("run_id")));
    }
    assert!(
        body_search.input_schema["required"]
            .as_array()
            .expect("search required")
            .contains(&json!("query"))
    );
    assert_eq!(
        body_search.input_schema["properties"]["query"]["maxLength"],
        json!(8192)
    );
    assert_eq!(
        body_search.input_schema["properties"]["limit"]["minimum"],
        json!(1)
    );
    assert_eq!(
        body_search.input_schema["properties"]["limit"]["maximum"],
        json!(50)
    );
    assert_eq!(
        body_search.input_schema["properties"]["context_bytes"]["maximum"],
        json!(1024)
    );
    assert!(
        body_extract.input_schema["required"]
            .as_array()
            .expect("extract required")
            .contains(&json!("selector"))
    );

    let wait = tools
        .iter()
        .find(|tool| tool.name == "wait_for_capture")
        .expect("wait_for_capture schema");
    let required = wait.input_schema["required"]
        .as_array()
        .expect("wait required fields");
    assert!(required.contains(&json!("instance")));
    assert!(required.contains(&json!("milestone")));
    let instance_schema = resolve_local_schema(
        wait.input_schema.as_ref(),
        &wait.input_schema["properties"]["instance"],
    );
    let instance_required = instance_schema["required"]
        .as_array()
        .expect("wait instance required fields");
    assert!(instance_required.contains(&json!("proxy_endpoint")));
    assert!(instance_required.contains(&json!("run_id")));
    let milestone_schema = resolve_local_schema(
        wait.input_schema.as_ref(),
        &wait.input_schema["properties"]["milestone"],
    );
    assert_eq!(
        milestone_schema["enum"],
        json!(["request_seen", "response_started", "exchange_terminal"])
    );
    assert_eq!(
        wait.input_schema["properties"]["timeout_ms"]["minimum"],
        json!(1)
    );
    assert_eq!(
        wait.input_schema["properties"]["timeout_ms"]["maximum"],
        json!(300_000)
    );
    let wait_description = wait.description.as_deref().expect("wait description");
    for required_phrase in [
        "exact run",
        "request_seen",
        "response_started",
        "exchange_terminal",
        "five minutes",
        "unmatched",
    ] {
        assert!(
            wait_description.contains(required_phrase),
            "wait description must contain {required_phrase:?}"
        );
    }
    let get_capture = tools
        .iter()
        .find(|tool| tool.name == "get_capture")
        .expect("get_capture schema");
    let get_capture_annotations = get_capture.annotations.as_ref().expect("annotations");
    assert_eq!(get_capture_annotations.read_only_hint, Some(true));
    assert_eq!(get_capture_annotations.destructive_hint, Some(false));
    assert_eq!(get_capture_annotations.idempotent_hint, Some(true));
    assert_eq!(get_capture_annotations.open_world_hint, Some(false));
    let get_capture_required = get_capture.input_schema["required"]
        .as_array()
        .expect("get_capture required fields");
    assert!(get_capture_required.contains(&json!("instance")));
    assert!(get_capture_required.contains(&json!("capture_id")));
    assert!(!get_capture_required.contains(&json!("expected_revision")));
    let get_capture_instance = resolve_local_schema(
        get_capture.input_schema.as_ref(),
        &get_capture.input_schema["properties"]["instance"],
    );
    let get_capture_instance_required = get_capture_instance["required"]
        .as_array()
        .expect("get_capture instance required fields");
    assert_eq!(get_capture_instance_required.len(), 2);
    assert!(get_capture_instance_required.contains(&json!("proxy_endpoint")));
    assert!(get_capture_instance_required.contains(&json!("run_id")));
    let capture_id_schema = resolve_local_schema(
        get_capture.input_schema.as_ref(),
        &get_capture.input_schema["properties"]["capture_id"],
    );
    assert_eq!(capture_id_schema["minimum"], json!(0));
    assert!(get_capture.input_schema["properties"]["expected_revision"].is_object());
    for name in ["get_broker_status", "list_instances"] {
        let tool = tools
            .iter()
            .find(|tool| tool.name == name)
            .expect("tool schema");
        assert_eq!(tool.input_schema["properties"], json!({}));
    }

    let resources = client.list_resources(None).await?;
    assert!(resources.resources.is_empty());
    assert!(resources.next_cursor.is_none());
    let templates = client.list_resource_templates(None).await?;
    assert_eq!(templates.resource_templates.len(), 3);
    assert_eq!(
        templates
            .resource_templates
            .iter()
            .map(|template| template.uri_template.as_str())
            .collect::<Vec<_>>(),
        vec![
            "wirelens://{+proxy_endpoint}/runs/{run_id}/captures/{capture_id}/revisions/{capture_revision}/bodies/{side}/content/{representation}{?offset,length}",
            "wirelens://{+proxy_endpoint}/runs/{run_id}/captures/{capture_id}/revisions/{capture_revision}/bodies/{side}/extract/json-pointer{?pointer,offset,length}",
            "wirelens://{+proxy_endpoint}/runs/{run_id}/captures/{capture_id}/revisions/{capture_revision}/bodies/{side}/extract/form-field{?key,offset,length}",
        ]
    );
    assert!(templates.next_cursor.is_none());

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

    for (name, arguments) in [
        (
            "get_mapping_settings",
            json!({
                "instance": {
                    "proxy_endpoint": "127.0.0.1:19899",
                    "run_id": "AAAAAAAAAAAAAAAAAAAAAA"
                }
            }),
        ),
        (
            "create_preset",
            json!({
                "instance": {
                    "proxy_endpoint": "127.0.0.1:19899",
                    "run_id": "AAAAAAAAAAAAAAAAAAAAAA"
                },
                "expected_settings_revision": 1,
                "name": "dev"
            }),
        ),
    ] {
        let error = client
            .call_tool(
                CallToolRequestParams::new(name)
                    .with_arguments(arguments.as_object().expect("mapping arguments").clone()),
            )
            .await
            .expect_err("selected missing instance");
        let ServiceError::McpError(error) = error else {
            panic!("expected typed mapping dispatch error");
        };
        assert_eq!(
            error.data.expect("typed mapping error data")["code"],
            json!("instance_not_found"),
            "{name}"
        );
    }

    let zero_timeout = client
        .call_tool(
            CallToolRequestParams::new("wait_for_capture").with_arguments(
                json!({
                    "instance": {
                        "proxy_endpoint": "127.0.0.1:19899",
                        "run_id": "AAAAAAAAAAAAAAAAAAAAAA"
                    },
                    "milestone": "request_seen",
                    "timeout_ms": 0
                })
                .as_object()
                .expect("wait arguments")
                .clone(),
            ),
        )
        .await
        .expect_err("zero wait timeout");
    let ServiceError::McpError(zero_timeout) = zero_timeout else {
        panic!("expected typed MCP error");
    };
    assert_eq!(
        zero_timeout.data.expect("typed MCP error data")["code"],
        json!("invalid_argument")
    );

    let invalid_resource = client
        .read_resource(ReadResourceRequestParams::new(
            "http://127.0.0.1:8989/not-a-wirelens-resource",
        ))
        .await
        .expect_err("strict resource URI");
    let ServiceError::McpError(invalid_resource) = invalid_resource else {
        panic!("expected typed resource MCP error");
    };
    assert_eq!(
        invalid_resource.data.expect("typed resource error data"),
        json!({
            "code": "invalid_argument",
            "retryable": false,
            "details": {}
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

fn resolve_local_schema<'a>(
    root: &'a serde_json::Map<String, Value>,
    schema: &'a Value,
) -> &'a Value {
    let mut resolved = schema;
    while let Some(reference) = resolved.get("$ref").and_then(Value::as_str) {
        let definition = reference
            .strip_prefix("#/$defs/")
            .expect("schema reference must be local");
        resolved = root
            .get("$defs")
            .and_then(Value::as_object)
            .and_then(|definitions| definitions.get(definition))
            .expect("referenced local schema definition");
    }
    resolved
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
