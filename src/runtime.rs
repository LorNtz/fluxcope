use crate::{
    app::{App, AppEvent},
    ca,
    logging::AppLogger,
    proxy_handler::{CapturedData, LogHandler},
    ui::RootView,
};
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use hudsucker::{ProxyBuilder, certificate_authority::RcgenAuthority, rustls::PrivateKey};
use std::{collections::HashMap, error::Error, io, net::SocketAddr, sync::Arc, time::Duration};
use tokio::sync::{Mutex, mpsc};

type PendingRequests = Arc<Mutex<HashMap<uuid::Uuid, CapturedData>>>;

pub async fn run() -> Result<(), Box<dyn Error>> {
    let (tx, rx) = mpsc::unbounded_channel::<AppEvent>();
    let pending_requests = Arc::new(Mutex::new(HashMap::new()));

    init_logger(tx.clone());
    start_proxy(tx, pending_requests);

    let tui = Tui::enter()?;
    let app = App::new();
    AppRuntime::new(app, rx, tui).run()
}

fn init_logger(tx: mpsc::UnboundedSender<AppEvent>) {
    if let Err(error) = AppLogger::init(tx) {
        eprintln!("Failed to initialize logger: {error}");
    }
    log::info!("Application started");
}

fn start_proxy(tx: mpsc::UnboundedSender<AppEvent>, pending_requests: PendingRequests) {
    let ca = ca::create_or_load_ca();
    export_ca_certificate();

    let authority = RcgenAuthority::new(
        PrivateKey(ca.key_der()),
        rustls::Certificate(ca.cert_der()),
        1_000,
    )
    .unwrap();

    let proxy = ProxyBuilder::new()
        .with_addr(SocketAddr::from(([127, 0, 0, 1], 8989)))
        .with_rustls_client()
        .with_ca(authority)
        .with_http_handler(LogHandler {
            tx,
            pending_requests,
        })
        .build();

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

fn export_ca_certificate() {
    let cert_src_path = ".certificate/ca_cert.pem";
    let cert_dst_path = "proxy_ca.pem";

    if let Err(error) = std::fs::copy(cert_src_path, cert_dst_path) {
        log::error!("Failed to copy CA certificate to {cert_dst_path}: {error}");
    } else {
        log::info!(
            "CA certificate available at {cert_dst_path}. Use it to trust the proxy (e.g., curl --cacert proxy_ca.pem ...)"
        );
    }
}

struct AppRuntime {
    app: App,
    ui: RootView,
    rx: mpsc::UnboundedReceiver<AppEvent>,
    tui: Tui,
}

impl AppRuntime {
    fn new(app: App, rx: mpsc::UnboundedReceiver<AppEvent>, tui: Tui) -> Self {
        Self {
            app,
            ui: RootView::new(),
            rx,
            tui,
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
            Event::Key(key) => Ok(self.app.handle_key_press(key.code)),
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
