use ratatui::{
    buffer::Buffer,
    layout::{Margin, Position, Rect},
    style::{Color, Modifier, Style},
    text::Line,
    widgets::{
        Block, BorderType, Borders, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState,
        StatefulWidget, Widget,
    },
};
use tui_textarea::{CursorMove, TextArea};

use crate::app::{PrefilterPatternEditState, SettingsPopup};
use crate::settings::RecordingPrefilterPatternSettings;

use super::{RuleParentState, SettingsTableViewport, fit_input_value, settings_table_height};

const PREFILTER_ON_COLUMN_WIDTH: usize = 4;
const PREFILTER_COLUMN_GAP: usize = 1;

pub(super) struct PrefilterTableWidget<'a> {
    patterns: &'a [RecordingPrefilterPatternSettings],
    parent_state: RuleParentState,
    selected: bool,
    active_pattern: Option<usize>,
    edit: Option<PrefilterPatternEditState<'a>>,
    scroll_offset: usize,
    max_height: u16,
}

impl PrefilterTableWidget<'_> {
    pub(super) fn height(&self) -> u16 {
        settings_table_height(self.row_count(), self.max_height)
    }

    pub(super) fn row_count(&self) -> usize {
        self.patterns.len()
    }

    pub(super) fn active_pattern_area(&self, area: Rect) -> Option<Rect> {
        let active_pattern = self.active_pattern?;
        if active_pattern >= self.row_count() {
            return None;
        }
        let y = self.viewport(area.height).row_y(area, active_pattern)?;
        Some(Rect::new(area.x, y, area.width, 1))
    }

    pub(super) fn viewport(&self, height: u16) -> SettingsTableViewport {
        SettingsTableViewport::new(self.row_count(), self.scroll_offset, height)
    }

    pub(super) fn pattern_hit(&self, area: Rect, position: Position) -> Option<(usize, bool)> {
        let inner = area.inner(Margin {
            horizontal: 1,
            vertical: 1,
        });
        if !inner.contains(position) {
            return None;
        }

        let viewport = self.viewport(area.height);
        let index = viewport
            .visible_range(self.row_count())
            .find(|index| viewport.row_y(area, *index) == Some(position.y))?;
        let checkbox_end = inner
            .x
            .saturating_add(u16::try_from(PREFILTER_ON_COLUMN_WIDTH).unwrap_or(u16::MAX));
        Some((index, position.x < checkbox_end))
    }
}

impl Widget for &PrefilterTableWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.is_empty() {
            return;
        }

        let border_style = if self.selected {
            Style::default().fg(Color::Green)
        } else {
            Style::default()
        };
        let block = Block::default()
            .title(" Included URL Patterns ")
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(border_style);
        let inner = block.inner(area);
        block.render(area, buf);
        if inner.is_empty() {
            return;
        }

        let viewport = self.viewport(area.height);
        let overflowing = viewport.overflowing(self.row_count());
        let table_width = if overflowing {
            inner.width.saturating_sub(1).max(1)
        } else {
            inner.width
        };
        let header_style = Style::default().fg(Color::DarkGray);
        render_prefilter_table_columns(
            "On",
            "Pattern",
            Rect::new(inner.x, inner.y, table_width, 1),
            header_style,
            header_style,
            buf,
        );

        if self.patterns.is_empty() {
            Paragraph::new(fit_input_value(
                "No patterns configured; all URLs are recorded",
                usize::from(table_width),
            ))
            .render(
                Rect::new(inner.x, inner.y.saturating_add(1), table_width, 1),
                buf,
            );
            return;
        }

        for index in viewport.visible_range(self.row_count()) {
            let y = inner
                .y
                .saturating_add(1)
                .saturating_add(u16::try_from(index - viewport.first_row).unwrap_or(u16::MAX));
            if y >= inner.bottom() {
                break;
            }
            let row_area = Rect::new(inner.x, y, table_width, 1);
            let row_selected = self.active_pattern == Some(index);
            let row_style = if row_selected {
                Style::default().bg(Color::White).fg(Color::DarkGray)
            } else {
                Style::default()
            };
            let checkbox_style = self.parent_state.checkbox_style(row_style, row_selected);
            if let Some(edit) = self.edit.filter(|edit| edit.index == index) {
                buf.set_style(row_area, row_style);
                render_prefilter_checkbox(
                    pattern_mark(self.patterns[index].enable),
                    row_area,
                    checkbox_style,
                    buf,
                );
                render_pattern_editor(edit, prefilter_pattern_area(row_area), buf);
                continue;
            }
            render_prefilter_table_columns(
                pattern_mark(self.patterns[index].enable),
                &self.patterns[index].pattern,
                row_area,
                row_style,
                checkbox_style,
                buf,
            );
        }

        if overflowing {
            let scrollbar_area = Rect::new(
                inner.x,
                inner.y.saturating_add(1),
                inner.width,
                u16::try_from(viewport.visible_rows).unwrap_or(u16::MAX),
            );
            let mut scrollbar = ScrollbarState::new(self.row_count())
                .position(viewport.scrollbar_position(self.row_count()))
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

fn render_prefilter_table_columns(
    on: &str,
    pattern: &str,
    area: Rect,
    row_style: Style,
    checkbox_style: Style,
    buf: &mut Buffer,
) {
    buf.set_style(area, row_style);
    render_prefilter_checkbox(on, area, checkbox_style, buf);
    let pattern_area = prefilter_pattern_area(area);
    if pattern_area.is_empty() {
        return;
    }
    Paragraph::new(Line::styled(
        fit_input_value(pattern, usize::from(pattern_area.width)),
        row_style,
    ))
    .render(pattern_area, buf);
}

fn render_prefilter_checkbox(mark: &str, area: Rect, style: Style, buf: &mut Buffer) {
    let checkbox_area = Rect::new(
        area.x,
        area.y,
        area.width
            .min(u16::try_from(PREFILTER_ON_COLUMN_WIDTH).unwrap_or(u16::MAX)),
        1,
    );
    buf.set_style(checkbox_area, style);
    Paragraph::new(Line::styled(mark, style)).render(checkbox_area, buf);
}

fn prefilter_pattern_area(area: Rect) -> Rect {
    let offset =
        u16::try_from(PREFILTER_ON_COLUMN_WIDTH + PREFILTER_COLUMN_GAP).unwrap_or(u16::MAX);
    Rect::new(
        area.x.saturating_add(offset),
        area.y,
        area.width.saturating_sub(offset),
        1,
    )
}

fn pattern_mark(enabled: bool) -> &'static str {
    if enabled { "[✓]" } else { "[ ]" }
}

fn render_pattern_editor(edit: PrefilterPatternEditState<'_>, area: Rect, buf: &mut Buffer) {
    if area.is_empty() {
        return;
    }
    let mut textarea = TextArea::new(vec![edit.value.to_string()]);
    let style = Style::default().fg(Color::Indexed(208));
    textarea.set_style(style);
    textarea.set_cursor_line_style(style);
    textarea.set_cursor_style(style.add_modifier(Modifier::REVERSED));
    let cursor = u16::try_from(edit.cursor).unwrap_or(u16::MAX);
    textarea.move_cursor(CursorMove::Jump(0, cursor));
    (&textarea).render(area, buf);
}

pub(super) fn prefilter_table_widget(
    popup: &SettingsPopup,
    max_height: u16,
) -> PrefilterTableWidget<'_> {
    PrefilterTableWidget {
        patterns: &popup.draft().recording.prefilter.include_url_patterns,
        parent_state: RuleParentState::from_enabled(popup.draft().recording.prefilter.enable),
        selected: popup.prefilter_table_is_selected(),
        active_pattern: popup.active_prefilter_pattern(),
        edit: popup.prefilter_pattern_edit(),
        scroll_offset: popup.prefilter_table_scroll_offset(),
        max_height,
    }
}
