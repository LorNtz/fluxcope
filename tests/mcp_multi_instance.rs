#![cfg(unix)]

use std::{
    collections::HashMap,
    fs,
    io::{Read, Write},
    net::{SocketAddr, TcpListener as StdTcpListener},
    os::unix::fs::{PermissionsExt, symlink},
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant as StdInstant},
};

use anyhow::{Context, Result, anyhow, bail, ensure};
use flate2::{Compression, write::GzEncoder};
use portable_pty::{Child as PtyChild, CommandBuilder, MasterPty, PtySize, native_pty_system};
use rmcp::{
    RoleClient, ServiceError, ServiceExt,
    model::{
        CallToolRequestParams, ClientCapabilities, ClientInfo, Implementation,
        ReadResourceRequestParams,
    },
    service::RunningService,
    transport::TokioChildProcess,
};
use serde_json::{Map, Value, json};
use tempfile::TempDir;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::Mutex as AsyncMutex,
    task::JoinHandle,
};

const PROCESS_TIMEOUT: Duration = Duration::from_secs(15);
const CALL_TIMEOUT: Duration = Duration::from_secs(20);
const CAPTURE_TIMEOUT: Duration = Duration::from_secs(15);
const OUTPUT_LIMIT: usize = 128 * 1024;

static ACCEPTANCE_LOCK: AsyncMutex<()> = AsyncMutex::const_new(());

type McpClient = RunningService<RoleClient, ClientInfo>;

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_fluxcope")
}

struct FluxcopeProcess {
    child: Box<dyn PtyChild + Send + Sync>,
    _master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    output: Arc<Mutex<Vec<u8>>>,
}

impl FluxcopeProcess {
    fn spawn(home: &Path, arguments: &[String]) -> Result<Self> {
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: 24,
                cols: 100,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("open Fluxcope PTY")?;
        let reader = pair
            .master
            .try_clone_reader()
            .context("clone Fluxcope PTY reader")?;
        let writer = pair
            .master
            .take_writer()
            .context("take Fluxcope PTY writer")?;
        let mut command = CommandBuilder::new(binary());
        command.args(arguments);
        command.env("HOME", home);
        command.env("TERM", "xterm-256color");
        command.env("RUST_LOG", "");
        let child = pair
            .slave
            .spawn_command(command)
            .context("spawn Fluxcope proxy")?;
        drop(pair.slave);

        let output = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&output);
        thread::spawn(move || drain_pty(reader, captured));
        Ok(Self {
            child,
            _master: pair.master,
            writer,
            output,
        })
    }

    fn output(&self) -> String {
        String::from_utf8_lossy(&self.output.lock().expect("PTY output lock")).into_owned()
    }

    fn wait_for_exit(&mut self, timeout: Duration) -> Result<()> {
        let deadline = StdInstant::now() + timeout;
        while StdInstant::now() < deadline {
            if self
                .child
                .try_wait()
                .context("poll Fluxcope process")?
                .is_some()
            {
                return Ok(());
            }
            thread::sleep(Duration::from_millis(20));
        }
        bail!("Fluxcope process did not exit; output: {}", self.output())
    }

    fn stop_gracefully(&mut self) -> Result<()> {
        self.writer
            .write_all(&[3])
            .context("send Ctrl-C to Fluxcope PTY")?;
        self.writer.flush().context("flush Fluxcope PTY")?;
        self.wait_for_exit(Duration::from_secs(5))
    }

    fn kill_and_wait(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
    }
}

impl Drop for FluxcopeProcess {
    fn drop(&mut self) {
        self.kill_and_wait();
    }
}

fn drain_pty(mut reader: Box<dyn Read + Send>, output: Arc<Mutex<Vec<u8>>>) {
    let mut buffer = [0_u8; 4096];
    while let Ok(read) = reader.read(&mut buffer) {
        if read == 0 {
            break;
        }
        let mut captured = output.lock().expect("PTY output lock");
        captured.extend_from_slice(&buffer[..read]);
        if captured.len() > OUTPUT_LIMIT {
            let excess = captured.len() - OUTPUT_LIMIT;
            captured.drain(..excess);
        }
    }
}

struct McpHarness {
    brokers: Vec<Option<McpClient>>,
    proxies: Vec<Option<FluxcopeProcess>>,
    home: TempDir,
}

impl McpHarness {
    fn new() -> Result<Self> {
        let home = tempfile::tempdir().context("create isolated HOME")?;
        let fluxcope_home = home.path().join(".fluxcope");
        fs::create_dir(&fluxcope_home).context("create isolated Fluxcope home")?;
        fs::set_permissions(&fluxcope_home, fs::Permissions::from_mode(0o700))?;
        Ok(Self {
            brokers: Vec::new(),
            proxies: Vec::new(),
            home,
        })
    }

    fn home(&self) -> &Path {
        self.home.path()
    }

    fn fluxcope_home(&self) -> PathBuf {
        self.home.path().join(".fluxcope")
    }

    fn default_config_path(&self) -> PathBuf {
        self.fluxcope_home().join("config.yml")
    }

    fn write_default_config(&self, port: u16) -> Result<PathBuf> {
        let path = self.default_config_path();
        write_config(&path, port)?;
        Ok(path)
    }

    fn write_read_only_config(&self, name: &str, port: u16) -> Result<PathBuf> {
        let path = self.home.path().join(name);
        write_config(&path, port)?;
        Ok(path)
    }

    fn spawn_proxy(&mut self, arguments: Vec<String>) -> Result<usize> {
        let process = FluxcopeProcess::spawn(self.home.path(), &arguments)?;
        self.proxies.push(Some(process));
        Ok(self.proxies.len() - 1)
    }

    fn spawn_default_proxy(&mut self) -> Result<usize> {
        self.spawn_proxy(vec!["--mcp".to_owned()])
    }

    fn spawn_read_only_proxy(&mut self, config: &Path) -> Result<usize> {
        self.spawn_proxy(vec![
            "--config".to_owned(),
            config.to_string_lossy().into_owned(),
            "--mcp".to_owned(),
        ])
    }

    fn spawn_temporary_proxy(&mut self, port: u16) -> Result<usize> {
        self.spawn_proxy(vec![
            "--host".to_owned(),
            "127.0.0.1".to_owned(),
            "--port".to_owned(),
            port.to_string(),
            "--mcp".to_owned(),
        ])
    }

    fn spawn_temporary_proxy_without_mcp(&mut self, port: u16) -> Result<usize> {
        self.spawn_proxy(vec![
            "--host".to_owned(),
            "127.0.0.1".to_owned(),
            "--port".to_owned(),
            port.to_string(),
        ])
    }

    async fn wait_for_proxy_listener(&self, index: usize, port: u16) -> Result<()> {
        let deadline = tokio::time::Instant::now() + PROCESS_TIMEOUT;
        loop {
            if TcpStream::connect(("127.0.0.1", port)).await.is_ok() {
                return Ok(());
            }
            if tokio::time::Instant::now() >= deadline {
                let output = self
                    .proxies
                    .get(index)
                    .and_then(Option::as_ref)
                    .map_or_else(
                        || "<process unavailable>".to_owned(),
                        FluxcopeProcess::output,
                    );
                bail!("proxy {port} did not listen; output: {output}");
            }
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
    }

    fn proxy_output(&self, index: usize) -> String {
        self.proxies
            .get(index)
            .and_then(Option::as_ref)
            .map_or_else(
                || "<process unavailable>".to_owned(),
                FluxcopeProcess::output,
            )
    }

    fn logs(&self) -> String {
        let logs = self.fluxcope_home().join("logs");
        let mut output = String::new();
        if let Ok(entries) = fs::read_dir(logs) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|extension| extension.to_str()) == Some("log") {
                    output.push_str(&format!("\n--- {} ---\n", path.display()));
                    output.push_str(&fs::read_to_string(path).unwrap_or_default());
                }
            }
        }
        output
    }

    fn proxy_mut(&mut self, index: usize) -> Result<&mut FluxcopeProcess> {
        self.proxies
            .get_mut(index)
            .and_then(Option::as_mut)
            .ok_or_else(|| anyhow!("proxy {index} is not running"))
    }

    fn stop_proxy(&mut self, index: usize, graceful: bool) -> Result<String> {
        let mut process = self
            .proxies
            .get_mut(index)
            .and_then(Option::take)
            .ok_or_else(|| anyhow!("proxy {index} is not running"))?;
        if graceful {
            process.stop_gracefully()?;
        } else {
            process.kill_and_wait();
        }
        Ok(process.output())
    }

    async fn start_broker(&mut self, client_name: &str) -> Result<usize> {
        let mut command = tokio::process::Command::new(binary());
        command
            .arg("mcp")
            .env("HOME", self.home.path())
            .env_remove("RUST_LOG")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let transport = TokioChildProcess::new(command).context("spawn MCP broker")?;
        let client_info = ClientInfo::new(
            ClientCapabilities::default(),
            Implementation::new(client_name, "1.0.0"),
        );
        let client = tokio::time::timeout(CALL_TIMEOUT, client_info.serve(transport))
            .await
            .context("initialize MCP broker timed out")??;
        self.brokers.push(Some(client));
        Ok(self.brokers.len() - 1)
    }

    fn broker(&self, index: usize) -> Result<&McpClient> {
        self.brokers
            .get(index)
            .and_then(Option::as_ref)
            .ok_or_else(|| anyhow!("broker {index} is not running"))
    }

    async fn stop_broker(&mut self, index: usize) -> Result<()> {
        let client = self
            .brokers
            .get_mut(index)
            .and_then(Option::take)
            .ok_or_else(|| anyhow!("broker {index} is not running"))?;
        tokio::time::timeout(Duration::from_secs(5), client.cancel())
            .await
            .context("MCP broker shutdown timed out")??;
        Ok(())
    }
}

impl Drop for McpHarness {
    fn drop(&mut self) {
        for process in self.proxies.iter_mut().flatten() {
            process.kill_and_wait();
        }
    }
}

fn write_config(path: &Path, port: u16) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(
        path,
        format!("server:\n  port: {port}\nmcp:\n  enable: true\n"),
    )?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(())
}

fn unused_port() -> Result<u16> {
    Ok(StdTcpListener::bind(("127.0.0.1", 0))?.local_addr()?.port())
}

async fn call_tool(client: &McpClient, name: &str, arguments: Value) -> Result<Value> {
    let mut parameters = CallToolRequestParams::new(name.to_owned());
    let object = arguments
        .as_object()
        .ok_or_else(|| anyhow!("tool arguments must be an object"))?;
    if !object.is_empty() {
        parameters = parameters.with_arguments(object.clone());
    }
    let result = tokio::time::timeout(CALL_TIMEOUT, client.call_tool(parameters))
        .await
        .with_context(|| format!("{name} timed out"))?
        .with_context(|| format!("{name} failed"))?;
    tool_json(&result)
}

async fn tool_error_code(client: &McpClient, name: &str, arguments: Value) -> Result<String> {
    let mut parameters = CallToolRequestParams::new(name.to_owned());
    if let Some(object) = arguments.as_object().filter(|object| !object.is_empty()) {
        parameters = parameters.with_arguments(object.clone());
    }
    let error = tokio::time::timeout(CALL_TIMEOUT, client.call_tool(parameters))
        .await
        .with_context(|| format!("{name} error timed out"))?
        .expect_err("tool call must fail");
    let ServiceError::McpError(error) = error else {
        bail!("{name} returned a non-MCP error: {error}");
    };
    error
        .data
        .and_then(|data| data.get("code").and_then(Value::as_str).map(str::to_owned))
        .ok_or_else(|| anyhow!("{name} error omitted its stable code"))
}

fn tool_json(result: &rmcp::model::CallToolResult) -> Result<Value> {
    if let Some(structured) = &result.structured_content {
        return Ok(structured.clone());
    }
    let encoded = serde_json::to_value(result)?;
    let text = encoded
        .pointer("/content/0/text")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("tool result omitted structured JSON"))?;
    serde_json::from_str(text).context("parse tool JSON text")
}

fn selector(instance: &Value) -> Value {
    instance["instance"].clone()
}

fn endpoint_port(instance: &Value) -> Result<u16> {
    let endpoint = instance["instance"]["proxy_endpoint"]
        .as_str()
        .ok_or_else(|| anyhow!("instance omitted proxy endpoint"))?;
    Ok(endpoint.parse::<SocketAddr>()?.port())
}

async fn list_instances(client: &McpClient) -> Result<Value> {
    call_tool(client, "list_instances", json!({})).await
}

async fn wait_for_instance_count(client: &McpClient, expected: usize) -> Result<Value> {
    let deadline = tokio::time::Instant::now() + PROCESS_TIMEOUT;
    loop {
        let listed = list_instances(client).await?;
        if listed["instances"].as_array().map(Vec::len) == Some(expected) {
            return Ok(listed);
        }
        if tokio::time::Instant::now() >= deadline {
            bail!("expected {expected} instances, got {listed}");
        }
        tokio::time::sleep(Duration::from_millis(40)).await;
    }
}

fn instances_by_port(listed: &Value) -> Result<HashMap<u16, Value>> {
    listed["instances"]
        .as_array()
        .ok_or_else(|| anyhow!("list_instances omitted instances"))?
        .iter()
        .map(|instance| Ok((endpoint_port(instance)?, instance.clone())))
        .collect()
}

fn gzip(bytes: &[u8]) -> Result<Vec<u8>> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(bytes)?;
    encoder.finish().map_err(Into::into)
}

struct OneShotUpstream {
    address: SocketAddr,
    task: JoinHandle<Result<Vec<u8>>>,
}

impl OneShotUpstream {
    async fn spawn(response_body: Vec<u8>, content_encoding: Option<&str>) -> Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
        let address = listener.local_addr()?;
        let encoding_header = content_encoding
            .map(|encoding| format!("Content-Encoding: {encoding}\r\n"))
            .unwrap_or_default();
        let task = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await?;
            let request = read_http_message(&mut stream).await?;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n{encoding_header}Content-Length: {}\r\nConnection: close\r\n\r\n",
                response_body.len()
            );
            stream.write_all(response.as_bytes()).await?;
            stream.write_all(&response_body).await?;
            stream.shutdown().await?;
            Ok(request)
        });
        Ok(Self { address, task })
    }

    async fn finish(self) -> Result<Vec<u8>> {
        tokio::time::timeout(CAPTURE_TIMEOUT, self.task)
            .await
            .context("upstream did not receive request")?
            .context("upstream task panicked")?
    }
}

async fn read_http_message(stream: &mut TcpStream) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 4096];
    let header_end = loop {
        let read = stream.read(&mut buffer).await?;
        if read == 0 {
            bail!("connection closed before HTTP headers");
        }
        bytes.extend_from_slice(&buffer[..read]);
        if let Some(index) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            break index + 4;
        }
        ensure!(bytes.len() <= 128 * 1024, "HTTP headers too large");
    };
    let header_text = String::from_utf8_lossy(&bytes[..header_end]);
    let content_length = header_text
        .lines()
        .find_map(|line| {
            line.split_once(':').and_then(|(name, value)| {
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().ok())
                    .flatten()
            })
        })
        .unwrap_or(0);
    while bytes.len() < header_end + content_length {
        let read = stream.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        bytes.extend_from_slice(&buffer[..read]);
    }
    Ok(bytes)
}

async fn proxy_exchange(
    proxy_port: u16,
    url: &str,
    request_body: &[u8],
    request_encoding: Option<&str>,
) -> Result<Vec<u8>> {
    let parsed = url::Url::parse(url)?;
    let host = parsed
        .host_str()
        .ok_or_else(|| anyhow!("test URL omitted host"))?;
    let host_header = parsed
        .port()
        .map(|port| format!("{host}:{port}"))
        .unwrap_or_else(|| host.to_owned());
    let encoding_header = request_encoding
        .map(|encoding| format!("Content-Encoding: {encoding}\r\n"))
        .unwrap_or_default();
    let mut stream = tokio::time::timeout(
        PROCESS_TIMEOUT,
        TcpStream::connect(("127.0.0.1", proxy_port)),
    )
    .await
    .context("connect to proxy timed out")??;
    let request = format!(
        "POST {url} HTTP/1.1\r\nHost: {host_header}\r\nContent-Type: application/json\r\n{encoding_header}Content-Length: {}\r\nConnection: close\r\n\r\n",
        request_body.len()
    );
    stream.write_all(request.as_bytes()).await?;
    stream.write_all(request_body).await?;
    let mut response = Vec::new();
    tokio::time::timeout(CAPTURE_TIMEOUT, stream.read_to_end(&mut response))
        .await
        .context("proxy response timed out")??;
    ensure!(
        response.starts_with(b"HTTP/1.1 200") || response.starts_with(b"HTTP/1.0 200"),
        "proxy response was not successful: {}",
        String::from_utf8_lossy(&response)
    );
    Ok(response)
}

async fn send_unique_capture(proxy_port: u16, token: &str) -> Result<String> {
    let response = format!(r#"{{"instance":"{token}"}}"#);
    let upstream = OneShotUpstream::spawn(response.into_bytes(), None).await?;
    let url = format!("http://{}/capture/{token}", upstream.address);
    proxy_exchange(proxy_port, &url, br#"{"request":true}"#, None).await?;
    let _ = upstream.finish().await?;
    Ok(url)
}

async fn search_until(client: &McpClient, instance: &Value, url: &str) -> Result<Value> {
    let deadline = tokio::time::Instant::now() + CAPTURE_TIMEOUT;
    loop {
        let result = call_tool(
            client,
            "search_captures",
            json!({
                "instance": instance,
                "query": {
                    "original_url": {"mode": "substring", "value": url},
                    "lifecycle": "complete"
                },
                "limit": 20
            }),
        )
        .await?;
        if result["captures"]
            .as_array()
            .is_some_and(|rows| !rows.is_empty())
        {
            return Ok(result);
        }
        if tokio::time::Instant::now() >= deadline {
            bail!("capture did not become searchable for {url}");
        }
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
}

async fn create_empty_preset(
    client: &McpClient,
    instance: &Value,
    revision: u64,
    name: &str,
) -> Result<Value> {
    call_tool(
        client,
        "create_preset",
        json!({
            "instance": instance,
            "expected_settings_revision": revision,
            "name": name
        }),
    )
    .await
}

async fn current_mapping(client: &McpClient, instance: &Value) -> Result<Value> {
    call_tool(
        client,
        "get_mapping_settings",
        json!({"instance": instance}),
    )
    .await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn multi_instance_modes_brokers_persistence_and_lifecycle_are_isolated() -> Result<()> {
    let _serial = ACCEPTANCE_LOCK.lock().await;
    let mut harness = McpHarness::new()?;
    let default_port = unused_port()?;
    let read_only_port = unused_port()?;
    let temporary_port = unused_port()?;
    let disabled_port = unused_port()?;
    let default_path = harness.write_default_config(default_port)?;
    let read_only_path = harness.write_read_only_config("read-only.yml", read_only_port)?;
    let read_only_original = fs::read(&read_only_path)?;

    let default_process = harness.spawn_default_proxy()?;
    let read_only_process = harness.spawn_read_only_proxy(&read_only_path)?;
    let temporary_process = harness.spawn_temporary_proxy(temporary_port)?;
    let disabled_process = harness.spawn_temporary_proxy_without_mcp(disabled_port)?;
    harness
        .wait_for_proxy_listener(default_process, default_port)
        .await?;
    harness
        .wait_for_proxy_listener(read_only_process, read_only_port)
        .await?;
    harness
        .wait_for_proxy_listener(temporary_process, temporary_port)
        .await?;
    harness
        .wait_for_proxy_listener(disabled_process, disabled_port)
        .await?;
    let broker_a = harness.start_broker("acceptance-a").await?;
    let listed_a = wait_for_instance_count(harness.broker(broker_a)?, 3).await?;
    let by_port = instances_by_port(&listed_a)?;
    assert_eq!(by_port[&default_port]["config_mode"], "default_owned");
    assert_eq!(by_port[&default_port]["persistence"], "persistent");
    assert_eq!(by_port[&read_only_port]["config_mode"], "read_only_file");
    assert_eq!(by_port[&read_only_port]["persistence"], "ephemeral");
    assert_eq!(by_port[&temporary_port]["config_mode"], "temporary");
    assert_eq!(by_port[&temporary_port]["persistence"], "ephemeral");
    assert!(
        !by_port.contains_key(&disabled_port),
        "an MCP-disabled proxy must keep forwarding without joining discovery"
    );

    let broker_b = harness.start_broker("acceptance-b").await?;
    let listed_b = wait_for_instance_count(harness.broker(broker_b)?, 3).await?;
    assert_eq!(listed_a["instances"], listed_b["instances"]);
    assert_eq!(
        tool_error_code(harness.broker(broker_a)?, "get_status", json!({})).await?,
        "instance_required"
    );

    let default_selector = selector(&by_port[&default_port]);
    let read_only_selector = selector(&by_port[&read_only_port]);
    let temporary_selector = selector(&by_port[&temporary_port]);
    for (client, instance, name) in [
        (
            harness.broker(broker_a)?,
            &default_selector,
            "persistent-preset",
        ),
        (
            harness.broker(broker_a)?,
            &read_only_selector,
            "readonly-preset",
        ),
        (
            harness.broker(broker_b)?,
            &temporary_selector,
            "temporary-preset",
        ),
    ] {
        let mapping = current_mapping(client, instance).await?;
        let revision = mapping["settings_revision"]
            .as_u64()
            .context("mapping revision")?;
        let changed = create_empty_preset(client, instance, revision, name).await?;
        assert_eq!(changed["outcome"], "committed");
    }
    assert!(fs::read_to_string(&default_path)?.contains("persistent-preset"));
    assert_eq!(fs::read(&read_only_path)?, read_only_original);
    assert!(!harness.home().join("temporary.yml").exists());

    let default_url = send_unique_capture(default_port, "default-only").await?;
    let read_only_url = send_unique_capture(read_only_port, "readonly-only").await?;
    let temporary_url = send_unique_capture(temporary_port, "temporary-only").await?;
    let client = harness.broker(broker_b)?;
    let (default_search, read_only_search, temporary_search) = tokio::join!(
        search_until(client, &default_selector, &default_url),
        search_until(client, &read_only_selector, &read_only_url),
        search_until(client, &temporary_selector, &temporary_url),
    );
    for (result, expected, unexpected) in [
        (default_search?, "default-only", "readonly-only"),
        (read_only_search?, "readonly-only", "temporary-only"),
        (temporary_search?, "temporary-only", "default-only"),
    ] {
        let encoded = serde_json::to_string(&result)?;
        assert!(encoded.contains(expected));
        assert!(!encoded.contains(unexpected));
        assert_eq!(result["captures"].as_array().map(Vec::len), Some(1));
    }

    let temporary_search = search_until(client, &temporary_selector, &temporary_url).await?;
    let old_capture = &temporary_search["captures"][0];
    let old_capture_id = old_capture["capture_sequence"].clone();
    let old_revision = old_capture["capture_revision"].clone();
    let old_detail = call_tool(
        client,
        "get_capture",
        json!({
            "instance": temporary_selector,
            "capture_id": old_capture_id,
            "expected_revision": old_revision
        }),
    )
    .await?;
    let old_resource = old_detail["body_resources"]["response"]["decoded"]
        .as_str()
        .context("old decoded resource")?
        .to_owned();
    let old_settings_revision =
        current_mapping(client, &temporary_selector).await?["settings_revision"]
            .as_u64()
            .context("old temporary settings revision")?;

    harness.stop_proxy(temporary_process, true)?;
    wait_for_instance_count(harness.broker(broker_b)?, 2).await?;
    let restarted_process = harness.spawn_temporary_proxy(temporary_port)?;
    harness
        .wait_for_proxy_listener(restarted_process, temporary_port)
        .await?;
    let restarted = wait_for_instance_count(harness.broker(broker_b)?, 3).await?;
    let restarted_by_port = instances_by_port(&restarted)?;
    let new_selector = selector(&restarted_by_port[&temporary_port]);
    assert_ne!(new_selector["run_id"], temporary_selector["run_id"]);
    assert_eq!(
        tool_error_code(
            harness.broker(broker_b)?,
            "get_status",
            json!({"instance": temporary_selector})
        )
        .await?,
        "instance_generation_conflict"
    );
    assert_eq!(
        tool_error_code(
            harness.broker(broker_b)?,
            "create_preset",
            json!({
                "instance": temporary_selector,
                "expected_settings_revision": old_settings_revision,
                "name": "stale-write"
            })
        )
        .await?,
        "instance_generation_conflict"
    );
    assert_eq!(
        tool_error_code(
            harness.broker(broker_b)?,
            "wait_for_capture",
            json!({
                "instance": temporary_selector,
                "milestone": "request_seen",
                "timeout_ms": 50
            })
        )
        .await?,
        "instance_generation_conflict"
    );
    let resource_error = harness
        .broker(broker_b)?
        .read_resource(ReadResourceRequestParams::new(old_resource))
        .await
        .expect_err("old run resource must fail");
    let ServiceError::McpError(resource_error) = resource_error else {
        bail!("old resource returned non-MCP error");
    };
    assert_eq!(
        resource_error.data.context("old resource error data")?["code"],
        "instance_generation_conflict"
    );

    harness.stop_broker(broker_a).await?;
    let after_broker_exit_url = send_unique_capture(default_port, "broker-exit-forwarding").await?;
    let after_broker_exit = search_until(
        harness.broker(broker_b)?,
        &default_selector,
        &after_broker_exit_url,
    )
    .await?;
    assert_eq!(
        after_broker_exit["captures"].as_array().map(Vec::len),
        Some(1)
    );
    let surviving = list_instances(harness.broker(broker_b)?).await?;
    let broker_c = harness.start_broker("acceptance-c").await?;
    let listed_c = wait_for_instance_count(harness.broker(broker_c)?, 3).await?;
    assert_eq!(
        surviving["instances"], listed_c["instances"],
        "a freshly launched broker must rediscover every surviving proxy"
    );
    harness.stop_broker(broker_c).await?;

    let duplicate = harness.spawn_default_proxy()?;
    harness
        .proxy_mut(duplicate)?
        .wait_for_exit(PROCESS_TIMEOUT)?;
    let duplicate_output = harness.stop_proxy(duplicate, false)?;
    assert!(
        duplicate_output.contains("already owned") || duplicate_output.contains("default config"),
        "default ownership failure was not explicit: {duplicate_output}"
    );

    let occupied = StdTcpListener::bind(("127.0.0.1", 0))?;
    let occupied_port = occupied.local_addr()?.port();
    let bind_failure = harness.spawn_temporary_proxy(occupied_port)?;
    harness
        .proxy_mut(bind_failure)?
        .wait_for_exit(PROCESS_TIMEOUT)?;
    let bind_output = harness.stop_proxy(bind_failure, false)?;
    assert!(
        bind_output.contains("bind proxy listener") || bind_output.contains("already in use"),
        "proxy bind failure was not explicit: {bind_output}"
    );
    drop(occupied);

    harness.stop_proxy(restarted_process, true)?;
    harness.stop_proxy(default_process, true)?;
    harness.stop_proxy(disabled_process, true)?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn broker_rejects_unsafe_registry_entries_without_losing_live_instances() -> Result<()> {
    let _serial = ACCEPTANCE_LOCK.lock().await;
    let mut harness = McpHarness::new()?;
    let port = unused_port()?;
    let process = harness.spawn_temporary_proxy(port)?;
    harness.wait_for_proxy_listener(process, port).await?;
    let broker = harness.start_broker("registry-security").await?;
    wait_for_instance_count(harness.broker(broker)?, 1).await?;

    let instances_dir = harness.fluxcope_home().join("run/instances");
    let wrong_mode_name = format!("{}.json", "a".repeat(64));
    let symlink_name = format!("{}.json", "b".repeat(64));
    let wrong_mode = instances_dir.join(&wrong_mode_name);
    fs::write(&wrong_mode, b"{}")?;
    fs::set_permissions(&wrong_mode, fs::Permissions::from_mode(0o644))?;
    let target = harness.home().join("descriptor-target.json");
    fs::write(&target, b"{}")?;
    symlink(&target, instances_dir.join(&symlink_name))?;
    let index_path = instances_dir.join(".registry-index.json");
    let mut index: Value = serde_json::from_slice(&fs::read(&index_path)?)?;
    let descriptors = index["descriptors"]
        .as_array_mut()
        .context("registry descriptor index")?;
    descriptors.push(json!(wrong_mode_name));
    descriptors.push(json!(symlink_name));
    fs::write(&index_path, serde_json::to_vec(&index)?)?;
    fs::set_permissions(&index_path, fs::Permissions::from_mode(0o600))?;

    let listed = list_instances(harness.broker(broker)?).await?;
    assert_eq!(listed["instances"].as_array().map(Vec::len), Some(1));
    let codes = listed["diagnostics"]["rejected"]
        .as_array()
        .context("rejected descriptors")?
        .iter()
        .filter_map(|entry| entry["code"].as_str())
        .collect::<Vec<_>>();
    assert!(codes.contains(&"descriptor_permissions"), "{codes:?}");
    assert!(codes.contains(&"descriptor_symlink"), "{codes:?}");
    assert!(wrong_mode.exists());
    assert!(target.exists());

    harness.stop_proxy(process, true)?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn official_client_completes_bounded_debugging_and_mapping_workflow() -> Result<()> {
    let _serial = ACCEPTANCE_LOCK.lock().await;
    let mut harness = McpHarness::new()?;
    let proxy_port = unused_port()?;
    let process = harness.spawn_temporary_proxy(proxy_port)?;
    harness.wait_for_proxy_listener(process, proxy_port).await?;
    let broker = harness.start_broker("debug-workflow").await?;
    let listed = wait_for_instance_count(harness.broker(broker)?, 1)
        .await
        .with_context(|| {
            format!(
                "proxy output: {}\nlogs: {}",
                harness.proxy_output(process),
                harness.logs()
            )
        })?;
    let instance = selector(&listed["instances"][0]);
    let client = harness.broker(broker)?;

    let broker_status = call_tool(client, "get_broker_status", json!({})).await?;
    assert_eq!(broker_status["limits"]["public_calls"], 32);
    let status = call_tool(client, "get_status", json!({"instance": instance})).await?;
    assert_eq!(status["config_mode"], "temporary");
    assert!(status["warnings"].is_array());
    let recording = call_tool(
        client,
        "set_recording_enabled",
        json!({"instance": instance, "enabled": true}),
    )
    .await?;
    assert_eq!(recording["current"], true);

    let request_json = br#"{"client":{"trace":"request-secret"}}"#;
    let response_json = br#"{"items":[{"id":"alpha","role":"reader"},{"id":"beta","role":"writer"}],"token":"traffic-secret"}"#;
    let request_body = gzip(request_json)?;
    let response_body = gzip(response_json)?;
    let upstream = OneShotUpstream::spawn(response_body, Some("gzip")).await?;
    let captured_url = format!("http://{}/workflow/compressed", upstream.address);
    let wait_arguments = json!({
        "instance": instance,
        "query": {"original_url": {"mode": "substring", "value": "/workflow/compressed"}},
        "milestone": "exchange_terminal",
        "timeout_ms": 10_000
    });
    let wait = call_tool(client, "wait_for_capture", wait_arguments);
    let traffic = async {
        tokio::time::sleep(Duration::from_millis(50)).await;
        proxy_exchange(proxy_port, &captured_url, &request_body, Some("gzip")).await
    };
    let (waited, response) = tokio::join!(wait, traffic);
    let waited = waited?;
    let _ = response.with_context(|| {
        format!(
            "proxy output: {}\nlogs: {}",
            harness.proxy_output(process),
            harness.logs()
        )
    })?;
    let upstream_request = upstream.finish().await?;
    assert!(upstream_request.ends_with(&request_body));
    assert_eq!(waited["matched"], true);

    let searched = search_until(client, &instance, &captured_url).await?;
    let capture = &searched["captures"][0];
    let capture_id = capture["capture_sequence"].clone();
    let capture_revision = capture["capture_revision"].clone();
    let detail = call_tool(
        client,
        "get_capture",
        json!({
            "instance": instance,
            "capture_id": capture_id,
            "expected_revision": capture_revision
        }),
    )
    .await?;
    assert_eq!(detail["capture"]["original_url"], captured_url);
    assert_eq!(detail["capture"]["status"], 200);
    assert_eq!(detail["capture"]["lifecycle"], "complete");

    let pointers = call_tool(
        client,
        "find_json_pointers",
        json!({
            "instance": instance,
            "capture_id": capture_id,
            "capture_revision": capture_revision,
            "side": "response",
            "field_name": "id",
            "match_mode": "exact",
            "limit": 20
        }),
    )
    .await?;
    assert_eq!(pointers["total_matches"], 2);
    assert_eq!(pointers["matches"][0]["pointer"], "/items/0/id");

    let probed = call_tool(
        client,
        "probe_json_pointer_pattern",
        json!({
            "instance": instance,
            "capture_id": capture_id,
            "capture_revision": capture_revision,
            "side": "response",
            "pattern": "/items/*/id"
        }),
    )
    .await?;
    assert_eq!(probed["match_count"], 2);

    let body_search = call_tool(
        client,
        "search_capture_body",
        json!({
            "instance": instance,
            "capture_id": capture_id,
            "capture_revision": capture_revision,
            "side": "response",
            "query": "beta",
            "limit": 10,
            "context_bytes": 32
        }),
    )
    .await?;
    assert_eq!(body_search["total_matches"], 1);

    let extracted = call_tool(
        client,
        "extract_capture_body",
        json!({
            "instance": instance,
            "capture_id": capture_id,
            "capture_revision": capture_revision,
            "side": "response",
            "selector": {"kind": "json_pointer", "pointer": "/items/1/id"}
        }),
    )
    .await?;
    assert!(
        extracted["inline"]
            .as_str()
            .is_some_and(|value| value.contains("beta"))
    );

    let decoded_uri = detail["body_resources"]["response"]["decoded"]
        .as_str()
        .context("decoded body resource URI")?;
    let resource = client
        .read_resource(ReadResourceRequestParams::new(decoded_uri))
        .await?;
    let resource_json = serde_json::to_value(&resource.contents[0])?;
    assert!(
        resource_json["text"]
            .as_str()
            .is_some_and(|text| text.contains("traffic-secret"))
    );
    assert_eq!(
        resource_json["_meta"]["fluxcope"]["decoded_encoding_chain"],
        json!(["gzip"])
    );

    let mapping_upstream = OneShotUpstream::spawn(
        br#"{"mapped":true,"marker":"mapped-response-secret"}"#.to_vec(),
        None,
    )
    .await?;
    let mapping_source = "http://map-source.invalid/workflow";
    let mapping_target = format!("http://{}/workflow", mapping_upstream.address);
    let proposed = json!({
        "enabled": true,
        "active_preset": "workflow-map",
        "presets": [{
            "name": "workflow-map",
            "map_remote": {
                "enabled": true,
                "rules": [{"from": mapping_source, "to": mapping_target, "enabled": true}]
            },
            "map_local": {"enabled": true, "rules": []}
        }]
    });
    let validated = call_tool(
        client,
        "validate_mapping_settings",
        json!({"instance": instance, "proxy": proposed}),
    )
    .await?;
    assert_eq!(validated["validation"]["diagnostics_total"], 0);
    let explained = call_tool(
        client,
        "explain_mapping",
        json!({
            "instance": instance,
            "url": mapping_source,
            "proposed_proxy": proposed
        }),
    )
    .await?;
    assert_eq!(explained["explanation"]["effective_url"], mapping_target);

    let mapping = current_mapping(client, &instance).await?;
    let initial_revision = mapping["settings_revision"].as_u64().context("revision")?;
    let created = call_tool(
        client,
        "create_preset",
        json!({
            "instance": instance,
            "expected_settings_revision": initial_revision,
            "name": "workflow-map",
            "initial": {
                "map_remote": {
                    "enabled": true,
                    "rules": [{"from": mapping_source, "to": mapping_target, "enabled": true}]
                },
                "map_local": {"enabled": true, "rules": []}
            }
        }),
    )
    .await?;
    let created_revision = created["settings_revision"]
        .as_u64()
        .context("created revision")?;
    let activated = call_tool(
        client,
        "set_active_preset",
        json!({
            "instance": instance,
            "expected_settings_revision": created_revision,
            "name": "workflow-map"
        }),
    )
    .await?;
    let active_revision = activated["settings_revision"]
        .as_u64()
        .context("active revision")?;
    assert_eq!(activated["outcome"], "committed");
    assert_eq!(
        tool_error_code(
            client,
            "set_mapping_gate",
            json!({
                "instance": instance,
                "expected_settings_revision": created_revision,
                "gate": {"kind": "global"},
                "enabled": false
            })
        )
        .await?,
        "settings_revision_conflict"
    );

    let mapped_response = proxy_exchange(proxy_port, mapping_source, b"{}", None).await?;
    assert!(mapped_response.ends_with(br#"{"mapped":true,"marker":"mapped-response-secret"}"#));
    let _ = mapping_upstream.finish().await?;
    let mapped_search = search_until(client, &instance, mapping_source).await?;
    let mapped_capture = &mapped_search["captures"][0];
    let mapped_detail = call_tool(
        client,
        "get_capture",
        json!({
            "instance": instance,
            "capture_id": mapped_capture["capture_sequence"],
            "expected_revision": mapped_capture["capture_revision"]
        }),
    )
    .await?;
    assert_eq!(mapped_detail["capture"]["mapping_path"], "remote_only");
    assert_eq!(mapped_detail["capture"]["effective_url"], mapping_target);

    let final_status = call_tool(client, "get_status", json!({"instance": instance})).await?;
    assert_eq!(final_status["settings_revision"], active_revision);
    assert!(
        final_status["audit"]["mutations"]
            .as_array()
            .is_some_and(|records| records.len() >= 2)
    );
    let audit_lines = wait_for_audit_lines(&harness.fluxcope_home(), 2).await?;
    let encoded_audit = audit_lines.join("\n");
    assert!(encoded_audit.contains("create_preset"));
    assert!(encoded_audit.contains("set_active_preset"));
    for secret in [
        "request-secret",
        "traffic-secret",
        "mapped-response-secret",
        mapping_source,
        mapping_target.as_str(),
    ] {
        assert!(
            !encoded_audit.contains(secret),
            "audit leaked sensitive value {secret:?}: {encoded_audit}"
        );
    }

    harness.stop_proxy(process, true)?;
    Ok(())
}

async fn wait_for_audit_lines(fluxcope_home: &Path, minimum: usize) -> Result<Vec<String>> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let mut audit_lines = Vec::new();
        let logs = fluxcope_home.join("logs");
        if let Ok(entries) = fs::read_dir(&logs) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|extension| extension.to_str()) != Some("log") {
                    continue;
                }
                let content = fs::read_to_string(path).unwrap_or_default();
                audit_lines.extend(
                    content
                        .lines()
                        .filter(|line| line.contains("[fluxcope::mcp_audit]"))
                        .map(str::to_owned),
                );
            }
        }
        if audit_lines.len() >= minimum {
            for line in &audit_lines {
                let payload = line
                    .split_once("] ")
                    .map(|(_, payload)| payload)
                    .context("audit log prefix")?;
                let _: Map<String, Value> = serde_json::from_str(payload)?;
            }
            return Ok(audit_lines);
        }
        if tokio::time::Instant::now() >= deadline {
            bail!("expected {minimum} structured MCP audit lines, found {audit_lines:?}");
        }
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
}
