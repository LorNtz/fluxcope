use crate::{
    app::{App, AppEvent},
    ca,
    logging::AppLogger,
    mapping::{MappingEngine, MappingStore},
    proxy_handler::LogHandler,
    settings::SettingsManager,
    ui::RootView,
};
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use hudsucker::{ProxyBuilder, certificate_authority::RcgenAuthority, rustls::PrivateKey};
use hyper::{
    Body, Method, Request, Response, Server,
    header::{CONTENT_DISPOSITION, CONTENT_TYPE},
    service::{make_service_fn, service_fn},
};
use std::{
    convert::Infallible,
    error::Error,
    io,
    net::{IpAddr, SocketAddr, UdpSocket},
    path::PathBuf,
    time::Duration,
};
use tokio::sync::mpsc;

pub async fn run() -> Result<(), Box<dyn Error>> {
    let settings = SettingsManager::load()?;
    let proxy_port = settings.server_port();
    let certificate_store_dir = settings.certificate_store_dir()?;
    let certificate_pem_filename = settings.certificate_pem_filename().to_string();

    // Check if port is already in use by another instance
    if let Err(e) = std::net::TcpListener::bind(("127.0.0.1", proxy_port)) {
        return Err(format!(
            "Failed to bind to proxy port {}: {}. Is another instance running?",
            proxy_port, e
        )
        .into());
    }

    let (tx, rx) = mpsc::unbounded_channel::<AppEvent>();

    init_logger(tx.clone());
    log::info!("Loaded settings from {}", settings.path().display());
    let mapping_engine = MappingEngine::compile(settings.proxy_settings());
    for diagnostic in mapping_engine.diagnostics() {
        log::warn!("{}", diagnostic.message);
    }
    let mapping_store = MappingStore::new(mapping_engine);
    start_proxy(
        tx,
        proxy_port,
        certificate_store_dir,
        certificate_pem_filename,
        mapping_store,
    );

    let tui = Tui::enter()?;
    let app = App::new(settings.ui_settings().clone());
    AppRuntime::new(app, rx, tui, settings).run()
}

fn init_logger(tx: mpsc::UnboundedSender<AppEvent>) {
    if let Err(error) = AppLogger::init(tx) {
        eprintln!("Failed to initialize logger: {error}");
    }
    log::info!("Application started");
}

fn start_proxy(
    tx: mpsc::UnboundedSender<AppEvent>,
    proxy_port: u16,
    certificate_store_dir: PathBuf,
    certificate_pem_filename: String,
    mapping_store: MappingStore,
) {
    let ca = ca::create_or_load_ca(&certificate_store_dir, &certificate_pem_filename);
    log::info!(
        "CA certificate available at {}",
        certificate_store_dir
            .join(&certificate_pem_filename)
            .display()
    );
    if let Some(download_url) =
        start_certificate_download_server(ca.cert_pem(), certificate_pem_filename)
    {
        let _ = tx.send(AppEvent::CertificateDownloadReady(download_url));
    }

    let authority = RcgenAuthority::new(
        PrivateKey(ca.key_der()),
        rustls::Certificate(ca.cert_der()),
        1_000,
    )
    .unwrap();

    let proxy = ProxyBuilder::new()
        .with_addr(SocketAddr::from(([127, 0, 0, 1], proxy_port)))
        .with_rustls_client()
        .with_ca(authority)
        .with_http_handler(LogHandler::new(tx, mapping_store))
        .build();

    log::info!("Proxy server listening on http://127.0.0.1:{proxy_port}");

    tokio::spawn(async move {
        if let Err(error) = proxy
            .start(async {
                let _ = tokio::signal::ctrl_c().await;
            })
            .await
        {
            panic!("Failed to establish proxy: {error}");
        }
    });
}

fn start_certificate_download_server(cert_pem: String, cert_filename: String) -> Option<String> {
    let listener = match std::net::TcpListener::bind(SocketAddr::from(([0, 0, 0, 0], 0))) {
        Ok(listener) => listener,
        Err(error) => {
            log::error!("Failed to start certificate download server: {error}");
            return None;
        }
    };

    let local_addr = match listener.local_addr() {
        Ok(addr) => addr,
        Err(error) => {
            log::error!("Failed to read certificate download server address: {error}");
            return None;
        }
    };

    if let Err(error) = listener.set_nonblocking(true) {
        log::error!("Failed to configure certificate download server: {error}");
        return None;
    }

    let host = detect_lan_ip()
        .map(format_url_host)
        .unwrap_or_else(|| "127.0.0.1".to_string());
    let url = format!("http://{host}:{}/{cert_filename}", local_addr.port());

    let make_service = make_service_fn(move |_| {
        let cert_pem = cert_pem.clone();
        let cert_filename = cert_filename.clone();
        async move {
            Ok::<_, Infallible>(service_fn(move |req| {
                let cert_pem = cert_pem.clone();
                let cert_filename = cert_filename.clone();
                async move {
                    Ok::<_, Infallible>(certificate_download_response(req, cert_pem, cert_filename))
                }
            }))
        }
    });

    let server = match Server::from_tcp(listener) {
        Ok(server) => server.serve(make_service),
        Err(error) => {
            log::error!("Failed to create certificate download server: {error}");
            return None;
        }
    };

    tokio::spawn(async move {
        if let Err(error) = server.await {
            log::error!("Certificate download server stopped: {error}");
        }
    });

    log::info!("CA certificate download URL: {url}");
    Some(url)
}

fn certificate_download_response(
    req: Request<Body>,
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

struct AppRuntime {
    app: App,
    ui: RootView,
    rx: mpsc::UnboundedReceiver<AppEvent>,
    tui: Tui,
    _settings: SettingsManager,
}

impl AppRuntime {
    fn new(
        app: App,
        rx: mpsc::UnboundedReceiver<AppEvent>,
        tui: Tui,
        settings: SettingsManager,
    ) -> Self {
        Self {
            app,
            ui: RootView::new(),
            rx,
            tui,
            _settings: settings,
        }
    }

    fn run(mut self) -> Result<(), Box<dyn Error>> {
        loop {
            self.tui.draw(&mut self.ui, &mut self.app)?;

            if self.handle_terminal_events()? {
                return Ok(());
            }

            self.handle_app_events();
        }
    }

    fn handle_terminal_events(&mut self) -> io::Result<bool> {
        if !event::poll(Duration::from_millis(10))? {
            return Ok(false);
        }

        match event::read()? {
            Event::Key(key) => Ok(self.app.handle_key_event(key)),
            Event::Mouse(mouse) => {
                self.ui.handle_mouse(mouse, &mut self.app);
                Ok(false)
            }
            _ => Ok(false),
        }
    }

    fn handle_app_events(&mut self) {
        while let Ok(event) = self.rx.try_recv() {
            self.app.handle_app_event(event);
        }
    }
}

struct Tui {
    terminal: ratatui::Terminal<ratatui::backend::CrosstermBackend<io::Stdout>>,
}

impl Tui {
    fn enter() -> io::Result<Self> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
        let backend = ratatui::backend::CrosstermBackend::new(stdout);
        let terminal = ratatui::Terminal::new(backend)?;
        Ok(Self { terminal })
    }

    fn draw(&mut self, ui: &mut RootView, app: &mut App) -> io::Result<()> {
        self.terminal
            .draw(|frame| ui.render(frame, app))
            .map(|_| ())
    }
}

impl Drop for Tui {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(
            self.terminal.backend_mut(),
            LeaveAlternateScreen,
            DisableMouseCapture
        );
    }
}
