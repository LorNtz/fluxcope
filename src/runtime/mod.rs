mod event_loop;
mod policy;
mod services;

use std::{
    convert::Infallible,
    io,
    net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket},
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

use crate::{
    app::App,
    ca,
    capture::{
        BodyTaskTracker, CapturePublisher, CaptureRecord, CaptureRetentionPolicy,
        start_decode_service,
    },
    logging::AppLogger,
    proxy_handler::LogHandler,
    recording::RecordingState,
    request_policy::{
        RequestPolicy, RequestPolicyDiagnostic, RequestPolicyDiagnosticSeverity, RequestPolicyStore,
    },
    request_search::start_request_search_service,
    settings::{AppSettings, SettingsManager},
};
use event_loop::{AppRuntime, Tui};
use policy::RuntimePolicy;
use services::{ServiceKind, ServiceSupervisor};

fn proxy_bind_addr(port: u16) -> SocketAddr {
    SocketAddr::from((Ipv4Addr::UNSPECIFIED, port))
}

pub async fn run() -> Result<()> {
    let mut settings = SettingsManager::load().context("failed to load Fluxcope settings")?;
    let policy = RuntimePolicy::default();
    let log_retention = policy.logging.retention;
    let shutdown = CancellationToken::new();

    let logging = AppLogger::init(
        settings.path().with_file_name("fluxcope.log"),
        policy.logging.clone(),
        shutdown.child_token(),
    )
    .map_err(|error| anyhow!("failed to install application logger: {error}"))?;
    log::info!("Application started");
    log::info!("Loaded settings from {}", settings.path().display());
    for diagnostic in settings.take_load_diagnostics() {
        log::error!("{}", diagnostic.message);
    }

    let proxy_port = settings.server_port();
    let proxy_addr = proxy_bind_addr(proxy_port);
    verify_proxy_port_available(proxy_addr)?;

    let compiled_policy = RequestPolicy::compile(settings.settings());
    log_request_policy_diagnostics(&compiled_policy.diagnostics);
    let request_policy_store = RequestPolicyStore::new(compiled_policy.policy);
    let recording = RecordingState::new(settings.recording_settings().start_record_on_launch);
    let (capture_tx, capture_rx) =
        mpsc::channel::<std::sync::Arc<CaptureRecord>>(policy.capture.queue_capacity);
    let capture_publisher = CapturePublisher::new(capture_tx, policy.capture.clone());
    let capture_metrics = capture_publisher.metrics();
    let capture_dirty = capture_publisher.dirty_signal();
    let body_tasks = BodyTaskTracker::new(shutdown.child_token());
    let decode = start_decode_service(policy.decode.clone(), shutdown.child_token());
    let request_search = start_request_search_service(shutdown.child_token());

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
    log::info!(
        "CA certificate available at {}",
        certificate_store_dir
            .join(&certificate_pem_filename)
            .display()
    );

    let certificate_download = match start_certificate_download_server(
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

    let proxy_task = start_proxy(
        proxy_addr,
        ca,
        capture_publisher,
        body_tasks.clone(),
        request_policy_store.clone(),
        recording.clone(),
        shutdown.child_token(),
    )?;

    let tui = Tui::enter().context("failed to initialize terminal UI")?;
    let mut app = App::with_runtime_policies(
        settings.settings().clone(),
        recording,
        log_retention,
        CaptureRetentionPolicy {
            max_records: policy.capture.retained_records,
            max_bytes: policy.capture.total_retained_bytes,
        },
    );
    app.set_decode_client(decode.client);
    if let Some(service) = certificate_download.as_ref() {
        app.set_certificate_download_url(service.url.clone());
    }

    let mut services = ServiceSupervisor::new(shutdown.clone());
    services.track_result(ServiceKind::Proxy, proxy_task);
    if let Some(service) = certificate_download {
        services.track_result(ServiceKind::CertificateDownload, service.task);
    }
    services.track_infallible(ServiceKind::Logger, logging.task);
    services.track_result(
        ServiceKind::BodyPumps,
        tokio::spawn(body_tasks.wait_for_shutdown(policy.render.shutdown_grace)),
    );
    services.track_result(ServiceKind::Decoder, decode.task);
    services.track_result(ServiceKind::RequestSearch, request_search.task);

    AppRuntime::new(
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
    )
    .run()
    .await
}

fn verify_proxy_port_available(proxy_addr: SocketAddr) -> Result<()> {
    std::net::TcpListener::bind(proxy_addr)
        .with_context(|| {
            format!("failed to bind proxy address {proxy_addr}; another instance may be running")
        })
        .map(drop)
}

fn start_proxy(
    proxy_addr: SocketAddr,
    ca: ca::CaData,
    capture_publisher: CapturePublisher,
    body_tasks: BodyTaskTracker,
    request_policy_store: RequestPolicyStore,
    recording: RecordingState,
    shutdown: CancellationToken,
) -> Result<JoinHandle<Result<()>>> {
    let key =
        KeyPair::try_from(ca.key_der().as_slice()).context("failed to decode proxy CA key")?;
    let issuer = Issuer::from_ca_cert_der(&CertificateDer::from(ca.cert_der()), key)
        .context("failed to decode proxy CA certificate")?;
    let authority = RcgenAuthority::new(issuer, 1_000, aws_lc_rs::default_provider());

    let proxy = Proxy::builder()
        .with_addr(proxy_addr)
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

fn save_settings_draft(
    settings: &mut SettingsManager,
    request_policy_store: &RequestPolicyStore,
    draft: AppSettings,
) -> io::Result<AppSettings> {
    let compiled_policy = RequestPolicy::compile(&draft);
    settings.update(|current| {
        *current = draft.clone();
    })?;
    let saved = settings.settings().clone();
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
    use std::fs;

    #[test]
    fn proxy_bind_address_uses_ipv4_unspecified_address() {
        assert_eq!(
            SocketAddr::from(([0, 0, 0, 0], 8989)),
            proxy_bind_addr(8989)
        );
    }

    #[test]
    fn save_settings_draft_persists_and_replaces_request_policy() -> io::Result<()> {
        use crate::settings::{
            ProxyMapRemoteRule, ProxyMapRemoteSettings, ProxyPresetSettings, ProxySettings,
            RecordingPrefilterPatternSettings,
        };

        let directory = tempfile::tempdir()?;
        let path = directory.path().join("config.yml");
        let mut manager = SettingsManager::load_from_path(&path)?;
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

        let saved = save_settings_draft(&mut manager, &store, draft)?;

        assert_eq!(9013, saved.server.port);
        assert!(fs::read_to_string(&path)?.contains("9013"));
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
