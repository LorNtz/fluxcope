#![cfg(unix)]

use std::{process::Stdio, time::Duration};

use anyhow::{Context, Result};
use serde_json::{Value, json};
use tokio::{io::AsyncWriteExt, process::Command};

fn client(home: &std::path::Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_wirelens"));
    command
        .arg("mcp")
        .env("HOME", home)
        .env_remove("RUST_LOG")
        .kill_on_drop(true);
    command
}

#[tokio::test]
async fn installed_client_discovers_compact_schemas_and_calls_after_initialization() -> Result<()> {
    let home = tempfile::tempdir()?;
    let catalog = client(home.path()).arg("tools").output().await?;
    assert!(
        catalog.status.success(),
        "{}",
        String::from_utf8_lossy(&catalog.stderr)
    );
    let catalog: Value = serde_json::from_slice(&catalog.stdout)?;
    let tools = catalog.as_array().context("tool catalog")?;
    assert!(tools.iter().any(|tool| tool["name"] == "list_instances"));
    assert!(
        tools
            .iter()
            .all(|tool| tool.get("inputSchema").is_none() && tool.get("outputSchema").is_none())
    );

    let schema = client(home.path())
        .args(["tools", "list_instances"])
        .output()
        .await?;
    assert!(
        schema.status.success(),
        "{}",
        String::from_utf8_lossy(&schema.stderr)
    );
    let schema: Value = serde_json::from_slice(&schema.stdout)?;
    assert_eq!(schema["name"], "list_instances");
    assert!(schema["inputSchema"].is_object());
    assert!(schema.get("outputSchema").is_none());

    let call = client(home.path())
        .args(["call", "list_instances", "--arguments", "{}"])
        .output()
        .await?;
    assert!(
        call.status.success(),
        "{}",
        String::from_utf8_lossy(&call.stderr)
    );
    let result: Value = serde_json::from_slice(&call.stdout)?;
    assert_eq!(result["instances"], json!([]));
    assert!(result.get("structuredContent").is_none());
    assert!(result.get("content").is_none());
    assert!(!String::from_utf8_lossy(&call.stdout).contains("mcp_disabled"));
    Ok(())
}

#[tokio::test]
async fn argument_files_and_stdin_are_owned_independently_of_broker_stdin() -> Result<()> {
    let home = tempfile::tempdir()?;
    let arguments = home.path().join("arguments.json");
    tokio::fs::write(&arguments, b"{}").await?;
    let from_file = client(home.path())
        .args(["call", "list_instances", "--arguments"])
        .arg(format!("@{}", arguments.display()))
        .output()
        .await?;
    assert!(
        from_file.status.success(),
        "{}",
        String::from_utf8_lossy(&from_file.stderr)
    );
    let mut child = client(home.path())
        .args(["call", "list_instances", "--arguments", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let mut stdin = child.stdin.take().context("stdin")?;
    stdin.write_all(b"{}").await?;
    drop(stdin);
    let from_stdin =
        tokio::time::timeout(Duration::from_secs(10), child.wait_with_output()).await??;
    assert!(
        from_stdin.status.success(),
        "{}",
        String::from_utf8_lossy(&from_stdin.stderr)
    );
    let from_file: Value = serde_json::from_slice(&from_file.stdout)?;
    let from_stdin: Value = serde_json::from_slice(&from_stdin.stdout)?;
    assert_eq!(from_stdin, from_file);
    Ok(())
}

#[tokio::test]
async fn invalid_arguments_fail_before_dispatch_and_preserve_parse_diagnostics() -> Result<()> {
    let home = tempfile::tempdir()?;
    for (arguments, code) in [
        ("{bad", "invalid_json"),
        ("[]", "invalid_arguments"),
        ("null", "invalid_arguments"),
    ] {
        let output = client(home.path())
            .args(["call", "list_instances", "--arguments", arguments])
            .output()
            .await?;
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        let error = String::from_utf8(output.stderr)?;
        assert!(error.contains(code), "{error}");
        assert!(error.contains("not_started"), "{error}");
    }
    let invalid_file = home.path().join("large.json");
    tokio::fs::write(&invalid_file, vec![b' '; 1024 * 1024 + 1]).await?;
    let output = client(home.path())
        .args(["call", "list_instances", "--arguments"])
        .arg(format!("@{}", invalid_file.display()))
        .output()
        .await?;
    assert!(!output.status.success());
    assert!(String::from_utf8(output.stderr)?.contains("input_too_large"));
    Ok(())
}

#[tokio::test]
async fn protocol_errors_and_unknown_resource_reads_exit_nonzero() -> Result<()> {
    let home = tempfile::tempdir()?;
    let call = client(home.path())
        .args(["call", "no_such_tool", "--arguments", "{}"])
        .output()
        .await?;
    assert!(!call.status.success());
    assert!(call.stdout.is_empty());
    let error = String::from_utf8(call.stderr)?;
    assert!(error.contains("unknown"), "{error}");
    assert!(error.contains("protocol_error"), "{error}");
    let read = client(home.path())
        .args(["read", "wirelens://invalid"])
        .output()
        .await?;
    assert!(!read.status.success());
    let error = String::from_utf8(read.stderr)?;
    assert!(error.contains("not_started"), "{error}");
    assert!(error.contains("invalid_argument"), "{error}");
    Ok(())
}

#[tokio::test]
async fn stalled_argument_stdin_obeys_deadline_without_waiting_for_eof() -> Result<()> {
    let home = tempfile::tempdir()?;
    let mut child = client(home.path())
        .args([
            "call",
            "list_instances",
            "--arguments",
            "-",
            "--timeout-secs",
            "1",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let _open_stdin = child.stdin.take().context("stdin")?;
    let output = tokio::time::timeout(Duration::from_secs(5), child.wait_with_output()).await??;
    assert!(!output.status.success());
    let error = String::from_utf8(output.stderr)?;
    assert!(error.contains("timeout"), "{error}");
    assert!(error.contains("not_started"), "{error}");
    Ok(())
}
