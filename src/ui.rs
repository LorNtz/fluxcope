#[cfg(test)]
use crate::app::ProxyRow;
use crate::app::{
    ActionDialog, App, BODY_TEXT_TAB_WIDTH, BodyViewerKey, FieldEditKind, MainDisplayTab,
    PROXY_PRESET_SELECT_MAX_VISIBLE_ITEMS, PanelFocus, PopupFocus, ProxyRuleTable, ProxyWidget,
    RULE_EDITOR_KEY_HINTS, RequestTreeEntry, RuleEditField, RuleEditorState, SelectTarget,
    SettingsKeyHint, SettingsPaneFocus, SettingsPopup, SettingsScrollRequest, SettingsSelectId,
    SettingsTopic,
};
use crate::select::{SelectItem, SelectState};
use crate::select_widget::SelectWidget;
use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use edtui::{EditorStatusLine, EditorTheme, EditorView};
use qrcode::{EcLevel, QrCode, render::unicode};
use ratatui::{
    Frame,
    buffer::Buffer,
    layout::{Alignment, Constraint, Direction, Layout, Margin, Position, Rect, Size},
    style::{Color, Modifier, Style},
    symbols,
    text::{Line, Span, Text},
    widgets::{
        Block, BorderType, Borders, Clear, Paragraph, Scrollbar, ScrollbarOrientation,
        ScrollbarState, StatefulWidget, Tabs, Widget, Wrap,
    },
};
use std::{borrow::Cow, collections::HashSet};
use tui_scrollview::{ScrollView, ScrollbarVisibility};
use tui_textarea::{CursorMove, TextArea};
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

        if app.settings_popup.visible {
            render_settings_popup(frame, app);
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
        if app.settings_popup.visible {
            return handle_settings_popup_mouse(mouse, app, self.area());
        }

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
            ("●", "Recording ON", Style::default().fg(Color::LightRed))
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
    leaf_count: usize,
    children: Vec<RequestTreeNode>,
}

impl RequestTreeNode {
    fn new(identifier: String, label: String) -> Self {
        Self {
            identifier,
            label,
            leaf_count: 0,
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
        let Self {
            identifier,
            label,
            leaf_count,
            children,
        } = self;

        if children.is_empty() {
            TreeItem::new_leaf(identifier, label)
        } else {
            TreeItem::new(
                identifier,
                subtree_label_with_count(label, leaf_count),
                children
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
    current.leaf_count += 1;
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
        current.leaf_count += 1;
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

fn subtree_label_with_count(label: String, leaf_count: usize) -> Line<'static> {
    Line::from(vec![
        Span::raw(label),
        Span::styled(
            format!(" {leaf_count}"),
            Style::default().fg(Color::DarkGray),
        ),
    ])
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
    use crate::app::{format_request_body, format_response_body};
    use crate::capture::{CaptureSequence, CapturedExchange};
    use crate::settings::{ProxyPresetSettings, ProxySettings, RequestListSettings, UiSettings};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton};
    use http::Method;
    use ratatui::{Terminal, backend::TestBackend, buffer::Buffer};

    fn ui_settings(auto_expand: bool) -> UiSettings {
        UiSettings {
            request_list: RequestListSettings { auto_expand },
        }
    }

    fn proxy_settings(active: &str, presets: &[&str]) -> ProxySettings {
        ProxySettings {
            enable: true,
            active_preset: Some(active.to_string()),
            presets: presets
                .iter()
                .map(|preset| ProxyPresetSettings {
                    name: (*preset).to_string(),
                    ..ProxyPresetSettings::default()
                })
                .collect(),
        }
    }

    fn proxy_settings_with_rule_counts(remote_count: usize, local_count: usize) -> ProxySettings {
        ProxySettings {
            enable: true,
            active_preset: Some("dev".to_string()),
            presets: vec![ProxyPresetSettings {
                name: "dev".to_string(),
                map_remote: crate::settings::ProxyMapRemoteSettings {
                    enable: true,
                    rules: (0..remote_count)
                        .map(|index| crate::settings::ProxyMapRemoteRule {
                            from: format!("https://api.example.com/v{index}"),
                            to: format!("http://localhost:30{index:02}"),
                            enable: true,
                        })
                        .collect(),
                },
                map_local: crate::settings::ProxyMapLocalSettings {
                    enable: true,
                    rules: (0..local_count)
                        .map(|index| crate::settings::ProxyMapLocalRule {
                            from: format!("https://static.example.com/app{index}.js"),
                            to: format!("~/fixtures/app{index}.js"),
                            enable: true,
                        })
                        .collect(),
                },
            }],
        }
    }

    fn captured(sequence: u64, uri: &str) -> CapturedExchange {
        CapturedExchange {
            sequence: CaptureSequence::new(sequence),
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
    fn mapped_response_body_expands_tabs_only_for_paragraph_rendering() {
        let raw_body = "{\n\t\"route\": true\n}";
        let mut req = captured(0, "https://a.com/api");
        req.local_path = Some("/tmp/api.json".to_string());
        req.res_body = Some(raw_body.to_string());
        let mut app = App::new(ui_settings(true));
        app.add_request(req);
        app.focus_panel(PanelFocus::Detail);
        app.detail_panel.select_tab(MainDisplayTab::ResponseBody);

        let (ui, buffer) = render_to_buffer(&mut app);

        assert!(
            buffer
                .content()
                .iter()
                .all(|cell| !cell.symbol().contains('\t'))
        );
        let key = BodyViewerKey::new(CaptureSequence::new(0), MainDisplayTab::ResponseBody);
        assert_eq!(app.cached_body_text(key), Some(raw_body));
        assert_eq!(
            app.cached_body_render_text(key),
            Some("{\n    \"route\": true\n}")
        );
        assert!(
            find_buffer_text(&buffer, ui.right_panel.detail.area(), "\"route\": true").is_some()
        );

        assert!(app.enter_current_body_viewer());
        assert_eq!(
            app.detail_panel.body_viewer.editor_mut().unwrap().lines,
            edtui::Lines::from(raw_body)
        );
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
    fn request_body_formats_json_like_response_body() {
        let raw_body = r#"{"z":1,"a":2}"#;
        let headers = Vec::new();
        let mut req = captured(0, "https://a.com/api");
        req.res_body = Some(raw_body.to_string());

        assert_eq!(
            format_request_body(Some(raw_body), &headers),
            format_response_body(&req)
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

    #[test]
    fn settings_popup_renders_topics_and_selected_content() {
        let mut app = App::new(ui_settings(true));
        app.open_settings_popup();

        let (_ui, buffer) = render_to_buffer(&mut app);
        let rendered = (0..buffer.area.height)
            .map(|row| buffer_row(&buffer, row, 0, buffer.area.width))
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains("Settings"));
        assert!(rendered.contains("Server"));
        assert!(rendered.contains("Proxy port"));
    }

    #[test]
    fn settings_popup_renders_content_column_border() {
        let mut app = App::new(ui_settings(true));
        app.open_settings_popup();

        let (_ui, buffer) = render_to_buffer(&mut app);
        let popup_area = settings_popup_area(buffer.area);
        let inner = popup_area.inner(Margin {
            vertical: 1,
            horizontal: 1,
        });
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(18), Constraint::Min(20)])
            .split(inner);
        let content_area = chunks[1];

        assert_eq!(
            buffer[(content_area.x, content_area.y)].symbol(),
            symbols::border::ROUNDED.top_left
        );
        assert_eq!(
            buffer[(content_area.right() - 1, content_area.y)].symbol(),
            symbols::border::ROUNDED.top_right
        );
    }

    #[test]
    fn settings_popup_footer_uses_topic_browse_key_hints() {
        let mut app = App::new(ui_settings(true));
        app.open_settings_popup();

        let (_ui, buffer) = render_to_buffer(&mut app);
        let footer = settings_popup_footer_row(&buffer);

        assert!(footer.contains("Save [s]"), "{footer}");
        assert!(footer.contains("Close [Esc]"), "{footer}");
        assert!(footer.contains("Pane [h/l]"), "{footer}");
        assert!(footer.contains("Move [j/k]"), "{footer}");
        assert!(footer.contains("Open [Enter]"), "{footer}");
    }

    #[test]
    fn settings_popup_footer_switches_to_text_edit_key_hints() {
        let mut app = App::new(ui_settings(true));
        app.open_settings_popup();
        focus_settings_content(&mut app);
        app.handle_key_event(key(KeyCode::Enter));

        let (_ui, buffer) = render_to_buffer(&mut app);
        let footer = settings_popup_footer_row(&buffer);

        assert!(footer.contains("Apply [Enter]"), "{footer}");
        assert!(footer.contains("Cancel [Esc]"), "{footer}");
        assert!(footer.contains("Cursor [Left/Right]"), "{footer}");
        assert!(footer.contains("Type [text]"), "{footer}");
        assert!(!footer.contains("Save [s]"), "{footer}");
    }

    #[test]
    fn settings_popup_footer_switches_to_select_key_hints() {
        let mut app = App::new(ui_settings(true));
        app.open_settings_popup();
        app.settings_popup
            .select_topic_for_tests(SettingsTopic::Proxy);
        app.settings_popup.draft_mut_for_tests().proxy =
            Some(proxy_settings("dev", &["dev", "qa", "prod"]));
        focus_settings_content(&mut app);
        app.handle_key_event(key(KeyCode::Enter));

        let (_ui, buffer) = render_to_buffer(&mut app);
        let footer = settings_popup_footer_row(&buffer);

        assert!(footer.contains("Choose [Enter]"), "{footer}");
        assert!(footer.contains("Cancel [Esc]"), "{footer}");
        assert!(footer.contains("Move [Up/Down]"), "{footer}");
        assert!(footer.contains("Filter [type]"), "{footer}");
        assert!(!footer.contains("Save [s]"), "{footer}");
    }

    #[test]
    fn settings_popup_footer_switches_to_rule_table_key_hints() {
        let mut app = App::new(ui_settings(true));
        app.open_settings_popup();
        app.settings_popup
            .select_topic_for_tests(SettingsTopic::Proxy);
        app.settings_popup.draft_mut_for_tests().proxy =
            Some(proxy_settings_with_rule_counts(2, 0));
        app.settings_popup
            .select_proxy_row_for_tests(ProxyRow::RemoteHeader);
        focus_settings_content(&mut app);
        app.handle_key_event(key(KeyCode::Enter));

        let (_ui, buffer) = render_to_buffer(&mut app);
        let footer = settings_popup_footer_row(&buffer);

        assert!(footer.contains("Back [Esc]"), "{footer}");
        assert!(footer.contains("Edit [Enter/e]"), "{footer}");
        assert!(footer.contains("Toggle [Space]"), "{footer}");
        assert!(footer.contains("Add/del [a/d]"), "{footer}");
        assert!(footer.contains("Move [j/k/Pg/J/K]"), "{footer}");
        assert!(!footer.contains("Save [s]"), "{footer}");
    }

    #[test]
    fn settings_popup_footer_uses_empty_rule_table_key_hints() {
        let mut app = App::new(ui_settings(true));
        app.open_settings_popup();
        app.settings_popup
            .select_topic_for_tests(SettingsTopic::Proxy);
        app.settings_popup.draft_mut_for_tests().proxy =
            Some(proxy_settings_with_rule_counts(0, 0));
        app.settings_popup
            .select_proxy_row_for_tests(ProxyRow::RemoteHeader);
        focus_settings_content(&mut app);
        app.handle_key_event(key(KeyCode::Enter));

        let (_ui, buffer) = render_to_buffer(&mut app);
        let footer = settings_popup_footer_row(&buffer);

        assert!(footer.contains("Back [Esc]"), "{footer}");
        assert!(footer.contains("Add [a]"), "{footer}");
        assert!(!footer.contains("Edit [Enter/e]"), "{footer}");
        assert!(!footer.contains("Toggle [Space]"), "{footer}");
        assert!(!footer.contains("Add/del [a/d]"), "{footer}");
        assert!(!footer.contains("Move [j/k/Pg/J/K]"), "{footer}");
    }

    #[test]
    fn settings_popup_input_field_renders_boxed_and_selected_green() {
        let mut app = App::new(ui_settings(true));
        app.open_settings_popup();
        focus_settings_content(&mut app);

        let (_ui, buffer) = render_to_buffer(&mut app);
        let content_area = settings_content_test_area(buffer.area);
        let label = find_buffer_text(&buffer, content_area, "Proxy port")
            .expect("input label should render");
        let top_left =
            find_buffer_text(&buffer, content_area, "╭").expect("input box should render");
        let bottom_left = find_buffer_text(&buffer, content_area, "╰")
            .expect("input bottom border should render");

        assert_eq!(buffer[label].fg, Color::Green);
        assert_eq!(buffer[top_left].fg, Color::Green);
        assert_eq!(buffer[bottom_left].fg, Color::Green);
    }

    #[test]
    fn settings_popup_input_field_uses_orange_while_editing() {
        let mut app = App::new(ui_settings(true));
        app.open_settings_popup();
        focus_settings_content(&mut app);
        app.handle_key_event(key(KeyCode::Enter));

        let (_ui, buffer) = render_to_buffer(&mut app);
        let content_area = settings_content_test_area(buffer.area);
        let label = find_buffer_text(&buffer, content_area, "Proxy port")
            .expect("input label should render");
        let top_left =
            find_buffer_text(&buffer, content_area, "╭").expect("input box should render");

        assert_eq!(buffer[label].fg, Color::Indexed(208));
        assert_eq!(buffer[top_left].fg, Color::Indexed(208));
    }

    #[test]
    fn settings_popup_input_field_centers_label_next_to_textarea() {
        let mut app = App::new(ui_settings(true));
        app.open_settings_popup();

        let (_ui, buffer) = render_to_buffer(&mut app);
        let content_area = settings_content_test_area(buffer.area);
        let label = find_buffer_text(&buffer, content_area, "Proxy port")
            .expect("input label should render");
        let top_left =
            find_buffer_text(&buffer, content_area, "╭").expect("input box should render");

        assert_eq!(label.y, top_left.y + 1);
    }

    #[test]
    fn settings_popup_port_input_uses_fixed_width_in_idle_and_edit_modes() {
        let mut app = App::new(ui_settings(true));
        app.open_settings_popup();

        let (_ui, buffer) = render_to_buffer(&mut app);
        let content_area = settings_content_test_area(buffer.area);
        let idle_width = field_box_width(&buffer, content_area, "Proxy port")
            .expect("port field box should render");

        assert_eq!(
            idle_width,
            PORT_INPUT_WIDTH_COLS.saturating_add(TEXT_INPUT_CHROME_WIDTH)
        );

        focus_settings_content(&mut app);
        app.handle_key_event(key(KeyCode::Enter));
        for ch in "12345678901234567890".chars() {
            app.handle_key_event(key(KeyCode::Char(ch)));
        }

        let (_ui, buffer) = render_to_buffer(&mut app);
        let content_area = settings_content_test_area(buffer.area);
        let edited_width = field_box_width(&buffer, content_area, "Proxy port")
            .expect("edited port field box should render");
        let top_left = field_box_top_left(&buffer, content_area, "Proxy port")
            .expect("edited port field box should have a left border");
        let top_right = field_box_top_right(&buffer, content_area, "Proxy port")
            .expect("edited port field box should have a right border");
        let cursor_inside_box = (top_left.x.saturating_add(1)..top_right.x).any(|x| {
            buffer[(x, top_left.y.saturating_add(1))]
                .modifier
                .contains(Modifier::REVERSED)
        });

        assert_eq!(edited_width, idle_width);
        assert!(cursor_inside_box);
    }

    #[test]
    fn settings_popup_pem_filename_input_uses_fixed_width() {
        let mut app = App::new(ui_settings(true));
        app.open_settings_popup();
        app.settings_popup
            .select_topic_for_tests(SettingsTopic::Certificate);

        let (_ui, buffer) = render_to_buffer(&mut app);
        let content_area = settings_content_test_area(buffer.area);
        let width = field_box_width(&buffer, content_area, "CA PEM filename")
            .expect("PEM filename field box should render");

        assert_eq!(
            width,
            PEM_FILENAME_INPUT_WIDTH_COLS.saturating_add(TEXT_INPUT_CHROME_WIDTH)
        );
    }

    #[test]
    fn settings_text_input_fixed_width_clamps_to_available_control_width() {
        let input = SettingsTextInputControl {
            value: Cow::Borrowed("8989"),
            cursor: None,
            fixed_edit_width_cols: Some(PORT_INPUT_WIDTH_COLS),
            hint: None,
        };

        assert_eq!(input.render_width(8), 8);
    }

    #[test]
    fn settings_popup_input_field_renders_textarea_cursor_at_edit_position() {
        let mut app = App::new(ui_settings(true));
        app.open_settings_popup();
        focus_settings_content(&mut app);
        app.handle_key_event(key(KeyCode::Enter));
        app.handle_key_event(key(KeyCode::Left));

        let (_ui, buffer) = render_to_buffer(&mut app);
        let content_area = settings_content_test_area(buffer.area);
        let value =
            find_buffer_text(&buffer, content_area, "8989").expect("field value should render");

        assert!(
            buffer[(value.x + 3, value.y)]
                .modifier
                .contains(Modifier::REVERSED)
        );
        assert!(
            !buffer[(value.x + 4, value.y)]
                .modifier
                .contains(Modifier::REVERSED)
        );
    }

    #[test]
    fn settings_popup_proxy_rule_edit_opens_secondary_textarea_popup() {
        let mut app = App::new(ui_settings(true));
        app.open_settings_popup();
        app.settings_popup
            .select_topic_for_tests(SettingsTopic::Proxy);
        app.settings_popup.draft_mut_for_tests().proxy = Some(crate::settings::ProxySettings {
            enable: true,
            active_preset: Some("dev".to_string()),
            presets: vec![crate::settings::ProxyPresetSettings {
                name: "dev".to_string(),
                map_remote: crate::settings::ProxyMapRemoteSettings {
                    enable: true,
                    rules: vec![crate::settings::ProxyMapRemoteRule {
                        from: "https://api.example.com".to_string(),
                        to: "http://localhost:3000".to_string(),
                        enable: true,
                    }],
                },
                map_local: crate::settings::ProxyMapLocalSettings::default(),
            }],
        });
        app.settings_popup
            .select_proxy_row_for_tests(ProxyRow::RemoteRule(0));

        app.handle_key_event(key(KeyCode::Enter));

        let (_ui, buffer) = render_to_buffer(&mut app);
        let popup_area = settings_popup_area(buffer.area);
        let label =
            find_buffer_text(&buffer, popup_area, "From").expect("rule edit label should render");
        let top_left = find_buffer_text(
            &buffer,
            Rect::new(
                label.x,
                label.y.saturating_sub(1),
                popup_area.right().saturating_sub(label.x),
                1,
            ),
            "╭",
        )
        .expect("rule edit box should render");
        let title = find_buffer_text(&buffer, popup_area, "Edit Map Remote Rule")
            .expect("rule editor title should render");

        assert!(title.y < label.y);
        assert_eq!(buffer[label].fg, Color::Indexed(208));
        assert_eq!(buffer[top_left].fg, Color::Indexed(208));
    }

    #[test]
    fn settings_popup_long_input_field_keeps_box_border_visible() {
        let mut app = App::new(ui_settings(true));
        app.open_settings_popup();
        app.settings_popup
            .select_topic_for_tests(SettingsTopic::Certificate);
        app.settings_popup
            .draft_mut_for_tests()
            .certificate
            .store_dir =
            "/very/long/path/that/would/otherwise/stretch/the/settings/popup/content/column"
                .to_string();

        let (_ui, buffer) = render_to_buffer(&mut app);
        let content_area = settings_content_test_area(buffer.area);
        let label = find_buffer_text(&buffer, content_area, "CA store dir")
            .expect("input label should render");
        let top_right = find_buffer_text(
            &buffer,
            Rect::new(
                label.x,
                label.y.saturating_sub(1),
                content_area.right().saturating_sub(label.x),
                1,
            ),
            "╮",
        )
        .expect("top input border should fit above the centered label row");

        assert_eq!(top_right.x, content_area.right().saturating_sub(1));
    }

    #[test]
    fn settings_popup_content_fields_share_control_column_within_topic() {
        let mut app = App::new(ui_settings(true));
        app.open_settings_popup();
        app.settings_popup
            .select_topic_for_tests(SettingsTopic::Certificate);

        let (_ui, buffer) = render_to_buffer(&mut app);
        let content_area = settings_content_test_area(buffer.area);
        let store_box = field_box_top_left(&buffer, content_area, "CA store dir")
            .expect("store dir field box should render");
        let pem_box = field_box_top_left(&buffer, content_area, "CA PEM filename")
            .expect("pem filename field box should render");

        assert_eq!(store_box.x, pem_box.x);
    }

    #[test]
    fn settings_popup_content_fields_do_not_render_visible_selection_marker() {
        let mut app = App::new(ui_settings(true));
        app.open_settings_popup();

        let (_ui, buffer) = render_to_buffer(&mut app);
        let content_area = settings_content_test_area(buffer.area);
        let label = find_buffer_text(&buffer, content_area, "Proxy port")
            .expect("selected content field should render");
        let row = buffer_row(&buffer, label.y, content_area.x, content_area.width);

        assert!(!row.contains("> Proxy port"), "{row}");
    }

    #[test]
    fn settings_popup_checkbox_field_renders_label_then_checkbox_with_green_selection() {
        let mut app = App::new(ui_settings(true));
        app.open_settings_popup();
        app.settings_popup
            .select_topic_for_tests(SettingsTopic::Recording);
        focus_settings_content(&mut app);

        let (_ui, buffer) = render_to_buffer(&mut app);
        let content_area = settings_content_test_area(buffer.area);
        let label = find_buffer_text(&buffer, content_area, "Start recording on launch")
            .expect("checkbox label should render");
        let checkbox =
            find_buffer_text(&buffer, content_area, "[✓]").expect("checkbox should render");
        let row = buffer_row(&buffer, label.y, content_area.x, content_area.width);

        assert!(row.contains("Start recording on launch  [✓]"));
        assert!(checkbox.x > label.x);
        assert_eq!(buffer[label].fg, Color::Green);
        assert_eq!(buffer[checkbox].fg, Color::Green);
        assert_eq!(buffer[label].bg, Color::Reset);
        assert_eq!(buffer[checkbox].bg, Color::Reset);
    }

    #[test]
    fn settings_popup_short_content_ignores_mouse_scroll() {
        let mut app = App::new(ui_settings(true));
        app.open_settings_popup();
        let ui = laid_out_ui(&app);
        let content_area = settings_content_test_area(Rect::new(0, 0, 100, 12));

        ui.handle_mouse(
            mouse(MouseEventKind::ScrollDown, content_area.x, content_area.y),
            &mut app,
        );

        assert_eq!(app.settings_popup.scroll.offset().y, 0);
    }

    #[test]
    fn settings_popup_overflowing_content_renders_vertical_scrollbar() {
        let mut app = App::new(ui_settings(true));
        app.open_settings_popup();
        app.settings_popup
            .select_topic_for_tests(SettingsTopic::Proxy);
        app.settings_popup.draft_mut_for_tests().proxy =
            Some(proxy_settings_with_rule_counts(3, 3));

        let (_ui, buffer) = render_to_buffer_with_size(&mut app, 100, 12);
        let content_area = settings_content_test_area(buffer.area);
        let scrollbar_column = (content_area.y..content_area.bottom())
            .map(|row| buffer[(content_area.right() - 1, row)].symbol())
            .collect::<String>();

        assert!(
            scrollbar_column.contains('▲')
                || scrollbar_column.contains('▼')
                || scrollbar_column.contains('█')
                || scrollbar_column.contains('║'),
            "{scrollbar_column:?}"
        );
    }

    #[test]
    fn settings_popup_keyboard_navigation_scrolls_multi_row_widget_into_view() {
        let mut app = App::new(ui_settings(true));
        app.open_settings_popup();
        app.settings_popup
            .select_topic_for_tests(SettingsTopic::Proxy);
        app.settings_popup.draft_mut_for_tests().proxy =
            Some(proxy_settings_with_rule_counts(3, 1));
        app.handle_key_event(key(KeyCode::Enter));

        let (_ui, _buffer) = render_to_buffer_with_size(&mut app, 100, 12);
        for _ in 0..4 {
            app.handle_key_event(key(KeyCode::Char('j')));
            let (_ui, _buffer) = render_to_buffer_with_size(&mut app, 100, 12);
        }

        assert!(app.settings_popup.scroll.offset().y > 0);
    }

    #[test]
    fn settings_popup_rule_table_navigation_scrolls_active_rule_into_view() {
        let mut app = App::new(ui_settings(true));
        app.open_settings_popup();
        app.settings_popup
            .select_topic_for_tests(SettingsTopic::Proxy);
        app.settings_popup.draft_mut_for_tests().proxy =
            Some(proxy_settings_with_rule_counts(8, 0));
        app.settings_popup
            .select_proxy_row_for_tests(ProxyRow::RemoteRule(0));

        let (_ui, _buffer) = render_to_buffer_with_size(&mut app, 100, 12);
        for _ in 0..5 {
            app.handle_key_event(key(KeyCode::Char('j')));
            let (_ui, _buffer) = render_to_buffer_with_size(&mut app, 100, 12);
        }

        assert_eq!(app.settings_popup.scroll.offset().y, 0);
        assert_eq!(
            app.settings_popup
                .rule_table_scroll_offset_for_tests(ProxyRuleTable::Remote),
            5
        );
    }

    #[test]
    fn settings_popup_rule_tables_cap_height_from_viewport_height() {
        let mut app = App::new(ui_settings(true));
        app.open_settings_popup();
        app.settings_popup
            .select_topic_for_tests(SettingsTopic::Proxy);
        app.settings_popup.draft_mut_for_tests().proxy =
            Some(proxy_settings_with_rule_counts(10, 10));

        let (_ui, buffer) = render_to_buffer_with_size(&mut app, 100, 28);
        let content_area = settings_content_test_area(buffer.area);
        let max_height = settings_rule_table_max_height(content_area.height);
        find_buffer_text(&buffer, content_area, "Map Remote Rules")
            .expect("remote rule table should render");
        let preset = active_preset(app.settings_popup.draft()).expect("active preset should exist");
        let table = proxy_rule_table_widget(
            &app.settings_popup,
            ProxyRuleTable::Remote,
            Some(preset),
            max_height,
        );

        assert_eq!(max_height, content_area.height / 2);
        assert_eq!(table.height(), max_height);
    }

    #[test]
    fn settings_popup_rule_table_hides_overflowing_rows_and_renders_scrollbar() {
        let mut app = App::new(ui_settings(true));
        app.open_settings_popup();
        app.settings_popup
            .select_topic_for_tests(SettingsTopic::Proxy);
        app.settings_popup.draft_mut_for_tests().proxy =
            Some(proxy_settings_with_rule_counts(10, 0));

        let (_ui, buffer) = render_to_buffer_with_size(&mut app, 100, 28);
        let content_area = settings_content_test_area(buffer.area);
        let remote_title = find_buffer_text(&buffer, content_area, "Map Remote Rules")
            .expect("remote rule table should render");
        let max_height =
            settings_rule_table_max_height(settings_content_test_area(buffer.area).height);
        let rendered = (0..buffer.area.height)
            .map(|row| buffer_row(&buffer, row, 0, buffer.area.width))
            .collect::<Vec<_>>()
            .join("\n");
        let scrollbar_x = rule_table_scrollbar_column(&buffer, content_area, "Map Remote Rules")
            .expect("remote rule table scrollbar column should render");
        let scrollbar_column = (remote_title.y + 2..remote_title.y + max_height - 1)
            .map(|row| buffer[(scrollbar_x, row)].symbol())
            .collect::<String>();

        assert!(rendered.contains("https://api.example.com/v0"));
        assert!(rendered.contains("https://api.example.com/v6"));
        assert!(!rendered.contains("https://api.example.com/v7"));
        assert!(
            scrollbar_column.contains('█') || scrollbar_column.contains('║'),
            "{scrollbar_column:?}"
        );
    }

    #[test]
    fn settings_popup_mouse_wheel_scrolls_overflowing_rule_table() {
        let mut app = App::new(ui_settings(true));
        app.open_settings_popup();
        app.settings_popup
            .select_topic_for_tests(SettingsTopic::Proxy);
        app.settings_popup.draft_mut_for_tests().proxy =
            Some(proxy_settings_with_rule_counts(10, 0));

        let (ui, buffer) = render_to_buffer_with_size(&mut app, 100, 28);
        let content_area = settings_content_test_area(buffer.area);
        let remote_title = find_buffer_text(&buffer, content_area, "Map Remote Rules")
            .expect("remote rule table should render");

        ui.handle_mouse(
            mouse(
                MouseEventKind::ScrollDown,
                content_area.x.saturating_add(2),
                remote_title.y.saturating_add(2),
            ),
            &mut app,
        );

        assert_eq!(app.settings_popup.scroll.offset().y, 0);
        assert_eq!(
            app.settings_popup
                .rule_table_scroll_offset_for_tests(ProxyRuleTable::Remote),
            1
        );
    }

    #[test]
    fn settings_popup_rule_table_scrollbar_reaches_bottom_on_last_page() {
        let mut app = App::new(ui_settings(true));
        app.open_settings_popup();
        app.settings_popup
            .select_topic_for_tests(SettingsTopic::Proxy);
        app.settings_popup.draft_mut_for_tests().proxy =
            Some(proxy_settings_with_rule_counts(20, 0));
        app.settings_popup
            .select_proxy_row_for_tests(ProxyRow::RemoteRule(19));

        let (_ui, buffer) = render_to_buffer_with_size(&mut app, 100, 28);
        let content_area = settings_content_test_area(buffer.area);
        let remote_title = find_buffer_text(&buffer, content_area, "Map Remote Rules")
            .expect("remote rule table should render");
        let max_height = settings_rule_table_max_height(content_area.height);
        let scrollbar_column =
            rule_table_scrollbar_column(&buffer, content_area, "Map Remote Rules")
                .expect("remote rule table scrollbar column should render");

        assert_eq!(
            buffer[(scrollbar_column, remote_title.y + max_height - 2)].symbol(),
            "█"
        );
    }

    #[test]
    fn settings_popup_preset_switch_resets_rule_table_scroll_offsets() {
        let mut app = App::new(ui_settings(true));
        app.open_settings_popup();
        app.settings_popup
            .select_topic_for_tests(SettingsTopic::Proxy);
        let mut proxy = proxy_settings_with_rule_counts(10, 0);
        proxy.presets.push(ProxyPresetSettings {
            name: "qa".to_string(),
            ..ProxyPresetSettings::default()
        });
        app.settings_popup.draft_mut_for_tests().proxy = Some(proxy);
        app.settings_popup
            .select_proxy_row_for_tests(ProxyRow::RemoteRule(0));
        let (_ui, _buffer) = render_to_buffer_with_size(&mut app, 100, 28);
        for _ in 0..7 {
            app.handle_key_event(key(KeyCode::Char('j')));
            let (_ui, _buffer) = render_to_buffer_with_size(&mut app, 100, 28);
        }
        assert!(
            app.settings_popup
                .rule_table_scroll_offset_for_tests(ProxyRuleTable::Remote)
                > 0
        );

        app.settings_popup.start_proxy_preset_select();
        app.handle_key_event(key(KeyCode::Down));
        app.handle_key_event(key(KeyCode::Enter));

        assert_eq!(
            app.settings_popup
                .draft()
                .proxy
                .as_ref()
                .and_then(|proxy| proxy.active_preset.as_deref()),
            Some("qa")
        );
        assert_eq!(
            app.settings_popup
                .rule_table_scroll_offset_for_tests(ProxyRuleTable::Remote),
            0
        );
    }

    #[test]
    fn settings_popup_mouse_scroll_does_not_snap_back_to_selected_row() {
        let mut app = App::new(ui_settings(true));
        app.open_settings_popup();
        app.settings_popup
            .select_topic_for_tests(SettingsTopic::Proxy);
        app.settings_popup.draft_mut_for_tests().proxy =
            Some(proxy_settings_with_rule_counts(3, 3));
        let (_ui, _buffer) = render_to_buffer_with_size(&mut app, 100, 12);
        let ui = laid_out_ui(&app);
        let content_area = settings_content_test_area(Rect::new(0, 0, 100, 12));

        ui.handle_mouse(
            mouse(MouseEventKind::ScrollDown, content_area.x, content_area.y),
            &mut app,
        );
        let (_ui, _buffer) = render_to_buffer_with_size(&mut app, 100, 12);

        assert_eq!(app.settings_popup.scroll.offset().y, 1);
    }

    #[test]
    fn settings_popup_proxy_page_renders_preset_dividers_and_rule_tables() {
        let mut app = App::new(ui_settings(true));
        app.open_settings_popup();
        app.settings_popup
            .select_topic_for_tests(SettingsTopic::Proxy);
        app.settings_popup.draft_mut_for_tests().proxy = Some(crate::settings::ProxySettings {
            enable: true,
            active_preset: Some("dev".to_string()),
            presets: vec![crate::settings::ProxyPresetSettings {
                name: "dev".to_string(),
                map_remote: crate::settings::ProxyMapRemoteSettings {
                    enable: false,
                    rules: vec![crate::settings::ProxyMapRemoteRule {
                        from: "https://api.example.com".to_string(),
                        to: "http://localhost:3000".to_string(),
                        enable: true,
                    }],
                },
                map_local: crate::settings::ProxyMapLocalSettings {
                    enable: false,
                    rules: vec![crate::settings::ProxyMapLocalRule {
                        from: "https://static.example.com".to_string(),
                        to: "~/fixtures/app.js".to_string(),
                        enable: false,
                    }],
                },
            }],
        });

        let (_ui, buffer) = render_to_buffer_with_size(&mut app, 100, 28);
        let rendered = (0..buffer.area.height)
            .map(|row| buffer_row(&buffer, row, 0, buffer.area.width))
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains("Preset"));
        assert!(rendered.contains("Preset name"));
        assert!(rendered.contains("Mapping enabled"));
        assert!(rendered.contains("─ Map Remote ─"));
        assert!(rendered.contains("Map remote enabled"));
        assert!(rendered.contains("─ Map Local ─"));
        assert!(rendered.contains("Map local enabled"));
        assert!(rendered.contains("dev"));
        assert!(rendered.contains("Map Remote Rules"));
        assert!(rendered.contains("Map Local Rules"));
        assert!(rendered.contains("On"));
        assert!(rendered.contains("From"));
        assert!(rendered.contains("To"));
        assert!(rendered.contains("https://api.example.com"));
        assert!(rendered.contains("http://localhost:3000"));
    }

    #[test]
    fn settings_popup_proxy_page_without_presets_renders_empty_state_only() {
        let mut app = App::new(ui_settings(true));
        app.open_settings_popup();
        app.settings_popup
            .select_topic_for_tests(SettingsTopic::Proxy);

        let (_ui, buffer) = render_to_buffer_with_size(&mut app, 100, 20);
        let rendered = (0..buffer.area.height)
            .map(|row| buffer_row(&buffer, row, 0, buffer.area.width))
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains("No proxy preset configured."));
        assert!(!rendered.contains("Preset name"));
        assert!(!rendered.contains("Mapping enabled"));
        assert!(!rendered.contains("─ Map Remote ─"));
        assert!(!rendered.contains("─ Map Local ─"));
        assert!(!rendered.contains("Map Remote Rules"));
        assert!(!rendered.contains("Map Local Rules"));
    }

    #[test]
    fn settings_popup_proxy_page_with_stale_active_preset_renders_only_select() {
        let mut app = App::new(ui_settings(true));
        app.open_settings_popup();
        app.settings_popup
            .select_topic_for_tests(SettingsTopic::Proxy);
        app.settings_popup.draft_mut_for_tests().proxy =
            Some(proxy_settings("missing", &["dev", "qa"]));

        let (_ui, buffer) = render_to_buffer_with_size(&mut app, 100, 20);
        let rendered = (0..buffer.area.height)
            .map(|row| buffer_row(&buffer, row, 0, buffer.area.width))
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains("Preset"));
        assert!(rendered.contains("(none)"));
        assert!(!rendered.contains("Preset name"));
        assert!(!rendered.contains("Mapping enabled"));
        assert!(!rendered.contains("─ Map Remote ─"));
        assert!(!rendered.contains("─ Map Local ─"));
        assert!(!rendered.contains("Map Remote Rules"));
        assert!(!rendered.contains("Map Local Rules"));
    }

    #[test]
    fn settings_popup_proxy_page_renders_active_preset_controls_in_order() {
        let mut app = App::new(ui_settings(true));
        app.open_settings_popup();
        app.settings_popup
            .select_topic_for_tests(SettingsTopic::Proxy);
        app.settings_popup.draft_mut_for_tests().proxy =
            Some(proxy_settings_with_rule_counts(1, 1));

        let (_ui, buffer) = render_to_buffer_with_size(&mut app, 100, 28);
        let content_area = settings_content_test_area(buffer.area);
        let preset_name = find_buffer_text(&buffer, content_area, "Preset name")
            .expect("preset name input should render");
        let mapping_enabled = find_buffer_text(&buffer, content_area, "Mapping enabled")
            .expect("mapping checkbox should render");
        let remote_divider = find_buffer_text(&buffer, content_area, "─ Map Remote ─")
            .expect("remote divider should render");
        let remote_enabled = find_buffer_text(&buffer, content_area, "Map remote enabled")
            .expect("remote checkbox should render");
        let remote_rules = find_buffer_text(&buffer, content_area, "Map Remote Rules")
            .expect("remote table should render");
        let local_divider = find_buffer_text(&buffer, content_area, "─ Map Local ─")
            .expect("local divider should render");
        let local_enabled = find_buffer_text(&buffer, content_area, "Map local enabled")
            .expect("local checkbox should render");
        let local_rules = find_buffer_text(&buffer, content_area, "Map Local Rules")
            .expect("local table should render");

        assert_centered_divider_with_padding(&buffer, content_area, remote_divider, "Map Remote");
        assert_centered_divider_with_padding(&buffer, content_area, local_divider, "Map Local");
        assert!(preset_name.y < mapping_enabled.y);
        assert!(mapping_enabled.y < remote_divider.y);
        assert!(remote_divider.y < remote_enabled.y);
        assert!(remote_enabled.y < remote_rules.y);
        assert!(remote_rules.y < local_divider.y);
        assert!(local_divider.y < local_enabled.y);
        assert!(local_enabled.y < local_rules.y);
    }

    #[test]
    fn settings_popup_duplicate_preset_name_hint_renders_below_input() {
        let mut app = App::new(ui_settings(true));
        app.open_settings_popup();
        app.settings_popup
            .select_topic_for_tests(SettingsTopic::Proxy);
        app.settings_popup.draft_mut_for_tests().proxy =
            Some(proxy_settings("dev", &["dev", "qa"]));
        app.settings_popup
            .select_proxy_row_for_tests(ProxyRow::PresetName);
        focus_settings_content(&mut app);

        app.handle_key_event(key(KeyCode::Enter));
        for _ in 0.."dev".chars().count() {
            app.handle_key_event(key(KeyCode::Backspace));
        }
        for ch in "qa".chars() {
            app.handle_key_event(key(KeyCode::Char(ch)));
        }
        app.handle_key_event(key(KeyCode::Enter));

        let (_ui, buffer) = render_to_buffer_with_size(&mut app, 100, 28);
        let content_area = settings_content_test_area(buffer.area);
        let input_box = field_box_bounds(&buffer, content_area, "Preset name")
            .expect("preset name input should render");
        let hint = find_buffer_text(&buffer, content_area, "preset name already exists")
            .expect("duplicate hint should render");

        assert_eq!(
            hint.y,
            input_box.0.y.saturating_add(SETTING_TEXT_FIELD_HEIGHT)
        );
        assert_eq!(buffer[hint].fg, Color::Red);
    }

    #[test]
    fn settings_popup_proxy_preset_select_renders_dropdown_options() {
        let mut app = App::new(ui_settings(true));
        app.open_settings_popup();
        app.settings_popup
            .select_topic_for_tests(SettingsTopic::Proxy);
        app.settings_popup.draft_mut_for_tests().proxy =
            Some(proxy_settings("dev", &["dev", "qa", "prod"]));
        focus_settings_content(&mut app);
        app.handle_key_event(key(KeyCode::Enter));

        let (_ui, buffer) = render_to_buffer(&mut app);
        let rendered = (0..buffer.area.height)
            .map(|row| buffer_row(&buffer, row, 0, buffer.area.width))
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains("Preset"));
        assert!(rendered.contains("▴"));
        assert!(rendered.contains("qa"));
        assert!(rendered.contains("prod"));
    }

    #[test]
    fn settings_popup_proxy_preset_select_uses_ellipsis_for_overflow() {
        let long_name = "very-long-proxy-preset-name-that-needs-truncation";
        let mut app = App::new(ui_settings(true));
        app.open_settings_popup();
        app.settings_popup
            .select_topic_for_tests(SettingsTopic::Proxy);
        app.settings_popup.draft_mut_for_tests().proxy =
            Some(proxy_settings(long_name, &[long_name]));

        let (_ui, buffer) = render_to_buffer(&mut app);
        let content_area = settings_content_test_area(buffer.area);
        let row = buffer_row(
            &buffer,
            content_area.y + 1,
            content_area.x,
            content_area.width,
        );

        assert!(row.contains("…"));
    }

    #[test]
    fn settings_popup_proxy_preset_select_handles_mouse_selection() {
        let mut app = App::new(ui_settings(true));
        app.open_settings_popup();
        app.settings_popup
            .select_topic_for_tests(SettingsTopic::Proxy);
        app.settings_popup.draft_mut_for_tests().proxy =
            Some(proxy_settings("dev", &["dev", "qa", "prod"]));

        let ui = laid_out_ui(&app);
        let root_area = Rect::new(0, 0, 100, 12);
        let content_area = settings_content_test_area(root_area);
        let content_width = content_area.width.saturating_sub(1).max(1);
        let items = settings_content_items_for_test(&app.settings_popup, root_area);
        let field_layout = settings_field_layout(&items);
        let content_height =
            settings_content_height(&items, content_area.height, field_layout, content_width);
        let layout = settings_select_layout(
            &items,
            Some(SelectTarget::ProxyPreset),
            field_layout,
            content_width,
            content_height,
        )
        .expect("proxy preset select layout should exist")
        .layout;
        let box_click = Position::new(
            content_area.x + layout.box_area.x + 1,
            content_area.y + layout.box_area.y + 1,
        );
        drop(items);

        ui.handle_mouse(
            mouse(
                MouseEventKind::Down(MouseButton::Left),
                box_click.x,
                box_click.y,
            ),
            &mut app,
        );

        let ui = laid_out_ui(&app);
        let items = settings_content_items_for_test(&app.settings_popup, root_area);
        let field_layout = settings_field_layout(&items);
        let content_height =
            settings_content_height(&items, content_area.height, field_layout, content_width);
        let layout = settings_select_layout(
            &items,
            Some(SelectTarget::ProxyPreset),
            field_layout,
            content_width,
            content_height,
        )
        .expect("proxy preset select layout should exist")
        .layout;
        let options_area = layout
            .options_area
            .expect("open preset select should expose option rows");
        let qa_click = Position::new(
            content_area.x + options_area.x + 1,
            content_area.y + options_area.y + 1,
        );
        drop(items);

        ui.handle_mouse(
            mouse(
                MouseEventKind::Down(MouseButton::Left),
                qa_click.x,
                qa_click.y,
            ),
            &mut app,
        );

        assert_eq!(
            app.settings_popup
                .draft()
                .proxy
                .as_ref()
                .and_then(|proxy| proxy.active_preset.as_deref()),
            Some("qa")
        );
    }

    #[test]
    fn settings_popup_proxy_preset_dropdown_does_not_change_content_height() {
        let mut app = App::new(ui_settings(true));
        app.open_settings_popup();
        app.settings_popup
            .select_topic_for_tests(SettingsTopic::Proxy);
        app.settings_popup.draft_mut_for_tests().proxy =
            Some(proxy_settings("dev", &["dev", "qa", "prod"]));
        let content_width = 60;
        let root_area = Rect::new(0, 0, 100, 12);
        let closed_items = settings_content_items_for_test(&app.settings_popup, root_area);
        let closed_layout = settings_field_layout(&closed_items);
        let closed_height = settings_content_height(&closed_items, 0, closed_layout, content_width);
        drop(closed_items);

        focus_settings_content(&mut app);
        app.handle_key_event(key(KeyCode::Enter));
        assert_eq!(
            app.settings_popup.active_select_target(),
            Some(SelectTarget::ProxyPreset)
        );

        let open_items = settings_content_items_for_test(&app.settings_popup, root_area);
        let open_layout = settings_field_layout(&open_items);
        let open_height = settings_content_height(&open_items, 0, open_layout, content_width);

        assert_eq!(closed_height, open_height);
        assert_eq!(
            settings_select_control(&app.settings_popup, SelectTarget::ProxyPreset)
                .height_for_width(content_width),
            crate::select_widget::SELECT_FIELD_HEIGHT
        );
    }

    #[test]
    fn settings_popup_proxy_select_overlay_anchors_to_shared_control_area() {
        let mut app = App::new(ui_settings(true));
        app.open_settings_popup();
        app.settings_popup
            .select_topic_for_tests(SettingsTopic::Proxy);
        app.settings_popup.draft_mut_for_tests().proxy =
            Some(proxy_settings("dev", &["dev", "qa", "prod"]));
        focus_settings_content(&mut app);
        app.handle_key_event(key(KeyCode::Enter));

        let root_area = Rect::new(0, 0, 100, 12);
        let content_area = settings_content_test_area(root_area);
        let content_width = content_area.width.saturating_sub(1).max(1);
        let items = settings_content_items_for_test(&app.settings_popup, root_area);
        let field_layout = settings_field_layout(&items);
        let content_height =
            settings_content_height(&items, content_area.height, field_layout, content_width);
        let row_area = Rect::new(
            0,
            0,
            content_width,
            items[0].height(field_layout, content_width),
        );
        let row_areas = field_layout.areas(row_area);
        let select_layout = settings_select_layout(
            &items,
            Some(SelectTarget::ProxyPreset),
            field_layout,
            content_width,
            content_height,
        )
        .expect("proxy preset select layout should exist")
        .layout;

        assert_eq!(select_layout.box_area.x, row_areas.control.x);
        assert_eq!(
            select_layout
                .dropdown_area
                .expect("dropdown should render")
                .x,
            row_areas.control.x
        );
    }

    #[test]
    fn settings_popup_proxy_rule_tables_render_as_bordered_widgets() {
        let mut app = App::new(ui_settings(true));
        app.open_settings_popup();
        app.settings_popup
            .select_topic_for_tests(SettingsTopic::Proxy);
        app.settings_popup.draft_mut_for_tests().proxy = Some(crate::settings::ProxySettings {
            enable: true,
            active_preset: Some("dev".to_string()),
            presets: vec![crate::settings::ProxyPresetSettings {
                name: "dev".to_string(),
                map_remote: crate::settings::ProxyMapRemoteSettings {
                    enable: true,
                    rules: vec![crate::settings::ProxyMapRemoteRule {
                        from: "https://api.example.com".to_string(),
                        to: "http://localhost:3000".to_string(),
                        enable: true,
                    }],
                },
                map_local: crate::settings::ProxyMapLocalSettings::default(),
            }],
        });

        let (_ui, buffer) = render_to_buffer_with_size(&mut app, 100, 28);
        let content_area = settings_content_test_area(buffer.area);
        let title = find_buffer_text(&buffer, content_area, "Map Remote Rules")
            .expect("remote rule table title should render");

        assert_eq!(buffer[(content_area.x, title.y)].symbol(), "╭");
        assert_eq!(buffer[(content_area.x, title.y + 1)].symbol(), "│");
        assert_eq!(buffer[(content_area.x, title.y + 3)].symbol(), "╰");
    }

    #[test]
    fn settings_popup_rule_editor_popup_renders_two_textareas() {
        let mut app = App::new(ui_settings(true));
        app.open_settings_popup();
        app.settings_popup
            .select_topic_for_tests(SettingsTopic::Proxy);
        app.settings_popup.draft_mut_for_tests().proxy = Some(crate::settings::ProxySettings {
            enable: true,
            active_preset: Some("dev".to_string()),
            presets: vec![crate::settings::ProxyPresetSettings {
                name: "dev".to_string(),
                map_remote: crate::settings::ProxyMapRemoteSettings {
                    enable: true,
                    rules: vec![crate::settings::ProxyMapRemoteRule {
                        from: "https://api.example.com".to_string(),
                        to: "http://localhost:3000".to_string(),
                        enable: true,
                    }],
                },
                map_local: crate::settings::ProxyMapLocalSettings::default(),
            }],
        });
        app.settings_popup
            .select_proxy_row_for_tests(ProxyRow::RemoteRule(0));
        app.handle_key_event(key(KeyCode::Enter));

        let (_ui, buffer) = render_to_buffer(&mut app);
        let rendered = (0..buffer.area.height)
            .map(|row| buffer_row(&buffer, row, 0, buffer.area.width))
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains("Edit Map Remote Rule"));
        assert!(rendered.contains("From"));
        assert!(rendered.contains("To"));
        assert!(rendered.contains("https://api.example.com"));
        assert!(rendered.contains("http://localhost:3000"));
        assert!(rendered.contains("Apply [Enter]"));
    }

    #[test]
    fn settings_popup_rule_editor_uses_its_own_footer_key_hints() {
        let mut app = App::new(ui_settings(true));
        app.open_settings_popup();
        app.settings_popup
            .select_topic_for_tests(SettingsTopic::Proxy);
        app.settings_popup.draft_mut_for_tests().proxy = Some(crate::settings::ProxySettings {
            enable: true,
            active_preset: Some("dev".to_string()),
            presets: vec![crate::settings::ProxyPresetSettings {
                name: "dev".to_string(),
                map_remote: crate::settings::ProxyMapRemoteSettings {
                    enable: true,
                    rules: vec![crate::settings::ProxyMapRemoteRule {
                        from: "https://api.example.com".to_string(),
                        to: "http://localhost:3000".to_string(),
                        enable: true,
                    }],
                },
                map_local: crate::settings::ProxyMapLocalSettings::default(),
            }],
        });
        app.settings_popup
            .select_proxy_row_for_tests(ProxyRow::RemoteRule(0));
        app.handle_key_event(key(KeyCode::Enter));

        let (_ui, buffer) = render_to_buffer(&mut app);
        let editor_footer = rule_editor_footer_row(&buffer);
        let settings_footer = settings_popup_footer_row(&buffer);

        assert!(editor_footer.contains("Apply [Enter]"), "{editor_footer}");
        assert!(editor_footer.contains("Cancel [Esc]"), "{editor_footer}");
        assert!(editor_footer.contains("Switch [Tab]"), "{editor_footer}");
        assert!(
            editor_footer.contains("Cursor [Left/Right]"),
            "{editor_footer}"
        );
        assert!(settings_footer.contains("Back [Esc]"), "{settings_footer}");
        assert!(
            settings_footer.contains("Edit [Enter/e]"),
            "{settings_footer}"
        );
        assert!(
            !settings_footer.contains("Apply [Enter]"),
            "{settings_footer}"
        );
        assert!(
            !settings_footer.contains("Cancel [Esc]"),
            "{settings_footer}"
        );
    }

    #[test]
    fn settings_popup_rule_editor_fields_share_local_control_column() {
        let mut app = App::new(ui_settings(true));
        app.open_settings_popup();
        app.settings_popup
            .select_topic_for_tests(SettingsTopic::Proxy);
        app.settings_popup.draft_mut_for_tests().proxy = Some(crate::settings::ProxySettings {
            enable: true,
            active_preset: Some("dev".to_string()),
            presets: vec![crate::settings::ProxyPresetSettings {
                name: "dev".to_string(),
                map_remote: crate::settings::ProxyMapRemoteSettings {
                    enable: true,
                    rules: vec![crate::settings::ProxyMapRemoteRule {
                        from: "https://api.example.com".to_string(),
                        to: "http://localhost:3000".to_string(),
                        enable: true,
                    }],
                },
                map_local: crate::settings::ProxyMapLocalSettings::default(),
            }],
        });
        app.settings_popup
            .select_proxy_row_for_tests(ProxyRow::RemoteRule(0));
        app.handle_key_event(key(KeyCode::Enter));

        let (_ui, buffer) = render_to_buffer(&mut app);
        let editor_area = rule_editor_test_area(buffer.area);
        let from_box =
            field_box_bounds(&buffer, editor_area, "From").expect("from field box should render");
        let to_box =
            field_box_bounds(&buffer, editor_area, "To").expect("to field box should render");
        let editor_inner = editor_area.inner(Margin {
            vertical: 1,
            horizontal: 1,
        });
        let expected_right = editor_inner.right().saturating_sub(2);

        assert_eq!(from_box.0.x, to_box.0.x);
        assert_eq!(from_box.1.x, expected_right);
        assert_eq!(to_box.1.x, expected_right);
    }

    #[test]
    fn unsaved_settings_dialog_renders_centered_action_labels() {
        let mut app = App::new(ui_settings(true));
        app.open_settings_popup();
        app.settings_popup
            .draft_mut_for_tests()
            .recording
            .start_record_on_launch = false;
        app.handle_key_event(key(KeyCode::Esc));

        let (_ui, buffer) = render_to_buffer(&mut app);
        let rendered = (0..buffer.area.height)
            .map(|row| buffer_row(&buffer, row, 0, buffer.area.width))
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains("Unsaved Settings"));
        assert!(rendered.contains("Save [Enter]"));
        assert!(rendered.contains("Discard [Esc]"));
    }

    #[test]
    fn unsaved_settings_dialog_suppresses_parent_footer_key_hints() {
        let mut app = App::new(ui_settings(true));
        app.open_settings_popup();
        app.settings_popup
            .draft_mut_for_tests()
            .recording
            .start_record_on_launch = false;
        app.handle_key_event(key(KeyCode::Esc));

        let (_ui, buffer) = render_to_buffer(&mut app);
        let footer = settings_popup_footer_row(&buffer);

        assert!(!footer.contains("Save [s]"), "{footer}");
        assert!(!footer.contains("Close [Esc]"), "{footer}");
    }

    #[test]
    fn unsaved_settings_dialog_renders_bordered_buttons_at_bottom() {
        let mut app = App::new(ui_settings(true));
        app.open_settings_popup();
        app.settings_popup
            .draft_mut_for_tests()
            .recording
            .start_record_on_launch = false;
        app.handle_key_event(key(KeyCode::Esc));

        let (_ui, buffer) = render_to_buffer(&mut app);
        let dialog_area = action_dialog_test_area(buffer.area);
        let rendered = (dialog_area.y..dialog_area.bottom())
            .map(|row| buffer_row(&buffer, row, dialog_area.x, dialog_area.width))
            .collect::<Vec<_>>()
            .join("\n");
        let save = find_buffer_text(&buffer, dialog_area, "Save [Enter]")
            .expect("save button label should render");
        let discard = find_buffer_text(&buffer, dialog_area, "Discard [Esc]")
            .expect("discard button label should render");

        assert!(!rendered.contains("[ Save [Enter] ]"));
        assert_eq!(save.y, dialog_area.bottom().saturating_sub(3));
        assert_eq!(discard.y, save.y);
        assert_eq!(buffer[(save.x.saturating_sub(2), save.y - 1)].symbol(), "╭");
        assert_eq!(
            buffer[(discard.x.saturating_sub(2), discard.y - 1)].symbol(),
            "╭"
        );
        assert!(discard.x > save.x + text_width("Save [Enter]") + 4);
        assert_ne!(buffer[save].bg, Color::Green);
    }

    #[test]
    fn unsaved_settings_dialog_centers_message_above_buttons() {
        let mut app = App::new(ui_settings(true));
        app.open_settings_popup();
        app.settings_popup
            .draft_mut_for_tests()
            .recording
            .start_record_on_launch = false;
        app.handle_key_event(key(KeyCode::Esc));

        let (_ui, buffer) = render_to_buffer(&mut app);
        let dialog_area = action_dialog_test_area(buffer.area);
        let message = find_buffer_text(&buffer, dialog_area, "You have unsaved setting changes.")
            .expect("message should render");

        assert_eq!(message.y, dialog_area.y + 3);
    }

    #[test]
    fn unsaved_settings_dialog_buttons_do_not_overwrite_narrow_dialog_border() {
        let mut app = App::new(ui_settings(true));
        app.open_settings_popup();
        app.settings_popup
            .draft_mut_for_tests()
            .recording
            .start_record_on_launch = false;
        app.handle_key_event(key(KeyCode::Esc));

        let (_ui, buffer) = render_to_buffer_with_size(&mut app, 36, 12);
        let dialog_area = action_dialog_test_area(buffer.area);
        let button_label_row = dialog_area.bottom().saturating_sub(3);

        assert_eq!(
            buffer[(dialog_area.right() - 1, button_label_row)].symbol(),
            "│"
        );
    }

    fn mouse(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::empty(),
        }
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::empty())
    }

    fn focus_settings_content(app: &mut App) {
        app.handle_key_event(key(KeyCode::Right));
        assert_eq!(app.settings_popup.focus, SettingsPaneFocus::Content);
    }

    fn laid_out_ui(app: &App) -> RootView {
        let mut ui = RootView::new();
        View::layout(&mut ui, Rect::new(0, 0, 100, 12), app);

        ui
    }

    fn render_to_buffer(app: &mut App) -> (RootView, Buffer) {
        render_to_buffer_with_size(app, 100, 12)
    }

    fn render_to_buffer_with_size(app: &mut App, width: u16, height: u16) -> (RootView, Buffer) {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("test backend should initialize");
        let mut ui = RootView::new();

        terminal
            .draw(|frame| RootView::render(&mut ui, frame, app))
            .expect("UI should render in tests");

        (ui, terminal.backend_mut().buffer().clone())
    }

    fn settings_content_test_area(area: Rect) -> Rect {
        let popup_area = settings_popup_area(area);
        let inner = popup_area.inner(Margin {
            vertical: 1,
            horizontal: 1,
        });
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(18), Constraint::Min(20)])
            .split(inner);

        chunks[1].inner(Margin {
            vertical: 1,
            horizontal: 1,
        })
    }

    fn settings_content_items_for_test<'a>(
        popup: &'a SettingsPopup,
        root_area: Rect,
    ) -> Vec<SettingsContentItem<'a>> {
        let table_max_height =
            settings_rule_table_max_height(settings_content_test_area(root_area).height);
        settings_content_items_with_error(popup, table_max_height)
    }

    fn action_dialog_test_area(area: Rect) -> Rect {
        let parent = settings_popup_area(area);
        let unsaved_button_width = text_width("Save [Enter]")
            .saturating_add(4)
            .saturating_add(2)
            .saturating_add(text_width("Discard [Esc]"))
            .saturating_add(4)
            .saturating_add(2);
        let width = 58
            .min(parent.width.saturating_sub(4))
            .max(32)
            .max(unsaved_button_width);
        let height = 9.min(parent.height.saturating_sub(2)).max(7);

        centered_rect(width, height, parent)
    }

    fn rule_editor_test_area(area: Rect) -> Rect {
        rule_editor_area(settings_popup_area(area))
    }

    fn settings_popup_footer_row(buffer: &Buffer) -> String {
        let area = settings_popup_area(buffer.area);
        buffer_row(buffer, area.bottom().saturating_sub(1), area.x, area.width)
    }

    fn rule_editor_footer_row(buffer: &Buffer) -> String {
        let area = rule_editor_test_area(buffer.area);
        buffer_row(buffer, area.bottom().saturating_sub(1), area.x, area.width)
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

    fn find_buffer_text(buffer: &Buffer, area: Rect, text: &str) -> Option<Position> {
        let symbols = text
            .chars()
            .map(|symbol| symbol.to_string())
            .collect::<Vec<_>>();
        let symbol_count = u16::try_from(symbols.len()).ok()?;
        if symbol_count == 0 || area.width < symbol_count {
            return None;
        }

        for y in area.y..area.bottom() {
            for x in area.x..=area.right().saturating_sub(symbol_count) {
                if symbols.iter().enumerate().all(|(offset, symbol)| {
                    buffer[(x + u16::try_from(offset).unwrap_or(u16::MAX), y)].symbol() == symbol
                }) {
                    return Some(Position::new(x, y));
                }
            }
        }

        None
    }

    fn assert_centered_divider_with_padding(
        buffer: &Buffer,
        area: Rect,
        divider: Position,
        title: &str,
    ) {
        let row = buffer_row(buffer, divider.y, area.x, area.width);
        let title_byte = row.find(title).expect("divider title should render");
        let before_title = &row[..title_byte];
        let after_title = &row[title_byte + title.len()..];
        let left_dividers = before_title.chars().filter(|ch| *ch == '─').count();
        let right_dividers = after_title.chars().filter(|ch| *ch == '─').count();

        assert!(left_dividers > 0, "{row:?}");
        assert!(right_dividers > 0, "{row:?}");
        assert!(
            left_dividers.abs_diff(right_dividers) <= 1,
            "{row:?}: left={left_dividers}, right={right_dividers}"
        );

        let padding_width = area.width.saturating_sub(1).max(1);
        let upper_padding = buffer_row(buffer, divider.y.saturating_sub(1), area.x, padding_width);
        let lower_padding = buffer_row(buffer, divider.y.saturating_add(1), area.x, padding_width);

        assert!(upper_padding.trim().is_empty(), "{upper_padding:?}");
        assert!(lower_padding.trim().is_empty(), "{lower_padding:?}");
    }

    fn rule_table_scrollbar_column(buffer: &Buffer, area: Rect, title: &str) -> Option<u16> {
        let title = find_buffer_text(buffer, area, title)?;
        let table_right = (title.x..area.right())
            .find(|x| buffer[(*x, title.y)].symbol() == symbols::border::ROUNDED.top_right)?;

        table_right.checked_sub(1)
    }

    fn field_box_top_left(buffer: &Buffer, area: Rect, label_text: &str) -> Option<Position> {
        let symbols = label_text
            .chars()
            .map(|symbol| symbol.to_string())
            .collect::<Vec<_>>();
        let symbol_count = u16::try_from(symbols.len()).ok()?;
        if symbol_count == 0 || area.width < symbol_count {
            return None;
        }

        for y in area.y..area.bottom() {
            for label_x in area.x..=area.right().saturating_sub(symbol_count) {
                if !symbols.iter().enumerate().all(|(offset, symbol)| {
                    buffer[(label_x + u16::try_from(offset).unwrap_or(u16::MAX), y)].symbol()
                        == symbol
                }) {
                    continue;
                }

                let border_y = y.saturating_sub(1);
                let search_start = label_x.saturating_add(symbol_count).min(area.right());
                for x in search_start..area.right() {
                    if buffer[(x, border_y)].symbol() == symbols::border::ROUNDED.top_left {
                        return Some(Position::new(x, border_y));
                    }
                }
            }
        }

        None
    }

    fn field_box_top_right(buffer: &Buffer, area: Rect, label_text: &str) -> Option<Position> {
        field_box_bounds(buffer, area, label_text).map(|(_, top_right)| top_right)
    }

    fn field_box_width(buffer: &Buffer, area: Rect, label_text: &str) -> Option<u16> {
        let (top_left, top_right) = field_box_bounds(buffer, area, label_text)?;

        Some(top_right.x.saturating_sub(top_left.x).saturating_add(1))
    }

    fn field_box_bounds(
        buffer: &Buffer,
        area: Rect,
        label_text: &str,
    ) -> Option<(Position, Position)> {
        let top_left = field_box_top_left(buffer, area, label_text)?;
        for x in top_left.x.saturating_add(1)..area.right() {
            if buffer[(x, top_left.y)].symbol() == symbols::border::ROUNDED.top_right {
                return Some((top_left, Position::new(x, top_left.y)));
            }
        }

        None
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
    fn request_tree_displays_subtree_leaf_counts_with_muted_style() {
        let mut app = App::new(ui_settings(true));
        app.add_request(captured(0, "https://a.com/path/one"));
        app.add_request(captured(1, "https://a.com/path/two"));
        app.add_request(captured(2, "https://a.com/other"));
        let (ui, buffer) = render_to_buffer(&mut app);
        let request_area = ui.request_list.area();

        assert!(find_buffer_text(&buffer, request_area, "https://a.com/ 3").is_some());
        let path_count = find_buffer_text(&buffer, request_area, "path/ 2")
            .expect("path subtree count should render");
        let count_column = path_count.x + text_width("path/ ");

        assert_eq!(buffer[(count_column, path_count.y)].fg, Color::DarkGray);
        assert!(find_buffer_text(&buffer, request_area, "other 1").is_none());
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

    #[test]
    fn entered_body_tab_renders_editor_content() {
        let mut app = App::new(ui_settings(true));
        let mut req = captured(0, "https://a.com/api");
        req.req_body = Some("alpha beta".to_string());
        app.add_request(req);
        app.focus_panel(PanelFocus::Detail);
        app.detail_panel.select_tab(MainDisplayTab::RequestBody);
        app.enter_current_body_viewer();

        let (ui, buffer) = render_to_buffer(&mut app);
        let detail_area = ui.right_panel.detail.area();
        let text_area = body_editor_text_area(detail_area);

        assert!(find_buffer_text(&buffer, text_area, "alpha beta").is_some());
        assert!(find_buffer_text(&buffer, detail_area, "Normal").is_some());
        assert!(app.detail_panel.body_viewer.editor_mut().is_some());
    }

    #[test]
    fn body_viewer_jump_overlay_labels_visible_match_and_jumps() {
        let mut app = App::new(ui_settings(true));
        let mut req = captured(0, "https://a.com/api");
        req.req_body = Some("alpha beta".to_string());
        app.add_request(req);
        app.focus_panel(PanelFocus::Detail);
        app.detail_panel.select_tab(MainDisplayTab::RequestBody);
        app.enter_current_body_viewer();

        app.handle_key_event(key(KeyCode::Char('s')));
        app.handle_key_event(key(KeyCode::Char('b')));
        app.handle_key_event(key(KeyCode::Char('e')));
        let (ui, buffer) = render_to_buffer(&mut app);
        let text_area = body_editor_text_area(ui.right_panel.detail.area());
        let label_position =
            find_buffer_text(&buffer, text_area, "aeta").expect("jump label should render");

        assert_eq!(buffer[label_position].bg, Color::Green);

        app.handle_key_event(key(KeyCode::Char('a')));
        let editor = app.detail_panel.body_viewer.editor_mut().unwrap();

        assert_eq!(editor.cursor.row, 0);
        assert_eq!(editor.cursor.col, 6);
    }

    #[test]
    fn body_viewer_jump_shows_safe_labels_after_first_query_char() {
        let mut app = App::new(ui_settings(true));
        let mut req = captured(0, "https://a.com/api");
        req.req_body = Some("be be be be be be".to_string());
        app.add_request(req);
        app.focus_panel(PanelFocus::Detail);
        app.detail_panel.select_tab(MainDisplayTab::RequestBody);
        app.enter_current_body_viewer();

        app.handle_key_event(key(KeyCode::Char('s')));
        app.handle_key_event(key(KeyCode::Char('b')));
        let (ui, buffer) = render_to_buffer(&mut app);
        let text_area = body_editor_text_area(ui.right_panel.detail.area());
        let label_position =
            find_buffer_text(&buffer, text_area, "ae").expect("jump label should render");

        assert_eq!(buffer[label_position].bg, Color::Green);
    }

    #[test]
    fn body_viewer_jump_skips_possible_refinement_chars_as_labels() {
        let mut app = App::new(ui_settings(true));
        let mut req = captured(0, "https://a.com/api");
        req.req_body = Some("be be be be be be be be be be be be".to_string());
        app.add_request(req);
        app.focus_panel(PanelFocus::Detail);
        app.detail_panel.select_tab(MainDisplayTab::RequestBody);
        app.enter_current_body_viewer();

        app.handle_key_event(key(KeyCode::Char('s')));
        app.handle_key_event(key(KeyCode::Char('b')));
        let (ui, buffer) = render_to_buffer(&mut app);
        let text_area = body_editor_text_area(ui.right_panel.detail.area());

        assert!(find_buffer_text(&buffer, text_area, "ee").is_none());
    }

    #[test]
    fn body_viewer_jump_refines_query_when_possible_continuation_is_pressed() {
        let mut app = App::new(ui_settings(true));
        let mut req = captured(0, "https://a.com/api");
        req.req_body = Some("xx be yy".to_string());
        app.add_request(req);
        app.focus_panel(PanelFocus::Detail);
        app.detail_panel.select_tab(MainDisplayTab::RequestBody);
        app.enter_current_body_viewer();

        app.handle_key_event(key(KeyCode::Char('s')));
        app.handle_key_event(key(KeyCode::Char('b')));
        render_to_buffer(&mut app);

        app.handle_key_event(key(KeyCode::Char('e')));
        let editor = app.detail_panel.body_viewer.editor_mut().unwrap();
        assert_eq!(editor.cursor.col, 0);

        let (ui, buffer) = render_to_buffer(&mut app);
        let text_area = body_editor_text_area(ui.right_panel.detail.area());
        let label_position =
            find_buffer_text(&buffer, text_area, "ae").expect("refined jump label should render");

        assert_eq!(buffer[label_position].bg, Color::Green);

        app.handle_key_event(key(KeyCode::Char('a')));
        let editor = app.detail_panel.body_viewer.editor_mut().unwrap();

        assert_eq!(editor.cursor.row, 0);
        assert_eq!(editor.cursor.col, 3);
    }

    #[test]
    fn body_viewer_jump_label_h_takes_priority_over_editor_motion() {
        let mut app = App::new(ui_settings(true));
        let mut req = captured(0, "https://a.com/api");
        req.req_body = Some("be be be be be be".to_string());
        app.add_request(req);
        app.focus_panel(PanelFocus::Detail);
        app.detail_panel.select_tab(MainDisplayTab::RequestBody);
        app.enter_current_body_viewer();

        app.handle_key_event(key(KeyCode::Char('s')));
        app.handle_key_event(key(KeyCode::Char('b')));
        let (ui, buffer) = render_to_buffer(&mut app);
        let text_area = body_editor_text_area(ui.right_panel.detail.area());
        let label_position =
            find_buffer_text(&buffer, text_area, "he").expect("h jump label should render");

        assert_eq!(buffer[label_position].bg, Color::Green);

        app.handle_key_event(key(KeyCode::Char('h')));
        let editor = app.detail_panel.body_viewer.editor_mut().unwrap();

        assert_eq!(editor.cursor.row, 0);
        assert_eq!(editor.cursor.col, 15);
    }

    #[test]
    fn body_viewer_jump_query_can_refine_before_overlay_render() {
        let mut app = App::new(ui_settings(true));
        let mut req = captured(0, "https://a.com/api");
        req.req_body = Some("xx be yy".to_string());
        app.add_request(req);
        app.focus_panel(PanelFocus::Detail);
        app.detail_panel.select_tab(MainDisplayTab::RequestBody);
        app.enter_current_body_viewer();

        app.handle_key_event(key(KeyCode::Char('s')));
        app.handle_key_event(key(KeyCode::Char('b')));
        app.handle_key_event(key(KeyCode::Char('e')));
        let (ui, buffer) = render_to_buffer(&mut app);
        let text_area = body_editor_text_area(ui.right_panel.detail.area());
        let label_position =
            find_buffer_text(&buffer, text_area, "ae").expect("refined jump label should render");

        assert_eq!(buffer[label_position].bg, Color::Green);

        app.handle_key_event(key(KeyCode::Char('a')));
        let editor = app.detail_panel.body_viewer.editor_mut().unwrap();

        assert_eq!(editor.cursor.row, 0);
        assert_eq!(editor.cursor.col, 3);
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct JumpCandidate {
    label: char,
    position: Position,
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
        if should_render_active_body_editor(app) {
            render_body_editor(frame, app, focused, self.area());
            frame.render_widget(
                detail_tabs(app.detail_panel.active_tab),
                detail_tabs_area(self.area()),
            );
            return;
        }

        match build_detail_content(app) {
            DetailContent::Text(detail_text) => {
                let mut paragraph = detail_text_paragraph(detail_text, focused);

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
            DetailContent::Body(body_key) => {
                if app.ensure_body_text_cached(body_key) {
                    let total_lines: u16 = app
                        .cached_body_render_text(body_key)
                        .map(|detail_text| {
                            detail_text_paragraph(detail_text, focused)
                                .line_count(self.area().width)
                        })
                        .unwrap_or_default()
                        .try_into()
                        .unwrap_or(u16::MAX);
                    app.detail_panel.scroll.max_offset =
                        total_lines.saturating_sub(self.area().height);
                    let offset = app
                        .detail_panel
                        .scroll
                        .offset
                        .min(app.detail_panel.scroll.max_offset);
                    app.detail_panel.scroll.offset = offset;

                    if let Some(detail_text) = app.cached_body_render_text(body_key) {
                        frame.render_widget(
                            detail_text_paragraph(detail_text, focused).scroll((offset, 0)),
                            self.area(),
                        );
                    }
                }
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

fn detail_text_paragraph<'a>(text: impl Into<Text<'a>>, focused: bool) -> Paragraph<'a> {
    Paragraph::new(text)
        .block(detail_panel_block(focused))
        .wrap(Wrap { trim: false })
}

fn should_render_active_body_editor(app: &mut App) -> bool {
    if !app.detail_panel.body_viewer.is_active() || !app.detail_panel.active_tab.is_body() {
        return false;
    }

    let Some(key) = app.current_body_viewer_key() else {
        return false;
    };

    app.ensure_current_body_viewer_content() && app.detail_panel.body_viewer.has_content_for(key)
}

fn render_body_editor(frame: &mut Frame, app: &mut App, focused: bool, area: Rect) {
    app.detail_panel.scroll.max_offset = 0;
    app.detail_panel.scroll.offset = 0;

    if let Some(editor) = app.detail_panel.body_viewer.editor_mut() {
        frame.render_widget(
            EditorView::new(editor)
                .theme(body_editor_theme(focused))
                .wrap(true)
                .tab_width(BODY_TEXT_TAB_WIDTH),
            area,
        );
    }

    render_jump_overlay(frame.buffer_mut(), app, body_editor_text_area(area));
}

fn body_editor_theme(focused: bool) -> EditorTheme<'static> {
    EditorTheme::default()
        .base(Style::default().fg(Color::Reset))
        .cursor_style(Style::default().bg(Color::Green).fg(Color::Black))
        .selection_style(Style::default().bg(Color::White).fg(Color::DarkGray))
        .block(detail_panel_block(focused))
        .status_line(
            EditorStatusLine::default()
                .style_text(
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::Green)
                        .add_modifier(Modifier::BOLD),
                )
                .style_line(Style::default().fg(Color::Reset)),
        )
}

fn body_editor_text_area(area: Rect) -> Rect {
    let content_area = detail_content_area(area);
    Rect {
        height: content_area.height.saturating_sub(1),
        ..content_area
    }
}

fn render_jump_overlay(buffer: &mut Buffer, app: &mut App, area: Rect) {
    let Some(query) = app
        .detail_panel
        .body_viewer
        .jump_overlay_query(area)
        .map(str::to_string)
    else {
        render_jump_targets(
            buffer,
            app.detail_panel.body_viewer.rendered_jump_targets(area),
        );
        return;
    };

    let candidates = visible_jump_candidates(buffer, area, &query);
    render_jump_candidates(buffer, &candidates.labeled);
    app.detail_panel.body_viewer.replace_visible_jump_targets(
        &query,
        area,
        candidates
            .labeled
            .iter()
            .map(|candidate| (candidate.label, candidate.position))
            .collect(),
        candidates.has_matches,
    );
}

fn visible_jump_candidates(buffer: &Buffer, area: Rect, query: &str) -> VisibleJumpCandidates {
    if area.is_empty() || query.is_empty() {
        return VisibleJumpCandidates::default();
    }

    let query = query
        .chars()
        .map(|ch| ch.to_ascii_lowercase())
        .collect::<Vec<_>>();
    let query_width = query.len();
    if query_width == 0 || query_width > area.width as usize {
        return VisibleJumpCandidates::default();
    }

    let mut matches = Vec::with_capacity(JUMP_LABEL_COUNT);
    let mut ambiguous_labels = HashSet::with_capacity(JUMP_LABEL_COUNT);
    let mut has_matches = false;
    for y in area.y..area.bottom() {
        let row = (area.x..area.right())
            .map(|x| {
                buffer[(x, y)]
                    .symbol()
                    .chars()
                    .next()
                    .unwrap_or(' ')
                    .to_ascii_lowercase()
            })
            .collect::<Vec<_>>();

        for column in 0..=row.len().saturating_sub(query_width) {
            if row[column..column + query_width] != query {
                continue;
            }
            has_matches = true;
            if let Some(next_ch) = row
                .get(column + query_width)
                .filter(|next_ch| JUMP_LABELS.contains(**next_ch))
            {
                ambiguous_labels.insert(*next_ch);
            }
            if matches.len() >= JUMP_LABEL_COUNT {
                continue;
            }
            let Ok(column) = u16::try_from(column) else {
                continue;
            };
            matches.push(Position::new(area.x.saturating_add(column), y));
        }
    }

    let labeled = JUMP_LABELS
        .chars()
        .filter(|label| !ambiguous_labels.contains(&label.to_ascii_lowercase()))
        .zip(matches.iter().copied())
        .map(|(label, position)| JumpCandidate { label, position })
        .collect();

    VisibleJumpCandidates {
        has_matches,
        labeled,
    }
}

fn render_jump_candidates(buffer: &mut Buffer, candidates: &[JumpCandidate]) {
    let style = jump_label_style();
    for candidate in candidates {
        render_jump_label(buffer, candidate.label, candidate.position, style);
    }
}

fn render_jump_targets(buffer: &mut Buffer, targets: &[(char, Position)]) {
    let style = jump_label_style();
    for (label, position) in targets {
        render_jump_label(buffer, *label, *position, style);
    }
}

fn render_jump_label(buffer: &mut Buffer, label: char, position: Position, style: Style) {
    buffer[position].set_char(label).set_style(style);
}

fn jump_label_style() -> Style {
    Style::default().fg(Color::Black).bg(Color::Green)
}

const JUMP_LABELS: &str = "asdfghjklqwertyuiopzxcvbnm";
const JUMP_LABEL_COUNT: usize = JUMP_LABELS.len();

#[derive(Default)]
struct VisibleJumpCandidates {
    has_matches: bool,
    labeled: Vec<JumpCandidate>,
}

impl MouseHandler for DetailView {
    fn handle_mouse(&self, mouse: MouseEvent, app: &mut App) -> bool {
        if !self.contains_mouse(mouse) {
            return false;
        }

        match mouse.kind {
            MouseEventKind::ScrollDown => {
                if app.detail_panel.body_viewer.is_active() {
                    app.detail_panel.body_viewer.scroll_down();
                } else {
                    app.detail_panel.scroll.scroll_down();
                }
                true
            }
            MouseEventKind::ScrollUp => {
                if app.detail_panel.body_viewer.is_active() {
                    app.detail_panel.body_viewer.scroll_up();
                } else {
                    app.detail_panel.scroll.scroll_up();
                }
                true
            }
            MouseEventKind::Down(button) => {
                app.focus_panel(PanelFocus::Detail);
                if let Some(tab) =
                    detail_tab_at_position(self.area(), Position::new(mouse.column, mouse.row))
                {
                    app.detail_panel.select_tab(tab);
                } else if app.detail_panel.body_viewer.is_active()
                    && button == MouseButton::Left
                    && body_editor_text_area(self.area())
                        .contains(Position::new(mouse.column, mouse.row))
                {
                    app.detail_panel.body_viewer.handle_mouse(mouse);
                } else if button == MouseButton::Left {
                    app.detail_panel.selected_header_row = header_table_row_at_position(
                        app,
                        self.area(),
                        Position::new(mouse.column, mouse.row),
                    );
                }
                true
            }
            MouseEventKind::Drag(MouseButton::Left) | MouseEventKind::Up(MouseButton::Left)
                if app.detail_panel.body_viewer.is_active()
                    && body_editor_text_area(self.area())
                        .contains(Position::new(mouse.column, mouse.row)) =>
            {
                app.detail_panel.body_viewer.handle_mouse(mouse);
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

fn settings_popup_area(area: Rect) -> Rect {
    let width = area.width.saturating_sub(4).clamp(40, 96);
    let height = area.height.saturating_sub(4).clamp(12, 28);
    centered_rect(width, height, area)
}

const SETTINGS_RULE_TABLE_MAX_HEIGHT_PERCENT: u16 = 50;
const SETTINGS_TABLE_MIN_HEIGHT: u16 = 4;

fn settings_rule_table_max_height(viewport_height: u16) -> u16 {
    let proportional = u16::try_from(
        u32::from(viewport_height) * u32::from(SETTINGS_RULE_TABLE_MAX_HEIGHT_PERCENT) / 100,
    )
    .unwrap_or(u16::MAX);
    proportional.max(SETTINGS_TABLE_MIN_HEIGHT)
}

#[derive(Clone, Copy)]
struct SettingsPopupLayout {
    area: Rect,
    topics: Rect,
    content_panel: Rect,
    content_viewport: Rect,
}

fn settings_popup_layout(area: Rect) -> SettingsPopupLayout {
    let popup_area = settings_popup_area(area);
    let inner = popup_area.inner(Margin {
        vertical: 1,
        horizontal: 1,
    });
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(18), Constraint::Min(20)])
        .split(inner);
    let content_viewport = chunks[1].inner(Margin {
        vertical: 1,
        horizontal: 1,
    });

    SettingsPopupLayout {
        area: popup_area,
        topics: chunks[0],
        content_panel: chunks[1],
        content_viewport,
    }
}

fn render_settings_popup(frame: &mut Frame, app: &mut App) {
    let layout = settings_popup_layout(frame.area());
    let area = layout.area;
    frame.render_widget(Clear, area);

    let dirty = if app.settings_popup.is_dirty() {
        " *"
    } else {
        ""
    };
    let mut block = Block::default()
        .title(format!(" Settings{dirty} "))
        .title_alignment(Alignment::Left)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(if app.is_popup_focused(PopupFocus::Settings) {
            Style::default().fg(Color::Green)
        } else {
            Style::default()
        });
    let key_hint_text =
        settings_key_hint_text(app.settings_popup.key_hints(), area.width.saturating_sub(2));
    if !key_hint_text.is_empty() {
        block = block.title_bottom(Line::from(key_hint_text).alignment(Alignment::Center));
    }
    frame.render_widget(block, area);

    render_settings_topics(frame, &app.settings_popup, layout.topics);
    render_settings_content(frame, &mut app.settings_popup, layout.content_panel);

    if let Some(dialog) = app.settings_popup.unsaved_dialog() {
        render_action_dialog(frame, &dialog, area);
    }

    if let Some(editor) = app.settings_popup.rule_editor() {
        render_rule_editor_popup(frame, editor, area);
    }
}

fn settings_key_hint_text(hints: &[SettingsKeyHint], max_width: u16) -> String {
    if hints.is_empty() || max_width == 0 {
        return String::new();
    }

    let max_width = usize::from(max_width);
    let mut text = String::with_capacity(max_width.min(96));
    let mut text_width_cols = 0usize;
    for hint in hints {
        let hint_width = usize::from(text_width(hint.label))
            .saturating_add(usize::from(text_width(hint.key)))
            .saturating_add(3);
        let separator_width = if text.is_empty() { 0 } else { 2 };
        if text_width_cols
            .saturating_add(separator_width)
            .saturating_add(hint_width)
            > max_width
        {
            break;
        }

        if !text.is_empty() {
            text.push_str("  ");
            text_width_cols = text_width_cols.saturating_add(2);
        }
        text.push_str(hint.label);
        text.push_str(" [");
        text.push_str(hint.key);
        text.push(']');
        text_width_cols = text_width_cols.saturating_add(hint_width);
    }

    if text.is_empty() {
        let hint = hints[0];
        fit_input_value(&format!("{} [{}]", hint.label, hint.key), max_width)
    } else {
        text
    }
}

fn render_settings_topics(frame: &mut Frame, popup: &SettingsPopup, area: Rect) {
    let lines = SettingsTopic::all()
        .iter()
        .map(|topic| {
            let prefix = if *topic == popup.topic { "> " } else { "  " };
            Line::from(format!("{prefix}{}", topic.title()))
        })
        .collect::<Vec<_>>();
    let focused = popup.focus == SettingsPaneFocus::Topics;
    frame.render_widget(
        Paragraph::new(lines).block(panel_block("Topics", focused)),
        area,
    );
}

fn render_settings_content(frame: &mut Frame, popup: &mut SettingsPopup, area: Rect) {
    frame.render_widget(
        base_panel_block(popup.focus == SettingsPaneFocus::Content),
        area,
    );
    let area = area.inner(Margin {
        vertical: 1,
        horizontal: 1,
    });
    if area.is_empty() {
        return;
    }

    let scroll_request = popup.take_scroll_request();
    let table_max_height = settings_rule_table_max_height(area.height);
    if popup.topic == SettingsTopic::Proxy {
        sync_settings_rule_table_views(popup, table_max_height);
    }
    let items = settings_content_items_with_error(popup, table_max_height);
    let field_layout = settings_field_layout(&items);
    let content_width =
        settings_content_width_for_viewport(&items, field_layout, area.width, area.height);
    let content_layout =
        SettingsContentLayout::new(&items, field_layout, content_width, area.height);
    let scroll_target = match scroll_request {
        Some(SettingsScrollRequest::EnsureSelectedVisible) => {
            content_layout.scroll_y_for_selected(popup.selected_row, popup.scroll.offset().y)
        }
        None => None,
    };
    let scrolling_enabled = content_layout.scrolling_enabled;
    let mut scroll_view = ScrollView::new(Size::new(
        content_layout.content_width,
        content_layout.buffer_height,
    ))
    .vertical_scrollbar_visibility(if scrolling_enabled {
        ScrollbarVisibility::Always
    } else {
        ScrollbarVisibility::Never
    })
    .horizontal_scrollbar_visibility(ScrollbarVisibility::Never);

    for (item, area) in content_layout.item_areas() {
        item.render(&mut scroll_view, area, field_layout);
    }

    for (item, area) in content_layout.item_areas() {
        let overlay_bounds = Rect::new(
            0,
            area.y,
            content_layout.content_width,
            content_layout.buffer_height.saturating_sub(area.y),
        );
        item.render_overlay(&mut scroll_view, area, field_layout, overlay_bounds);
    }
    drop(items);
    if let Some(y) = scroll_target {
        let offset = popup.scroll.offset();
        popup.scroll.set_offset(Position::new(offset.x, y));
    } else if !scrolling_enabled {
        popup.scroll.set_offset(Position::ORIGIN);
    }
    frame.render_stateful_widget(scroll_view, area, &mut popup.scroll);
}

fn sync_settings_rule_table_views(popup: &mut SettingsPopup, table_max_height: u16) {
    for table in [ProxyRuleTable::Remote, ProxyRuleTable::Local] {
        let row_count = popup.rule_table_row_count(table);
        let height = settings_table_height(row_count, table_max_height);
        popup.sync_rule_table_view(table, settings_table_visible_rows(height));
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SettingsFieldLayoutConfig {
    selected_prefix: &'static str,
    idle_prefix: &'static str,
    label_control_gap: u16,
}

impl Default for SettingsFieldLayoutConfig {
    fn default() -> Self {
        Self {
            selected_prefix: "  ",
            idle_prefix: "  ",
            label_control_gap: 2,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SettingsFieldLayout {
    selected_prefix: &'static str,
    idle_prefix: &'static str,
    prefix_width: u16,
    label_width: u16,
    label_control_gap: u16,
}

impl SettingsFieldLayout {
    fn from_config(label_width: u16, config: SettingsFieldLayoutConfig) -> Self {
        Self {
            selected_prefix: config.selected_prefix,
            idle_prefix: config.idle_prefix,
            prefix_width: text_width(config.selected_prefix).max(text_width(config.idle_prefix)),
            label_width,
            label_control_gap: config.label_control_gap,
        }
    }

    fn control_width(self, row_width: u16) -> u16 {
        let consumed = self
            .prefix_width
            .saturating_add(self.label_width)
            .saturating_add(self.label_control_gap);
        row_width.saturating_sub(consumed)
    }

    fn areas(self, area: Rect) -> SettingsFieldRowAreas {
        let label_y = area.y.saturating_add(area.height.saturating_sub(1) / 2);
        let prefix_width = self.prefix_width.min(area.width);
        let prefix = Rect::new(area.x, label_y, prefix_width, area.height.min(1));

        let label_x = area.x.saturating_add(self.prefix_width).min(area.right());
        let label_right = label_x.saturating_add(self.label_width).min(area.right());
        let label = Rect::new(
            label_x,
            label_y,
            label_right.saturating_sub(label_x),
            area.height.min(1),
        );

        let control_x = label_x
            .saturating_add(self.label_width)
            .saturating_add(self.label_control_gap)
            .min(area.right());
        let control = Rect::new(
            control_x,
            area.y,
            area.right().saturating_sub(control_x),
            area.height,
        );

        SettingsFieldRowAreas {
            prefix,
            label,
            control,
        }
    }

    fn prefix(self, selected: bool) -> &'static str {
        if selected {
            self.selected_prefix
        } else {
            self.idle_prefix
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SettingsFieldRowAreas {
    prefix: Rect,
    label: Rect,
    control: Rect,
}

#[derive(Clone, Copy)]
struct SettingsFieldStyle {
    control: Style,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SettingsControlActivity {
    Idle,
    Active,
}

fn settings_field_style(selected: bool, activity: SettingsControlActivity) -> SettingsFieldStyle {
    let style = match activity {
        SettingsControlActivity::Active => Style::default().fg(Color::Indexed(208)),
        SettingsControlActivity::Idle if selected => Style::default().fg(Color::Green),
        SettingsControlActivity::Idle => Style::default(),
    };
    SettingsFieldStyle { control: style }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SettingsSelectLayout {
    target: SelectTarget,
    layout: crate::select_widget::SelectWidgetLayout,
}

impl SettingsSelectLayout {
    fn box_contains(self, position: Position) -> bool {
        self.layout.box_area.contains(position)
    }

    fn dropdown_contains(self, position: Position) -> bool {
        self.layout
            .dropdown_area
            .is_some_and(|area| area.contains(position))
    }
}

trait SettingsControlView {
    fn height_for_width(&self, width: u16) -> u16;
    fn activity(&self) -> SettingsControlActivity {
        SettingsControlActivity::Idle
    }
    fn render(&self, area: Rect, style: SettingsFieldStyle, buf: &mut Buffer);

    fn has_overlay(&self) -> bool {
        false
    }

    fn render_overlay(&self, _control_area: Rect, _overlay_bounds: Rect, _buf: &mut Buffer) {}

    fn select_layout(
        &self,
        _control_area: Rect,
        _overlay_bounds: Rect,
    ) -> Option<SettingsSelectLayout> {
        None
    }
}

struct SettingsFieldRow<'a> {
    label: Cow<'a, str>,
    selected: bool,
    control: Box<dyn SettingsControlView + 'a>,
}

impl<'a> SettingsFieldRow<'a> {
    fn new(
        label: impl Into<Cow<'a, str>>,
        selected: bool,
        control: impl SettingsControlView + 'a,
    ) -> Self {
        Self {
            label: label.into(),
            selected,
            control: Box::new(control),
        }
    }

    fn label_width(&self) -> u16 {
        text_width(self.label.as_ref())
    }

    fn height(&self, layout: SettingsFieldLayout, width: u16) -> u16 {
        let control_width = layout.control_width(width);
        self.control.height_for_width(control_width).max(1)
    }

    fn render_into(&self, area: Rect, layout: SettingsFieldLayout, buf: &mut Buffer) {
        if area.is_empty() {
            return;
        }

        let height = self.height(layout, area.width).min(area.height);
        let area = Rect::new(area.x, area.y, area.width, height);
        let areas = layout.areas(area);
        let style = settings_field_style(self.selected, self.control.activity());

        if !areas.prefix.is_empty() {
            Line::styled(layout.prefix(self.selected), style.control).render(areas.prefix, buf);
        }
        if !areas.label.is_empty() {
            Line::styled(self.label.as_ref(), style.control).render(areas.label, buf);
        }
        self.control.render(areas.control, style, buf);
    }

    fn render_overlay(
        &self,
        area: Rect,
        layout: SettingsFieldLayout,
        overlay_bounds: Rect,
        buf: &mut Buffer,
    ) {
        if area.is_empty() || !self.control.has_overlay() {
            return;
        }

        let height = self.height(layout, area.width).min(area.height);
        let area = Rect::new(area.x, area.y, area.width, height);
        let areas = layout.areas(area);
        self.control
            .render_overlay(areas.control, overlay_bounds, buf);
    }

    fn select_layout(
        &self,
        area: Rect,
        layout: SettingsFieldLayout,
        overlay_bounds: Rect,
    ) -> Option<SettingsSelectLayout> {
        let height = self.height(layout, area.width).min(area.height);
        let area = Rect::new(area.x, area.y, area.width, height);
        let areas = layout.areas(area);
        self.control.select_layout(areas.control, overlay_bounds)
    }
}

struct SettingsFieldRowWidget<'a, 'b> {
    row: &'b SettingsFieldRow<'a>,
    layout: SettingsFieldLayout,
}

impl Widget for SettingsFieldRowWidget<'_, '_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        self.row.render_into(area, self.layout, buf);
    }
}

struct SettingsFieldRowOverlayWidget<'a, 'b> {
    row: &'b SettingsFieldRow<'a>,
    layout: SettingsFieldLayout,
    overlay_bounds: Rect,
}

impl Widget for SettingsFieldRowOverlayWidget<'_, '_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        self.row
            .render_overlay(area, self.layout, self.overlay_bounds, buf);
    }
}

enum SettingsContentItem<'a> {
    Line(Line<'static>),
    Divider {
        title: &'static str,
    },
    Field {
        row: usize,
        field: SettingsFieldRow<'a>,
    },
    FullWidthRuleTable {
        row: usize,
        table: ProxyRuleTableWidget<'a>,
    },
}

impl SettingsContentItem<'_> {
    fn height(&self, layout: SettingsFieldLayout, width: u16) -> u16 {
        match self {
            Self::Line(_) => 1,
            Self::Divider { .. } => SETTINGS_DIVIDER_HEIGHT,
            Self::Field { field, .. } => field.height(layout, width),
            Self::FullWidthRuleTable { table, .. } => table.height(),
        }
    }

    fn render(&self, scroll_view: &mut ScrollView, area: Rect, layout: SettingsFieldLayout) {
        match self {
            Self::Line(line) => scroll_view.render_widget(Paragraph::new(line.clone()), area),
            Self::Divider { title } => scroll_view.render_widget(SettingsDivider { title }, area),
            Self::Field { field, .. } => {
                scroll_view.render_widget(SettingsFieldRowWidget { row: field, layout }, area)
            }
            Self::FullWidthRuleTable { table, .. } => scroll_view.render_widget(table, area),
        }
    }

    fn render_overlay(
        &self,
        scroll_view: &mut ScrollView,
        area: Rect,
        layout: SettingsFieldLayout,
        overlay_bounds: Rect,
    ) {
        if let Self::Field { field, .. } = self {
            scroll_view.render_widget(
                SettingsFieldRowOverlayWidget {
                    row: field,
                    layout,
                    overlay_bounds,
                },
                area,
            );
        }
    }

    fn field_label_width(&self) -> Option<u16> {
        match self {
            Self::Field { field, .. } => Some(field.label_width()),
            Self::Line(_) | Self::Divider { .. } | Self::FullWidthRuleTable { .. } => None,
        }
    }

    fn focus_row(&self) -> Option<usize> {
        match self {
            Self::Field { row, .. } | Self::FullWidthRuleTable { row, .. } => Some(*row),
            Self::Line(_) | Self::Divider { .. } => None,
        }
    }

    fn focused_area(&self, area: Rect) -> Rect {
        match self {
            Self::FullWidthRuleTable { table, .. } => table.active_rule_area(area).unwrap_or(area),
            Self::Line(_) | Self::Divider { .. } | Self::Field { .. } => area,
        }
    }

    fn select_layout(
        &self,
        target: Option<SelectTarget>,
        area: Rect,
        layout: SettingsFieldLayout,
        overlay_bounds: Rect,
    ) -> Option<SettingsSelectLayout> {
        let Self::Field { field, .. } = self else {
            return None;
        };
        let select = field.select_layout(area, layout, overlay_bounds)?;
        match target {
            Some(target) if select.target != target => None,
            _ => Some(select),
        }
    }
}

struct SettingsDivider {
    title: &'static str,
}

const SETTINGS_DIVIDER_HEIGHT: u16 = 3;

impl Widget for SettingsDivider {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.is_empty() {
            return;
        }

        Paragraph::new(Line::styled(
            titled_divider_text(self.title, area.width),
            Style::default().fg(Color::DarkGray),
        ))
        .render(
            Rect::new(
                area.x,
                area.y.saturating_add(area.height / 2),
                area.width,
                1,
            ),
            buf,
        );
    }
}

fn titled_divider_text(title: &str, width: u16) -> String {
    let title = format!(" {title} ");
    let title_width = text_width(&title);
    if width <= title_width {
        return fit_input_value(&title, usize::from(width));
    }

    let divider_width = width.saturating_sub(title_width);
    let prefix_width = divider_width / 2;
    let suffix_width = divider_width.saturating_sub(prefix_width);
    format!(
        "{}{title}{}",
        "─".repeat(usize::from(prefix_width)),
        "─".repeat(usize::from(suffix_width))
    )
}

fn settings_content_items_with_error<'a>(
    popup: &'a SettingsPopup,
    table_max_height: u16,
) -> Vec<SettingsContentItem<'a>> {
    let mut items = settings_content_items(popup, table_max_height);
    if let Some(error) = popup.error() {
        items.push(SettingsContentItem::Line(Line::from("")));
        items.push(SettingsContentItem::Line(Line::styled(
            format!("! {error}"),
            Style::default().fg(Color::Red),
        )));
    }
    items
}

fn settings_field_layout(items: &[SettingsContentItem<'_>]) -> SettingsFieldLayout {
    settings_field_layout_with_config(items, SettingsFieldLayoutConfig::default())
}

fn settings_field_layout_with_config(
    items: &[SettingsContentItem<'_>],
    config: SettingsFieldLayoutConfig,
) -> SettingsFieldLayout {
    let label_width = items
        .iter()
        .filter_map(SettingsContentItem::field_label_width)
        .max()
        .unwrap_or(0);
    SettingsFieldLayout::from_config(label_width, config)
}

fn settings_field_layout_for_rows(
    rows: &[SettingsFieldRow<'_>],
    config: SettingsFieldLayoutConfig,
) -> SettingsFieldLayout {
    let label_width = rows
        .iter()
        .map(SettingsFieldRow::label_width)
        .max()
        .unwrap_or(0);
    SettingsFieldLayout::from_config(label_width, config)
}

fn settings_content_height(
    items: &[SettingsContentItem<'_>],
    min_height: u16,
    layout: SettingsFieldLayout,
    content_width: u16,
) -> u16 {
    items
        .iter()
        .map(|item| item.height(layout, content_width))
        .fold(0u16, u16::saturating_add)
        .max(min_height)
}

fn settings_content_width_for_viewport(
    items: &[SettingsContentItem<'_>],
    field_layout: SettingsFieldLayout,
    viewport_width: u16,
    viewport_height: u16,
) -> u16 {
    let full_width = viewport_width.max(1);
    let body_height = settings_content_height(items, 0, field_layout, full_width);

    if body_height > viewport_height {
        full_width.saturating_sub(1).max(1)
    } else {
        full_width
    }
}

struct SettingsContentLayout<'items, 'content> {
    items: &'items [SettingsContentItem<'content>],
    field_layout: SettingsFieldLayout,
    content_width: u16,
    body_height: u16,
    buffer_height: u16,
    viewport_height: u16,
    scrolling_enabled: bool,
}

impl<'items, 'content> SettingsContentLayout<'items, 'content> {
    fn new(
        items: &'items [SettingsContentItem<'content>],
        field_layout: SettingsFieldLayout,
        content_width: u16,
        viewport_height: u16,
    ) -> Self {
        let body_height = settings_content_height(items, 0, field_layout, content_width);

        Self {
            items,
            field_layout,
            content_width,
            body_height,
            buffer_height: body_height.max(viewport_height),
            viewport_height,
            scrolling_enabled: body_height > viewport_height,
        }
    }

    fn item_areas(&self) -> impl Iterator<Item = (&SettingsContentItem<'content>, Rect)> + '_ {
        let mut y = 0;
        self.items.iter().map(move |item| {
            let height = item.height(self.field_layout, self.content_width);
            let area = Rect::new(0, y, self.content_width, height);
            y = y.saturating_add(height);

            (item, area)
        })
    }

    fn selected_item_area(&self, selected_row: usize) -> Option<Rect> {
        self.item_areas().find_map(|(item, area)| {
            (item.focus_row() == Some(selected_row)).then(|| item.focused_area(area))
        })
    }

    fn scroll_y_for_selected(&self, selected_row: usize, current_y: u16) -> Option<u16> {
        if !self.scrolling_enabled || self.viewport_height == 0 {
            return None;
        }

        let selected = self.selected_item_area(selected_row)?;
        let max_y = self.body_height.saturating_sub(self.viewport_height);
        let visible_bottom = current_y.saturating_add(self.viewport_height);
        let selected_bottom = selected.y.saturating_add(selected.height);
        let target = if selected.height > self.viewport_height || selected.y < current_y {
            selected.y
        } else if selected_bottom > visible_bottom {
            selected_bottom.saturating_sub(self.viewport_height)
        } else {
            current_y
        }
        .min(max_y);

        (target != current_y).then_some(target)
    }
}

fn settings_select_layout(
    items: &[SettingsContentItem<'_>],
    target: Option<SelectTarget>,
    layout: SettingsFieldLayout,
    content_width: u16,
    content_height: u16,
) -> Option<SettingsSelectLayout> {
    let mut y = 0;
    for item in items {
        let height = item.height(layout, content_width);
        let row_area = Rect::new(0, y, content_width, height);
        let overlay_bounds = Rect::new(0, y, content_width, content_height.saturating_sub(y));
        if let Some(select) = item.select_layout(target, row_area, layout, overlay_bounds) {
            return Some(select);
        }
        y = y.saturating_add(height);
    }

    None
}

fn settings_select_layout_at_position(
    items: &[SettingsContentItem<'_>],
    position: Position,
    layout: SettingsFieldLayout,
    content_width: u16,
    content_height: u16,
) -> Option<SettingsSelectLayout> {
    let mut y = 0;
    for item in items {
        let height = item.height(layout, content_width);
        let row_area = Rect::new(0, y, content_width, height);
        let overlay_bounds = Rect::new(0, y, content_width, content_height.saturating_sub(y));
        if let Some(select) = item.select_layout(None, row_area, layout, overlay_bounds)
            && select.box_contains(position)
        {
            return Some(select);
        }
        y = y.saturating_add(height);
    }

    None
}

fn settings_content_items<'a>(
    popup: &'a SettingsPopup,
    table_max_height: u16,
) -> Vec<SettingsContentItem<'a>> {
    match popup.topic {
        SettingsTopic::Server => vec![setting_text_field(
            popup,
            0,
            "Proxy port",
            Cow::Owned(popup.draft().server.port.to_string()),
            FieldEditKind::ServerPort,
            Some(PORT_INPUT_WIDTH_COLS),
        )],
        SettingsTopic::Certificate => {
            let mut items = vec![setting_text_field(
                popup,
                0,
                "CA store dir",
                Cow::Borrowed(popup.draft().certificate.store_dir.as_str()),
                FieldEditKind::CertificateStoreDir,
                None,
            )];
            items.push(setting_text_field(
                popup,
                1,
                "CA PEM filename",
                Cow::Borrowed(popup.draft().certificate.pem_filename.as_str()),
                FieldEditKind::CertificatePemFilename,
                Some(PEM_FILENAME_INPUT_WIDTH_COLS),
            ));
            items
        }
        SettingsTopic::Recording => vec![checkbox_row(
            popup,
            0,
            popup.draft().recording.start_record_on_launch,
            "Start recording on launch",
        )],
        SettingsTopic::Interface => vec![checkbox_row(
            popup,
            0,
            popup.draft().ui.request_list.auto_expand,
            "Auto-expand request tree",
        )],
        SettingsTopic::Proxy => proxy_items(popup, table_max_height),
    }
}

const SETTING_TEXT_FIELD_HEIGHT: u16 = 3;
// Border columns plus the explicit leading/trailing spaces in the rendered textarea text.
const TEXT_INPUT_CHROME_WIDTH: u16 = 4;
const PORT_INPUT_WIDTH_COLS: u16 = 10;
const PEM_FILENAME_INPUT_WIDTH_COLS: u16 = 32;

fn setting_text_field<'a>(
    popup: &'a SettingsPopup,
    row: usize,
    label: &'static str,
    value: Cow<'a, str>,
    kind: FieldEditKind,
    fixed_edit_width_cols: Option<u16>,
) -> SettingsContentItem<'a> {
    let edit = popup.active_field_edit(kind);
    SettingsContentItem::Field {
        row,
        field: SettingsFieldRow::new(
            label,
            popup.focus == SettingsPaneFocus::Content && popup.selected_row == row,
            SettingsTextInputControl {
                value: edit.map_or(value, |edit| Cow::Borrowed(edit.value)),
                cursor: edit.map(|edit| edit.cursor),
                fixed_edit_width_cols,
                hint: popup.field_hint(kind),
            },
        ),
    }
}

struct SettingsTextInputControl<'a> {
    value: Cow<'a, str>,
    cursor: Option<usize>,
    fixed_edit_width_cols: Option<u16>,
    hint: Option<&'a str>,
}

impl SettingsTextInputControl<'_> {
    fn render_width(&self, available_width: u16) -> u16 {
        self.fixed_edit_width_cols
            .map(|cols| cols.saturating_add(TEXT_INPUT_CHROME_WIDTH))
            .unwrap_or(available_width)
            .min(available_width)
    }
}

impl SettingsControlView for SettingsTextInputControl<'_> {
    fn height_for_width(&self, _width: u16) -> u16 {
        SETTING_TEXT_FIELD_HEIGHT + u16::from(self.hint.is_some())
    }

    fn activity(&self) -> SettingsControlActivity {
        if self.cursor.is_some() {
            SettingsControlActivity::Active
        } else {
            SettingsControlActivity::Idle
        }
    }

    fn render(&self, area: Rect, style: SettingsFieldStyle, buf: &mut Buffer) {
        if area.is_empty() {
            return;
        }

        let field_width = self.render_width(area.width);
        if field_width < 3 || area.height < SETTING_TEXT_FIELD_HEIGHT {
            return;
        }

        let mut textarea = TextArea::new(vec![format!(" {} ", self.value.as_ref())]);
        textarea.set_style(style.control);
        textarea.set_block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(style.control),
        );
        textarea.set_cursor_line_style(Style::default());
        if let Some(cursor) = self.cursor {
            let cursor = u16::try_from(cursor.saturating_add(1)).unwrap_or(u16::MAX);
            textarea.move_cursor(CursorMove::Jump(0, cursor));
            textarea.set_cursor_style(style.control.add_modifier(Modifier::REVERSED));
        } else {
            textarea.set_cursor_style(Style::default());
        }
        (&textarea).render(
            Rect::new(area.x, area.y, field_width, SETTING_TEXT_FIELD_HEIGHT),
            buf,
        );
        if let Some(hint) = self.hint
            && area.height > SETTING_TEXT_FIELD_HEIGHT
        {
            Paragraph::new(Line::styled(hint, Style::default().fg(Color::Red))).render(
                Rect::new(
                    area.x,
                    area.y.saturating_add(SETTING_TEXT_FIELD_HEIGHT),
                    area.width,
                    1,
                ),
                buf,
            );
        }
    }
}

fn fit_input_value(value: &str, width: usize) -> String {
    let value_len = value.chars().count();
    if value_len <= width {
        return format!("{value:<width$}");
    }

    if width == 1 {
        return "…".to_string();
    }

    let mut clipped = value.chars().take(width - 1).collect::<String>();
    clipped.push('…');
    clipped
}

fn checkbox_row<'a>(
    popup: &SettingsPopup,
    row: usize,
    checked: bool,
    label: impl Into<Cow<'a, str>>,
) -> SettingsContentItem<'a> {
    let selected = popup.focus == SettingsPaneFocus::Content && popup.selected_row == row;
    SettingsContentItem::Field {
        row,
        field: SettingsFieldRow::new(label, selected, SettingsCheckboxControl { checked }),
    }
}

struct SettingsCheckboxControl {
    checked: bool,
}

impl SettingsControlView for SettingsCheckboxControl {
    fn height_for_width(&self, _width: u16) -> u16 {
        1
    }

    fn render(&self, area: Rect, style: SettingsFieldStyle, buf: &mut Buffer) {
        if area.is_empty() {
            return;
        }

        let mark = if self.checked { "[✓]" } else { "[ ]" };
        Paragraph::new(Line::styled(mark, style.control))
            .render(Rect::new(area.x, area.y, area.width.min(3), 1), buf);
    }
}

fn proxy_items(popup: &SettingsPopup, table_max_height: u16) -> Vec<SettingsContentItem<'_>> {
    let preset = active_preset(popup.draft());
    let widgets = popup.visible_proxy_widgets();
    if widgets.is_empty() {
        return vec![SettingsContentItem::Line(Line::from(
            "No proxy preset configured.",
        ))];
    }

    let mut items = Vec::with_capacity(widgets.len().saturating_add(2));
    for (row, widget) in widgets.iter().enumerate() {
        let item = match widget {
            ProxyWidget::Preset => proxy_preset_select(popup, row),
            ProxyWidget::PresetName => setting_text_field(
                popup,
                row,
                "Preset name",
                Cow::Borrowed(preset.map_or("", |preset| preset.name.as_str())),
                FieldEditKind::ProxyPresetName,
                None,
            ),
            ProxyWidget::MappingEnabled => checkbox_row(
                popup,
                row,
                popup
                    .draft()
                    .proxy
                    .as_ref()
                    .is_some_and(|proxy| proxy.enable),
                "Mapping enabled",
            ),
            ProxyWidget::MapRemoteEnabled => checkbox_row(
                popup,
                row,
                preset.is_some_and(|preset| preset.map_remote.enable),
                "Map remote enabled",
            ),
            ProxyWidget::RemoteRules => SettingsContentItem::FullWidthRuleTable {
                row,
                table: proxy_rule_table_widget(
                    popup,
                    ProxyRuleTable::Remote,
                    preset,
                    table_max_height,
                ),
            },
            ProxyWidget::MapLocalEnabled => checkbox_row(
                popup,
                row,
                preset.is_some_and(|preset| preset.map_local.enable),
                "Map local enabled",
            ),
            ProxyWidget::LocalRules => SettingsContentItem::FullWidthRuleTable {
                row,
                table: proxy_rule_table_widget(
                    popup,
                    ProxyRuleTable::Local,
                    preset,
                    table_max_height,
                ),
            },
        };
        items.push(item);

        match widget {
            ProxyWidget::MappingEnabled => items.push(SettingsContentItem::Divider {
                title: "Map Remote",
            }),
            ProxyWidget::RemoteRules => {
                items.push(SettingsContentItem::Divider { title: "Map Local" })
            }
            _ => {}
        }
    }

    items
}

fn proxy_preset_select(popup: &SettingsPopup, row: usize) -> SettingsContentItem<'_> {
    settings_select(popup, row, SelectTarget::ProxyPreset, "Preset")
}

fn settings_select<'a>(
    popup: &'a SettingsPopup,
    row: usize,
    target: SelectTarget,
    label: &'static str,
) -> SettingsContentItem<'a> {
    SettingsContentItem::Field {
        row,
        field: SettingsFieldRow::new(
            label,
            popup.select_is_selected(target),
            settings_select_control(popup, target),
        ),
    }
}

fn settings_select_control<'a>(
    popup: &'a SettingsPopup,
    target: SelectTarget,
) -> SettingsSelectControl<'a> {
    SettingsSelectControl {
        target,
        selected_label: popup.select_selected_label(target),
        items: popup.select_items(target),
        state: popup.select_state(target),
        max_visible_items: PROXY_PRESET_SELECT_MAX_VISIBLE_ITEMS,
    }
}

struct SettingsSelectControl<'a> {
    target: SelectTarget,
    selected_label: &'a str,
    items: Vec<SelectItem<'a, SettingsSelectId>>,
    state: Option<&'a SelectState>,
    max_visible_items: usize,
}

impl SettingsSelectControl<'_> {
    fn is_open(&self) -> bool {
        self.state.is_some_and(SelectState::is_open)
    }

    fn widget(&self, style: Style) -> SelectWidget<'_, SettingsSelectId> {
        SelectWidget::new(self.selected_label, &self.items, self.state)
            .with_style(style)
            .max_visible_items(self.max_visible_items)
            .with_min_width(24)
    }

    fn layout(
        &self,
        control_area: Rect,
        overlay_bounds: Rect,
    ) -> crate::select_widget::SelectWidgetLayout {
        self.widget(Style::default())
            .layout(select_overlay_area(control_area, overlay_bounds))
    }
}

impl SettingsControlView for SettingsSelectControl<'_> {
    fn height_for_width(&self, _width: u16) -> u16 {
        self.widget(Style::default()).height()
    }

    fn activity(&self) -> SettingsControlActivity {
        if self.is_open() {
            SettingsControlActivity::Active
        } else {
            SettingsControlActivity::Idle
        }
    }

    fn render(&self, area: Rect, style: SettingsFieldStyle, buf: &mut Buffer) {
        self.widget(style.control).render(area, buf);
    }

    fn has_overlay(&self) -> bool {
        self.is_open()
    }

    fn render_overlay(&self, control_area: Rect, overlay_bounds: Rect, buf: &mut Buffer) {
        self.widget(Style::default())
            .overlay()
            .render(select_overlay_area(control_area, overlay_bounds), buf);
    }

    fn select_layout(
        &self,
        control_area: Rect,
        overlay_bounds: Rect,
    ) -> Option<SettingsSelectLayout> {
        Some(SettingsSelectLayout {
            target: self.target,
            layout: self.layout(control_area, overlay_bounds),
        })
    }
}

fn select_overlay_area(control_area: Rect, overlay_bounds: Rect) -> Rect {
    Rect::new(
        control_area.x,
        control_area.y,
        control_area
            .width
            .min(overlay_bounds.right().saturating_sub(control_area.x)),
        overlay_bounds.bottom().saturating_sub(control_area.y),
    )
}

#[derive(Clone, Copy)]
struct ProxyRuleTableRow<'a> {
    enabled: bool,
    from: &'a str,
    to: &'a str,
}

enum ProxyRuleTableRows<'a> {
    Remote(&'a [crate::settings::ProxyMapRemoteRule]),
    Local(&'a [crate::settings::ProxyMapLocalRule]),
}

impl ProxyRuleTableRows<'_> {
    fn len(&self) -> usize {
        match self {
            Self::Remote(rows) => rows.len(),
            Self::Local(rows) => rows.len(),
        }
    }

    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn row(&self, index: usize) -> Option<ProxyRuleTableRow<'_>> {
        match self {
            Self::Remote(rows) => rows.get(index).map(|rule| ProxyRuleTableRow {
                enabled: rule.enable,
                from: rule.from.as_str(),
                to: rule.to.as_str(),
            }),
            Self::Local(rows) => rows.get(index).map(|rule| ProxyRuleTableRow {
                enabled: rule.enable,
                from: rule.from.as_str(),
                to: rule.to.as_str(),
            }),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SettingsTableViewport {
    visible_rows: usize,
    first_row: usize,
}

impl SettingsTableViewport {
    fn new(row_count: usize, scroll_offset: usize, height: u16) -> Self {
        let visible_rows = settings_table_visible_rows(height);
        let first_row = scroll_offset.min(row_count.saturating_sub(visible_rows));

        Self {
            visible_rows,
            first_row,
        }
    }

    fn overflowing(self, row_count: usize) -> bool {
        row_count > self.visible_rows
    }

    fn visible_range(self, row_count: usize) -> std::ops::Range<usize> {
        self.first_row
            ..self
                .first_row
                .saturating_add(self.visible_rows)
                .min(row_count)
    }

    fn row_y(self, area: Rect, row_index: usize) -> Option<u16> {
        if row_index < self.first_row
            || row_index >= self.first_row.saturating_add(self.visible_rows)
        {
            return None;
        }

        Some(
            area.y
                .saturating_add(2)
                .saturating_add(u16::try_from(row_index - self.first_row).unwrap_or(u16::MAX)),
        )
    }

    fn scrollbar_position(self, row_count: usize) -> usize {
        let max_scroll_offset = row_count.saturating_sub(self.visible_rows);
        if max_scroll_offset == 0 {
            return 0;
        }

        self.first_row
            .saturating_mul(row_count.saturating_sub(1))
            .div_ceil(max_scroll_offset)
    }
}

fn settings_table_height(row_count: usize, max_height: u16) -> u16 {
    let natural_height = 3u16.saturating_add(u16::try_from(row_count.max(1)).unwrap_or(u16::MAX));
    natural_height.min(max_height.max(SETTINGS_TABLE_MIN_HEIGHT))
}

fn settings_table_visible_rows(height: u16) -> usize {
    usize::from(height.saturating_sub(3)).max(1)
}

struct ProxyRuleTableWidget<'a> {
    table: ProxyRuleTable,
    rows: Option<ProxyRuleTableRows<'a>>,
    selected: bool,
    active_rule: Option<usize>,
    scroll_offset: usize,
    max_height: u16,
}

impl ProxyRuleTableWidget<'_> {
    fn height(&self) -> u16 {
        settings_table_height(self.row_count(), self.max_height)
    }

    fn row_count(&self) -> usize {
        self.rows.as_ref().map_or(0, ProxyRuleTableRows::len)
    }

    fn is_empty(&self) -> bool {
        self.rows.as_ref().is_none_or(ProxyRuleTableRows::is_empty)
    }

    fn active_rule_area(&self, area: Rect) -> Option<Rect> {
        let active_rule = self.active_rule?;
        if active_rule >= self.row_count() {
            return None;
        }

        let y = self.viewport(area.height).row_y(area, active_rule)?;

        Some(Rect::new(area.x, y, area.width, 1))
    }

    fn viewport(&self, height: u16) -> SettingsTableViewport {
        SettingsTableViewport::new(self.row_count(), self.scroll_offset, height)
    }
}

impl Widget for ProxyRuleTableWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        self.render_into(area, buf);
    }
}

impl Widget for &ProxyRuleTableWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        self.render_into(area, buf);
    }
}

impl ProxyRuleTableWidget<'_> {
    fn render_into(&self, area: Rect, buf: &mut Buffer) {
        if area.is_empty() {
            return;
        }

        let border_style = if self.selected {
            Style::default().fg(Color::Green)
        } else {
            Style::default()
        };
        let block = Block::default()
            .title(format!(" {} ", self.table.title()))
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(border_style);
        let inner = block.inner(area);
        block.render(area, buf);
        if inner.is_empty() {
            return;
        }

        let viewport = self.viewport(area.height);
        let row_count = self.row_count();
        let overflowing = viewport.overflowing(row_count);
        let table_width = if overflowing {
            inner.width.saturating_sub(1).max(1)
        } else {
            inner.width
        };

        let header = format_rule_table_header(table_width);
        Paragraph::new(Line::styled(header, Style::default().fg(Color::DarkGray)))
            .render(Rect::new(inner.x, inner.y, table_width, 1), buf);

        if self.is_empty() {
            let empty = fit_input_value("No rules configured", usize::from(table_width));
            Paragraph::new(Line::raw(empty)).render(
                Rect::new(inner.x, inner.y.saturating_add(1), table_width, 1),
                buf,
            );
            return;
        }

        let Some(rows) = &self.rows else {
            return;
        };
        for index in viewport.visible_range(rows.len()) {
            let y = inner
                .y
                .saturating_add(1)
                .saturating_add(u16::try_from(index - viewport.first_row).unwrap_or(u16::MAX));
            if y >= inner.bottom() {
                break;
            }
            let style = if self.active_rule == Some(index) {
                Style::default().bg(Color::White).fg(Color::DarkGray)
            } else {
                Style::default()
            };
            let Some(row) = rows.row(index) else {
                continue;
            };
            let line = format_rule_table_row(&row, table_width);
            Paragraph::new(Line::styled(line, style))
                .render(Rect::new(inner.x, y, table_width, 1), buf);
        }

        if overflowing {
            let scrollbar_area = Rect::new(
                inner.x,
                inner.y.saturating_add(1),
                inner.width,
                u16::try_from(viewport.visible_rows).unwrap_or(u16::MAX),
            );
            let mut scrollbar = ScrollbarState::new(row_count)
                .position(viewport.scrollbar_position(row_count))
                .viewport_content_length(viewport.visible_rows);
            StatefulWidget::render(
                Scrollbar::default()
                    .orientation(ScrollbarOrientation::VerticalRight)
                    .begin_symbol(None)
                    .end_symbol(None),
                scrollbar_area,
                buf,
                &mut scrollbar,
            );
        }
    }
}

fn proxy_rule_table_widget<'a>(
    popup: &SettingsPopup,
    table: ProxyRuleTable,
    preset: Option<&'a crate::settings::ProxyPresetSettings>,
    max_height: u16,
) -> ProxyRuleTableWidget<'a> {
    let rows = preset.map(|preset| match table {
        ProxyRuleTable::Remote => ProxyRuleTableRows::Remote(&preset.map_remote.rules),
        ProxyRuleTable::Local => ProxyRuleTableRows::Local(&preset.map_local.rules),
    });

    ProxyRuleTableWidget {
        table,
        rows,
        selected: popup.proxy_table_is_selected(table),
        active_rule: popup.active_proxy_table_rule(table),
        scroll_offset: popup.rule_table_scroll_offset(table),
        max_height,
    }
}

fn format_rule_table_header(content_width: u16) -> String {
    let (from_width, to_width) = rule_table_column_widths(content_width);
    format!("{:<4} {:<from_width$} {:<to_width$}", "On", "From", "To")
}

fn format_rule_table_row(row: &ProxyRuleTableRow<'_>, content_width: u16) -> String {
    let (from_width, to_width) = rule_table_column_widths(content_width);
    let mark = if row.enabled { "[✓]" } else { "[ ]" };
    format!(
        "{:<4} {} {}",
        mark,
        fit_input_value(row.from, from_width),
        fit_input_value(row.to, to_width)
    )
}

fn rule_table_column_widths(content_width: u16) -> (usize, usize) {
    let available = usize::from(content_width).saturating_sub(8);
    let width = (available / 2).max(1);

    (width, width)
}

fn active_preset(
    settings: &crate::settings::AppSettings,
) -> Option<&crate::settings::ProxyPresetSettings> {
    let proxy = settings.proxy.as_ref()?;
    let active = proxy.active_preset.as_deref()?;
    proxy.presets.iter().find(|preset| preset.name == active)
}

fn render_rule_editor_popup(frame: &mut Frame, editor: RuleEditorState<'_>, parent: Rect) {
    let area = rule_editor_area(parent);
    frame.render_widget(Clear, area);
    let mut block = Block::default()
        .title(editor.title)
        .title_alignment(Alignment::Center)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Green));
    let key_hint_text = settings_key_hint_text(RULE_EDITOR_KEY_HINTS, area.width.saturating_sub(2));
    if !key_hint_text.is_empty() {
        block = block.title_bottom(Line::from(key_hint_text).alignment(Alignment::Center));
    }
    frame.render_widget(block, area);

    let inner = area.inner(Margin {
        vertical: 1,
        horizontal: 1,
    });
    if inner.height < SETTING_TEXT_FIELD_HEIGHT.saturating_mul(2) || inner.width == 0 {
        return;
    }

    let from_selected = editor.active_field == RuleEditField::From;
    let to_selected = editor.active_field == RuleEditField::To;
    let field_width = inner.width.saturating_sub(2);
    let rows = [
        SettingsFieldRow::new(
            "From",
            from_selected,
            SettingsTextInputControl {
                value: Cow::Borrowed(editor.from),
                cursor: from_selected.then_some(editor.from_cursor),
                fixed_edit_width_cols: None,
                hint: None,
            },
        ),
        SettingsFieldRow::new(
            "To",
            to_selected,
            SettingsTextInputControl {
                value: Cow::Borrowed(editor.to),
                cursor: to_selected.then_some(editor.to_cursor),
                fixed_edit_width_cols: None,
                hint: None,
            },
        ),
    ];
    let field_layout = settings_field_layout_for_rows(&rows, SettingsFieldLayoutConfig::default());

    for (index, row) in rows.iter().enumerate() {
        frame.render_widget(
            SettingsFieldRowWidget {
                row,
                layout: field_layout,
            },
            Rect::new(
                inner.x.saturating_add(1),
                inner.y.saturating_add(
                    SETTING_TEXT_FIELD_HEIGHT
                        .saturating_mul(u16::try_from(index).unwrap_or(u16::MAX)),
                ),
                field_width,
                SETTING_TEXT_FIELD_HEIGHT,
            ),
        );
    }
}

fn rule_editor_area(parent: Rect) -> Rect {
    let max_width = parent.width.saturating_sub(4).max(1);
    let min_width = 44.min(max_width);
    let width = max_width.min(76).max(min_width);
    let height = 11.min(parent.height.saturating_sub(2)).max(9);

    centered_rect(width, height, parent)
}

fn render_action_dialog(frame: &mut Frame, dialog: &ActionDialog, parent: Rect) {
    let button_width = action_buttons_full_width(dialog).saturating_add(2);
    let width = 58
        .min(parent.width.saturating_sub(4))
        .max(32)
        .max(button_width);
    let height = 9.min(parent.height.saturating_sub(2)).max(7);
    let area = centered_rect(width, height, parent);
    frame.render_widget(Clear, area);

    frame.render_widget(
        Block::default()
            .title(dialog.title)
            .title_alignment(Alignment::Center)
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(Color::Green)),
        area,
    );

    let inner = area.inner(Margin {
        vertical: 1,
        horizontal: 1,
    });
    if inner.is_empty() {
        return;
    }

    let button_height = 3.min(inner.height);
    let button_area = Rect::new(
        inner.x,
        inner.bottom().saturating_sub(button_height),
        inner.width,
        button_height,
    );
    render_action_buttons(frame, dialog, button_area);

    let message_height = button_area.y.saturating_sub(inner.y);
    let line_count = u16::try_from(dialog.message_lines.len())
        .unwrap_or(u16::MAX)
        .min(message_height);
    if line_count == 0 {
        return;
    }

    let message_y = inner
        .y
        .saturating_add(message_height.saturating_sub(line_count).div_ceil(2));
    let lines = dialog
        .message_lines
        .iter()
        .take(usize::from(line_count))
        .map(|line| Line::from(*line).alignment(Alignment::Center))
        .collect::<Vec<_>>();
    frame.render_widget(
        Paragraph::new(lines).alignment(Alignment::Center),
        Rect::new(inner.x, message_y, inner.width, line_count),
    );
}

fn render_action_buttons(frame: &mut Frame, dialog: &ActionDialog, area: Rect) {
    if area.height < 3 || dialog.actions.is_empty() {
        return;
    }

    let labels = dialog
        .actions
        .iter()
        .map(|action| format!("{} [{}]", action.label, action.key_hint))
        .collect::<Vec<_>>();
    let mut widths = labels
        .iter()
        .map(|label| text_width(label).saturating_add(4))
        .collect::<Vec<_>>();
    let mut gap = 2;
    let mut total_width = action_button_width_sum(&widths, gap);
    if total_width > area.width {
        gap = u16::from(labels.len() > 1 && area.width >= labels.len() as u16 * 4);
        let gap_width =
            gap.saturating_mul(u16::try_from(labels.len().saturating_sub(1)).unwrap_or(u16::MAX));
        let button_width = area.width.saturating_sub(gap_width);
        let button_count = u16::try_from(labels.len()).unwrap_or(u16::MAX);
        if button_count == 0 || button_width < button_count.saturating_mul(3) {
            return;
        }

        let base_width = button_width / button_count;
        let extra_width = button_width % button_count;
        widths = (0..button_count)
            .map(|index| base_width + u16::from(index < extra_width))
            .collect();
        total_width = action_button_width_sum(&widths, gap);
    }
    let mut x = area.x + area.width.saturating_sub(total_width) / 2;

    for (label, width) in labels.iter().zip(widths) {
        let button_area = Rect::new(x, area.y, width, 3);
        frame.render_widget(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded),
            button_area,
        );
        let label_area = button_area.inner(Margin {
            vertical: 1,
            horizontal: 1,
        });
        let label = if text_width(label) > label_area.width {
            fit_input_value(label, usize::from(label_area.width))
        } else {
            label.clone()
        };
        frame.render_widget(
            Paragraph::new(label).alignment(Alignment::Center),
            label_area,
        );
        x = x.saturating_add(width).saturating_add(gap);
    }
}

fn action_buttons_full_width(dialog: &ActionDialog) -> u16 {
    let widths = dialog
        .actions
        .iter()
        .map(|action| {
            text_width(action.label)
                .saturating_add(text_width(action.key_hint))
                .saturating_add(7)
        })
        .collect::<Vec<_>>();

    action_button_width_sum(&widths, 2)
}

fn action_button_width_sum(widths: &[u16], gap: u16) -> u16 {
    widths.iter().copied().sum::<u16>().saturating_add(
        gap.saturating_mul(u16::try_from(widths.len().saturating_sub(1)).unwrap_or(u16::MAX)),
    )
}

fn handle_settings_popup_mouse(mouse: MouseEvent, app: &mut App, root_area: Rect) -> bool {
    if handle_proxy_preset_select_mouse(mouse, app, root_area) {
        return true;
    }

    match mouse.kind {
        MouseEventKind::ScrollDown | MouseEventKind::ScrollUp => {
            handle_settings_content_scroll_mouse(mouse, app, root_area);
            true
        }
        _ => true,
    }
}

fn handle_settings_content_scroll_mouse(mouse: MouseEvent, app: &mut App, root_area: Rect) {
    let scroll_down = matches!(mouse.kind, MouseEventKind::ScrollDown);
    let layout = settings_popup_layout(root_area);
    let viewport = layout.content_viewport;
    if viewport.is_empty() {
        app.settings_popup.scroll.set_offset(Position::ORIGIN);
        return;
    }

    let (table_hit, scrolling_enabled) = {
        let position = Position::new(mouse.column, mouse.row);
        let table_max_height = settings_rule_table_max_height(viewport.height);
        let items = settings_content_items_with_error(&app.settings_popup, table_max_height);
        let field_layout = settings_field_layout(&items);
        let content_width = settings_content_width_for_viewport(
            &items,
            field_layout,
            viewport.width,
            viewport.height,
        );
        let content_layout =
            SettingsContentLayout::new(&items, field_layout, content_width, viewport.height);
        let scrolling_enabled = content_layout.scrolling_enabled;

        let table_hit = if app.settings_popup.topic == SettingsTopic::Proxy
            && app.settings_popup.active_select_target().is_none()
            && app.settings_popup.unsaved_dialog().is_none()
            && app.settings_popup.rule_editor().is_none()
            && viewport.contains(position)
        {
            let scroll_y = if scrolling_enabled {
                app.settings_popup.scroll.offset().y
            } else {
                0
            };
            let local_position = Position::new(
                mouse.column.saturating_sub(viewport.x),
                mouse
                    .row
                    .saturating_sub(viewport.y)
                    .saturating_add(scroll_y),
            );
            content_layout.item_areas().find_map(|(item, area)| {
                let SettingsContentItem::FullWidthRuleTable { table, .. } = item else {
                    return None;
                };
                let viewport = table.viewport(area.height);
                (area.contains(local_position) && viewport.overflowing(table.row_count()))
                    .then_some(table.table)
            })
        } else {
            None
        };

        (table_hit, scrolling_enabled)
    };

    if let Some(table) = table_hit {
        let changed = if scroll_down {
            app.settings_popup.scroll_rule_table_down(table)
        } else {
            app.settings_popup.scroll_rule_table_up(table)
        };
        if changed {
            return;
        }
    }

    if scrolling_enabled {
        if scroll_down {
            app.settings_popup.scroll.scroll_down();
        } else {
            app.settings_popup.scroll.scroll_up();
        }
    } else {
        app.settings_popup.scroll.set_offset(Position::ORIGIN);
    }
}

fn handle_proxy_preset_select_mouse(mouse: MouseEvent, app: &mut App, root_area: Rect) -> bool {
    if app.settings_popup.topic != SettingsTopic::Proxy
        || app.settings_popup.unsaved_dialog().is_some()
        || app.settings_popup.rule_editor().is_some()
    {
        return false;
    }

    let layout = settings_popup_layout(root_area);
    let viewport = layout.content_viewport;
    let position = Position::new(mouse.column, mouse.row);
    let select_open = app.settings_popup.active_select_target().is_some();
    let needs_select_layout = match mouse.kind {
        MouseEventKind::Down(_) => true,
        MouseEventKind::ScrollDown | MouseEventKind::ScrollUp => select_open,
        _ => false,
    };
    if !needs_select_layout {
        return false;
    }

    if !viewport.contains(position) {
        if select_open && matches!(mouse.kind, MouseEventKind::Down(_)) {
            app.settings_popup.close_active_select();
            return true;
        }
        return false;
    }

    let table_max_height = settings_rule_table_max_height(viewport.height);
    let items = settings_content_items_with_error(&app.settings_popup, table_max_height);
    let field_layout = settings_field_layout(&items);
    let content_width =
        settings_content_width_for_viewport(&items, field_layout, viewport.width, viewport.height);
    let content_layout =
        SettingsContentLayout::new(&items, field_layout, content_width, viewport.height);
    let content_height = content_layout.buffer_height;
    let scroll_y = if content_layout.scrolling_enabled {
        app.settings_popup.scroll.offset().y
    } else {
        0
    };
    let local_position = Position::new(
        mouse.column.saturating_sub(viewport.x),
        mouse
            .row
            .saturating_sub(viewport.y)
            .saturating_add(scroll_y),
    );
    let active_select = app
        .settings_popup
        .active_select_target()
        .and_then(|target| {
            settings_select_layout(
                &items,
                Some(target),
                field_layout,
                content_width,
                content_height,
            )
        });
    let clicked_select = settings_select_layout_at_position(
        &items,
        local_position,
        field_layout,
        content_width,
        content_height,
    );
    let box_hit = active_select.is_some_and(|select| select.box_contains(local_position));
    let dropdown_hit = active_select.is_some_and(|select| select.dropdown_contains(local_position));
    let option_hit = active_select.and_then(|select| select.layout.option_at(local_position));
    let clicked_select_target = clicked_select.map(|select| select.target);
    drop(items);

    match mouse.kind {
        MouseEventKind::ScrollDown if dropdown_hit => {
            app.settings_popup.scroll_active_select_down();
            true
        }
        MouseEventKind::ScrollUp if dropdown_hit => {
            app.settings_popup.scroll_active_select_up();
            true
        }
        MouseEventKind::Down(MouseButton::Left) if box_hit && select_open => true,
        MouseEventKind::Down(MouseButton::Left) if clicked_select_target.is_some() => {
            if let Some(target) = clicked_select_target {
                app.settings_popup.start_select(target);
            }
            true
        }
        MouseEventKind::Down(MouseButton::Left) if option_hit.is_some() => {
            if let Some(filtered_index) = option_hit {
                app.settings_popup
                    .commit_active_select_filtered_index(filtered_index);
            }
            true
        }
        MouseEventKind::Down(MouseButton::Left) if dropdown_hit => true,
        MouseEventKind::Down(_) if select_open => {
            app.settings_popup.close_active_select();
            true
        }
        _ => false,
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
    Text(&'static str),
    Body(BodyViewerKey),
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
    if app.detail_panel.active_tab.is_body() {
        return app
            .current_body_viewer_key()
            .map(DetailContent::Body)
            .unwrap_or(DetailContent::Text("Select a request"));
    }

    let Some(req) = app.selected_request() else {
        return DetailContent::Text("Select a request");
    };

    match app.detail_panel.active_tab {
        MainDisplayTab::RequestHeader => DetailContent::Table(request_header_rows(req)),
        MainDisplayTab::ResponseHeader => DetailContent::Table(response_header_rows(req)),
        MainDisplayTab::RequestBody | MainDisplayTab::ResponseBody => unreachable!(),
    }
}

fn request_header_rows(req: &crate::capture::CapturedExchange) -> Vec<TableRow> {
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

fn response_header_rows(req: &crate::capture::CapturedExchange) -> Vec<TableRow> {
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

fn request_header_row_refs(req: &crate::capture::CapturedExchange) -> Vec<HeaderTableRowRef<'_>> {
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

fn response_header_row_refs(req: &crate::capture::CapturedExchange) -> Vec<HeaderTableRowRef<'_>> {
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
