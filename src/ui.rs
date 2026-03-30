use crate::app::{App, MainDisplayTab};
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout, Margin},
    style::{Color, Style},
    symbols,
    text::Line,
    widgets::{
        Block, BorderType, Borders, List, ListItem, Paragraph, Scrollbar, ScrollbarOrientation,
        ScrollbarState, Tabs, Wrap,
    },
};

pub fn render(frame: &mut Frame, app: &mut App) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(30), Constraint::Percentage(70)])
        .split(frame.area());

    render_request_list(frame, app, chunks[0]);
    render_right_panel(frame, app, chunks[1]);
}

fn render_request_list(frame: &mut Frame, app: &mut App, area: ratatui::layout::Rect) {
    let items: Vec<ListItem> = app
        .requests
        .iter()
        .map(|req| ListItem::new(format!("{} {}", req.method, req.uri)))
        .collect();

    let selection_title = if app.requests.is_empty() {
        String::new()
    } else {
        let selected = app
            .request_list
            .state
            .selected()
            .map(|i| i + 1)
            .unwrap_or(0);
        format!("{} of {}", selected, app.requests.len())
    };

    let list = List::new(items)
        .block(
            Block::default()
                .title("Requests")
                .title_bottom(Line::from(selection_title).alignment(Alignment::Right))
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded),
        )
        .highlight_style(Style::default().bg(Color::White).fg(Color::DarkGray));

    frame.render_stateful_widget(list, area, &mut app.request_list.state);
    app.request_list.rect = area;
}

fn render_right_panel(frame: &mut Frame, app: &mut App, area: ratatui::layout::Rect) {
    let main_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(0)])
        .split(area);

    let content_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(if app.log_panel.visible {
            [Constraint::Percentage(50), Constraint::Percentage(50)]
        } else {
            [Constraint::Percentage(100), Constraint::Percentage(0)]
        })
        .split(main_chunks[1]);

    render_tabs(frame, app, main_chunks[0]);
    render_detail_panel(frame, app, content_chunks[0]);
    app.log_panel.rect = content_chunks[1];

    if app.log_panel.visible {
        render_log_panel(frame, app, content_chunks[1]);
    }
}

fn render_tabs(frame: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let tabs = Tabs::new(vec![
        MainDisplayTab::RequestHeader.title(),
        MainDisplayTab::RequestBody.title(),
        MainDisplayTab::ResponseHeader.title(),
        MainDisplayTab::ResponseBody.title(),
    ])
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded),
    )
    .divider(symbols::DOT)
    .select(app.detail_panel.active_tab.index());

    frame.render_widget(tabs, area);
}

fn render_detail_panel(frame: &mut Frame, app: &mut App, area: ratatui::layout::Rect) {
    let detail_text = build_detail_text(app);
    let mut paragraph = Paragraph::new(detail_text)
        .block(
            Block::default()
                .title(app.detail_panel.active_tab.title())
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded),
        )
        .wrap(Wrap { trim: false });

    let total_lines: u16 = paragraph
        .line_count(area.width)
        .try_into()
        .unwrap_or(u16::MAX);
    app.detail_panel.scroll.max_offset = total_lines.saturating_sub(area.height);
    paragraph = paragraph.scroll((
        app.detail_panel
            .scroll
            .offset
            .min(app.detail_panel.scroll.max_offset),
        0,
    ));

    frame.render_widget(paragraph, area);
    app.detail_panel.rect = area;

    render_scrollbar(
        frame,
        area,
        app.detail_panel.scroll.max_offset,
        app.detail_panel.scroll.offset,
    );
}

fn render_log_panel(frame: &mut Frame, app: &mut App, area: ratatui::layout::Rect) {
    let mut paragraph = Paragraph::new(app.log_panel.logs.join("\n"))
        .wrap(Wrap { trim: false })
        .block(
            Block::default()
                .title("Logs")
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded),
        );

    let total_lines: u16 = paragraph
        .line_count(area.width)
        .try_into()
        .unwrap_or(u16::MAX);
    app.log_panel.scroll.max_offset = total_lines.saturating_sub(area.height);
    paragraph = paragraph.scroll((
        app.log_panel
            .scroll
            .offset
            .min(app.log_panel.scroll.max_offset),
        0,
    ));

    frame.render_widget(paragraph, area);
    render_scrollbar(
        frame,
        area,
        app.log_panel.scroll.max_offset,
        app.log_panel.scroll.offset,
    );
}

fn render_scrollbar(frame: &mut Frame, area: ratatui::layout::Rect, max_offset: u16, offset: u16) {
    let scrollbar = Scrollbar::default()
        .orientation(ScrollbarOrientation::VerticalRight)
        .begin_symbol(Some("↑"))
        .end_symbol(Some("↓"));

    let mut state = ScrollbarState::new(max_offset as usize).position(offset as usize);
    frame.render_stateful_widget(
        scrollbar,
        area.inner(Margin {
            vertical: 1,
            horizontal: 0,
        }),
        &mut state,
    );
}

fn build_detail_text(app: &App) -> String {
    let Some(req) = app.selected_request() else {
        return "Select a request".to_string();
    };

    match app.detail_panel.active_tab {
        MainDisplayTab::RequestHeader => format!(
            "Method: {}\nURI: {}\n\nHeaders:\n{}",
            req.method,
            req.uri,
            join_headers(&req.req_headers)
        ),
        MainDisplayTab::RequestBody => {
            format_request_body(req.req_body.as_deref(), &req.req_headers)
        }
        MainDisplayTab::ResponseHeader => format!(
            "Status: {}\n\nHeaders:\n{}",
            req.status
                .map_or("N/A".to_string(), |status| status.to_string()),
            join_headers(&req.res_headers)
        ),
        MainDisplayTab::ResponseBody => format_response_body(req.res_body.as_deref()),
    }
}

fn join_headers(headers: &[(String, String)]) -> String {
    headers
        .iter()
        .map(|(key, value)| format!("{key}: {value}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn format_request_body(body: Option<&str>, headers: &[(String, String)]) -> String {
    match body {
        Some("") => "(Empty body)".to_string(),
        Some(body) if is_form_data(headers) => format_form_body(body),
        Some(body) => body.to_string(),
        None => "(No body)".to_string(),
    }
}

fn format_response_body(body: Option<&str>) -> String {
    match body {
        Some("") => "(Empty body)".to_string(),
        Some(body) => format_json_body(body),
        None => "(No body)".to_string(),
    }
}

fn is_form_data(headers: &[(String, String)]) -> bool {
    headers.iter().any(|(key, value)| {
        key.eq_ignore_ascii_case("content-type")
            && value
                .to_ascii_lowercase()
                .contains("application/x-www-form-urlencoded")
    })
}

fn format_form_body(body: &str) -> String {
    body.split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let mut parts = pair.splitn(2, '=');
            let key = parts.next().unwrap_or("");
            let value = parts.next().unwrap_or("");
            format!(
                "{}: {}",
                decode_url_component(key),
                decode_url_component(value)
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn format_json_body(body: &str) -> String {
    serde_json::from_str::<serde_json::Value>(body)
        .map(|value| serde_json::to_string_pretty(&value).unwrap_or_else(|_| body.to_string()))
        .unwrap_or_else(|_| body.to_string())
}

fn decode_url_component(input: &str) -> String {
    let mut result = String::new();
    let mut bytes = input.as_bytes().iter();

    while let Some(&byte) = bytes.next() {
        if byte == b'%' {
            let hex1 = bytes.next();
            let hex2 = bytes.next();

            if let (Some(&h1), Some(&h2)) = (hex1, hex2) {
                if let (Some(hi), Some(lo)) = (hex_to_nibble(h1), hex_to_nibble(h2)) {
                    result.push((hi * 16 + lo) as char);
                } else {
                    result.push(byte as char);
                    result.push(h1 as char);
                    result.push(h2 as char);
                }
            } else {
                result.push(byte as char);
                if let Some(&h1) = hex1 {
                    result.push(h1 as char);
                }
            }
        } else if byte == b'+' {
            result.push(' ');
        } else {
            result.push(byte as char);
        }
    }

    result
}

fn hex_to_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}
