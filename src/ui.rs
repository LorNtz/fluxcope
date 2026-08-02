use crate::app::{
    ActionDialog, App, FieldEditKind, PROXY_PRESET_SELECT_MAX_VISIBLE_ITEMS, PanelFocus,
    PopupFocus, ProxyRuleTable, ProxyWidget, RULE_EDITOR_KEY_HINTS, RecordingWidget,
    RequestTreeNodeSnapshot, RuleEditField, RuleEditorState, SelectTarget, SettingsKeyHint,
    SettingsPaneFocus, SettingsPopup, SettingsScrollRequest, SettingsSelectId, SettingsTopic,
};
#[cfg(test)]
use crate::app::{BODY_LOADING_TEXT, BodyViewerKey, MainDisplayTab, ProxyRow};
use crate::select::{SelectItem, SelectState};
use crate::select_widget::SelectWidget;
use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
#[cfg(test)]
use ratatui::symbols;
use ratatui::{
    Frame,
    buffer::Buffer,
    layout::{Alignment, Constraint, Direction, Layout, Margin, Position, Rect, Size},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, BorderType, Borders, Clear, Paragraph, Scrollbar, ScrollbarOrientation,
        ScrollbarState, StatefulWidget, Widget,
    },
};
use std::borrow::Cow;
use tui_scrollview::{ScrollView, ScrollbarVisibility};
use tui_textarea::{CursorMove, TextArea};

mod certificate_popup;
use certificate_popup::render_certificate_popup;
mod chrome;
use chrome::{base_panel_block, centered_rect, panel_block};
mod detail;
use detail::DetailView;
#[cfg(test)]
use detail::{body_editor_text_area, detail_content_area};
mod log_view;
use log_view::LogView;
mod recording_settings;
use recording_settings::{PrefilterTableWidget, prefilter_table_widget};
mod request_list;
use request_list::RequestListView;
#[cfg(test)]
use request_list::build_request_tree_items;
mod terminal_text;
use terminal_text::text_width;

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
            Span::styled(
                format!("  •  {} captures", app.capture_count()),
                Style::default().fg(Color::DarkGray),
            ),
        ]);

        frame.render_widget(
            Paragraph::new(status)
                .block(panel_block("Status", false))
                .alignment(Alignment::Left),
            self.area(),
        );
    }
}

#[cfg(test)]
mod tests;

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

fn settings_popup_area(area: Rect) -> Rect {
    let width = area.width.saturating_sub(4).clamp(40, 96);
    let height = area.height.saturating_sub(4).clamp(12, 28);
    centered_rect(width, height, area)
}

const SETTINGS_TABLE_MAX_HEIGHT_PERCENT: u16 = 50;
const SETTINGS_TABLE_MIN_HEIGHT: u16 = 4;

fn settings_table_max_height(viewport_height: u16) -> u16 {
    let proportional = u16::try_from(
        u32::from(viewport_height) * u32::from(SETTINGS_TABLE_MAX_HEIGHT_PERCENT) / 100,
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
    let table_max_height = settings_table_max_height(area.height);
    match popup.topic {
        SettingsTopic::Recording => sync_prefilter_table_view(popup, table_max_height),
        SettingsTopic::Proxy => sync_settings_rule_table_views(popup, table_max_height),
        _ => {}
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

fn sync_prefilter_table_view(popup: &mut SettingsPopup, table_max_height: u16) {
    let row_count = popup.prefilter_pattern_count();
    let height = settings_table_height(row_count, table_max_height);
    popup.sync_prefilter_table_view(settings_table_visible_rows(height));
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

enum SettingsFullWidthTable<'a> {
    Proxy(ProxyRuleTableWidget<'a>),
    Prefilter(PrefilterTableWidget<'a>),
}

impl SettingsFullWidthTable<'_> {
    fn height(&self) -> u16 {
        match self {
            Self::Proxy(table) => table.height(),
            Self::Prefilter(table) => table.height(),
        }
    }

    fn render(&self, scroll_view: &mut ScrollView, area: Rect) {
        match self {
            Self::Proxy(table) => scroll_view.render_widget(table, area),
            Self::Prefilter(table) => scroll_view.render_widget(table, area),
        }
    }

    fn focused_area(&self, area: Rect) -> Rect {
        match self {
            Self::Proxy(table) => table.active_rule_area(area).unwrap_or(area),
            Self::Prefilter(table) => table.active_pattern_area(area).unwrap_or(area),
        }
    }

    fn scroll_hit(&self, area: Rect, position: Position) -> Option<SettingsTableHit> {
        match self {
            Self::Proxy(table) => {
                let viewport = table.viewport(area.height);
                (area.contains(position) && viewport.overflowing(table.row_count()))
                    .then_some(SettingsTableHit::Proxy(table.table))
            }
            Self::Prefilter(table) => {
                let viewport = table.viewport(area.height);
                (area.contains(position) && viewport.overflowing(table.row_count()))
                    .then_some(SettingsTableHit::Prefilter)
            }
        }
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
    FullWidthTable {
        row: usize,
        table: SettingsFullWidthTable<'a>,
    },
}

impl SettingsContentItem<'_> {
    fn height(&self, layout: SettingsFieldLayout, width: u16) -> u16 {
        match self {
            Self::Line(_) => 1,
            Self::Divider { .. } => SETTINGS_DIVIDER_HEIGHT,
            Self::Field { field, .. } => field.height(layout, width),
            Self::FullWidthTable { table, .. } => table.height(),
        }
    }

    fn render(&self, scroll_view: &mut ScrollView, area: Rect, layout: SettingsFieldLayout) {
        match self {
            Self::Line(line) => scroll_view.render_widget(Paragraph::new(line.clone()), area),
            Self::Divider { title } => scroll_view.render_widget(SettingsDivider { title }, area),
            Self::Field { field, .. } => {
                scroll_view.render_widget(SettingsFieldRowWidget { row: field, layout }, area)
            }
            Self::FullWidthTable { table, .. } => table.render(scroll_view, area),
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
            Self::Line(_) | Self::Divider { .. } | Self::FullWidthTable { .. } => None,
        }
    }

    fn focus_row(&self) -> Option<usize> {
        match self {
            Self::Field { row, .. } | Self::FullWidthTable { row, .. } => Some(*row),
            Self::Line(_) | Self::Divider { .. } => None,
        }
    }

    fn focused_area(&self, area: Rect) -> Rect {
        match self {
            Self::FullWidthTable { table, .. } => table.focused_area(area),
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

fn inspect_settings_content_at_mouse<R>(
    popup: &SettingsPopup,
    viewport: Rect,
    mouse: MouseEvent,
    inspect: impl FnOnce(&SettingsContentLayout<'_, '_>, Position) -> R,
) -> R {
    let table_max_height = settings_table_max_height(viewport.height);
    let items = settings_content_items_with_error(popup, table_max_height);
    let field_layout = settings_field_layout(&items);
    let content_width =
        settings_content_width_for_viewport(&items, field_layout, viewport.width, viewport.height);
    let content_layout =
        SettingsContentLayout::new(&items, field_layout, content_width, viewport.height);
    let scroll_y = if content_layout.scrolling_enabled {
        popup.scroll.offset().y
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
    inspect(&content_layout, local_position)
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
        SettingsTopic::Recording => vec![
            checkbox_row(
                popup,
                RecordingWidget::StartRecordingOnLaunch.row(),
                popup.draft().recording.start_record_on_launch,
                "Start recording on launch",
            ),
            SettingsContentItem::Divider {
                title: "URL Prefilter",
            },
            checkbox_row(
                popup,
                RecordingWidget::PrefilterEnabled.row(),
                popup.draft().recording.prefilter.enable,
                "URL prefilter enabled",
            ),
            SettingsContentItem::FullWidthTable {
                row: RecordingWidget::IncludeUrlPatterns.row(),
                table: SettingsFullWidthTable::Prefilter(prefilter_table_widget(
                    popup,
                    table_max_height,
                )),
            },
        ],
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
            ProxyWidget::RemoteRules => SettingsContentItem::FullWidthTable {
                row,
                table: SettingsFullWidthTable::Proxy(proxy_rule_table_widget(
                    popup,
                    ProxyRuleTable::Remote,
                    preset,
                    table_max_height,
                )),
            },
            ProxyWidget::MapLocalEnabled => checkbox_row(
                popup,
                row,
                preset.is_some_and(|preset| preset.map_local.enable),
                "Map local enabled",
            ),
            ProxyWidget::LocalRules => SettingsContentItem::FullWidthTable {
                row,
                table: SettingsFullWidthTable::Proxy(proxy_rule_table_widget(
                    popup,
                    ProxyRuleTable::Local,
                    preset,
                    table_max_height,
                )),
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RuleParentState {
    Enabled,
    Disabled,
}

impl RuleParentState {
    fn from_enabled(enabled: bool) -> Self {
        if enabled {
            Self::Enabled
        } else {
            Self::Disabled
        }
    }

    fn checkbox_style(self, row_style: Style, row_selected: bool) -> Style {
        match (self, row_selected) {
            (Self::Enabled, _) => row_style,
            (Self::Disabled, true) => row_style.fg(Color::Gray),
            (Self::Disabled, false) => row_style.fg(Color::DarkGray),
        }
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
    parent_state: RuleParentState,
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
            let row_selected = self.active_rule == Some(index);
            let row_style = if row_selected {
                Style::default().bg(Color::White).fg(Color::DarkGray)
            } else {
                Style::default()
            };
            let Some(row) = rows.row(index) else {
                continue;
            };
            let line = format_rule_table_row(
                &row,
                table_width,
                row_style,
                self.parent_state,
                row_selected,
            );
            Paragraph::new(line).render(Rect::new(inner.x, y, table_width, 1), buf);
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
    let mapping_enabled = popup
        .draft()
        .proxy
        .as_ref()
        .is_some_and(|proxy| proxy.enable);
    let section_enabled = preset.is_some_and(|preset| match table {
        ProxyRuleTable::Remote => preset.map_remote.enable,
        ProxyRuleTable::Local => preset.map_local.enable,
    });
    let rows = preset.map(|preset| match table {
        ProxyRuleTable::Remote => ProxyRuleTableRows::Remote(&preset.map_remote.rules),
        ProxyRuleTable::Local => ProxyRuleTableRows::Local(&preset.map_local.rules),
    });

    ProxyRuleTableWidget {
        table,
        rows,
        parent_state: RuleParentState::from_enabled(mapping_enabled && section_enabled),
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

fn format_rule_table_row(
    row: &ProxyRuleTableRow<'_>,
    content_width: u16,
    row_style: Style,
    parent_state: RuleParentState,
    row_selected: bool,
) -> Line<'static> {
    let (from_width, to_width) = rule_table_column_widths(content_width);
    let mark = if row.enabled { "[✓] " } else { "[ ] " };
    Line::from(vec![
        Span::styled(mark, parent_state.checkbox_style(row_style, row_selected)),
        Span::styled(" ", row_style),
        Span::styled(fit_input_value(row.from, from_width), row_style),
        Span::styled(" ", row_style),
        Span::styled(fit_input_value(row.to, to_width), row_style),
    ])
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

fn action_dialog_area(dialog: &ActionDialog, parent: Rect) -> Rect {
    let button_width = action_buttons_full_width(dialog).saturating_add(2);
    let width = 58
        .min(parent.width.saturating_sub(4))
        .max(32)
        .max(button_width);
    let height = 9.min(parent.height.saturating_sub(2)).max(7);
    centered_rect(width, height, parent)
}

fn render_action_dialog(frame: &mut Frame, dialog: &ActionDialog, parent: Rect) {
    let area = action_dialog_area(dialog, parent);
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
    if handle_prefilter_table_mouse(mouse, app, root_area) {
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

#[derive(Clone, Copy)]
enum SettingsTableHit {
    Prefilter,
    Proxy(ProxyRuleTable),
}

fn handle_settings_content_scroll_mouse(mouse: MouseEvent, app: &mut App, root_area: Rect) {
    let scroll_down = matches!(mouse.kind, MouseEventKind::ScrollDown);
    let layout = settings_popup_layout(root_area);
    let viewport = layout.content_viewport;
    if viewport.is_empty() {
        app.settings_popup.scroll.set_offset(Position::ORIGIN);
        return;
    }

    let position = Position::new(mouse.column, mouse.row);
    let (table_hit, scrolling_enabled) = inspect_settings_content_at_mouse(
        &app.settings_popup,
        viewport,
        mouse,
        |content_layout, local_position| {
            let scrolling_enabled = content_layout.scrolling_enabled;

            let table_hit = if matches!(
                app.settings_popup.topic,
                SettingsTopic::Recording | SettingsTopic::Proxy
            ) && app.settings_popup.active_select_target().is_none()
                && app.settings_popup.unsaved_dialog().is_none()
                && app.settings_popup.rule_editor().is_none()
                && viewport.contains(position)
            {
                content_layout.item_areas().find_map(|(item, area)| {
                    let SettingsContentItem::FullWidthTable { table, .. } = item else {
                        return None;
                    };
                    table.scroll_hit(area, local_position)
                })
            } else {
                None
            };

            (table_hit, scrolling_enabled)
        },
    );

    if let Some(table) = table_hit {
        let changed = match (table, scroll_down) {
            (SettingsTableHit::Prefilter, true) => app.settings_popup.scroll_prefilter_table_down(),
            (SettingsTableHit::Prefilter, false) => app.settings_popup.scroll_prefilter_table_up(),
            (SettingsTableHit::Proxy(table), true) => {
                app.settings_popup.scroll_rule_table_down(table)
            }
            (SettingsTableHit::Proxy(table), false) => {
                app.settings_popup.scroll_rule_table_up(table)
            }
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

fn handle_prefilter_table_mouse(mouse: MouseEvent, app: &mut App, root_area: Rect) -> bool {
    if !matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left))
        || app.settings_popup.topic != SettingsTopic::Recording
        || app.settings_popup.unsaved_dialog().is_some()
        || app.settings_popup.prefilter_pattern_edit().is_some()
    {
        return false;
    }

    let layout = settings_popup_layout(root_area);
    let viewport = layout.content_viewport;
    let position = Position::new(mouse.column, mouse.row);
    if viewport.is_empty() || !viewport.contains(position) {
        return false;
    }

    let hit = inspect_settings_content_at_mouse(
        &app.settings_popup,
        viewport,
        mouse,
        |content_layout, local_position| {
            content_layout.item_areas().find_map(|(item, area)| {
                let SettingsContentItem::FullWidthTable {
                    table: SettingsFullWidthTable::Prefilter(table),
                    ..
                } = item
                else {
                    return None;
                };
                if !area.contains(local_position) {
                    return None;
                }
                table.pattern_hit(area, local_position)
            })
        },
    );

    if let Some((index, toggle)) = hit {
        app.settings_popup.select_prefilter_pattern(index);
        if toggle {
            app.settings_popup.toggle_prefilter_pattern(index);
        }
        return true;
    }
    false
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

    let (active_select, clicked_select, local_position) = inspect_settings_content_at_mouse(
        &app.settings_popup,
        viewport,
        mouse,
        |content_layout, local_position| {
            let active_select = app
                .settings_popup
                .active_select_target()
                .and_then(|target| {
                    settings_select_layout(
                        content_layout.items,
                        Some(target),
                        content_layout.field_layout,
                        content_layout.content_width,
                        content_layout.buffer_height,
                    )
                });
            let clicked_select = settings_select_layout_at_position(
                content_layout.items,
                local_position,
                content_layout.field_layout,
                content_layout.content_width,
                content_layout.buffer_height,
            );
            (active_select, clicked_select, local_position)
        },
    );
    let box_hit = active_select.is_some_and(|select| select.box_contains(local_position));
    let dropdown_hit = active_select.is_some_and(|select| select.dropdown_contains(local_position));
    let option_hit = active_select.and_then(|select| select.layout.option_at(local_position));
    let clicked_select_target = clicked_select.map(|select| select.target);
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
