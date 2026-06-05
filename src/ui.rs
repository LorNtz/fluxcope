use crate::app::{App, MainDisplayTab, PanelFocus, PopupFocus, RequestTreeEntry};
use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use qrcode::{EcLevel, QrCode, render::unicode};
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout, Margin, Position, Rect},
    style::{Color, Modifier, Style},
    symbols,
    text::{Line, Span},
    widgets::{
        Block, BorderType, Borders, Clear, Paragraph, Scrollbar, ScrollbarOrientation,
        ScrollbarState, Tabs, Wrap,
    },
};
use std::borrow::Cow;
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
    log: LogView,
}

impl RootView {
    pub fn new() -> Self {
        Self {
            area: Rect::default(),
            status: StatusView::new(),
            request_list: RequestListView::new(),
            right_panel: RightPanelView::new(),
            log: LogView::new(),
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
        if app.log_panel.visible {
            self.log.render(frame, app);
        } else {
            self.request_list.render(frame, app);
            self.right_panel.render(frame, app);
        }

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

        self.status.layout(root_chunks[0], app);
        if app.log_panel.visible {
            self.request_list.layout(Rect::default(), app);
            self.right_panel.layout(Rect::default(), app);
            self.log.layout(root_chunks[1], app);
        } else {
            let chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(30), Constraint::Percentage(70)])
                .split(root_chunks[1]);

            self.request_list.layout(chunks[0], app);
            self.right_panel.layout(chunks[1], app);
            self.log.layout(Rect::default(), app);
        }
    }
}

impl MouseHandler for RootView {
    fn handle_mouse(&self, mouse: MouseEvent, app: &mut App) -> bool {
        if app.certificate_popup.visible {
            return true;
        }

        if app.log_panel.visible {
            self.log.handle_mouse(mouse, app)
        } else {
            self.right_panel.handle_mouse(mouse, app) || self.request_list.handle_mouse(mouse, app)
        }
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

    let current = root_mut_or_insert(
        roots,
        origin_identifier.clone(),
        subtree_label(&entry.origin),
    );
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
        current = current.branch_child_mut_or_insert(identifier.clone(), subtree_label(label));
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

fn subtree_label(label: &str) -> String {
    if label.ends_with('/') {
        label.to_string()
    } else {
        format!("{label}/")
    }
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
    use ratatui::{Terminal, backend::TestBackend, buffer::Buffer};

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

    #[test]
    fn form_request_body_decodes_percent_encoded_utf8_text() {
        let headers = vec![(
            "content-type".to_string(),
            "application/x-www-form-urlencoded".to_string(),
        )];
        let body = "name=%E4%B8%AD%E6%96%87&city=%E5%8C%97%E4%BA%AC";

        assert_eq!(
            format_request_body(Some(body), &headers),
            "name: 中文\ncity: 北京"
        );
    }

    #[test]
    fn header_table_keeps_value_column_at_least_half_width_when_key_wraps() {
        let rows = vec![header_table_row(
            "x-very-long-header-name",
            "value-that-also-needs-wrapping",
        )];
        let columns = header_table_columns(&rows, 12);
        let table = header_table_render(&rows, 12, None);

        assert!(columns.value_width >= 6);
        assert!(columns.key_width < text_width(&rows[0].key));
        assert!(table.lines.len() > 1);
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

    fn render_to_buffer(app: &mut App) -> (RootView, Buffer) {
        let backend = TestBackend::new(100, 12);
        let mut terminal = Terminal::new(backend).expect("test backend should initialize");
        let mut ui = RootView::new();

        terminal
            .draw(|frame| RootView::render(&mut ui, frame, app))
            .expect("UI should render in tests");

        (ui, terminal.backend_mut().buffer().clone())
    }

    fn render_detail_top_row(app: &mut App) -> (RootView, String) {
        let (ui, buffer) = render_to_buffer(app);
        let detail_area = ui.right_panel.detail.area();
        let detail_top_row = buffer_row(&buffer, detail_area.y, detail_area.x, detail_area.width);

        (ui, detail_top_row)
    }

    fn buffer_row(buffer: &Buffer, y: u16, x: u16, width: u16) -> String {
        let mut row = String::new();
        for column in x..x.saturating_add(width) {
            row.push_str(buffer[(column, y)].symbol());
        }

        row
    }

    fn tab_click_position(area: Rect, target: MainDisplayTab) -> Position {
        let mut column = area.x.saturating_add(1);
        for (index, tab) in MainDisplayTab::all().iter().copied().enumerate() {
            if index > 0 {
                column = column.saturating_add(1);
            }
            if tab == target {
                return Position::new(column.saturating_add(1), area.y);
            }
            column = column.saturating_add(text_width(tab.title()).saturating_add(2));
        }

        panic!("target tab should exist")
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
    fn request_tree_displays_subtree_nodes_with_trailing_slashes() {
        let mut app = App::new(ui_settings(true));
        app.add_request(captured(0, "https://a.com/path/to/api"));
        let (ui, buffer) = render_to_buffer(&mut app);
        let request_area = ui.request_list.area();
        let rendered_rows = (request_area.y..request_area.bottom())
            .map(|row| buffer_row(&buffer, row, request_area.x, request_area.width))
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered_rows.contains("https://a.com/"), "{rendered_rows}");
        assert!(rendered_rows.contains("path/"), "{rendered_rows}");
        assert!(rendered_rows.contains("to/"), "{rendered_rows}");
        assert!(rendered_rows.contains("api"), "{rendered_rows}");
        assert!(!rendered_rows.contains("api/"), "{rendered_rows}");
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

        app.log_panel.visible = true;
        app.focus_panel(PanelFocus::Log);
        let ui = laid_out_ui(&app);
        ui.handle_mouse(
            mouse_inside(MouseEventKind::ScrollDown, ui.log.area()),
            &mut app,
        );
        assert!(app.is_panel_focused(PanelFocus::Log));
    }

    #[test]
    fn mouse_click_focuses_clicked_panel() {
        let mut app = App::new(ui_settings(true));
        let ui = laid_out_ui(&app);

        ui.handle_mouse(mouse_down_inside(ui.right_panel.detail.area()), &mut app);
        assert!(app.is_panel_focused(PanelFocus::Detail));

        ui.handle_mouse(mouse_down_inside(ui.request_list.area()), &mut app);
        assert!(app.is_panel_focused(PanelFocus::RequestList));

        app.log_panel.visible = true;
        let ui = laid_out_ui(&app);
        ui.handle_mouse(mouse_down_inside(ui.log.area()), &mut app);
        assert!(app.is_panel_focused(PanelFocus::Log));
    }

    #[test]
    fn status_panel_does_not_take_focus() {
        let mut app = App::new(ui_settings(true));
        let ui = laid_out_ui(&app);

        ui.handle_mouse(mouse_down_inside(ui.status.area()), &mut app);

        assert!(app.is_panel_focused(PanelFocus::RequestList));
    }

    #[test]
    fn hidden_log_panel_leaves_detail_on_right_panel() {
        let app = App::new(ui_settings(true));
        let ui = laid_out_ui(&app);

        assert_eq!(ui.right_panel.detail.area(), ui.right_panel.area());
        assert_eq!(ui.log.area(), Rect::default());
    }

    #[test]
    fn visible_log_panel_uses_workspace_below_status() {
        let mut app = App::new(ui_settings(true));
        app.log_panel.visible = true;
        let ui = laid_out_ui(&app);

        assert_eq!(ui.status.area(), Rect::new(0, 0, 100, 3));
        assert_eq!(ui.log.area(), Rect::new(0, 3, 100, 9));
        assert_eq!(ui.request_list.area(), Rect::default());
        assert_eq!(ui.right_panel.area(), Rect::default());
        assert_eq!(ui.right_panel.detail.area(), Rect::default());
    }

    #[test]
    fn detail_tabs_render_on_panel_border_without_tabs_box() {
        let mut app = App::new(ui_settings(true));
        let (ui, detail_top_row) = render_detail_top_row(&mut app);
        let detail_area = ui.right_panel.detail.area();

        assert_eq!(detail_area.y, ui.right_panel.area().y);
        assert!(
            detail_top_row.contains("Request Header"),
            "{detail_top_row:?}"
        );
        assert!(
            detail_top_row.contains("Response Body"),
            "{detail_top_row:?}"
        );
    }

    #[test]
    fn focused_detail_panel_keeps_unselected_tabs_default_color() {
        let mut app = App::new(ui_settings(true));
        app.focus_panel(PanelFocus::Detail);
        let (ui, buffer) = render_to_buffer(&mut app);
        let detail_area = ui.right_panel.detail.area();
        let selected_tab = tab_click_position(detail_area, MainDisplayTab::RequestHeader);
        let unselected_tab = tab_click_position(detail_area, MainDisplayTab::RequestBody);

        assert_eq!(buffer[selected_tab].fg, Color::Green);
        assert!(buffer[selected_tab].modifier.contains(Modifier::BOLD));
        assert_eq!(buffer[unselected_tab].fg, Color::Reset);
        assert!(!buffer[unselected_tab].modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn mouse_click_selects_detail_tab_on_panel_border() {
        let mut app = App::new(ui_settings(true));
        app.detail_panel.scroll.offset = 3;
        let ui = laid_out_ui(&app);
        let position =
            tab_click_position(ui.right_panel.detail.area(), MainDisplayTab::ResponseHeader);

        ui.handle_mouse(
            mouse(
                MouseEventKind::Down(MouseButton::Left),
                position.x,
                position.y,
            ),
            &mut app,
        );

        assert_eq!(app.detail_panel.active_tab, MainDisplayTab::ResponseHeader);
        assert_eq!(app.detail_panel.scroll.offset, 0);
        assert!(app.is_panel_focused(PanelFocus::Detail));
    }

    #[test]
    fn mouse_click_selects_single_header_table_row_without_breaking_tabs() {
        let mut app = App::new(ui_settings(true));
        let mut req = captured(0, "https://a.com/api");
        req.req_headers = vec![
            ("accept".to_string(), "application/json".to_string()),
            ("x-token".to_string(), "secret".to_string()),
        ];
        app.add_request(req);

        let ui = laid_out_ui(&app);
        let content_area = detail_content_area(ui.right_panel.detail.area());
        let header_row = Position::new(content_area.x, content_area.y.saturating_add(2));

        ui.handle_mouse(
            mouse(
                MouseEventKind::Down(MouseButton::Left),
                header_row.x,
                header_row.y,
            ),
            &mut app,
        );

        assert_eq!(app.detail_panel.selected_header_row, Some(2));

        let (ui, buffer) = render_to_buffer(&mut app);
        let content_area = detail_content_area(ui.right_panel.detail.area());
        assert_eq!(
            buffer[(content_area.x, content_area.y.saturating_add(2))].bg,
            Color::White
        );

        let position =
            tab_click_position(ui.right_panel.detail.area(), MainDisplayTab::RequestBody);
        ui.handle_mouse(
            mouse(
                MouseEventKind::Down(MouseButton::Left),
                position.x,
                position.y,
            ),
            &mut app,
        );

        assert_eq!(app.detail_panel.active_tab, MainDisplayTab::RequestBody);
        assert_eq!(app.detail_panel.selected_header_row, None);
    }
}

struct RightPanelView {
    area: Rect,
    detail: DetailView,
}

impl RightPanelView {
    fn new() -> Self {
        Self {
            area: Rect::default(),
            detail: DetailView::new(),
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
        self.detail.render(frame, app);
    }

    fn layout(&mut self, area: Rect, app: &App) {
        self.set_area(area);
        self.detail.layout(area, app);
    }
}

impl MouseHandler for RightPanelView {
    fn handle_mouse(&self, mouse: MouseEvent, app: &mut App) -> bool {
        self.detail.handle_mouse(mouse, app)
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
        let focused = app.is_panel_focused(PanelFocus::Detail);
        match build_detail_content(app) {
            DetailContent::Text(detail_text) => {
                let mut paragraph = Paragraph::new(detail_text)
                    .block(detail_panel_block(focused))
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
            }
            DetailContent::Table(rows) => {
                let table_area = detail_content_area(self.area());
                let table = header_table_render(
                    &rows,
                    table_area.width,
                    app.detail_panel.selected_header_row,
                );
                let total_lines: u16 = table.lines.len().try_into().unwrap_or(u16::MAX);
                app.detail_panel.scroll.max_offset = total_lines.saturating_sub(table_area.height);
                let offset = app
                    .detail_panel
                    .scroll
                    .offset
                    .min(app.detail_panel.scroll.max_offset);
                app.detail_panel.scroll.offset = offset;

                frame.render_widget(
                    Paragraph::new(table.lines)
                        .block(detail_panel_block(focused))
                        .scroll((offset, 0)),
                    self.area(),
                );
            }
        }
        frame.render_widget(
            detail_tabs(app.detail_panel.active_tab),
            detail_tabs_area(self.area()),
        );
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
            MouseEventKind::Down(button) => {
                app.focus_panel(PanelFocus::Detail);
                if let Some(tab) =
                    detail_tab_at_position(self.area(), Position::new(mouse.column, mouse.row))
                {
                    app.detail_panel.select_tab(tab);
                } else if button == MouseButton::Left {
                    app.detail_panel.selected_header_row = header_table_row_at_position(
                        app,
                        self.area(),
                        Position::new(mouse.column, mouse.row),
                    );
                }
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
    base_panel_block(focused).title(title)
}

fn detail_panel_block(focused: bool) -> Block<'static> {
    base_panel_block(focused)
}

fn base_panel_block(focused: bool) -> Block<'static> {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded);
    apply_focus_border(block, focused)
}

fn detail_tabs(active_tab: MainDisplayTab) -> Tabs<'static> {
    let titles = MainDisplayTab::all()
        .iter()
        .map(|tab| tab.title())
        .collect::<Vec<_>>();

    Tabs::new(titles)
        .style(Style::default().fg(Color::Reset))
        .divider(symbols::line::VERTICAL)
        .highlight_style(
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        )
        .select(active_tab.index())
}

fn detail_tabs_area(area: Rect) -> Rect {
    Rect {
        x: area.x.saturating_add(1),
        y: area.y,
        width: area.width.saturating_sub(2),
        height: u16::from(area.height > 0 && area.width > 2),
    }
}

fn detail_tab_at_position(area: Rect, position: Position) -> Option<MainDisplayTab> {
    let tabs_area = detail_tabs_area(area);
    if tabs_area.is_empty() || position.y != tabs_area.y {
        return None;
    }

    if position.x < tabs_area.x || position.x >= tabs_area.right() {
        return None;
    }

    let mut cursor = tabs_area.x;
    for (index, tab) in MainDisplayTab::all().iter().copied().enumerate() {
        if index > 0 {
            cursor = cursor.saturating_add(1);
        }
        if cursor >= tabs_area.right() {
            return None;
        }

        let width = text_width(tab.title()).saturating_add(2);
        let tab_end = cursor.saturating_add(width).min(tabs_area.right());
        if position.x >= cursor && position.x < tab_end {
            return Some(tab);
        }
        cursor = cursor.saturating_add(width);
    }

    None
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

enum DetailContent {
    Text(String),
    Table(Vec<TableRow>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct TableRow {
    key: String,
    value: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct HeaderTableColumns {
    key_width: u16,
    value_width: u16,
    gap: u16,
    total_width: u16,
}

struct HeaderTableRender {
    lines: Vec<Line<'static>>,
}

trait HeaderTableRowText {
    fn key(&self) -> &str;
    fn value(&self) -> &str;
}

impl HeaderTableRowText for TableRow {
    fn key(&self) -> &str {
        &self.key
    }

    fn value(&self) -> &str {
        &self.value
    }
}

struct HeaderTableRowRef<'a> {
    key: &'a str,
    value: Cow<'a, str>,
}

impl HeaderTableRowText for HeaderTableRowRef<'_> {
    fn key(&self) -> &str {
        self.key
    }

    fn value(&self) -> &str {
        self.value.as_ref()
    }
}

fn build_detail_content(app: &App) -> DetailContent {
    let Some(req) = app.selected_request() else {
        return DetailContent::Text("Select a request".to_string());
    };

    match app.detail_panel.active_tab {
        MainDisplayTab::RequestHeader => DetailContent::Table(request_header_rows(req)),
        MainDisplayTab::RequestBody => DetailContent::Text(format_request_body(
            req.req_body.as_deref(),
            &req.req_headers,
        )),
        MainDisplayTab::ResponseHeader => DetailContent::Table(response_header_rows(req)),
        MainDisplayTab::ResponseBody => DetailContent::Text(format_response_body(req)),
    }
}

fn request_header_rows(req: &crate::proxy_handler::CapturedData) -> Vec<TableRow> {
    let mut lines = vec![
        header_table_row("Method", req.method.to_string()),
        header_table_row("URI", req.uri.clone()),
    ];
    if let Some(mapped_uri) = &req.mapped_uri {
        lines.push(header_table_row("Mapped URI", mapped_uri.clone()));
    }
    if let Some(local_path) = &req.local_path {
        lines.push(header_table_row("Map Local File", local_path.clone()));
    }

    lines.extend(
        req.req_headers
            .iter()
            .map(|(key, value)| header_table_row(key.clone(), value.clone())),
    );

    lines
}

fn response_header_rows(req: &crate::proxy_handler::CapturedData) -> Vec<TableRow> {
    let mut lines = vec![header_table_row(
        "Status",
        req.status
            .map_or("N/A".to_string(), |status| status.to_string()),
    )];
    lines.extend(
        req.res_headers
            .iter()
            .map(|(key, value)| header_table_row(key.clone(), value.clone())),
    );

    lines
}

fn header_table_row(key: impl Into<String>, value: impl Into<String>) -> TableRow {
    TableRow {
        key: key.into(),
        value: value.into(),
    }
}

fn header_table_render(
    rows: &[TableRow],
    width: u16,
    selected_row: Option<usize>,
) -> HeaderTableRender {
    let columns = header_table_columns(rows, width);
    let mut lines = Vec::new();

    for (row_index, row) in rows.iter().enumerate() {
        lines.extend(header_table_row_lines(
            row,
            columns,
            selected_row == Some(row_index),
        ));
    }

    if lines.is_empty() {
        lines.push(Line::from("(No headers)"));
    }

    HeaderTableRender { lines }
}

fn header_table_columns<T: HeaderTableRowText>(rows: &[T], width: u16) -> HeaderTableColumns {
    let total_width = width.max(1);
    let gap = if total_width >= 4 {
        2
    } else if total_width >= 3 {
        1
    } else {
        0
    };
    let available_width = total_width.saturating_sub(gap);
    let minimum_value_width = if available_width >= 2 {
        total_width / 2
    } else {
        0
    };
    let maximum_key_width = available_width
        .saturating_sub(minimum_value_width)
        .max(u16::from(available_width > 0));
    let natural_key_width = rows
        .iter()
        .map(|row| text_width(row.key()))
        .max()
        .unwrap_or(1)
        .max(1);
    let key_width = natural_key_width.min(maximum_key_width);
    let value_width = available_width.saturating_sub(key_width);

    HeaderTableColumns {
        key_width,
        value_width,
        gap,
        total_width,
    }
}

fn header_table_row_lines(
    row: &impl HeaderTableRowText,
    columns: HeaderTableColumns,
    selected: bool,
) -> Vec<Line<'static>> {
    let key_lines = wrap_cell_text(row.key(), columns.key_width);
    let value_lines = wrap_cell_text(row.value(), columns.value_width);
    let row_height = key_lines.len().max(value_lines.len()).max(1);
    let key_top_padding = (row_height - key_lines.len()) / 2;
    let value_top_padding = (row_height - value_lines.len()) / 2;

    (0..row_height)
        .map(|line_index| {
            let key = centered_line_segment(&key_lines, line_index, key_top_padding);
            let value = centered_line_segment(&value_lines, line_index, value_top_padding);
            let line = format_header_table_line(key, value, columns);
            let span = if selected {
                Span::styled(line, Style::default().bg(Color::White).fg(Color::DarkGray))
            } else {
                Span::raw(line)
            };
            Line::from(span)
        })
        .collect()
}

fn header_table_row_height(row: &impl HeaderTableRowText, columns: HeaderTableColumns) -> usize {
    wrapped_line_count(row.key(), columns.key_width)
        .max(wrapped_line_count(row.value(), columns.value_width))
        .max(1)
}

fn wrapped_line_count(text: &str, max_width: u16) -> usize {
    let max_width = max_width as usize;
    if max_width == 0 {
        return 1;
    }

    text.split('\n')
        .map(|source_line| {
            let width = source_line.chars().count();
            width.max(1).div_ceil(max_width)
        })
        .sum::<usize>()
        .max(1)
}

fn centered_line_segment(lines: &[String], line_index: usize, top_padding: usize) -> &str {
    line_index
        .checked_sub(top_padding)
        .and_then(|index| lines.get(index))
        .map(String::as_str)
        .unwrap_or("")
}

fn format_header_table_line(key: &str, value: &str, columns: HeaderTableColumns) -> String {
    let mut line = pad_to_width(key, columns.key_width);
    if columns.value_width > 0 {
        line.push_str(&" ".repeat(columns.gap as usize));
        line.push_str(value);
    }

    pad_to_width(&line, columns.total_width)
}

fn wrap_cell_text(text: &str, max_width: u16) -> Vec<String> {
    let max_width = max_width as usize;
    if max_width == 0 {
        return vec![String::new()];
    }

    let mut lines = Vec::new();
    for source_line in text.split('\n') {
        let mut current = String::new();
        for ch in source_line.chars() {
            if current.chars().count() >= max_width {
                lines.push(current);
                current = String::new();
            }
            current.push(ch);
        }
        lines.push(current);
    }

    if lines.is_empty() {
        vec![String::new()]
    } else {
        lines
    }
}

fn pad_to_width(text: &str, width: u16) -> String {
    let mut padded = text.to_string();
    let padding = width.saturating_sub(text_width(text)) as usize;
    padded.push_str(&" ".repeat(padding));
    padded
}

fn detail_content_area(area: Rect) -> Rect {
    area.inner(Margin {
        vertical: 1,
        horizontal: 1,
    })
}

fn header_table_row_at_position(app: &App, area: Rect, position: Position) -> Option<usize> {
    let content_area = detail_content_area(area);
    if !content_area.contains(position) {
        return None;
    }

    let req = app.selected_request()?;
    let rows = match app.detail_panel.active_tab {
        MainDisplayTab::RequestHeader => request_header_row_refs(req),
        MainDisplayTab::ResponseHeader => response_header_row_refs(req),
        MainDisplayTab::RequestBody | MainDisplayTab::ResponseBody => return None,
    };
    let line_index = app.detail_panel.scroll.offset as usize
        + position.y.saturating_sub(content_area.y) as usize;
    let columns = header_table_columns(&rows, content_area.width);
    let mut row_start = 0;

    rows.iter().enumerate().find_map(|(index, row)| {
        let row_end = row_start + header_table_row_height(row, columns);
        let matches = (row_start..row_end).contains(&line_index);
        row_start = row_end;
        matches.then_some(index)
    })
}

fn request_header_row_refs(req: &crate::proxy_handler::CapturedData) -> Vec<HeaderTableRowRef<'_>> {
    let extra_rows = usize::from(req.mapped_uri.is_some()) + usize::from(req.local_path.is_some());
    let mut rows = Vec::with_capacity(2 + extra_rows + req.req_headers.len());
    rows.push(header_table_row_ref("Method", req.method.to_string()));
    rows.push(header_table_row_ref("URI", req.uri.as_str()));
    if let Some(mapped_uri) = &req.mapped_uri {
        rows.push(header_table_row_ref("Mapped URI", mapped_uri.as_str()));
    }
    if let Some(local_path) = &req.local_path {
        rows.push(header_table_row_ref("Map Local File", local_path.as_str()));
    }
    rows.extend(
        req.req_headers
            .iter()
            .map(|(key, value)| header_table_row_ref(key.as_str(), value.as_str())),
    );

    rows
}

fn response_header_row_refs(
    req: &crate::proxy_handler::CapturedData,
) -> Vec<HeaderTableRowRef<'_>> {
    let mut rows = Vec::with_capacity(1 + req.res_headers.len());
    rows.push(header_table_row_ref(
        "Status",
        req.status.map_or_else(
            || Cow::Borrowed("N/A"),
            |status| Cow::Owned(status.to_string()),
        ),
    ));
    rows.extend(
        req.res_headers
            .iter()
            .map(|(key, value)| header_table_row_ref(key.as_str(), value.as_str())),
    );

    rows
}

fn header_table_row_ref<'a>(key: &'a str, value: impl Into<Cow<'a, str>>) -> HeaderTableRowRef<'a> {
    HeaderTableRowRef {
        key,
        value: value.into(),
    }
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
    let input_bytes = input.as_bytes();
    let mut decoded = Vec::with_capacity(input_bytes.len());
    let mut cursor = 0;

    while cursor < input_bytes.len() {
        match input_bytes[cursor] {
            b'%' if cursor + 2 < input_bytes.len() => {
                let h1 = input_bytes[cursor + 1];
                let h2 = input_bytes[cursor + 2];
                if let (Some(hi), Some(lo)) = (hex_to_nibble(h1), hex_to_nibble(h2)) {
                    decoded.push(hi * 16 + lo);
                    cursor += 3;
                    continue;
                }
                decoded.push(input_bytes[cursor]);
            }
            b'+' => decoded.push(b' '),
            byte => decoded.push(byte),
        }
        cursor += 1;
    }

    String::from_utf8(decoded)
        .unwrap_or_else(|error| String::from_utf8_lossy(error.as_bytes()).into_owned())
}

fn hex_to_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}
