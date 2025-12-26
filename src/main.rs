use std::{error::Error, io, net::SocketAddr};
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    widgets::{Block, Borders, Paragraph, Tabs, Wrap, List, ListItem},
    Terminal,
    style::{Style, Modifier},
};
use hudsucker::{ProxyBuilder, certificate_authority::RcgenAuthority, rustls::{PrivateKey}};
use tokio::sync::mpsc;

mod ca;
mod proxy_handler;
mod app;
mod logging;

use app::{App, AppEvent};
use proxy_handler::LogHandler;
use logging::AppLogger;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    // 1. Setup Channel
    let (tx, mut rx) = mpsc::unbounded_channel::<AppEvent>();

    // 2. Setup Logger
    if let Err(e) = AppLogger::init(tx.clone()) {
        eprintln!("Failed to initialize logger: {}", e);
    }
    log::info!("Application started");

    // 3. Setup Proxy (Background Task)
    let cert = ca::create_ca();
    
    // let (ca_cert, ca_key) = ca::create_ca();
    let cert_der = cert.serialize_der().unwrap();
    let key_der = cert.serialize_private_key_der(); // This returns Vec<u8>

    let rustls_cert = rustls::Certificate(cert_der);
    let rustls_key = PrivateKey(key_der);
    
    let ca = RcgenAuthority::new(
        rustls_key,  // Private Key comes first
        rustls_cert, // Certificate comes second
        1_000        // Cache size
    ).unwrap();

    let proxy = ProxyBuilder::new()
        .with_addr(SocketAddr::from(([127, 0, 0, 1], 8989)))
        .with_rustls_client()
        .with_ca(ca)
        .with_http_handler(LogHandler { tx: tx.clone() })
        .build();

    tokio::spawn(async move {
        if let Err(e) = proxy.start(async {
            let _ = tokio::signal::ctrl_c().await;
        }).await {
            panic!("Failed to establish proxy: {}", e);
            // eprintln!("Proxy failed: {}", e);
        }
    });

    // 4. Setup TUI
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new();

    loop {
        // Draw UI
        terminal.draw(|frame| {
            let chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(30), Constraint::Percentage(70)].as_ref())
                .split(frame.area());
            
            // Left Panel: List
            let items: Vec<ListItem> = app.requests
                .iter()
                .map(|req| ListItem::new(format!("{} {}", req.method, req.uri)))
                .collect();

            let list = List::new(items)
                .block(Block::default().title("Requests").borders(Borders::ALL))
                .highlight_style(Style::default().add_modifier(Modifier::BOLD))
                .highlight_symbol(">> ");

            frame.render_stateful_widget(list, chunks[0], &mut app.state);

            // Right Panel: Details
            let right_main_chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(3), // Tabs
                    Constraint::Min(0),    // Rest
                ].as_ref())
                .split(chunks[1]);

            let right_content_chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Percentage(50), // Main Details
                    Constraint::Percentage(50), // Logs
                ].as_ref())
                .split(right_main_chunks[1]);

            let tabs = Tabs::new(vec!["Request", "Response", "Body"])
                .block(Block::default().borders(Borders::ALL).title("Info"))
                .select(match app.active_tab {
                    app::ActiveTab::Request => 0,
                    app::ActiveTab::Response => 1,
                    app::ActiveTab::Body => 2,
                });
            frame.render_widget(tabs, right_main_chunks[0]);

            // Detail Content
            let info_text = match app.state.selected() {
                Some(idx) => {
                    if idx < app.requests.len() {
                        format!("Details for {}", app.requests[idx].uri)
                    } else {
                        "Selected index out of bounds".to_string()
                    }
                },
                None => "Select a request".to_string(),
            };
            frame.render_widget(Paragraph::new(info_text).wrap(Wrap { trim: true }), right_content_chunks[0]);

            // Log Panel
            let log_items: Vec<ListItem> = app.logs
                .iter()
                .rev() // Show newest at the bottom? List usually renders top to bottom.
                       // If we want auto-scroll, we just keep appending and scrolling.
                       // For now, let's just show them order by time.
                .map(|l| ListItem::new(l.clone()))
                .collect();
            
            let log_list = List::new(log_items)
                .block(Block::default().title("Logs").borders(Borders::ALL));
            
            frame.render_stateful_widget(log_list, right_content_chunks[1], &mut app.log_state);
        })?;

        // Handle Input & Network Events
        if crossterm::event::poll(std::time::Duration::from_millis(10))? {
            if let Event::Key(key) = event::read()? {
                match key.code {
                    KeyCode::Char('q') => break,
                    KeyCode::Tab => app.next_tab(),
                    KeyCode::Char('j') | KeyCode::Down => app.next(),
                    KeyCode::Char('k') | KeyCode::Up => app.previous(),
                    _ => {} 
                }
            }
        }

        // Poll Channel for new events
        while let Ok(event) = rx.try_recv() {
            match event {
                AppEvent::NetworkRequest(req) => app.add_request(req),
                AppEvent::LogMessage(msg) => app.add_log(msg),
            }
        }
    }

    // Cleanup
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen, DisableMouseCapture)?;
    Ok(())
}
