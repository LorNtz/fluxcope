use crate::app::{App, MainDisplayTab};
use crossterm::event::{MouseEvent, MouseEventKind};
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout, Margin, Rect},
    style::{Color, Style},
    symbols,
    text::Line,
    widgets::{
        Block, BorderType, Borders, List, ListItem, Paragraph, Scrollbar, ScrollbarOrientation,
        ScrollbarState, Tabs, Wrap,
    },
};

trait View {
    fn area(&self) -> Rect;
    fn set_area(&mut self, area: Rect);
    fn render(&self, frame: &mut Frame, app: &mut App);

    fn layout(&mut self, area: Rect, _app: &App) {
        self.set_area(area);
    }

    fn contains_mouse(&self, mouse: MouseEvent) -> bool {
        self.area().contains((mouse.column, mouse.row).into())
    }
}

trait MouseHandler: View {
    fn handle_mouse(&self, _mouse: MouseEvent, _app: &mut App) -> bool {
        false
    }
}

pub struct RootView {
    area: Rect,
    request_list: RequestListView,
    right_panel: RightPanelView,
}

impl RootView {
    pub fn new() -> Self {
        Self {
            area: Rect::default(),
            request_list: RequestListView::new(),
            right_panel: RightPanelView::new(),
        }
    }

    pub fn render(&mut self, frame: &mut Frame, app: &mut App) {
        View::layout(self, frame.area(), app);
        View::render(self, frame, app);
    }

    pub fn handle_mouse(&self, mouse: MouseEvent, app: &mut App) {
        let _ = MouseHandler::handle_mouse(self, mouse, app);
    }
}

impl View for RootView {
    fn area(&self) -> Rect {
        self.area
    }

    fn set_area(&mut self, area: Rect) {
        self.area = area;
    }

    fn render(&self, frame: &mut Frame, app: &mut App) {
        self.request_list.render(frame, app);
        self.right_panel.render(frame, app);
    }

    fn layout(&mut self, area: Rect, app: &App) {
        self.set_area(area);

        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(30), Constraint::Percentage(70)])
            .split(area);

        self.request_list.layout(chunks[0], app);
        self.right_panel.layout(chunks[1], app);
    }
}

impl MouseHandler for RootView {
    fn handle_mouse(&self, mouse: MouseEvent, app: &mut App) -> bool {
        self.right_panel.handle_mouse(mouse, app) || self.request_list.handle_mouse(mouse, app)
    }
}

struct RequestListView {
    area: Rect,
}

impl RequestListView {
    fn new() -> Self {
        Self {
            area: Rect::default(),
        }
    }
}

impl View for RequestListView {
    fn area(&self) -> Rect {
        self.area
    }

    fn set_area(&mut self, area: Rect) {
        self.area = area;
    }

    fn render(&self, frame: &mut Frame, app: &mut App) {
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

        frame.render_stateful_widget(list, self.area(), &mut app.request_list.state);
    }
}

impl MouseHandler for RequestListView {
    fn handle_mouse(&self, mouse: MouseEvent, app: &mut App) -> bool {
        if !self.contains_mouse(mouse) {
            return false;
        }

        match mouse.kind {
            MouseEventKind::ScrollDown => {
                app.next();
                true
            }
            MouseEventKind::ScrollUp => {
                app.previous();
                true
            }
            _ => false,
        }
    }
}

struct RightPanelView {
    area: Rect,
    tabs: TabsView,
    detail: DetailView,
    log: LogView,
}

impl RightPanelView {
    fn new() -> Self {
        Self {
            area: Rect::default(),
            tabs: TabsView::new(),
            detail: DetailView::new(),
            log: LogView::new(),
        }
    }
}

impl View for RightPanelView {
    fn area(&self) -> Rect {
        self.area
    }

    fn set_area(&mut self, area: Rect) {
        self.area = area;
    }

    fn render(&self, frame: &mut Frame, app: &mut App) {
        self.tabs.render(frame, app);
        self.detail.render(frame, app);

        if app.log_panel.visible {
            self.log.render(frame, app);
        }
    }

    fn layout(&mut self, area: Rect, app: &App) {
        self.set_area(area);

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

        self.tabs.layout(main_chunks[0], app);
        self.detail.layout(content_chunks[0], app);
        self.log.layout(content_chunks[1], app);
    }
}

impl MouseHandler for RightPanelView {
    fn handle_mouse(&self, mouse: MouseEvent, app: &mut App) -> bool {
        (app.log_panel.visible && self.log.handle_mouse(mouse, app))
            || self.detail.handle_mouse(mouse, app)
    }
}

struct TabsView {
    area: Rect,
}

impl TabsView {
    fn new() -> Self {
        Self {
            area: Rect::default(),
        }
    }
}

impl View for TabsView {
    fn area(&self) -> Rect {
        self.area
    }

    fn set_area(&mut self, area: Rect) {
        self.area = area;
    }

    fn render(&self, frame: &mut Frame, app: &mut App) {
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

        frame.render_widget(tabs, self.area());
    }
}

struct DetailView {
    area: Rect,
}

impl DetailView {
    fn new() -> Self {
        Self {
            area: Rect::default(),
        }
    }
}

impl View for DetailView {
    fn area(&self) -> Rect {
        self.area
    }

    fn set_area(&mut self, area: Rect) {
        self.area = area;
    }

    fn render(&self, frame: &mut Frame, app: &mut App) {
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
            .line_count(self.area().width)
            .try_into()
            .unwrap_or(u16::MAX);
        app.detail_panel.scroll.max_offset = total_lines.saturating_sub(self.area().height);
        paragraph = paragraph.scroll((
            app.detail_panel
                .scroll
                .offset
                .min(app.detail_panel.scroll.max_offset),
            0,
        ));

        frame.render_widget(paragraph, self.area());
        render_scrollbar(
            frame,
            self.area(),
            app.detail_panel.scroll.max_offset,
            app.detail_panel.scroll.offset,
        );
    }
}

impl MouseHandler for DetailView {
    fn handle_mouse(&self, mouse: MouseEvent, app: &mut App) -> bool {
        if !self.contains_mouse(mouse) {
            return false;
        }

        match mouse.kind {
            MouseEventKind::ScrollDown => {
                app.detail_panel.scroll.scroll_down();
                true
            }
            MouseEventKind::ScrollUp => {
                app.detail_panel.scroll.scroll_up();
                true
            }
            _ => false,
        }
    }
}

struct LogView {
    area: Rect,
}

impl LogView {
    fn new() -> Self {
        Self {
            area: Rect::default(),
        }
    }
}

impl View for LogView {
    fn area(&self) -> Rect {
        self.area
    }

    fn set_area(&mut self, area: Rect) {
        self.area = area;
    }

    fn render(&self, frame: &mut Frame, app: &mut App) {
        let mut paragraph = Paragraph::new(app.log_panel.logs.join("\n"))
            .wrap(Wrap { trim: false })
            .block(
                Block::default()
                    .title("Logs")
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded),
            );

        let total_lines: u16 = paragraph
            .line_count(self.area().width)
            .try_into()
            .unwrap_or(u16::MAX);
        app.log_panel.scroll.max_offset = total_lines.saturating_sub(self.area().height);
        paragraph = paragraph.scroll((
            app.log_panel
                .scroll
                .offset
                .min(app.log_panel.scroll.max_offset),
            0,
        ));

        frame.render_widget(paragraph, self.area());
        render_scrollbar(
            frame,
            self.area(),
            app.log_panel.scroll.max_offset,
            app.log_panel.scroll.offset,
        );
    }
}

impl MouseHandler for LogView {
    fn handle_mouse(&self, mouse: MouseEvent, app: &mut App) -> bool {
        if !self.contains_mouse(mouse) {
            return false;
        }

        match mouse.kind {
            MouseEventKind::ScrollDown => {
                app.log_panel.scroll.scroll_down();
                true
            }
            MouseEventKind::ScrollUp => {
                app.log_panel.scroll.scroll_up();
                true
            }
            _ => false,
        }
    }
}

fn render_scrollbar(frame: &mut Frame, area: Rect, max_offset: u16, offset: u16) {
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
