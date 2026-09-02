use crate::{
    control::{CaptureStoreRuntimeStatus, InstanceRuntimeMetrics, MappingRuntimeStatus},
    control_rpc::{
        client::ControlRpcClient,
        protocol::{ControlError, ControlOperation, ControlResult, DeclaredClient, InstanceScope},
        server::{ControlCallContext, ControlRpcHandler, ControlRpcServer},
    },
    instance::InstanceIdentity,
    instance_registry::{InstanceDescriptor, RegistryPublisher, RegistryScanner},
    mcp::broker::Broker,
    settings::{AppSettings, ConfigMode, PersistenceMode, SettingsSession},
};
use std::{
    fs,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    os::unix::fs::PermissionsExt as _,
    sync::Arc,
    time::{Duration, Instant},
};
use tempfile::TempDir;
use tokio::{net::UnixListener, runtime::Runtime, task::JoinHandle};
use tokio_util::sync::CancellationToken;

const BENCHMARK_DEADLINE: Duration = Duration::from_secs(10);

fn benchmark_client() -> DeclaredClient {
    DeclaredClient {
        name: "criterion".to_owned(),
        version: "1".to_owned(),
    }
}

#[derive(Clone)]
struct BenchmarkHandler {
    instance: InstanceScope,
    config_mode: ConfigMode,
    persistence: PersistenceMode,
}

impl ControlRpcHandler for BenchmarkHandler {
    async fn handle(
        &self,
        _context: ControlCallContext,
        operation: ControlOperation,
        _cancelled: CancellationToken,
    ) -> Result<ControlResult, ControlError> {
        match operation {
            ControlOperation::DescribeInstance => Ok(ControlResult::DescribeInstance {
                instance: self.instance.clone(),
                config_mode: self.config_mode,
                persistence: self.persistence,
                recording_enabled: true,
                retained_capture_count: 10_000,
                settings_revision: 7,
            }),
            ControlOperation::GetStatus => Ok(ControlResult::GetStatus {
                instance: self.instance.clone(),
                local_proxy_url: format!("http://{}", self.instance.proxy_endpoint),
                wirelens_version: env!("CARGO_PKG_VERSION").to_owned(),
                rpc_version: 1,
                config_source: None,
                config_mode: self.config_mode,
                persistence: self.persistence,
                recording_enabled: true,
                retained_capture_count: 10_000,
                settings_revision: 7,
                mapping: MappingRuntimeStatus::default(),
                capture_store: CaptureStoreRuntimeStatus::default(),
                capture_change_epoch: 1,
                metrics: Box::new(InstanceRuntimeMetrics::default()),
                private_rpc: crate::control::ControlRpcRuntimeStatus::default(),
                body_work: Box::new(crate::control::BodyWorkRuntimeStatus::default()),
                search_work: crate::control::SearchWorkRuntimeStatus::default(),
                audit: Box::default(),
            }),
            _ => Err(ControlError::invalid_argument(
                "benchmark handler does not implement this operation",
            )),
        }
    }
}

pub struct LiveInstanceFixture {
    runtime: Option<Runtime>,
    home: TempDir,
    scanner: RegistryScanner,
    broker: Broker,
    descriptors: Vec<InstanceDescriptor>,
    publishers: Vec<RegistryPublisher>,
    tasks: Vec<JoinHandle<()>>,
    shutdown: CancellationToken,
}

impl LiveInstanceFixture {
    pub fn descriptor_count(&self) -> usize {
        self.descriptors.len()
    }

    pub fn endpoint(&self, index: usize) -> SocketAddr {
        self.descriptors[index % self.descriptors.len()].proxy_endpoint()
    }

    pub fn run_id(&self, index: usize) -> String {
        self.descriptors[index % self.descriptors.len()]
            .run_id()
            .to_string()
    }

    pub fn wirelens_home(&self) -> &std::path::Path {
        self.home.path()
    }
}

impl Drop for LiveInstanceFixture {
    fn drop(&mut self) {
        self.shutdown.cancel();
        for task in self.tasks.drain(..) {
            task.abort();
        }
        self.publishers.clear();
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_background();
        }
    }
}

pub fn live_instance_fixture(instance_count: usize) -> LiveInstanceFixture {
    assert!((1..=256).contains(&instance_count));
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(8)
        .enable_all()
        .build()
        .expect("benchmark runtime should build");
    let home = tempfile::tempdir().expect("benchmark home should be created");
    fs::set_permissions(home.path(), fs::Permissions::from_mode(0o700))
        .expect("benchmark home should be owner-only");
    let shutdown = CancellationToken::new();
    let mut publishers = Vec::with_capacity(instance_count);
    let mut tasks = Vec::with_capacity(instance_count);

    runtime.block_on(async {
        for index in 0..instance_count {
            let endpoint = SocketAddr::new(
                IpAddr::V4(Ipv4Addr::LOCALHOST),
                20_000_u16 + u16::try_from(index).expect("instance index should fit"),
            );
            let identity = InstanceIdentity::new(endpoint).expect("identity should be generated");
            let scope = InstanceScope {
                proxy_endpoint: endpoint,
                run_id: identity.run_id().clone(),
            };
            let settings = SettingsSession::temporary(AppSettings::default());
            let mut publisher =
                RegistryPublisher::prepare(home.path(), identity.clone(), &settings)
                    .expect("registry publisher should prepare");
            let listener = publisher
                .take_listener()
                .expect("control listener should be available");
            listener
                .set_nonblocking(true)
                .expect("control listener should become nonblocking");
            let listener = UnixListener::from_std(listener)
                .expect("control listener should become asynchronous");
            let server = Arc::new(ControlRpcServer::new(
                identity,
                BenchmarkHandler {
                    instance: scope,
                    config_mode: ConfigMode::Temporary,
                    persistence: PersistenceMode::Ephemeral,
                },
            ));
            let cancelled = shutdown.child_token();
            tasks.push(tokio::spawn(async move {
                loop {
                    tokio::select! {
                        biased;
                        () = cancelled.cancelled() => break,
                        accepted = listener.accept() => {
                            let Ok((stream, _)) = accepted else { break };
                            let server = Arc::clone(&server);
                            let call_cancelled = cancelled.child_token();
                            tokio::spawn(async move {
                                let _ = server.serve_connection_until(stream, call_cancelled).await;
                            });
                        }
                    }
                }
            }));
            publisher.publish().expect("descriptor should publish");
            publishers.push(publisher);
        }
    });

    let scanner = RegistryScanner::new(home.path()).expect("registry scanner should open");
    let descriptors = scanner.scan_all().expect("registry should scan").candidates;
    for publisher in &mut publishers {
        publisher.benchmark_disarm_cleanup();
    }
    let broker = Broker::new(home.path()).expect("broker should initialize");
    LiveInstanceFixture {
        runtime: Some(runtime),
        home,
        scanner,
        broker,
        descriptors,
        publishers,
        tasks,
        shutdown,
    }
}

pub fn registry_full_scan(fixture: &LiveInstanceFixture) -> usize {
    fixture
        .scanner
        .scan_all()
        .expect("benchmark registry scan should succeed")
        .candidates
        .len()
}

pub fn registry_targeted_lookup(fixture: &LiveInstanceFixture, index: usize) -> usize {
    fixture
        .scanner
        .read_endpoint(fixture.endpoint(index))
        .expect("benchmark targeted lookup should succeed")
        .candidates
        .len()
}

pub async fn broker_full_discovery(fixture: &LiveInstanceFixture) -> usize {
    fixture
        .broker
        .discover(
            benchmark_client(),
            tokio::time::Instant::now() + BENCHMARK_DEADLINE,
            CancellationToken::new(),
        )
        .await
        .expect("benchmark discovery should succeed")
        .live_instances
        .len()
}

pub async fn private_rpc_round_trip(fixture: &LiveInstanceFixture, index: usize) -> usize {
    let descriptor = &fixture.descriptors[index % fixture.descriptors.len()];
    match ControlRpcClient::call(
        descriptor,
        ControlOperation::DescribeInstance,
        Instant::now() + BENCHMARK_DEADLINE,
        benchmark_client(),
        CancellationToken::new(),
    )
    .await
    .expect("benchmark private RPC call should succeed")
    {
        ControlResult::DescribeInstance {
            retained_capture_count,
            ..
        } => retained_capture_count,
        _ => unreachable!("describe call returned the wrong result"),
    }
}

pub async fn concurrent_private_routing(fixture: &LiveInstanceFixture, call_count: usize) -> usize {
    let futures = (0..call_count).map(|index| private_rpc_round_trip(fixture, index));
    futures::future::join_all(futures).await.into_iter().sum()
}

pub struct DefaultLockFixture {
    _home: TempDir,
    path: std::path::PathBuf,
    _held: crate::instance::DefaultConfigLease,
}

pub fn default_lock_fixture() -> DefaultLockFixture {
    let home = tempfile::tempdir().expect("benchmark lock home should be created");
    let path = home.path().join("run/default-config.lock");
    let held = crate::instance::DefaultConfigLease::acquire(&path)
        .expect("benchmark lock should be acquired");
    DefaultLockFixture {
        _home: home,
        path,
        _held: held,
    }
}

pub fn contended_default_lock(fixture: &DefaultLockFixture) -> bool {
    crate::instance::DefaultConfigLease::acquire(&fixture.path)
        .is_err_and(|error| error.kind() == std::io::ErrorKind::AlreadyExists)
}

pub struct SettingsWriteFixture {
    _home: TempDir,
    path: std::path::PathBuf,
    settings: AppSettings,
}

pub fn settings_write_fixture(rule_count: usize) -> SettingsWriteFixture {
    let home = tempfile::tempdir().expect("benchmark settings home should be created");
    let path = home.path().join("config.yml");
    let mut settings = AppSettings::default();
    settings.proxy = Some(crate::settings::ProxySettings {
        presets: vec![crate::settings::ProxyPresetSettings {
            name: "benchmark".to_owned(),
            map_remote: crate::settings::ProxyMapRemoteSettings {
                rules: (0..rule_count)
                    .map(|index| crate::settings::ProxyMapRemoteRule {
                        from: format!("https://source{index}.example/**"),
                        to: format!("https://target{index}.example/"),
                        enable: true,
                    })
                    .collect(),
                ..crate::settings::ProxyMapRemoteSettings::default()
            },
            ..crate::settings::ProxyPresetSettings::default()
        }],
        active_preset: Some("benchmark".to_owned()),
        ..crate::settings::ProxySettings::default()
    });
    crate::settings::benchmark_atomic_settings_write(&path, &settings)
        .expect("initial benchmark settings should persist");
    SettingsWriteFixture {
        _home: home,
        path,
        settings,
    }
}

pub fn atomic_settings_write(fixture: &SettingsWriteFixture) -> usize {
    crate::settings::benchmark_atomic_settings_write(&fixture.path, &fixture.settings)
        .expect("benchmark settings write should succeed");
    fs::metadata(&fixture.path)
        .expect("benchmark settings file should exist")
        .len() as usize
}

pub struct LogRotationFixture {
    _home: TempDir,
    path: std::path::PathBuf,
    retained_files: usize,
}

pub fn log_rotation_fixture(retained_files: usize) -> LogRotationFixture {
    let home = tempfile::tempdir().expect("benchmark logging home should be created");
    LogRotationFixture {
        path: home.path().join("endpoint.log"),
        _home: home,
        retained_files,
    }
}

pub async fn rotate_endpoint_log(fixture: &LogRotationFixture) -> usize {
    crate::logging::benchmark_log_rotation(&fixture.path, fixture.retained_files)
        .await
        .expect("benchmark log rotation should succeed");
    fixture.retained_files
}

pub fn format_endpoint_logs(count: usize, payload_bytes: usize) -> usize {
    crate::logging::benchmark_format_endpoint_records(count, payload_bytes)
}
