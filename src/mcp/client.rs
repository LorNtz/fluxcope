//! One-command MCP client. The SDK owns negotiation, request IDs and protocol state.

#[cfg(test)]
mod tests;
mod transport;

use std::{fmt, io::Read, process::Stdio, time::Duration};

use rmcp::{
    ServiceError, ServiceExt,
    model::{
        CallToolRequestParams, CallToolResult, ClientCapabilities, ClientInfo, Implementation,
        PaginatedRequestParams, ReadResourceRequestParams,
    },
    service::ClientInitializeError,
};
use serde_json::{Map, Value, json};
use tokio::{io::AsyncWriteExt, process::Command, time::Instant};

use crate::cli::McpClientCommand;
use transport::{ClientTransport, REQUEST_MAX_BYTES, TransportState};

const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_TOOL_PAGES: usize = 128;
const MAX_TOOLS: usize = 4096;

#[derive(Debug)]
struct ClientFailure(Value);

impl ClientFailure {
    fn new(stage: &str, code: &str, message: impl fmt::Display, dispatched: bool) -> Self {
        Self(json!({
            "ok": false, "stage": stage, "code": code, "message": message.to_string(),
            "mutation_state": if dispatched { "unknown" } else { "not_started" },
        }))
    }

    fn service(error: ServiceError, state: &TransportState) -> Self {
        if let Some(fault) = state.take_failure() {
            return fault;
        }
        let code = match &error {
            ServiceError::TransportClosed | ServiceError::TransportSend(_) => "disconnect",
            ServiceError::Timeout { .. } => "timeout",
            ServiceError::Cancelled { .. } => "interrupted",
            _ => "protocol_error",
        };
        let mut failure = Self::new("request", code, &error, state.dispatched());
        if let ServiceError::McpError(error) = error {
            failure.0["error"] = json!(error);
            if let Some(code) = failure
                .0
                .pointer("/error/data/code")
                .and_then(Value::as_str)
            {
                failure.0["code"] = json!(code);
            }
        }
        failure
    }
}

impl fmt::Display for ClientFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl std::error::Error for ClientFailure {}

pub(crate) async fn run(command: McpClientCommand, timeout_secs: u64) -> anyhow::Result<()> {
    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    let arguments = if let McpClientCommand::Call { arguments, .. } = &command {
        Some(
            tokio::time::timeout_at(deadline, load_arguments(arguments.clone()))
                .await
                .map_err(|_| {
                    ClientFailure::new("input", "timeout", "argument read deadline exceeded", false)
                })??,
        )
    } else {
        None
    };
    let executable = std::env::current_exe()
        .map_err(|error| ClientFailure::new("connect", "spawn_failed", error, false))?;
    let mut child_command = Command::new(executable);
    child_command.arg("mcp");
    let value = execute(child_command, command, arguments, deadline).await?;
    // Never truncate resolved results or replace them with a shutdown error.
    let mut bytes = serde_json::to_vec(&value)?;
    bytes.push(b'\n');
    let mut stdout = tokio::io::stdout();
    stdout.write_all(&bytes).await?;
    stdout.flush().await?;
    Ok(())
}

async fn load_arguments(source: String) -> Result<Map<String, Value>, ClientFailure> {
    if source.len() > REQUEST_MAX_BYTES {
        return Err(ClientFailure::new(
            "input",
            "input_too_large",
            "arguments exceed 1 MiB",
            false,
        ));
    }
    let bytes = if source == "-" || source.starts_with('@') {
        // Tokio stdin/fs use uncancellable blocking-pool reads. A dedicated thread
        // allows a stalled stdin/FIFO to time out without blocking runtime shutdown.
        let (sender, receiver) = tokio::sync::oneshot::channel();
        std::thread::Builder::new()
            .name("mcp-arguments".into())
            .spawn(move || {
                let result = if source == "-" {
                    read_arguments(std::io::stdin().lock())
                } else {
                    std::fs::File::open(&source[1..]).and_then(read_arguments)
                };
                let _ = sender.send(result);
            })
            .map_err(|error| ClientFailure::new("input", "input_error", error, false))?;
        receiver
            .await
            .map_err(|error| ClientFailure::new("input", "input_error", error, false))?
            .map_err(|error| ClientFailure::new("input", "input_error", error, false))?
    } else {
        source.into_bytes()
    };
    if bytes.len() > REQUEST_MAX_BYTES {
        return Err(ClientFailure::new(
            "input",
            "input_too_large",
            "arguments exceed 1 MiB",
            false,
        ));
    }
    match serde_json::from_slice(&bytes)
        .map_err(|error| ClientFailure::new("input", "invalid_json", error, false))?
    {
        Value::Object(arguments) => Ok(arguments),
        _ => Err(ClientFailure::new(
            "input",
            "invalid_arguments",
            "arguments must be a JSON object",
            false,
        )),
    }
}

fn read_arguments(reader: impl Read) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader
        .take((REQUEST_MAX_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    Ok(bytes)
}

async fn execute(
    mut child_command: Command,
    command: McpClientCommand,
    arguments: Option<Map<String, Value>>,
    deadline: Instant,
) -> Result<Value, ClientFailure> {
    let mut child = child_command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| ClientFailure::new("connect", "spawn_failed", error, false))?;
    let reader = child.stdout.take().expect("piped child stdout");
    let writer = child.stdin.take().expect("piped child stdin");
    let mut stderr = child.stderr.take().expect("piped child stderr");
    let mut drain =
        tokio::spawn(async move { tokio::io::copy(&mut stderr, &mut tokio::io::stderr()).await });
    let state = TransportState::default();
    let transport = ClientTransport::new(reader, writer, state.clone());
    let info = ClientInfo::new(
        ClientCapabilities::default(),
        Implementation::new("fluxcope-cli", env!("CARGO_PKG_VERSION")),
    );
    let operation = async {
        let client = info.serve(transport).await.map_err(|error| {
            state.take_failure().unwrap_or_else(|| {
                let code = match &error {
                    ClientInitializeError::ConnectionClosed(_)
                    | ClientInitializeError::ExpectedInitResponse(None)
                    | ClientInitializeError::TransportError { .. } => "disconnect",
                    _ => "protocol_error",
                };
                let mut failure = ClientFailure::new("initialize", code, &error, false);
                if let ClientInitializeError::JsonRpcError(error) = error {
                    failure.0["error"] = json!(error);
                }
                failure
            })
        })?;
        let result = async {
            match command {
                McpClientCommand::Tools { name } => {
                    let mut tools = Vec::new();
                    let mut cursor = None;
                    let mut completed = false;
                    let mut total_bytes = 0;
                    for _ in 0..MAX_TOOL_PAGES {
                        let page = client
                            .list_tools(Some(PaginatedRequestParams::default().with_cursor(cursor)))
                            .await
                            .map_err(|error| ClientFailure::service(error, &state))?;
                        total_bytes += serde_json::to_vec(&page)
                            .map_err(|error| {
                                ClientFailure::new("discovery", "protocol_error", error, false)
                            })?
                            .len();
                        if total_bytes > 16 * 1024 * 1024 {
                            return Err(ClientFailure::new(
                                "discovery",
                                "output_too_large",
                                "tool catalog exceeds 16 MiB",
                                false,
                            ));
                        }
                        if tools.len() + page.tools.len() > MAX_TOOLS {
                            return Err(ClientFailure::new(
                                "discovery",
                                "output_too_large",
                                "tool catalog exceeds 4096 tools",
                                false,
                            ));
                        }
                        tools.extend(page.tools);
                        cursor = page.next_cursor;
                        if cursor.is_none() {
                            completed = true;
                            break;
                        }
                    }
                    if !completed {
                        return Err(ClientFailure::new(
                            "discovery",
                            "output_too_large",
                            "tool catalog exceeds 128 pages",
                            false,
                        ));
                    }
                    if let Some(name) = name {
                        let tool = tools
                            .into_iter()
                            .find(|tool| tool.name.as_ref() == name)
                            .ok_or_else(|| {
                                ClientFailure::new(
                                    "discovery",
                                    "unknown_tool",
                                    format!("tool not advertised: {name}"),
                                    false,
                                )
                            })?;
                        Ok(json!({ "name": tool.name, "description": tool.description,
                            "inputSchema": tool.input_schema, "annotations": tool.annotations }))
                    } else {
                        Ok(Value::Array(
                            tools
                                .into_iter()
                                .map(|tool| {
                                    json!({
                                        "name": tool.name, "description": tool.description,
                                    })
                                })
                                .collect(),
                        ))
                    }
                }
                McpClientCommand::Call { tool, .. } => {
                    let result = client
                        .call_tool(
                            CallToolRequestParams::new(tool)
                                .with_arguments(arguments.unwrap_or_default()),
                        )
                        .await
                        .map_err(|error| ClientFailure::service(error, &state))?;
                    normalize(result)
                }
                McpClientCommand::Read { uri } => {
                    let result = client
                        .read_resource(ReadResourceRequestParams::new(uri))
                        .await
                        .map_err(|error| ClientFailure::service(error, &state))?;
                    serde_json::to_value(result).map_err(|error| {
                        ClientFailure::new("result", "protocol_error", error, false)
                    })
                }
            }
        }
        .await;
        Ok((result, client))
    };
    let result = tokio::select! {
        biased;
        result = operation => result,
        _ = tokio::time::sleep_until(deadline) => Err(ClientFailure::new(
            "command", "timeout", "command deadline exceeded; no automatic retry", state.dispatched())),
        signal = tokio::signal::ctrl_c() => Err(ClientFailure::new(
            "command", "interrupted", match signal { Ok(()) => "interrupted".to_string(), Err(error) => error.to_string() }, state.dispatched())),
    };
    let mut result = match result {
        Ok((result, mut client)) => {
            match tokio::time::timeout(SHUTDOWN_TIMEOUT, client.close()).await {
                Ok(Ok(_)) => {}
                Ok(Err(error)) => {
                    eprintln!("MCP shutdown warning: {error}; resolved result retained")
                }
                Err(error) => eprintln!("MCP shutdown warning: {error}; resolved result retained"),
            }
            result
        }
        Err(error) => Err(error),
    };
    // Kill-on-drop is the final fallback even if explicit kill/reaping stalls.
    if !matches!(
        tokio::time::timeout(SHUTDOWN_TIMEOUT, child.wait()).await,
        Ok(Ok(_))
    ) {
        match tokio::time::timeout(SHUTDOWN_TIMEOUT, child.kill()).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => eprintln!("MCP child cleanup warning: {error}"),
            Err(error) => eprintln!("MCP child cleanup warning: {error}"),
        }
    }
    match tokio::time::timeout(SHUTDOWN_TIMEOUT, &mut drain).await {
        Ok(Ok(Ok(_))) => {}
        Ok(Ok(Err(error))) => eprintln!("MCP stderr drain warning: {error}"),
        Ok(Err(error)) => eprintln!("MCP stderr drain warning: {error}"),
        Err(error) => {
            drain.abort();
            eprintln!("MCP stderr drain warning: {error}");
        }
    }
    if let Err(failure) = &mut result
        && state.dispatched()
    {
        failure.0["mutation_state"] = json!("unknown");
    }
    result
}

fn normalize(result: CallToolResult) -> Result<Value, ClientFailure> {
    let failed = result.is_error == Some(true);
    let (value, additional) = match result.structured_content {
        Some(value) => {
            let additional: Vec<_> = result
                .content
                .into_iter()
                .filter(|content| {
                    // Text is allowed to be prose. Only discard a successfully parsed,
                    // exact JSON mirror; retain all non-JSON and non-text diagnostics.
                    !content.as_text().is_some_and(|text| {
                        serde_json::from_str::<Value>(&text.text).is_ok_and(|text| text == value)
                    })
                })
                .collect();
            (value, additional)
        }
        None => (json!({ "content": result.content }), Vec::new()),
    };
    if failed {
        let code = value
            .get("code")
            .and_then(Value::as_str)
            .or_else(|| value.pointer("/error/code").and_then(Value::as_str))
            .unwrap_or("tool_error");
        let mut failure = ClientFailure::new("tool", code, "tool returned isError", true);
        failure.0["result"] = value;
        if !additional.is_empty() {
            failure.0["content"] = json!(additional);
        }
        Err(failure)
    } else if additional.is_empty() {
        Ok(value)
    } else {
        Ok(json!({ "result": value, "content": additional }))
    }
}
