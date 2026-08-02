use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::{
        Block, BorderType, Borders, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState,
        StatefulWidget, Widget,
    },
};

use crate::app::{ProxyRuleTable, SettingsPopup};
use crate::settings::{ProxyMapLocalRule, ProxyMapRemoteRule, ProxyPresetSettings};

use super::SETTINGS_TABLE_MIN_HEIGHT;
use super::text::fit_input_value;

#[derive(Clone, Copy)]
struct ProxyRuleTableRow<'a> {
    enabled: bool,
    from: &'a str,
    to: &'a str,
}

enum ProxyRuleTableRows<'a> {
    Remote(&'a [ProxyMapRemoteRule]),
    Local(&'a [ProxyMapLocalRule]),
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
pub(super) struct SettingsTableViewport {
    pub(super) visible_rows: usize,
    pub(super) first_row: usize,
}

impl SettingsTableViewport {
    pub(super) fn new(row_count: usize, scroll_offset: usize, height: u16) -> Self {
        let visible_rows = settings_table_visible_rows(height);
        let first_row = scroll_offset.min(row_count.saturating_sub(visible_rows));

        Self {
            visible_rows,
            first_row,
        }
    }

    pub(super) fn overflowing(self, row_count: usize) -> bool {
        row_count > self.visible_rows
    }

    pub(super) fn visible_range(self, row_count: usize) -> std::ops::Range<usize> {
        self.first_row
            ..self
                .first_row
                .saturating_add(self.visible_rows)
                .min(row_count)
    }

    pub(super) fn row_y(self, area: Rect, row_index: usize) -> Option<u16> {
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

    pub(super) fn scrollbar_position(self, row_count: usize) -> usize {
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
pub(super) enum RuleParentState {
    Enabled,
    Disabled,
}

impl RuleParentState {
    pub(super) fn from_enabled(enabled: bool) -> Self {
        if enabled {
            Self::Enabled
        } else {
            Self::Disabled
        }
    }

    pub(super) fn checkbox_style(self, row_style: Style, row_selected: bool) -> Style {
        match (self, row_selected) {
            (Self::Enabled, _) => row_style,
            (Self::Disabled, true) => row_style.fg(Color::Gray),
            (Self::Disabled, false) => row_style.fg(Color::DarkGray),
        }
    }
}

pub(super) fn settings_table_height(row_count: usize, max_height: u16) -> u16 {
    let natural_height = 3u16.saturating_add(u16::try_from(row_count.max(1)).unwrap_or(u16::MAX));
    natural_height.min(max_height.max(SETTINGS_TABLE_MIN_HEIGHT))
}

pub(super) fn settings_table_visible_rows(height: u16) -> usize {
    usize::from(height.saturating_sub(3)).max(1)
}

pub(in crate::ui) struct ProxyRuleTableWidget<'a> {
    pub(super) table: ProxyRuleTable,
    rows: Option<ProxyRuleTableRows<'a>>,
    parent_state: RuleParentState,
    selected: bool,
    active_rule: Option<usize>,
    scroll_offset: usize,
    max_height: u16,
}

impl ProxyRuleTableWidget<'_> {
    pub(in crate::ui) fn height(&self) -> u16 {
        settings_table_height(self.row_count(), self.max_height)
    }

    pub(super) fn row_count(&self) -> usize {
        self.rows.as_ref().map_or(0, ProxyRuleTableRows::len)
    }

    fn is_empty(&self) -> bool {
        self.rows.as_ref().is_none_or(ProxyRuleTableRows::is_empty)
    }

    pub(super) fn active_rule_area(&self, area: Rect) -> Option<Rect> {
        let active_rule = self.active_rule?;
        if active_rule >= self.row_count() {
            return None;
        }

        let y = self.viewport(area.height).row_y(area, active_rule)?;

        Some(Rect::new(area.x, y, area.width, 1))
    }

    pub(super) fn viewport(&self, height: u16) -> SettingsTableViewport {
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

pub(in crate::ui) fn proxy_rule_table_widget<'a>(
    popup: &SettingsPopup,
    table: ProxyRuleTable,
    preset: Option<&'a ProxyPresetSettings>,
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
