use crate::app::{App, MainDisplayTab, PanelFocus, PopupFocus, RequestTreeEntry};
use crossterm::event::{MouseEvent, MouseEventKind};
use qrcode::{EcLevel, QrCode, render::unicode};
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout, Margin, Position, Rect},
    style::{Color, Style},
    symbols,
    text::{Line, Span},
    widgets::{
        Block, BorderType, Borders, Clear, Paragraph, Scrollbar, ScrollbarOrientation,
        ScrollbarState, Tabs, Wrap,
    },
};
use tui_tree_widget::{Tree, TreeItem};

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
    status: StatusView,
    request_list: RequestListView,
    right_panel: RightPanelView,
}

impl RootView {
    pub fn new() -> Self {
        Self {
            area: Rect::default(),
            status: StatusView::new(),
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
        self.status.render(frame, app);
        self.request_list.render(frame, app);
        self.right_panel.render(frame, app);

        if app.certificate_popup.visible {
            render_certificate_popup(frame, app);
        }
    }

    fn layout(&mut self, area: Rect, app: &App) {
        self.set_area(area);

        let root_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(3), Constraint::Min(0)])
            .split(area);

        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(30), Constraint::Percentage(70)])
            .split(root_chunks[1]);

        self.status.layout(root_chunks[0], app);
        self.request_list.layout(chunks[0], app);
        self.right_panel.layout(chunks[1], app);
    }
}

impl MouseHandler for RootView {
    fn handle_mouse(&self, mouse: MouseEvent, app: &mut App) -> bool {
        if app.certificate_popup.visible {
            return true;
        }

        self.right_panel.handle_mouse(mouse, app) || self.request_list.handle_mouse(mouse, app)
    }
}

struct StatusView {
    area: Rect,
}

impl StatusView {
    fn new() -> Self {
        Self {
            area: Rect::default(),
        }
    }
}

impl View for StatusView {
    fn area(&self) -> Rect {
        self.area
    }

    fn set_area(&mut self, area: Rect) {
        self.area = area;
    }

    fn render(&self, frame: &mut Frame, app: &mut App) {
        let (icon, label, style) = if app.is_recording() {
            ("●", "Recording ON", Style::default().fg(Color::Red))
        } else {
            ("○", "Recording OFF", Style::default().fg(Color::DarkGray))
        };
        let status = Line::from(vec![
            Span::styled(icon, style),
            Span::raw(format!(" {label}")),
        ]);

        frame.render_widget(
            Paragraph::new(status)
                .block(panel_block("Status", false))
                .alignment(Alignment::Left),
            self.area(),
        );
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
        let items = build_request_tree_items(app);
        let focused = app.is_panel_focused(PanelFocus::RequestList);
        let tree = Tree::new(&items)
            .expect("request tree identifiers are unique")
            .block(panel_block("Requests", focused))
            .highlight_style(Style::default().bg(Color::White).fg(Color::DarkGray))
            .node_closed_symbol("▶ ")
            .node_open_symbol("▼ ")
            .node_no_children_symbol("  ")
            .experimental_scrollbar(Some(
                Scrollbar::default()
                    .orientation(ScrollbarOrientation::VerticalRight)
                    .begin_symbol(Some("↑"))
                    .end_symbol(Some("↓")),
            ));

        frame.render_stateful_widget(tree, self.area(), &mut app.request_list.state);
    }
}

impl MouseHandler for RequestListView {
    fn handle_mouse(&self, mouse: MouseEvent, app: &mut App) -> bool {
        if !self.contains_mouse(mouse) {
            return false;
        }

        match mouse.kind {
            MouseEventKind::ScrollDown => {
                app.request_list.scroll_down();
                true
            }
            MouseEventKind::ScrollUp => {
                app.request_list.scroll_up();
                true
            }
            MouseEventKind::Down(_) => {
                app.focus_panel(PanelFocus::RequestList);
                let changed = app
                    .request_list
                    .click_at(Position::new(mouse.column, mouse.row));
                app.apply_request_list_change(changed);
                true
            }
            _ => false,
        }
    }
}

#[derive(Debug)]
struct RequestTreeNode {
    identifier: String,
    label: String,
    children: Vec<RequestTreeNode>,
}

impl RequestTreeNode {
    fn new(identifier: String, label: String) -> Self {
        Self {
            identifier,
            label,
            children: Vec::new(),
        }
    }

    fn branch_child_mut_or_insert(&mut self, identifier: String, label: String) -> &mut Self {
        if let Some(index) = self
            .children
            .iter()
            .position(|child| child.identifier == identifier)
        {
            return &mut self.children[index];
        }

        let insert_index = self
            .children
            .iter()
            .position(RequestTreeNode::is_leaf)
            .unwrap_or(self.children.len());
        self.children
            .insert(insert_index, Self::new(identifier, label));
        &mut self.children[insert_index]
    }

    fn is_leaf(&self) -> bool {
        self.children.is_empty()
    }

    fn into_tree_item(self) -> TreeItem<'static, String> {
        if self.children.is_empty() {
            TreeItem::new_leaf(self.identifier, self.label)
        } else {
            TreeItem::new(
                self.identifier,
                self.label,
                self.children
                    .into_iter()
                    .map(RequestTreeNode::into_tree_item)
                    .collect(),
            )
            .expect("request tree node child identifiers are unique")
        }
    }
}

fn build_request_tree_items(app: &App) -> Vec<TreeItem<'static, String>> {
    let mut roots = Vec::new();

    for req in &app.requests {
        insert_request_tree_entry(&mut roots, RequestTreeEntry::from(req));
    }

    roots
        .into_iter()
        .map(RequestTreeNode::into_tree_item)
        .collect()
}

fn insert_request_tree_entry(roots: &mut Vec<RequestTreeNode>, entry: RequestTreeEntry) {
    let identifiers = entry.request_path();
    let Some(origin_identifier) = identifiers.first() else {
        return;
    };
    let Some(request_identifier) = identifiers.last() else {
        return;
    };

    let current = root_mut_or_insert(roots, origin_identifier.clone(), entry.origin.clone());
    let parent_segments = entry
        .segments
        .split_last()
        .map_or(&[] as &[String], |(_, parent_segments)| parent_segments);

    let mut current = current;
    let parent_identifiers = if identifiers.len() > 2 {
        &identifiers[1..identifiers.len() - 1]
    } else {
        &[]
    };
    for (identifier, label) in parent_identifiers.iter().zip(parent_segments) {
        current = current.branch_child_mut_or_insert(identifier.clone(), label.clone());
    }

    let leaf_label = entry
        .segments
        .last()
        .cloned()
        .unwrap_or_else(|| "/".to_string());
    current
        .children
        .push(RequestTreeNode::new(request_identifier.clone(), leaf_label));
}

fn root_mut_or_insert(
    roots: &mut Vec<RequestTreeNode>,
    identifier: String,
    label: String,
) -> &mut RequestTreeNode {
    if let Some(index) = roots.iter().position(|root| root.identifier == identifier) {
        return &mut roots[index];
    }

    roots.push(RequestTreeNode::new(identifier, label));
    roots
        .last_mut()
        .expect("root was inserted immediately before access")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proxy_handler::CapturedData;
    use crate::settings::{RequestListSettings, UiSettings};
    use crossterm::event::{KeyModifiers, MouseButton};
    use http::Method;

    fn ui_settings(auto_expand: bool) -> UiSettings {
        UiSettings {
            request_list: RequestListSettings { auto_expand },
        }
    }

    fn captured(sequence: u64, uri: &str) -> CapturedData {
        CapturedData {
            id: uuid::Uuid::nil(),
            sequence,
            method: Method::GET,
            uri: uri.to_string(),
            mapped_uri: None,
            local_path: None,
            status: None,
            req_headers: vec![],
            res_headers: vec![],
            req_body: None,
            res_body: None,
        }
    }

    #[test]
    fn map_local_response_body_preserves_raw_text() {
        let raw_body = "{\n  \"z\": 1,\n  \"a\": 2\n}\n";
        let mut req = captured(0, "https://a.com/api");
        req.local_path = Some("/tmp/api.json".to_string());
        req.res_body = Some(raw_body.to_string());

        assert_eq!(format_response_body(&req), raw_body);
    }

    #[test]
    fn non_local_response_body_keeps_json_formatting() {
        let raw_body = r#"{"z":1,"a":2}"#;
        let mut req = captured(0, "https://a.com/api");
        req.res_body = Some(raw_body.to_string());

        assert_ne!(format_response_body(&req), raw_body);
    }

    fn mouse(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::empty(),
        }
    }

    fn laid_out_ui(app: &App) -> RootView {
        let mut ui = RootView::new();
        View::layout(&mut ui, Rect::new(0, 0, 100, 12), app);

        ui
    }

    fn mouse_inside(kind: MouseEventKind, area: Rect) -> MouseEvent {
        mouse(kind, area.x.saturating_add(1), area.y.saturating_add(1))
    }

    fn mouse_down_inside(area: Rect) -> MouseEvent {
        mouse_inside(MouseEventKind::Down(MouseButton::Left), area)
    }

    #[test]
    fn request_tree_orders_branch_nodes_before_leaf_requests() {
        let mut app = App::new(ui_settings(true));
        app.add_request(captured(0, "https://a.com/some/api2"));
        app.add_request(captured(1, "https://a.com/some/path/api1"));

        let items = build_request_tree_items(&app);
        let origin = &items[0];
        let some = &origin.children()[0];
        let child_identifiers = some
            .children()
            .iter()
            .map(|child| child.identifier().as_str())
            .collect::<Vec<_>>();

        assert_eq!(child_identifiers, ["segment:path", "request:0"]);
    }

    #[test]
    fn request_tree_preserves_branch_incoming_order_before_leaves() {
        let mut app = App::new(ui_settings(true));
        app.add_request(captured(0, "https://a.com/some/api0"));
        app.add_request(captured(1, "https://a.com/some/b/api1"));
        app.add_request(captured(2, "https://a.com/some/a/api2"));

        let items = build_request_tree_items(&app);
        let some = &items[0].children()[0];
        let child_identifiers = some
            .children()
            .iter()
            .map(|child| child.identifier().as_str())
            .collect::<Vec<_>>();

        assert_eq!(child_identifiers, ["segment:b", "segment:a", "request:0"]);
    }

    #[test]
    fn mouse_movement_and_scroll_do_not_move_panel_focus() {
        let mut app = App::new(ui_settings(true));
        let ui = laid_out_ui(&app);

        ui.handle_mouse(
            mouse_inside(MouseEventKind::Moved, ui.right_panel.detail.area()),
            &mut app,
        );
        assert!(app.is_panel_focused(PanelFocus::RequestList));

        ui.handle_mouse(
            mouse_inside(MouseEventKind::ScrollDown, ui.right_panel.log.area()),
            &mut app,
        );
        assert!(app.is_panel_focused(PanelFocus::RequestList));
    }

    #[test]
    fn mouse_click_focuses_clicked_panel() {
        let mut app = App::new(ui_settings(true));
        let ui = laid_out_ui(&app);

        ui.handle_mouse(mouse_down_inside(ui.right_panel.detail.area()), &mut app);
        assert!(app.is_panel_focused(PanelFocus::Detail));

        ui.handle_mouse(mouse_down_inside(ui.right_panel.log.area()), &mut app);
        assert!(app.is_panel_focused(PanelFocus::Log));

        ui.handle_mouse(mouse_down_inside(ui.request_list.area()), &mut app);
        assert!(app.is_panel_focused(PanelFocus::RequestList));
    }

    #[test]
    fn status_panel_does_not_take_focus() {
        let mut app = App::new(ui_settings(true));
        let ui = laid_out_ui(&app);

        ui.handle_mouse(mouse_down_inside(ui.status.area()), &mut app);

        assert!(app.is_panel_focused(PanelFocus::RequestList));
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
        self.tabs.handle_mouse(mouse, app)
            || (app.log_panel.visible && self.log.handle_mouse(mouse, app))
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
        let focused = app.is_panel_focused(PanelFocus::Detail);
        let tabs = Tabs::new(vec![
            MainDisplayTab::RequestHeader.title(),
            MainDisplayTab::RequestBody.title(),
            MainDisplayTab::ResponseHeader.title(),
            MainDisplayTab::ResponseBody.title(),
        ])
        .block(untitled_panel_block(focused))
        .divider(symbols::line::VERTICAL)
        .select(app.detail_panel.active_tab.index());

        frame.render_widget(tabs, self.area());
    }
}

impl MouseHandler for TabsView {
    fn handle_mouse(&self, mouse: MouseEvent, app: &mut App) -> bool {
        if !self.contains_mouse(mouse) {
            return false;
        }

        if matches!(mouse.kind, MouseEventKind::Down(_)) {
            app.focus_panel(PanelFocus::Detail);
            return true;
        }

        false
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
        let focused = app.is_panel_focused(PanelFocus::Detail);
        let mut paragraph = Paragraph::new(detail_text)
            .block(panel_block(app.detail_panel.active_tab.title(), focused))
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
            MouseEventKind::Down(_) => {
                app.focus_panel(PanelFocus::Detail);
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
        let focused = app.is_panel_focused(PanelFocus::Log);
        let mut paragraph = Paragraph::new(app.log_panel.logs.join("\n"))
            .wrap(Wrap { trim: false })
            .block(panel_block("Logs", focused));

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
            MouseEventKind::Down(_) => {
                app.focus_panel(PanelFocus::Log);
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

fn panel_block(title: &'static str, focused: bool) -> Block<'static> {
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded);
    apply_focus_border(block, focused)
}

fn untitled_panel_block(focused: bool) -> Block<'static> {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded);
    apply_focus_border(block, focused)
}

fn apply_focus_border(block: Block<'static>, focused: bool) -> Block<'static> {
    if focused {
        block.border_style(Style::default().fg(Color::Green))
    } else {
        block
    }
}

fn render_certificate_popup(frame: &mut Frame, app: &App) {
    let download_url = app.certificate_popup.download_url.as_deref();
    let qr_lines = download_url.map(build_qr_lines).unwrap_or_default();
    let description_lines = [
        "Connect your mobile phone to the same LAN as this device",
        "then scan to download the proxy CA certificate and install it.",
        "Don't forget to manually trust the CA if you're on iOS 10 or later.",
    ];
    let qr_width = qr_lines
        .iter()
        .map(|line| line.chars().count() as u16)
        .max()
        .unwrap_or(0);
    let text_width = description_lines
        .iter()
        .copied()
        .chain([
            "Certificate download URL is unavailable.",
            "Press Esc to close",
        ])
        .chain(download_url)
        .map(text_width)
        .max()
        .unwrap_or(0);
    let available_width = frame.area().width.saturating_sub(4).max(1);
    let width = qr_width
        .max(text_width)
        .saturating_add(4)
        .max(56)
        .min(available_width);
    let wrap_width = width.saturating_sub(4).max(1);
    let available_height = frame.area().height.saturating_sub(2).max(1);
    let content_height = available_height.saturating_sub(2) as usize;

    let mut lines = PopupLines::new();
    lines.push_blank();
    for description in description_lines {
        lines.push_centered_wrapped(description, wrap_width);
    }
    lines.push_blank();

    if qr_lines.is_empty() {
        lines.push_centered_wrapped("Certificate download URL is unavailable.", wrap_width);
    } else {
        lines.extend_centered(qr_lines);
        if let Some(download_url) = download_url {
            let url_lines = wrap_text(download_url, wrap_width);
            if lines.len() + url_lines.len() < content_height {
                lines.push_blank();
                lines.extend_centered(url_lines);
                lines.push_blank();
            }
        }
    }

    let height = lines
        .len()
        .try_into()
        .unwrap_or(u16::MAX)
        .saturating_add(2)
        .min(available_height);
    let area = centered_rect(width, height, frame.area());

    let content = Paragraph::new(lines.into_lines())
        .block(
            Block::default()
                .title("Install Certificate")
                .title_alignment(Alignment::Center)
                .title_bottom(Line::from("Press Esc to close").alignment(Alignment::Center))
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(if app.is_popup_focused(PopupFocus::Certificate) {
                    Style::default().fg(Color::Green)
                } else {
                    Style::default()
                }),
        )
        .alignment(Alignment::Center)
        .wrap(Wrap { trim: false });

    frame.render_widget(Clear, area);
    frame.render_widget(content, area);
}

struct PopupLines {
    lines: Vec<Line<'static>>,
}

impl PopupLines {
    fn new() -> Self {
        Self { lines: Vec::new() }
    }

    fn len(&self) -> usize {
        self.lines.len()
    }

    fn push_blank(&mut self) {
        self.lines.push(Line::from(""));
    }

    fn push_centered_wrapped(&mut self, text: &str, max_width: u16) {
        self.extend_centered(wrap_text(text, max_width));
    }

    fn extend_centered(&mut self, lines: impl IntoIterator<Item = String>) {
        self.lines.extend(
            lines
                .into_iter()
                .map(|line| Line::from(line).alignment(Alignment::Center)),
        );
    }

    fn into_lines(self) -> Vec<Line<'static>> {
        self.lines
    }
}

fn wrap_text(text: &str, max_width: u16) -> Vec<String> {
    let max_width = max_width as usize;
    if max_width == 0 || text.is_empty() {
        return vec![text.to_string()];
    }

    let mut lines = Vec::new();
    let mut current = String::new();

    for word in text.split_whitespace() {
        let word_width = text_width(word) as usize;
        if current.is_empty() {
            if word_width <= max_width {
                current.push_str(word);
            } else {
                lines.extend(hard_wrap_text(word, max_width));
            }
            continue;
        }

        let next_width = text_width(&current) as usize + 1 + word_width;
        if next_width <= max_width {
            current.push(' ');
            current.push_str(word);
        } else {
            lines.push(current);
            current = String::new();
            if word_width <= max_width {
                current.push_str(word);
            } else {
                lines.extend(hard_wrap_text(word, max_width));
            }
        }
    }

    if !current.is_empty() {
        lines.push(current);
    }

    if lines.is_empty() {
        vec![text.to_string()]
    } else {
        lines
    }
}

fn hard_wrap_text(text: &str, max_width: usize) -> Vec<String> {
    if max_width == 0 {
        return vec![text.to_string()];
    }

    let mut lines = Vec::new();
    let mut current = String::new();

    for ch in text.chars() {
        if current.chars().count() >= max_width {
            lines.push(current);
            current = String::new();
        }
        current.push(ch);
    }

    if !current.is_empty() {
        lines.push(current);
    }

    lines
}

fn text_width(text: &str) -> u16 {
    text.chars().count().try_into().unwrap_or(u16::MAX)
}

fn build_qr_lines(download_url: &str) -> Vec<String> {
    match QrCode::with_error_correction_level(download_url.as_bytes(), EcLevel::L) {
        Ok(code) => code
            .render::<unicode::Dense1x2>()
            .quiet_zone(false)
            .build()
            .lines()
            .map(str::to_string)
            .collect(),
        Err(_) => vec!["Unable to generate QR code".to_string()],
    }
}

fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    let x = area.x + area.width.saturating_sub(width) / 2;
    let y = area.y + area.height.saturating_sub(height) / 2;

    Rect {
        x,
        y,
        width,
        height,
    }
}

fn build_detail_text(app: &App) -> String {
    let Some(req) = app.selected_request() else {
        return "Select a request".to_string();
    };

    match app.detail_panel.active_tab {
        MainDisplayTab::RequestHeader => format_request_header(req),
        MainDisplayTab::RequestBody => {
            format_request_body(req.req_body.as_deref(), &req.req_headers)
        }
        MainDisplayTab::ResponseHeader => format!(
            "Status: {}\n\nHeaders:\n{}",
            req.status
                .map_or("N/A".to_string(), |status| status.to_string()),
            join_headers(&req.res_headers)
        ),
        MainDisplayTab::ResponseBody => format_response_body(req),
    }
}

fn format_request_header(req: &crate::proxy_handler::CapturedData) -> String {
    let mut lines = vec![
        format!("Method: {}", req.method),
        format!("URI: {}", req.uri),
    ];
    if let Some(mapped_uri) = &req.mapped_uri {
        lines.push(format!("Mapped URI: {mapped_uri}"));
    }
    if let Some(local_path) = &req.local_path {
        lines.push(format!("Map Local File: {local_path}"));
    }
    lines.push(String::new());
    lines.push("Headers:".to_string());
    lines.push(join_headers(&req.req_headers));
    lines.join("\n")
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

fn format_response_body(req: &crate::proxy_handler::CapturedData) -> String {
    match req.res_body.as_deref() {
        Some(body) if req.local_path.is_some() => body.to_string(),
        None if req.local_path.is_some() => String::new(), // empty local file
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
