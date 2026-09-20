use crate::{
    control_rpc::protocol::{ControlResult, InstanceScope, RPC_VERSION},
    instance::RunId,
    instance_registry::InstanceDescriptor,
    settings::{ConfigMode, PersistenceMode},
};
use serde_json::{Value, json};
use std::{
    net::SocketAddr,
    path::Path,
    str::FromStr,
    sync::{Arc, Condvar, LazyLock, Mutex, MutexGuard},
    thread::ThreadId,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    sync::mpsc,
};

pub(crate) const RUN_ID: &str = "AAAAAAAAAAAAAAAAAAAAAA";
pub(crate) const OTHER_RUN_ID: &str = "AQEBAQEBAQEBAQEBAQEBAQ";
pub(crate) const ENDPOINT: &str = "127.0.0.1:19001";
pub(crate) const OTHER_ENDPOINT: &str = "127.0.0.1:19002";

pub(crate) fn run_id() -> RunId {
    RunId::from_str(RUN_ID).expect("canonical test run ID")
}

pub(crate) fn endpoint() -> SocketAddr {
    ENDPOINT.parse().expect("test endpoint")
}

pub(crate) fn instance_scope() -> InstanceScope {
    InstanceScope {
        proxy_endpoint: endpoint(),
        run_id: run_id(),
    }
}

pub(crate) fn describe_result() -> ControlResult {
    ControlResult::DescribeInstance {
        instance: instance_scope(),
        config_mode: ConfigMode::Temporary,
        persistence: PersistenceMode::Ephemeral,
        recording_enabled: false,
        retained_capture_count: 0,
        settings_revision: 0,
    }
}

pub(crate) fn request_value(run_id: &str, deadline_ms: u64) -> Value {
    json!({
        "protocol_version": RPC_VERSION,
        "request_id": "request-1",
        "run_id": run_id,
        "deadline_ms": deadline_ms,
        "client": {"name": "test-client", "version": "1.0"},
        "operation": "describe_instance",
        "arguments": {}
    })
}

pub(crate) fn request_json(run_id: &str, deadline_ms: u64) -> Vec<u8> {
    serde_json::to_vec(&request_value(run_id, deadline_ms)).expect("request JSON")
}

pub(crate) fn success_response_value(request_id: &str, scope: Value) -> Value {
    json!({
        "protocol_version": RPC_VERSION,
        "request_id": request_id,
        "result": {
            "operation": "describe_instance",
            "instance": scope,
            "config_mode": "temporary",
            "persistence": "ephemeral",
            "recording_enabled": false,
            "retained_capture_count": 0,
            "settings_revision": 0
        }
    })
}

pub(crate) fn scope_value(endpoint: &str, run_id: &str) -> Value {
    json!({"proxy_endpoint": endpoint, "run_id": run_id})
}

pub(crate) fn framed(payload: &[u8]) -> Vec<u8> {
    let length = u32::try_from(payload.len()).expect("test frame length");
    let mut frame = Vec::with_capacity(4 + payload.len());
    frame.extend_from_slice(&length.to_be_bytes());
    frame.extend_from_slice(payload);
    frame
}

pub(crate) async fn write_payload<W>(writer: &mut W, payload: &[u8])
where
    W: AsyncWrite + Unpin,
{
    writer
        .write_all(&framed(payload))
        .await
        .expect("write framed payload");
}

pub(crate) async fn read_payload<R>(reader: &mut R) -> Vec<u8>
where
    R: AsyncRead + Unpin,
{
    let mut prefix = [0_u8; 4];
    reader
        .read_exact(&mut prefix)
        .await
        .expect("read frame prefix");
    let length = u32::from_be_bytes(prefix) as usize;
    let mut payload = vec![0_u8; length];
    reader
        .read_exact(&mut payload)
        .await
        .expect("read frame payload");
    payload
}

pub(crate) fn descriptor(socket_path: &Path) -> InstanceDescriptor {
    let value = json!({
        "schema_version": 1,
        "rpc_version": RPC_VERSION,
        "binary_version": "test",
        "pid": std::process::id(),
        "proxy_endpoint": ENDPOINT,
        "local_proxy_url": "http://127.0.0.1:19001",
        "run_id": RUN_ID,
        "started_at": "2026-08-24T00:00:00Z",
        "socket_path": socket_path,
        "config_mode": "temporary",
        "persistence": "ephemeral",
        "config_source": null
    });
    let encoded = serde_json::to_string(&value).expect("serialize test instance descriptor");
    serde_json::from_str(&encoded).expect("test instance descriptor")
}

struct ArgumentParseProbeInner {
    request_id: String,
    entered: mpsc::Sender<ThreadId>,
    release: Arc<(Mutex<bool>, Condvar)>,
}

static ARGUMENT_PARSE_PROBE_SERIAL: Mutex<()> = Mutex::new(());
static ARGUMENT_PARSE_PROBE: LazyLock<Mutex<Option<Arc<ArgumentParseProbeInner>>>> =
    LazyLock::new(|| Mutex::new(None));

pub(crate) struct ArgumentParseProbe {
    _serial: MutexGuard<'static, ()>,
    entered: mpsc::Receiver<ThreadId>,
    release: Arc<(Mutex<bool>, Condvar)>,
}

impl ArgumentParseProbe {
    pub(crate) async fn entered(&mut self) -> ThreadId {
        self.entered.recv().await.expect("argument parser entered")
    }

    pub(crate) fn release(&self) {
        let (lock, wake) = &*self.release;
        *lock.lock().expect("argument parser gate") = true;
        wake.notify_all();
    }
}

impl Drop for ArgumentParseProbe {
    fn drop(&mut self) {
        self.release();
        *ARGUMENT_PARSE_PROBE
            .lock()
            .expect("argument parser probe slot") = None;
    }
}

pub(crate) fn install_argument_parse_probe(request_id: &str) -> ArgumentParseProbe {
    let serial = ARGUMENT_PARSE_PROBE_SERIAL
        .lock()
        .expect("argument parser probe serial lock");
    let (entered_tx, entered_rx) = mpsc::channel(1);
    let release = Arc::new((Mutex::new(false), Condvar::new()));
    *ARGUMENT_PARSE_PROBE
        .lock()
        .expect("argument parser probe slot") = Some(Arc::new(ArgumentParseProbeInner {
        request_id: request_id.to_owned(),
        entered: entered_tx,
        release: Arc::clone(&release),
    }));
    ArgumentParseProbe {
        _serial: serial,
        entered: entered_rx,
        release,
    }
}

pub(crate) fn notify_argument_parse_probe(request_id: &str) {
    let probe = ARGUMENT_PARSE_PROBE
        .lock()
        .expect("argument parser probe slot")
        .clone();
    let Some(probe) = probe else {
        return;
    };
    if probe.request_id != request_id {
        return;
    }
    probe
        .entered
        .blocking_send(std::thread::current().id())
        .expect("argument parser observer");
    let (lock, wake) = &*probe.release;
    let mut released = lock.lock().expect("argument parser gate");
    while !*released {
        released = wake.wait(released).expect("argument parser gate wait");
    }
}
