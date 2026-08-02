use std::borrow::Cow;

use ratatui::{
    buffer::Buffer,
    layout::{Position, Rect},
    style::{Color, Style},
    text::Line,
    widgets::{Paragraph, Widget},
};
use tui_scrollview::ScrollView;

use crate::app::{ProxyRuleTable, SelectTarget};

use super::prefilter::{PrefilterTableHitRegion, PrefilterTableWidget};
use super::tables::ProxyRuleTableWidget;
use super::text::fit_input_value;
use crate::ui::terminal_text::text_width;

#[derive(Clone, Copy)]
pub(super) enum SettingsTableHit {
    Prefilter,
    Proxy(ProxyRuleTable),
}

#[derive(Clone, Copy)]
pub(super) struct SettingsTableScrollRegion {
    area: Rect,
    hit: SettingsTableHit,
}

impl SettingsTableScrollRegion {
    pub(super) fn hit_at(self, position: Position) -> Option<SettingsTableHit> {
        self.area.contains(position).then_some(self.hit)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct SettingsFieldLayoutConfig {
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
pub(in crate::ui) struct SettingsFieldLayout {
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

    pub(in crate::ui) fn areas(self, area: Rect) -> SettingsFieldRowAreas {
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
pub(in crate::ui) struct SettingsFieldRowAreas {
    prefix: Rect,
    label: Rect,
    pub(in crate::ui) control: Rect,
}

#[derive(Clone, Copy)]
pub(super) struct SettingsFieldStyle {
    pub(super) control: Style,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SettingsControlActivity {
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
pub(in crate::ui) struct SettingsSelectLayout {
    pub(super) target: SelectTarget,
    pub(in crate::ui) layout: crate::select_widget::SelectWidgetLayout,
}

impl SettingsSelectLayout {
    pub(super) fn box_contains(self, position: Position) -> bool {
        self.layout.box_area.contains(position)
    }

    pub(super) fn dropdown_contains(self, position: Position) -> bool {
        self.layout
            .dropdown_area
            .is_some_and(|area| area.contains(position))
    }
}

pub(super) trait SettingsControlView {
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

pub(in crate::ui) struct SettingsFieldRow<'a> {
    label: Cow<'a, str>,
    selected: bool,
    control: Box<dyn SettingsControlView + 'a>,
}

impl<'a> SettingsFieldRow<'a> {
    pub(super) fn new(
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

pub(super) struct SettingsFieldRowWidget<'a, 'b> {
    pub(super) row: &'b SettingsFieldRow<'a>,
    pub(super) layout: SettingsFieldLayout,
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

pub(in crate::ui) enum SettingsFullWidthTable<'a> {
    Proxy(ProxyRuleTableWidget<'a>),
    Prefilter(PrefilterTableWidget<'a>),
}

impl SettingsFullWidthTable<'_> {
    pub(super) fn height(&self) -> u16 {
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

    pub(super) fn scroll_region(&self, area: Rect) -> Option<SettingsTableScrollRegion> {
        match self {
            Self::Proxy(table) => {
                let viewport = table.viewport(area.height);
                viewport
                    .overflowing(table.row_count())
                    .then_some(SettingsTableScrollRegion {
                        area,
                        hit: SettingsTableHit::Proxy(table.table),
                    })
            }
            Self::Prefilter(table) => {
                let viewport = table.viewport(area.height);
                viewport
                    .overflowing(table.row_count())
                    .then_some(SettingsTableScrollRegion {
                        area,
                        hit: SettingsTableHit::Prefilter,
                    })
            }
        }
    }

    pub(super) fn prefilter_hit_region(&self, area: Rect) -> Option<PrefilterTableHitRegion> {
        match self {
            Self::Prefilter(table) => Some(table.hit_region(area)),
            Self::Proxy(_) => None,
        }
    }
}

pub(in crate::ui) enum SettingsContentItem<'a> {
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
    pub(in crate::ui) fn height(&self, layout: SettingsFieldLayout, width: u16) -> u16 {
        match self {
            Self::Line(_) => 1,
            Self::Divider { .. } => SETTINGS_DIVIDER_HEIGHT,
            Self::Field { field, .. } => field.height(layout, width),
            Self::FullWidthTable { table, .. } => table.height(),
        }
    }

    pub(super) fn render(
        &self,
        scroll_view: &mut ScrollView,
        area: Rect,
        layout: SettingsFieldLayout,
    ) {
        match self {
            Self::Line(line) => scroll_view.render_widget(Paragraph::new(line.clone()), area),
            Self::Divider { title } => scroll_view.render_widget(SettingsDivider { title }, area),
            Self::Field { field, .. } => {
                scroll_view.render_widget(SettingsFieldRowWidget { row: field, layout }, area)
            }
            Self::FullWidthTable { table, .. } => table.render(scroll_view, area),
        }
    }

    pub(super) fn render_overlay(
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

    pub(super) fn select_layout(
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

pub(in crate::ui) fn settings_field_layout(
    items: &[SettingsContentItem<'_>],
) -> SettingsFieldLayout {
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

pub(super) fn settings_field_layout_for_rows(
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

pub(in crate::ui) fn settings_content_height(
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

pub(super) fn settings_content_width_for_viewport(
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

pub(super) struct SettingsContentLayout<'items, 'content> {
    pub(super) items: &'items [SettingsContentItem<'content>],
    pub(super) field_layout: SettingsFieldLayout,
    pub(super) content_width: u16,
    body_height: u16,
    pub(super) buffer_height: u16,
    viewport_height: u16,
    pub(super) scrolling_enabled: bool,
}

impl<'items, 'content> SettingsContentLayout<'items, 'content> {
    pub(super) fn new(
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

    pub(super) fn item_areas(
        &self,
    ) -> impl Iterator<Item = (&SettingsContentItem<'content>, Rect)> + '_ {
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

    pub(super) fn scroll_y_for_selected(&self, selected_row: usize, current_y: u16) -> Option<u16> {
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

#[cfg(test)]
pub(in crate::ui) fn settings_select_layout(
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
