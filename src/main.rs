use std::{error::Error, io, net::SocketAddr};
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::{ Alignment, Constraint, Direction, Layout },
    style::{ Modifier, Style },
    symbols,
    text::Line,
    widgets::{ Block, BorderType, Borders, List, ListItem, Paragraph, Tabs, Wrap, Scrollbar, ScrollbarOrientation, ScrollbarState },
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
    // Load or create CA certificate (persists across runs)
    let ca = ca::create_or_load_ca();

    // Get DER bytes for the proxy
    let cert_der = ca.cert_der();
    let key_der = ca.key_der();

    // Export CA certificate as PEM for client trust (copy from .certificate directory)
    let cert_src_path = ".certificate/ca_cert.pem";
    let cert_dst_path = "proxy_ca.pem";
    if let Err(e) = std::fs::copy(cert_src_path, cert_dst_path) {
        log::error!("Failed to copy CA certificate to {}: {}", cert_dst_path, e);
    } else {
        log::info!("CA certificate available at {}. Use it to trust the proxy (e.g., curl --cacert proxy_ca.pem ...)", cert_dst_path);
    }

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

            // Selection indicator title (bottom-right aligned)
            let selection_title = if app.requests.is_empty() {
                String::new()
            } else {
                let selected = app.state.selected().map(|i| i + 1).unwrap_or(0);
                let total = app.requests.len();
                format!("{} of {}", selected, total)
            };

            let list = List::new(items)
                .block(Block::default()
                    .title("Requests")
                    .title_bottom(Line::from(selection_title).alignment(Alignment::Right))
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded))
                .highlight_style(Style::default().add_modifier(Modifier::BOLD))
                .highlight_symbol("→ ");

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

            let tabs = Tabs::new(vec!["Request Header", "Request Body", "Response Header", "Response Body"])
                .block(Block::default().borders(Borders::ALL).border_type(BorderType::Rounded))
                .divider(symbols::DOT)
                .select(match app.active_tab {
                    app::ActiveTab::RequestHeader => 0,
                    app::ActiveTab::RequestBody => 1,
                    app::ActiveTab::ResponseHeader => 2,
                    app::ActiveTab::ResponseBody => 3
                });
            frame.render_widget(tabs, right_main_chunks[0]);

            // Detail Content
            let info_text = match app.state.selected() {
                Some(idx) => {
                    if idx < app.requests.len() {
                        let req = &app.requests[idx];
                        match app.active_tab {
                            app::ActiveTab::RequestHeader => {
                                format!("Method: {}\nURI: {}\n\nHeaders:\n{}", 
                                    req.method, 
                                    req.uri,
                                    req.req_headers.iter().map(|(k,v)| format!("{}: {}", k, v)).collect::<Vec<_>>().join("\n")
                                )
                            },
                            app::ActiveTab::RequestBody => {
                                "Unimplemented".to_string()
                            },
                            app::ActiveTab::ResponseHeader => {
                                "Unimplemented".to_string()
                            },
                            app::ActiveTab::ResponseBody => {
                                "Unimplemented".to_string()
                            }
                        }
                    } else {
                        "Selected index out of bounds".to_string()
                    }
                },
                None => "Select a request".to_string(),
            };

            let line_count = info_text.lines().count();
            let main_display = Paragraph::new(info_text)
                .block(Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .title(match app.active_tab {
                        app::ActiveTab::RequestHeader => "Request Header",
                        app::ActiveTab::RequestBody => "Request Body",
                        app::ActiveTab::ResponseHeader => "Response Header",
                        app::ActiveTab::ResponseBody => "Response Body"
                    }))
                .wrap(Wrap { trim: false }) 
                .scroll((app.vertical_scroll, 0));

            frame.render_widget(main_display, right_content_chunks[0]);

            // Scrollbar
            let scrollbar = Scrollbar::default()
                .orientation(ScrollbarOrientation::VerticalRight)
                .begin_symbol(Some("↑"))
                .end_symbol(Some("↓"));
            let mut scrollbar_state = ScrollbarState::new(line_count).position(app.vertical_scroll as usize);
            frame.render_stateful_widget(
                scrollbar,
                right_content_chunks[0].inner(ratatui::layout::Margin { vertical: 1, horizontal: 0 }),
                &mut scrollbar_state,
            );


            // Log Panel
            let log_items: Vec<ListItem> = app.logs
                .iter()
                .rev()
                .map(|l| ListItem::new(l.clone()))
                .collect();

            let log_list = List::new(log_items)
                .block(Block::default().title("Logs").borders(Borders::ALL).border_type(BorderType::Rounded));

            frame.render_stateful_widget(log_list, right_content_chunks[1], &mut app.log_state);
        })?;

        // Handle Input & Network Events
        if crossterm::event::poll(std::time::Duration::from_millis(10))? {
            if let Event::Key(key) = event::read()? {
                match key.code {
                    KeyCode::Char('q') => break,
                    KeyCode::Tab => app.next_tab(),
                    KeyCode::Char('J') => app.scroll_down(),
                    KeyCode::Char('K') => app.scroll_up(),
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
