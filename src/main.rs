use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, MouseEventKind},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use hudsucker::{ProxyBuilder, certificate_authority::RcgenAuthority, rustls::PrivateKey};
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout},
    style::{Modifier, Style},
    symbols,
    text::Line,
    widgets::{
        Block, BorderType, Borders, List, ListItem, Paragraph, Scrollbar, ScrollbarOrientation,
        ScrollbarState, Tabs, Wrap,
    },
};
use std::{error::Error, io, net::SocketAddr};
use tokio::sync::mpsc;

mod app;
mod ca;
mod logging;
mod proxy_handler;

use app::{App, AppEvent};
use logging::AppLogger;
use proxy_handler::LogHandler;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

/// Format URL-encoded form data as key-value pairs
/// Returns formatted string like "key1: value1\nkey2: value2"
fn format_form_body(body: &str) -> String {
    body.split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let mut parts = pair.splitn(2, '=');
            let key = parts.next().unwrap_or("");
            let value = parts.next().unwrap_or("");
            // URL-decode both key and value
            let key_decoded = decode_url_component(key);
            let value_decoded = decode_url_component(value);
            format!("{}: {}", key_decoded, value_decoded)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Convert a hex character to its numeric value (0-15)
fn hex_to_nibble(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

/// Simple URL decoder for form data
fn decode_url_component(s: &str) -> String {
    let mut result = String::new();
    let mut bytes = s.as_bytes().iter();
    while let Some(&b) = bytes.next() {
        if b == b'%' {
            let hex1 = bytes.next();
            let hex2 = bytes.next();
            if let (Some(&h1), Some(&h2)) = (hex1, hex2) {
                if let (Some(hi), Some(lo)) = (hex_to_nibble(h1), hex_to_nibble(h2)) {
                    result.push((hi * 16 + lo) as char);
                } else {
                    result.push(b as char);
                    result.push(h1 as char);
                    result.push(h2 as char);
                }
            } else {
                result.push(b as char);
                if let Some(&h1) = hex1 {
                    result.push(h1 as char);
                }
            }
        } else if b == b'+' {
            result.push(' ');
        } else {
            result.push(b as char);
        }
    }
    result
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    // 1. Setup Channel
    let (tx, mut rx) = mpsc::unbounded_channel::<AppEvent>();

    // Setup pending requests storage for request-response matching
    let pending_requests = Arc::new(Mutex::new(
        HashMap::<uuid::Uuid, proxy_handler::CapturedData>::new(),
    ));

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
        log::info!(
            "CA certificate available at {}. Use it to trust the proxy (e.g., curl --cacert proxy_ca.pem ...)",
            cert_dst_path
        );
    }

    let rustls_cert = rustls::Certificate(cert_der);
    let rustls_key = PrivateKey(key_der);

    let ca = RcgenAuthority::new(
        rustls_key,  // Private Key comes first
        rustls_cert, // Certificate comes second
        1_000,       // Cache size
    )
    .unwrap();

    let proxy = ProxyBuilder::new()
        .with_addr(SocketAddr::from(([127, 0, 0, 1], 8989)))
        .with_rustls_client()
        .with_ca(ca)
        .with_http_handler(LogHandler {
            tx: tx.clone(),
            pending_requests: Arc::clone(&pending_requests),
        })
        .build();

    tokio::spawn(async move {
        if let Err(e) = proxy
            .start(async {
                let _ = tokio::signal::ctrl_c().await;
            })
            .await
        {
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
            let items: Vec<ListItem> = app
                .requests
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
                .block(
                    Block::default()
                        .title("Requests")
                        .title_bottom(Line::from(selection_title).alignment(Alignment::Right))
                        .borders(Borders::ALL)
                        .border_type(BorderType::Rounded),
                )
                .highlight_style(Style::default().add_modifier(Modifier::BOLD))
                .highlight_symbol("→ ");

            frame.render_stateful_widget(list, chunks[0], &mut app.state);
            app.request_list_rect = chunks[0];

            // Right Panel: Details
            let right_main_chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints(
                    [
                        Constraint::Length(3), // Tabs
                        Constraint::Min(0),    // Rest
                    ]
                    .as_ref(),
                )
                .split(chunks[1]);

            let right_content_chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints(if app.log_panel_visible {
                    [
                        Constraint::Percentage(50), // Main Details
                        Constraint::Percentage(50), // Logs
                    ]
                    .as_ref()
                } else {
                    [
                        Constraint::Percentage(100), // Main Details (full)
                        Constraint::Percentage(0),   // Logs (hidden)
                    ]
                    .as_ref()
                })
                .split(right_main_chunks[1]);

            let tabs = Tabs::new(vec![
                "Request Header",
                "Request Body",
                "Response Header",
                "Response Body",
            ])
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded),
            )
            .divider(symbols::DOT)
            .select(match app.active_tab {
                app::ActiveTab::RequestHeader => 0,
                app::ActiveTab::RequestBody => 1,
                app::ActiveTab::ResponseHeader => 2,
                app::ActiveTab::ResponseBody => 3,
            });
            frame.render_widget(tabs, right_main_chunks[0]);

            // Detail Content
            let info_text = match app.state.selected() {
                Some(idx) => {
                    if idx < app.requests.len() {
                        let req = &app.requests[idx];
                        match app.active_tab {
                            app::ActiveTab::RequestHeader => {
                                format!(
                                    "Method: {}\nURI: {}\n\nHeaders:\n{}",
                                    req.method,
                                    req.uri,
                                    req.req_headers
                                        .iter()
                                        .map(|(k, v)| format!("{}: {}", k, v))
                                        .collect::<Vec<_>>()
                                        .join("\n")
                                )
                            }
                            app::ActiveTab::RequestBody => match &req.req_body {
                                Some(body) => {
                                    if body.is_empty() {
                                        "(Empty body)".to_string()
                                    } else {
                                        // Check if this is URL-encoded form data
                                        let is_form_data = req.req_headers.iter()
                                            .any(|(k, v)| {
                                                k.to_lowercase() == "content-type"
                                                    && v.to_lowercase().contains("application/x-www-form-urlencoded")
                                            });
                                        if is_form_data {
                                            format_form_body(body)
                                        } else {
                                            body.clone()
                                        }
                                    }
                                }
                                None => "(No body)".to_string(),
                            },
                            app::ActiveTab::ResponseHeader => {
                                format!(
                                    "Status: {}\n\nHeaders:\n{}",
                                    req.status.map_or("N/A".to_string(), |s| s.to_string()),
                                    req.res_headers
                                        .iter()
                                        .map(|(k, v)| format!("{}: {}", k, v))
                                        .collect::<Vec<_>>()
                                        .join("\n")
                                )
                            }
                            app::ActiveTab::ResponseBody => match &req.res_body {
                                Some(body) => {
                                    if body.is_empty() {
                                        "(Empty body)".to_string()
                                    } else {
                                        body.clone()
                                    }
                                }
                                None => "(No body)".to_string(),
                            },
                        }
                    } else {
                        "Selected index out of bounds".to_string()
                    }
                }
                None => "Select a request".to_string(),
            };

            let mut main_display = Paragraph::new(info_text)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_type(BorderType::Rounded)
                        .title(match app.active_tab {
                            app::ActiveTab::RequestHeader => "Request Header",
                            app::ActiveTab::RequestBody => "Request Body",
                            app::ActiveTab::ResponseHeader => "Response Header",
                            app::ActiveTab::ResponseBody => "Response Body",
                        }),
                )
                .wrap(Wrap { trim: false });
            let total_lines: u16 = main_display.line_count(right_content_chunks[0].width).try_into().unwrap();
            app.max_vertical_scroll = total_lines.saturating_sub(right_content_chunks[0].height);
            main_display = main_display.scroll((app.vertical_scroll.min(app.max_vertical_scroll), 0));

            frame.render_widget(main_display, right_content_chunks[0]);
            app.main_display_rect = right_content_chunks[0];
            app.log_panel_rect = right_content_chunks[1];
            
            // Scrollbar
            let scrollbar = Scrollbar::default()
                .orientation(ScrollbarOrientation::VerticalRight)
                .begin_symbol(Some("↑"))
                .end_symbol(Some("↓"));
            let mut scrollbar_state =
                ScrollbarState::new(app.max_vertical_scroll.into()).position(app.vertical_scroll as usize);
            frame.render_stateful_widget(
                scrollbar,
                right_content_chunks[0].inner(ratatui::layout::Margin {
                    vertical: 1,
                    horizontal: 0,
                }),
                &mut scrollbar_state,
            );

            // Log Panel (only render when visible)
            if app.log_panel_visible {
                // Join all log messages with newlines for paragraph display
                // Newer logs should be at the end, so we don't reverse
                let log_text = app.logs.join("\n");

                let mut log_paragraph = Paragraph::new(log_text)
                    .wrap(Wrap { trim: false })
                    .block(
                        Block::default()
                            .title("Logs")
                            .borders(Borders::ALL)
                            .border_type(BorderType::Rounded),
                    );
                let log_total_lines: u16 = log_paragraph.line_count(right_content_chunks[1].width).try_into().unwrap();
                app.max_log_scroll = log_total_lines.saturating_sub(right_content_chunks[1].height);
                log_paragraph = log_paragraph.scroll((app.log_scroll.min(app.max_log_scroll), 0));

                frame.render_widget(log_paragraph, right_content_chunks[1]);

                let mut log_scrollbar_state = ScrollbarState::default()
                    .content_length(app.max_log_scroll.into())
                    .position(app.log_scroll.into());

                let log_scrollbar = Scrollbar::default()
                    .orientation(ScrollbarOrientation::VerticalRight)
                    .begin_symbol(Some("↑"))
                    .end_symbol(Some("↓"));

                frame.render_stateful_widget(
                    log_scrollbar,
                    right_content_chunks[1].inner(ratatui::layout::Margin {
                        vertical: 1,
                        horizontal: 0,
                    }),
                    &mut log_scrollbar_state,
                );
            }
        })?;

        // Handle Input & Network Events
        if crossterm::event::poll(std::time::Duration::from_millis(10))? {
            match event::read()? {
                Event::Key(key) => match key.code {
                    KeyCode::Char('q') => break,
                    KeyCode::Tab => app.next_tab(),
                    KeyCode::Char('J') => app.scroll_down(),
                    KeyCode::Char('K') => app.scroll_up(),
                    KeyCode::Char('j') | KeyCode::Down => app.next(),
                    KeyCode::Char('k') | KeyCode::Up => app.previous(),
                    KeyCode::Char('@') => app.toggle_log_panel(),
                    _ => {}
                },
                Event::Mouse(mouse) => {
                    let col = mouse.column;
                    let row = mouse.row;
                    match mouse.kind {
                        MouseEventKind::ScrollDown => {
                            if app.log_panel_visible && app.log_panel_rect.contains((col, row).into()) {
                                app.scroll_log_down();
                            } else if app.main_display_rect.contains((col, row).into()) {
                                app.scroll_down();
                            } else if app.request_list_rect.contains((col, row).into()) {
                                app.next();
                            }
                        }
                        MouseEventKind::ScrollUp => {
                            if app.log_panel_visible && app.log_panel_rect.contains((col, row).into()) {
                                app.scroll_log_up();
                            } else if app.main_display_rect.contains((col, row).into()) {
                                app.scroll_up();
                            } else if app.request_list_rect.contains((col, row).into()) {
                                app.previous();
                            }
                        }
                        _ => {}
                    }
                }
                _ => {}
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
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    Ok(())
}
