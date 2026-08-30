#[cfg(unix)]
mod control;
mod event_loop;
mod policy;
mod services;
#[cfg(test)]
mod startup_tests;

#[cfg(test)]
use std::path::PathBuf;
use std::{
    convert::Infallible,
    io,
    net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, UdpSocket},
};

use anyhow::{Context, Result, anyhow};
use hudsucker::{Body, Proxy, certificate_authority::RcgenAuthority};
use hyper::{
    Method, Request, Response,
    header::{CONTENT_DISPOSITION, CONTENT_TYPE},
    service::service_fn,
};
use hyper_util::rt::TokioIo;
use rcgen::{Issuer, KeyPair};
use rustls::{crypto::aws_lc_rs, pki_types::CertificateDer};
use tokio::{sync::mpsc, task::JoinHandle};
use tokio_util::sync::CancellationToken;

#[cfg(unix)]
use crate::instance::InstanceIdentity;
use crate::{
    app::App,
    ca,
    capture::{
        BodyTaskTracker, BodyWorkAdmission, CapturePublisher, CaptureRecord,
        CaptureRetentionPolicy, start_decode_service_with_admission,
    },
    cli::{ConfigSelection, McpOverride, ProxyStartup},
    instance::wirelens_home_dir,
    logging::{AppLogger, endpoint_log_path},
    proxy_handler::LogHandler,
    recording::RecordingState,
    request_policy::{
        RequestPolicy, RequestPolicyDiagnostic, RequestPolicyDiagnosticSeverity, RequestPolicyStore,
    },
    request_search::start_request_search_service,
    settings::{AppSettings, SettingsSession},
};
#[cfg(unix)]
use control::{
    ControlRpcDescriptorProbe, ControlServiceContext, PrivateControlStartup, RuntimeControlHandler,
    RuntimeGateway,
};
use event_loop::{AppRuntime, Tui};
use policy::RuntimePolicy;
use services::{ServiceKind, ServiceSupervisor};

fn proxy_bind_addr(port: u16) -> SocketAddr {
    SocketAddr::from((Ipv4Addr::UNSPECIFIED, port))
}
fn resolve_proxy_bind_addr(selection: &ConfigSelection, settings: &AppSettings) -> SocketAddr {
    match selection {
        ConfigSelection::Temporary { host, port } => SocketAddr::new(*host, *port),
        ConfigSelection::DefaultOwned | ConfigSelection::ReadOnlyFile(_) => {
            proxy_bind_addr(settings.server.port)
        }
    }
}

pub(crate) async fn run(startup: ProxyStartup) -> Result<()> {
    let mut settings =
        SettingsSession::load(&startup.config).context("failed to load Fluxcope settings")?;
    let settings_snapshot = settings.snapshot();
    let mcp_enabled = effective_mcp_enabled(startup.mcp, &settings_snapshot);
    crate::mcp::ensure_supported_platform(mcp_enabled)?;
    let requested_proxy_addr = resolve_proxy_bind_addr(&startup.config, &settings_snapshot);
    let proxy_listener_lease = bind_proxy_listener(requested_proxy_addr)?;
    let proxy_addr = proxy_listener_lease
        .local_addr()
        .context("failed to read bound proxy address")?;
    let wirelens_home = wirelens_home_dir().context("failed to resolve Wirelens home directory")?;
    #[cfg(unix)]
    let identity = InstanceIdentity::new(proxy_addr)?;
    #[cfg(unix)]
    let prepared_control =
        PrivateControlStartup::prepare(mcp_enabled, &wirelens_home, identity.clone(), &settings)
            .map_err(anyhow::Error::new)?;
    let settings_context = settings.ui_context();
    let policy = RuntimePolicy::default();
    let log_retention = policy.logging.retention;
    let certificate_store_dir = settings
        .certificate_store_dir()
        .context("failed to resolve certificate store directory")?;
    let certificate_pem_filename = settings.certificate_pem_filename().to_string();
    let ca = ca::create_or_load_ca(&certificate_store_dir, &certificate_pem_filename)
        .with_context(|| {
            format!(
                "failed to create or load certificate authority in {}",
                certificate_store_dir.display()
            )
        })?;
    let shutdown = CancellationToken::new();

    let logging = AppLogger::init(
        endpoint_log_path(&wirelens_home, proxy_addr),
        policy.logging.clone(),
        shutdown.child_token(),
    )
    .map_err(|error| anyhow!("failed to install application logger: {error}"))?;
    log::info!("Application started");
    match settings.source_path() {
        Some(path) => log::info!("Loaded settings from {}", path.display()),
        None => log::info!("Loaded temporary settings"),
    }
    log::info!(
        "Embedded MCP server {}",
        if mcp_enabled { "enabled" } else { "disabled" }
    );
    for diagnostic in settings.take_load_diagnostics() {
        log::error!("{}", diagnostic.message);
    }

    let compiled_policy = RequestPolicy::compile(&settings_snapshot);
    log_request_policy_diagnostics(&compiled_policy.diagnostics);
    let request_policy_store = RequestPolicyStore::new(compiled_policy.policy);
    let recording = RecordingState::new(settings.recording_settings().start_record_on_launch);
    let (capture_tx, capture_rx) =
        mpsc::channel::<std::sync::Arc<CaptureRecord>>(policy.capture.queue_capacity);
    let capture_publisher = CapturePublisher::new(capture_tx, policy.capture.clone());
    let capture_metrics = capture_publisher.metrics();
    let capture_dirty = capture_publisher.dirty_signal();
    #[cfg(unix)]
    let capture_changes = capture_publisher.change_feed();
    let body_tasks = BodyTaskTracker::new(shutdown.child_token());
    let body_work = std::sync::Arc::new(BodyWorkAdmission::new());
    let decode = start_decode_service_with_admission(
        policy.decode.clone(),
        shutdown.child_token(),
        std::sync::Arc::clone(&body_work),
    );
    let request_search = start_request_search_service(shutdown.child_token());

    log::info!(
        "CA certificate available at {}",
        certificate_store_dir
            .join(&certificate_pem_filename)
            .display()
    );

    let mut certificate_download = match start_certificate_download_server(
        ca.cert_pem(),
        certificate_pem_filename,
        shutdown.child_token(),
    ) {
        Ok(service) => Some(service),
        Err(error) => {
            log::error!("certificate download server disabled: {error:#}");
            None
        }
    };

    let mut app = App::with_runtime_policies(
        settings_snapshot.as_ref().clone(),
        recording.clone(),
        log_retention,
        CaptureRetentionPolicy {
            max_records: policy.capture.retained_records,
            max_bytes: policy.capture.total_retained_bytes,
        },
        settings_context,
    );
    app.set_decode_client(decode.client);
    if let Some(service) = certificate_download.as_ref() {
        app.set_certificate_download_url(service.url.clone());
    }

    let mut services = ServiceSupervisor::new(shutdown.clone());
    if let Some(service) = certificate_download.take() {
        services.track_result(ServiceKind::CertificateDownload, service.task);
    }
    services.track_infallible(ServiceKind::Logger, logging.task);
    services.track_result(
        ServiceKind::BodyPumps,
        tokio::spawn(
            body_tasks
                .clone()
                .wait_for_shutdown(policy.render.shutdown_grace),
        ),
    );
    services.track_result(ServiceKind::Decoder, decode.task);
    services.track_result(ServiceKind::RequestSearch, request_search.task);

    #[cfg(unix)]
    let (control_rx, mut running_control) = if let Some(prepared) = prepared_control {
        let (client, receiver) = RuntimeGateway::channel(64);
        match prepared.start(
            RuntimeControlHandler::new(ControlServiceContext {
                runtime: client,
                capture_changes,
                body_work: std::sync::Arc::clone(&body_work),
            }),
            shutdown.child_token(),
        ) {
            Ok(running) => (Some(receiver), Some(running)),
            Err(error) => {
                shutdown.cancel();
                services.shutdown(policy.render.shutdown_grace).await;
                return Err(anyhow::Error::new(error));
            }
        }
    } else {
        (None, None)
    };

    let proxy_listener = match proxy_listener_lease
        .try_clone_for_proxy()
        .context("failed to clone proxy listener for proxy task")
    {
        Ok(listener) => listener,
        Err(error) => {
            shutdown.cancel();
            services.shutdown(policy.render.shutdown_grace).await;
            #[cfg(unix)]
            if let Some(running) = running_control.take() {
                let _ = running.rollback(policy.render.shutdown_grace).await;
            }
            return Err(error);
        }
    };

    let proxy_task = match start_proxy(
        proxy_listener,
        ca,
        capture_publisher,
        body_tasks.clone(),
        request_policy_store.clone(),
        recording,
        shutdown.child_token(),
    ) {
        Ok(task) => task,
        Err(error) => {
            shutdown.cancel();
            services.shutdown(policy.render.shutdown_grace).await;
            #[cfg(unix)]
            if let Some(running) = running_control.take() {
                let _ = running.rollback(policy.render.shutdown_grace).await;
            }
            return Err(error);
        }
    };
    services.track_result(ServiceKind::Proxy, proxy_task);

    #[cfg(unix)]
    if let Some(running) = running_control.as_mut()
        && let Err(error) = running
            .publish_after_probe(
                &ControlRpcDescriptorProbe,
                std::time::Instant::now() + std::time::Duration::from_secs(30),
                shutdown.child_token(),
            )
            .await
    {
        shutdown.cancel();
        services.shutdown(policy.render.shutdown_grace).await;
        if let Some(running) = running_control.take() {
            let _ = running.rollback(policy.render.shutdown_grace).await;
        }
        return Err(anyhow::Error::new(error));
    }

    let tui = match Tui::enter().context("failed to initialize terminal UI") {
        Ok(tui) => tui,
        Err(error) => {
            shutdown.cancel();
            services.shutdown(policy.render.shutdown_grace).await;
            #[cfg(unix)]
            if let Some(running) = running_control.take() {
                let _ = running.rollback(policy.render.shutdown_grace).await;
            }
            return Err(error);
        }
    };

    #[cfg(unix)]
    if let Some(running) = running_control.as_mut()
        && let Err(error) = running.ensure_running().await
    {
        shutdown.cancel();
        services.shutdown(policy.render.shutdown_grace).await;
        if let Some(running) = running_control.take() {
            let _ = running.rollback(policy.render.shutdown_grace).await;
        }
        return Err(anyhow::Error::new(error));
    }

    #[cfg(unix)]
    let control_publisher = if let Some(running) = running_control.take() {
        match running.into_supervised_parts() {
            Ok((publisher, task)) => {
                services.track_result(ServiceKind::ControlRpc, task);
                Some(publisher)
            }
            Err(error) => {
                shutdown.cancel();
                services.shutdown(policy.render.shutdown_grace).await;
                return Err(anyhow::Error::new(error));
            }
        }
    } else {
        None
    };

    let runtime = AppRuntime::new(
        app,
        capture_rx,
        logging.records,
        logging.statuses,
        logging.metrics,
        capture_dirty,
        capture_metrics,
        decode.results,
        decode.metrics,
        request_search.client,
        request_search.results,
        tui,
        settings,
        request_policy_store,
        policy.render,
        services,
        shutdown,
    );
    #[cfg(unix)]
    let runtime = runtime.with_control(identity, control_rx, control_publisher);
    runtime.run().await
}

struct BoundProxyListener {
    lease: TcpListener,
}

impl BoundProxyListener {
    fn local_addr(&self) -> io::Result<SocketAddr> {
        self.lease.local_addr()
    }

    fn try_clone_for_proxy(&self) -> io::Result<TcpListener> {
        self.lease.try_clone()
    }
}

fn bind_proxy_listener(proxy_addr: SocketAddr) -> Result<BoundProxyListener> {
    let listener = TcpListener::bind(proxy_addr).with_context(|| {
        format!("failed to bind proxy address {proxy_addr}; another instance may be running")
    })?;
    listener
        .set_nonblocking(true)
        .context("failed to make proxy listener nonblocking")?;
    Ok(BoundProxyListener { lease: listener })
}

fn start_proxy(
    proxy_listener: TcpListener,
    ca: ca::CaData,
    capture_publisher: CapturePublisher,
    body_tasks: BodyTaskTracker,
    request_policy_store: RequestPolicyStore,
    recording: RecordingState,
    shutdown: CancellationToken,
) -> Result<JoinHandle<Result<()>>> {
    let proxy_addr = proxy_listener
        .local_addr()
        .context("failed to read proxy listener address")?;
    let proxy_listener = tokio::net::TcpListener::from_std(proxy_listener)
        .context("failed to create proxy listener")?;
    let key =
        KeyPair::try_from(ca.key_der().as_slice()).context("failed to decode proxy CA key")?;
    let issuer = Issuer::from_ca_cert_der(&CertificateDer::from(ca.cert_der()), key)
        .context("failed to decode proxy CA certificate")?;
    let authority = RcgenAuthority::new(issuer, 1_000, aws_lc_rs::default_provider());

    let proxy = Proxy::builder()
        .with_listener(proxy_listener)
        .with_ca(authority)
        .with_rustls_connector(aws_lc_rs::default_provider())
        .with_http_handler(LogHandler::new(
            capture_publisher,
            body_tasks,
            request_policy_store,
            recording,
        ))
        .with_graceful_shutdown(shutdown.cancelled_owned())
        .build()
        .context("failed to build proxy")?;

    log::info!("Proxy server listening on {proxy_addr}");
    Ok(tokio::spawn(async move {
        proxy
            .start()
            .await
            .with_context(|| format!("proxy service failed at {proxy_addr}"))
    }))
}

struct CertificateDownloadService {
    url: String,
    task: JoinHandle<Result<()>>,
}

fn start_certificate_download_server(
    cert_pem: String,
    cert_filename: String,
    shutdown: CancellationToken,
) -> Result<CertificateDownloadService> {
    let listener = std::net::TcpListener::bind(SocketAddr::from(([0, 0, 0, 0], 0)))
        .context("failed to bind certificate download listener")?;
    let local_addr = listener
        .local_addr()
        .context("failed to read certificate download listener address")?;
    listener
        .set_nonblocking(true)
        .context("failed to make certificate download listener nonblocking")?;

    let host = detect_lan_ip()
        .map(format_url_host)
        .unwrap_or_else(|| "127.0.0.1".to_string());
    let url = format!("http://{host}:{}/{cert_filename}", local_addr.port());

    let listener = tokio::net::TcpListener::from_std(listener)
        .context("failed to create certificate download server")?;
    let task = tokio::spawn(async move {
        let mut connections = tokio::task::JoinSet::new();
        loop {
            tokio::select! {
                _ = shutdown.cancelled() => break,
                completed = connections.join_next(), if !connections.is_empty() => {
                    if let Some(Err(error)) = completed {
                        log::debug!("CA download connection stopped: {error}");
                    }
                }
                accepted = listener.accept() => {
                    let (stream, _) = match accepted {
                        Ok(connection) => connection,
                        Err(error) => {
                            log::debug!("CA download listener temporarily unavailable: {error}");
                            if !matches!(error.kind(), io::ErrorKind::ConnectionAborted | io::ErrorKind::ConnectionReset) {
                                tokio::select! {
                                    _ = shutdown.cancelled() => break,
                                    _ = tokio::time::sleep(std::time::Duration::from_secs(1)) => {}
                                }
                            }
                            continue;
                        }
                    };
                    let cert_pem = cert_pem.clone();
                    let cert_filename = cert_filename.clone();
                    connections.spawn(async move {
                        let service = service_fn(move |req| {
                            let response = certificate_download_response(req, cert_pem.clone(), cert_filename.clone());
                            async { Ok::<_, Infallible>(response) }
                        });
                        if let Err(error) = hyper::server::conn::http1::Builder::new()
                            .serve_connection(TokioIo::new(stream), service).await {
                            log::debug!("CA download request failed: {error}");
                        }
                    });
                }
            }
        }
        connections.shutdown().await;
        Ok(())
    });

    log::info!("CA certificate download URL: {url}");
    Ok(CertificateDownloadService { url, task })
}

fn certificate_download_response(
    req: Request<hyper::body::Incoming>,
    cert_pem: String,
    cert_filename: String,
) -> Response<Body> {
    if req.method() == Method::GET && req.uri().path() == format!("/{cert_filename}") {
        return Response::builder()
            .header(CONTENT_TYPE, "application/x-x509-ca-cert")
            .header(
                CONTENT_DISPOSITION,
                format!("attachment; filename=\"{cert_filename}\""),
            )
            .body(Body::from(cert_pem))
            .unwrap_or_else(|_| Response::new(Body::empty()));
    }

    Response::builder()
        .status(404)
        .body(Body::from("Not found"))
        .unwrap_or_else(|_| Response::new(Body::empty()))
}

fn detect_lan_ip() -> Option<IpAddr> {
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("8.8.8.8:80").ok()?;
    let ip = socket.local_addr().ok()?.ip();

    if ip.is_loopback() { None } else { Some(ip) }
}

fn format_url_host(ip: IpAddr) -> String {
    match ip {
        IpAddr::V4(ip) => ip.to_string(),
        IpAddr::V6(ip) => format!("[{ip}]"),
    }
}

fn effective_mcp_enabled(mcp_override: McpOverride, settings: &AppSettings) -> bool {
    match mcp_override {
        McpOverride::Inherit => settings.mcp.enable,
        McpOverride::Enabled => true,
        McpOverride::Disabled => false,
    }
}

fn save_settings_draft(
    settings: &mut SettingsSession,
    request_policy_store: &RequestPolicyStore,
    draft: AppSettings,
) -> io::Result<AppSettings> {
    let compiled_policy = RequestPolicy::compile(&draft);
    settings.commit(draft)?;
    let saved = settings.snapshot().as_ref().clone();
    request_policy_store.replace(compiled_policy.policy);
    log_request_policy_diagnostics(&compiled_policy.diagnostics);
    Ok(saved)
}

fn log_request_policy_diagnostics(diagnostics: &[RequestPolicyDiagnostic]) {
    for diagnostic in diagnostics {
        match diagnostic.severity {
            RequestPolicyDiagnosticSeverity::Warning => log::warn!("{}", diagnostic.message),
            RequestPolicyDiagnosticSeverity::Error => log::error!("{}", diagnostic.message),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proxy_bind_address_uses_ipv4_unspecified_address() {
        assert_eq!(
            SocketAddr::from(([0, 0, 0, 0], 8989)),
            proxy_bind_addr(8989)
        );
    }

    #[test]
    fn proxy_listener_is_bound_and_nonblocking_before_proxy_service_startup() {
        let bound =
            bind_proxy_listener("127.0.0.1:0".parse().expect("ephemeral loopback endpoint"))
                .expect("bind proxy listener");
        let listener = bound
            .try_clone_for_proxy()
            .expect("clone listener for proxy task");
        let endpoint = bound.local_addr().expect("bound proxy endpoint");

        let error = listener
            .accept()
            .expect_err("nonblocking listener should not wait for a client");
        assert_eq!(error.kind(), io::ErrorKind::WouldBlock);

        let client = std::net::TcpStream::connect(endpoint)
            .expect("bound listener should be connectable before proxy startup");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
        let (_server, peer) = loop {
            match listener.accept() {
                Ok(connection) => break connection,
                Err(error)
                    if error.kind() == io::ErrorKind::WouldBlock
                        && std::time::Instant::now() < deadline =>
                {
                    std::thread::yield_now();
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    panic!("timed out waiting for the queued proxy connection");
                }
                Err(error) => panic!("failed to accept queued proxy connection: {error}"),
            }
        };
        assert_eq!(peer, client.local_addr().expect("client endpoint"));
    }

    #[test]
    fn listener_lease_outlives_the_proxy_task_listener() {
        let bound =
            bind_proxy_listener("127.0.0.1:0".parse().expect("ephemeral loopback endpoint"))
                .expect("bind proxy listener");
        let endpoint = bound.local_addr().expect("actual proxy endpoint");
        let proxy_task_listener = bound
            .try_clone_for_proxy()
            .expect("clone listener for proxy task");

        drop(proxy_task_listener);
        let duplicate = std::net::TcpListener::bind(endpoint)
            .expect_err("instance lease must retain endpoint after proxy task exit");
        assert_eq!(duplicate.kind(), io::ErrorKind::AddrInUse);

        drop(bound);
        std::net::TcpListener::bind(endpoint)
            .expect("dropping the instance lease should release the endpoint");
    }
    #[test]
    fn every_config_selection_resolves_the_expected_proxy_endpoint() {
        let mut settings = AppSettings::default();
        settings.server.port = 9345;
        let cases = [
            (
                ConfigSelection::DefaultOwned,
                SocketAddr::from(([0, 0, 0, 0], 9345)),
            ),
            (
                ConfigSelection::ReadOnlyFile(PathBuf::from("readonly.yml")),
                SocketAddr::from(([0, 0, 0, 0], 9345)),
            ),
            (
                ConfigSelection::Temporary {
                    host: IpAddr::V6(std::net::Ipv6Addr::LOCALHOST),
                    port: 9346,
                },
                SocketAddr::from((std::net::Ipv6Addr::LOCALHOST, 9346)),
            ),
        ];

        for (selection, expected) in cases {
            assert_eq!(
                resolve_proxy_bind_addr(&selection, &settings),
                expected,
                "unexpected endpoint for {selection:?}"
            );
        }
    }

    #[test]
    fn every_mcp_override_has_cli_over_settings_precedence() {
        let cases = [
            (McpOverride::Inherit, false, false),
            (McpOverride::Inherit, true, true),
            (McpOverride::Enabled, false, true),
            (McpOverride::Enabled, true, true),
            (McpOverride::Disabled, false, false),
            (McpOverride::Disabled, true, false),
        ];

        for (mcp_override, configured, expected) in cases {
            let mut settings = AppSettings::default();
            settings.mcp.enable = configured;
            assert_eq!(
                effective_mcp_enabled(mcp_override, &settings),
                expected,
                "unexpected MCP result for {mcp_override:?} over configured={configured}"
            );
        }
    }

    #[test]
    fn save_settings_draft_commits_and_replaces_request_policy() -> io::Result<()> {
        use crate::settings::{
            ProxyMapRemoteRule, ProxyMapRemoteSettings, ProxyPresetSettings, ProxySettings,
            RecordingPrefilterPatternSettings,
        };

        let mut session = SettingsSession::temporary(AppSettings::default());
        let store = RequestPolicyStore::default();
        let mut draft = AppSettings::default();
        draft.server.port = 9013;
        draft.recording.prefilter.include_url_patterns =
            vec![RecordingPrefilterPatternSettings::new(
                "http://mapped.example.com:80/*",
            )];
        draft.proxy = Some(ProxySettings {
            enable: true,
            active_preset: Some("test".to_string()),
            presets: vec![ProxyPresetSettings {
                name: "test".to_string(),
                map_remote: ProxyMapRemoteSettings {
                    enable: true,
                    rules: vec![ProxyMapRemoteRule {
                        from: "https://api.example.com".to_string(),
                        to: "http://mapped.example.com".to_string(),
                        enable: true,
                    }],
                },
                ..ProxyPresetSettings::default()
            }],
        });

        let saved = save_settings_draft(&mut session, &store, draft)?;

        assert_eq!(9013, saved.server.port);
        assert_eq!(session.snapshot().server.port, 9013);
        let original_uri = "https://api.example.com/orders?x=1"
            .parse()
            .expect("test URI should parse");
        let (mapping, urls) = store.current().evaluate(&original_uri, true).into_parts();
        assert_eq!(
            mapping.mapped_uri.map(|uri| uri.to_string()),
            Some("http://mapped.example.com/orders?x=1".to_string())
        );
        let recording = urls
            .recording()
            .expect("recording evaluation should be available");
        assert_eq!(
            recording.effective_url,
            "http://mapped.example.com/orders?x=1"
        );
        assert!(recording.included);
        Ok(())
    }
}
