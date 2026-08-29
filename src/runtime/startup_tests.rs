#![cfg(unix)]

use super::{
    bind_proxy_listener,
    control::{
        ControlServiceFactory, ExistingDescriptorProbe, FailingControlServiceFactory,
        PrivateControlStartup, PrivateControlStartupError, RuntimeControlHandler, RuntimeGateway,
    },
};
use crate::{
    control_rpc::protocol::{ControlError, InstanceScope},
    instance::InstanceIdentity,
    instance_registry::{InstanceDescriptor, RegistryPublisher, RegistryScanner},
    settings::{AppSettings, SettingsSession},
};
use futures::future::BoxFuture;
use std::{
    io,
    os::unix::fs::PermissionsExt,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
use tempfile::TempDir;
use tokio::{
    sync::{Notify, oneshot},
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
struct StubProbe {
    result: Result<InstanceScope, ControlError>,
    calls: Arc<AtomicUsize>,
}

impl StubProbe {
    fn unavailable() -> Self {
        Self {
            result: Err(ControlError::instance_unavailable("stale private service")),
            calls: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn live(scope: InstanceScope) -> Self {
        Self {
            result: Ok(scope),
            calls: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl ExistingDescriptorProbe for StubProbe {
    fn probe<'a>(
        &'a self,
        _descriptor: &'a InstanceDescriptor,
        _deadline: Instant,
        _cancelled: CancellationToken,
    ) -> BoxFuture<'a, Result<InstanceScope, ControlError>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let result = self.result.clone();
        Box::pin(async move { result })
    }
}

fn identity(endpoint: &str) -> InstanceIdentity {
    InstanceIdentity::new(endpoint.parse().expect("endpoint")).expect("instance identity")
}

fn settings() -> SettingsSession {
    SettingsSession::temporary(AppSettings::default())
}

fn runtime_handler() -> (
    RuntimeControlHandler,
    super::control::RuntimeControlReceiver,
) {
    let (client, receiver) = RuntimeGateway::new(64);
    (RuntimeControlHandler::new(client), receiver)
}

struct CompletesBeforeReadyFactory;

impl ControlServiceFactory for CompletesBeforeReadyFactory {
    fn start(
        self,
        _listener: std::os::unix::net::UnixListener,
        _identity: InstanceIdentity,
        _handler: RuntimeControlHandler,
        _shutdown: CancellationToken,
    ) -> io::Result<(JoinHandle<anyhow::Result<()>>, oneshot::Receiver<()>)> {
        let (ready, ready_rx) = oneshot::channel();
        std::mem::forget(ready);
        let task = tokio::spawn(async { Err(anyhow::anyhow!("exited before readiness")) });
        Ok((task, ready_rx))
    }
}

#[derive(Clone)]
struct ControlledExit {
    release: Arc<Notify>,
    exiting: Arc<Notify>,
}

impl ControlledExit {
    fn new() -> Self {
        Self {
            release: Arc::new(Notify::new()),
            exiting: Arc::new(Notify::new()),
        }
    }

    async fn release_and_wait(&self) {
        let exiting = self.exiting.notified();
        self.release.notify_one();
        exiting.await;
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

struct ControlledExitFactory {
    control: ControlledExit,
}

impl ControlServiceFactory for ControlledExitFactory {
    fn start(
        self,
        _listener: std::os::unix::net::UnixListener,
        _identity: InstanceIdentity,
        _handler: RuntimeControlHandler,
        _shutdown: CancellationToken,
    ) -> io::Result<(JoinHandle<anyhow::Result<()>>, oneshot::Receiver<()>)> {
        let (ready, ready_rx) = oneshot::channel();
        let control = self.control;
        let task = tokio::spawn(async move {
            let _ = ready.send(());
            control.release.notified().await;
            control.exiting.notify_one();
            Err(anyhow::anyhow!("controlled listener exit"))
        });
        Ok((task, ready_rx))
    }
}

#[derive(Clone)]
struct ExitDuringProbe {
    control: ControlledExit,
}

impl ExistingDescriptorProbe for ExitDuringProbe {
    fn probe<'a>(
        &'a self,
        _descriptor: &'a InstanceDescriptor,
        _deadline: Instant,
        _cancelled: CancellationToken,
    ) -> BoxFuture<'a, Result<InstanceScope, ControlError>> {
        Box::pin(async move {
            self.control.release_and_wait().await;
            Err(ControlError::instance_unavailable(
                "probe observed unavailable service",
            ))
        })
    }
}

#[derive(Clone)]
struct CancellingUnavailableProbe {
    cancelled: CancellationToken,
}

impl ExistingDescriptorProbe for CancellingUnavailableProbe {
    fn probe<'a>(
        &'a self,
        _descriptor: &'a InstanceDescriptor,
        _deadline: Instant,
        _cancelled: CancellationToken,
    ) -> BoxFuture<'a, Result<InstanceScope, ControlError>> {
        Box::pin(async move {
            self.cancelled.cancel();
            Err(ControlError::instance_unavailable("probe was cancelled"))
        })
    }
}

#[test]
fn disabled_mode_prepares_neither_control_socket_nor_descriptor() {
    let home = TempDir::new().expect("temporary Wirelens home");
    let identity = identity("127.0.0.1:19020");
    let prepared = PrivateControlStartup::prepare(false, home.path(), identity, &settings())
        .expect("disabled startup");

    assert!(prepared.is_none());
    assert!(!home.path().join("run/instances").exists());
}

#[test]
fn enabled_control_bind_failure_is_a_fatal_startup_error_without_publication() {
    let home = TempDir::new().expect("temporary Wirelens home");
    let identity = identity("127.0.0.1:19021");
    let _socket_owner =
        PrivateControlStartup::prepare(true, home.path(), identity.clone(), &settings())
            .expect("first preparation")
            .expect("enabled control");

    let error = PrivateControlStartup::prepare(true, home.path(), identity, &settings())
        .expect_err("same run socket bind must fail");

    assert!(matches!(error, PrivateControlStartupError::ControlBind(_)));
    let instances = home.path().join("run/instances");
    assert!(
        !instances.exists()
            || std::fs::read_dir(instances)
                .expect("instances directory")
                .next()
                .is_none()
    );
}

#[tokio::test]
async fn proxy_and_control_are_ready_before_descriptor_publication() {
    let home = TempDir::new().expect("temporary Wirelens home");
    let proxy =
        bind_proxy_listener("127.0.0.1:0".parse().expect("endpoint")).expect("pre-bind proxy");
    let endpoint = proxy.local_addr().expect("proxy endpoint");
    let identity = InstanceIdentity::new(endpoint).expect("instance identity");
    let (handler, _control_rx) = runtime_handler();
    let shutdown = CancellationToken::new();
    let prepared = PrivateControlStartup::prepare(true, home.path(), identity, &settings())
        .expect("prepare control")
        .expect("enabled control");
    let descriptor_path = prepared.descriptor_path().to_path_buf();
    let socket_path = prepared.socket_path().to_path_buf();
    assert!(!descriptor_path.exists());

    let mut running = prepared
        .start(handler, shutdown.clone())
        .expect("start control listener");
    running
        .wait_until_ready()
        .await
        .expect("control listener ready");
    std::net::TcpStream::connect(endpoint).expect("proxy listener ready");
    tokio::net::UnixStream::connect(&socket_path)
        .await
        .expect("private listener ready");
    assert!(
        !descriptor_path.exists(),
        "readiness alone must not publish"
    );

    let probe = StubProbe::unavailable();
    running
        .publish_after_probe(
            &probe,
            Instant::now() + Duration::from_secs(1),
            CancellationToken::new(),
        )
        .await
        .expect("publish ready services");
    assert!(descriptor_path.exists());
    assert_eq!(probe.calls(), 0, "no prior descriptor means no probe");

    running
        .shutdown(Duration::from_secs(1))
        .await
        .expect("graceful control shutdown");
    assert!(!descriptor_path.exists());
    assert!(!socket_path.exists());
}

#[tokio::test]
async fn live_same_endpoint_descriptor_is_a_fatal_preflight_conflict() {
    let home = TempDir::new().expect("temporary Wirelens home");
    let endpoint = "127.0.0.1:19022";
    let existing_identity = identity(endpoint);
    let mut existing =
        RegistryPublisher::prepare(home.path(), existing_identity.clone(), &settings())
            .expect("existing publisher");
    existing.publish().expect("existing descriptor");
    let probe = StubProbe::live(InstanceScope {
        proxy_endpoint: existing_identity.proxy_endpoint(),
        run_id: existing_identity.run_id().clone(),
    });
    let replacement_identity = identity(endpoint);
    let (handler, _control_rx) = runtime_handler();
    let prepared = PrivateControlStartup::prepare(
        true,
        home.path(),
        replacement_identity.clone(),
        &settings(),
    )
    .expect("prepare replacement")
    .expect("enabled control");
    let mut running = prepared
        .start(handler, CancellationToken::new())
        .expect("start replacement control");

    let error = running
        .publish_after_probe(
            &probe,
            Instant::now() + Duration::from_secs(1),
            CancellationToken::new(),
        )
        .await
        .expect_err("live descriptor conflict");

    assert!(matches!(
        error,
        PrivateControlStartupError::LiveEndpointConflict {
            existing: ref live,
            proposed: ref new,
        } if live == existing_identity.run_id() && new == replacement_identity.run_id()
    ));
    assert_eq!(probe.calls(), 1);
    let report = RegistryScanner::new(home.path())
        .expect("scanner")
        .scan_all()
        .expect("scan registry");
    assert_eq!(report.candidates.len(), 1);
    assert_eq!(report.candidates[0].run_id(), existing_identity.run_id());

    running
        .rollback(Duration::from_secs(1))
        .await
        .expect("rollback unpublished replacement");
}

#[tokio::test]
async fn unavailable_private_probe_replaces_only_the_exact_stale_descriptor() {
    let home = TempDir::new().expect("temporary Wirelens home");
    let endpoint = "127.0.0.1:19023";
    let stale_identity = identity(endpoint);
    let mut stale = RegistryPublisher::prepare(home.path(), stale_identity.clone(), &settings())
        .expect("stale publisher");
    stale.publish().expect("stale descriptor");
    let replacement_identity = identity(endpoint);
    let replacement_run_id = replacement_identity.run_id().clone();
    let (handler, _control_rx) = runtime_handler();
    let prepared =
        PrivateControlStartup::prepare(true, home.path(), replacement_identity, &settings())
            .expect("prepare replacement")
            .expect("enabled control");
    let mut running = prepared
        .start(handler, CancellationToken::new())
        .expect("start replacement control");
    let probe = StubProbe::unavailable();

    running
        .publish_after_probe(
            &probe,
            Instant::now() + Duration::from_secs(1),
            CancellationToken::new(),
        )
        .await
        .expect("replace stale descriptor");

    assert_eq!(probe.calls(), 1);
    let report = RegistryScanner::new(home.path())
        .expect("scanner")
        .scan_all()
        .expect("scan registry");
    assert_eq!(report.candidates.len(), 1);
    assert_eq!(report.candidates[0].run_id(), &replacement_run_id);

    running
        .shutdown(Duration::from_secs(1))
        .await
        .expect("graceful shutdown");
}

#[tokio::test]
async fn control_service_start_failure_rolls_back_the_unpublished_socket() {
    let home = TempDir::new().expect("temporary Wirelens home");
    let identity = identity("127.0.0.1:19024");
    let (handler, _control_rx) = runtime_handler();
    let prepared = PrivateControlStartup::prepare(true, home.path(), identity, &settings())
        .expect("prepare control")
        .expect("enabled control");
    let socket_path = prepared.socket_path().to_path_buf();
    let descriptor_path = prepared.descriptor_path().to_path_buf();

    let error = prepared
        .start_with_factory(
            handler,
            CancellationToken::new(),
            FailingControlServiceFactory::new(io::Error::other("injected service start failure")),
        )
        .expect_err("fatal control service start failure");

    assert!(matches!(error, PrivateControlStartupError::ServiceStart(_)));
    assert!(!descriptor_path.exists());
    assert!(!socket_path.exists());
}

#[tokio::test]
async fn preflight_conflict_cancels_service_and_removes_unpublished_socket() {
    let home = TempDir::new().expect("temporary Wirelens home");
    let endpoint = "127.0.0.1:19025";
    let existing_identity = identity(endpoint);
    let mut existing =
        RegistryPublisher::prepare(home.path(), existing_identity.clone(), &settings())
            .expect("existing publisher");
    existing.publish().expect("existing descriptor");
    let replacement_identity = identity(endpoint);
    let (handler, _control_rx) = runtime_handler();
    let prepared =
        PrivateControlStartup::prepare(true, home.path(), replacement_identity, &settings())
            .expect("prepare replacement")
            .expect("enabled control");
    let replacement_socket = prepared.socket_path().to_path_buf();
    let mut running = prepared
        .start(handler, CancellationToken::new())
        .expect("start control service");
    let probe = StubProbe::live(InstanceScope {
        proxy_endpoint: existing_identity.proxy_endpoint(),
        run_id: existing_identity.run_id().clone(),
    });

    running
        .publish_after_probe(
            &probe,
            Instant::now() + Duration::from_secs(1),
            CancellationToken::new(),
        )
        .await
        .expect_err("publication preflight conflict");
    running
        .rollback(Duration::from_secs(1))
        .await
        .expect("rollback failed publication");

    assert!(!replacement_socket.exists());
    assert!(existing.descriptor_path().exists());
}

#[tokio::test]
async fn atomic_descriptor_publication_failure_is_fatal_and_rolls_back() {
    let home = TempDir::new().expect("temporary Wirelens home");
    let identity = identity("127.0.0.1:19026");
    let (handler, _control_rx) = runtime_handler();
    let prepared = PrivateControlStartup::prepare(true, home.path(), identity, &settings())
        .expect("prepare control")
        .expect("enabled control");
    let socket_path = prepared.socket_path().to_path_buf();
    let descriptor_path = prepared.descriptor_path().to_path_buf();
    let instances_dir = descriptor_path.parent().expect("instances directory");
    let mut running = prepared
        .start(handler, CancellationToken::new())
        .expect("start control service");
    std::fs::set_permissions(instances_dir, std::fs::Permissions::from_mode(0o500))
        .expect("make publication directory read-only");

    let error = running
        .publish_after_probe(
            &StubProbe::unavailable(),
            Instant::now() + Duration::from_secs(1),
            CancellationToken::new(),
        )
        .await
        .expect_err("descriptor publication failure");

    assert!(matches!(error, PrivateControlStartupError::Publication(_)));
    assert!(!descriptor_path.exists());
    std::fs::set_permissions(instances_dir, std::fs::Permissions::from_mode(0o700))
        .expect("restore owner access for cleanup");
    running
        .rollback(Duration::from_secs(1))
        .await
        .expect("rollback publication failure");
    assert!(!socket_path.exists());
}

#[tokio::test]
async fn graceful_shutdown_cancels_admitted_calls_before_registry_cleanup() {
    use crate::control_rpc::{
        client::ControlRpcClient,
        protocol::{ControlOperation, DeclaredClient},
    };

    let home = TempDir::new().expect("temporary Wirelens home");
    let identity = identity("127.0.0.1:19027");
    let (handler, mut control_rx) = runtime_handler();
    let prepared = PrivateControlStartup::prepare(true, home.path(), identity, &settings())
        .expect("prepare control")
        .expect("enabled control");
    let descriptor_path = prepared.descriptor_path().to_path_buf();
    let socket_path = prepared.socket_path().to_path_buf();
    let mut running = prepared
        .start(handler, CancellationToken::new())
        .expect("start control service");
    running
        .publish_after_probe(
            &StubProbe::unavailable(),
            Instant::now() + Duration::from_secs(1),
            CancellationToken::new(),
        )
        .await
        .expect("publish descriptor");
    let descriptor = RegistryScanner::new(home.path())
        .expect("scanner")
        .scan_all()
        .expect("scan registry")
        .candidates
        .into_iter()
        .next()
        .expect("published descriptor");
    let client_call = tokio::spawn(async move {
        ControlRpcClient::call(
            &descriptor,
            ControlOperation::DescribeInstance,
            Instant::now() + Duration::from_secs(5),
            DeclaredClient {
                name: "shutdown-order-test".to_owned(),
                version: "1".to_owned(),
            },
            CancellationToken::new(),
        )
        .await
    });
    let command = control_rx.recv().await.expect("admitted runtime call");
    assert!(!command.cancelled.is_cancelled());

    let shutdown = tokio::spawn(async move { running.shutdown(Duration::from_secs(1)).await });
    tokio::time::timeout(Duration::from_secs(1), command.cancelled.cancelled())
        .await
        .expect("active call cancelled before shutdown completes");
    client_call
        .await
        .expect("client task")
        .expect_err("shutdown interrupts the private call");
    shutdown
        .await
        .expect("shutdown task")
        .expect("graceful shutdown");

    assert!(!descriptor_path.exists());
    assert!(!socket_path.exists());
}

#[tokio::test]
async fn cancellation_during_unavailable_probe_never_replaces_the_existing_descriptor() {
    let home = TempDir::new().expect("temporary Wirelens home");
    let endpoint = "127.0.0.1:19028";
    let existing_identity = identity(endpoint);
    let mut existing =
        RegistryPublisher::prepare(home.path(), existing_identity.clone(), &settings())
            .expect("existing publisher");
    existing.publish().expect("existing descriptor");
    let replacement_identity = identity(endpoint);
    let (handler, _control_rx) = runtime_handler();
    let prepared =
        PrivateControlStartup::prepare(true, home.path(), replacement_identity, &settings())
            .expect("prepare replacement")
            .expect("enabled control");
    let mut running = prepared
        .start(handler, CancellationToken::new())
        .expect("start replacement");
    let cancelled = CancellationToken::new();
    let probe = CancellingUnavailableProbe {
        cancelled: cancelled.clone(),
    };

    let error = running
        .publish_after_probe(&probe, Instant::now() + Duration::from_secs(1), cancelled)
        .await
        .expect_err("cancelled probe cannot authorize stale replacement");

    assert!(matches!(error, PrivateControlStartupError::Probe(_)));
    let report = RegistryScanner::new(home.path())
        .expect("scanner")
        .scan_all()
        .expect("scan registry");
    assert_eq!(report.candidates.len(), 1);
    assert_eq!(report.candidates[0].run_id(), existing_identity.run_id());
    running
        .rollback(Duration::from_secs(1))
        .await
        .expect("rollback cancelled preflight");
}

#[tokio::test]
async fn completion_consumed_while_waiting_for_readiness_can_still_roll_back() {
    let home = TempDir::new().expect("temporary Wirelens home");
    let identity = identity("127.0.0.1:19029");
    let (handler, _control_rx) = runtime_handler();
    let prepared = PrivateControlStartup::prepare(true, home.path(), identity, &settings())
        .expect("prepare control")
        .expect("enabled control");
    let mut running = prepared
        .start_with_factory(
            handler,
            CancellationToken::new(),
            CompletesBeforeReadyFactory,
        )
        .expect("start controlled service");

    let error = running
        .wait_until_ready()
        .await
        .expect_err("service completed before readiness");
    assert!(matches!(error, PrivateControlStartupError::ServiceStart(_)));
    running
        .rollback(Duration::from_secs(1))
        .await
        .expect("completed readiness task is not awaited twice");
}

#[tokio::test]
async fn service_exit_during_probe_prevents_stale_replacement_and_publication() {
    let home = TempDir::new().expect("temporary Wirelens home");
    let endpoint = "127.0.0.1:19030";
    let existing_identity = identity(endpoint);
    let mut existing =
        RegistryPublisher::prepare(home.path(), existing_identity.clone(), &settings())
            .expect("existing publisher");
    existing.publish().expect("existing descriptor");
    let replacement_identity = identity(endpoint);
    let (handler, _control_rx) = runtime_handler();
    let control = ControlledExit::new();
    let prepared =
        PrivateControlStartup::prepare(true, home.path(), replacement_identity, &settings())
            .expect("prepare replacement")
            .expect("enabled control");
    let mut running = prepared
        .start_with_factory(
            handler,
            CancellationToken::new(),
            ControlledExitFactory {
                control: control.clone(),
            },
        )
        .expect("start controlled service");

    let error = running
        .publish_after_probe(
            &ExitDuringProbe { control },
            Instant::now() + Duration::from_secs(1),
            CancellationToken::new(),
        )
        .await
        .expect_err("dead control service cannot publish a replacement");

    assert!(matches!(error, PrivateControlStartupError::ServiceStart(_)));
    let report = RegistryScanner::new(home.path())
        .expect("scanner")
        .scan_all()
        .expect("scan registry");
    assert_eq!(report.candidates.len(), 1);
    assert_eq!(report.candidates[0].run_id(), existing_identity.run_id());
    running
        .rollback(Duration::from_secs(1))
        .await
        .expect("rollback dead control service");
}

#[tokio::test]
async fn service_exit_after_publication_is_detected_before_supervision_handoff() {
    let home = TempDir::new().expect("temporary Wirelens home");
    let identity = identity("127.0.0.1:19031");
    let (handler, _control_rx) = runtime_handler();
    let control = ControlledExit::new();
    let prepared = PrivateControlStartup::prepare(true, home.path(), identity, &settings())
        .expect("prepare control")
        .expect("enabled control");
    let descriptor_path = prepared.descriptor_path().to_path_buf();
    let mut running = prepared
        .start_with_factory(
            handler,
            CancellationToken::new(),
            ControlledExitFactory {
                control: control.clone(),
            },
        )
        .expect("start controlled service");
    running
        .publish_after_probe(
            &StubProbe::unavailable(),
            Instant::now() + Duration::from_secs(1),
            CancellationToken::new(),
        )
        .await
        .expect("publish live control service");
    assert!(descriptor_path.exists());
    control.release_and_wait().await;

    let error = running
        .ensure_running()
        .await
        .expect_err("service died before supervision handoff");

    assert!(matches!(error, PrivateControlStartupError::ServiceStart(_)));
    running
        .rollback(Duration::from_secs(1))
        .await
        .expect("rollback published dead service");
    assert!(!descriptor_path.exists());
}
