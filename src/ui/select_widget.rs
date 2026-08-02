use super::terminal_text::{fit_text, text_width, text_width_bounded};
use crate::app::BODY_TEXT_TAB_WIDTH;
use crate::select::{SelectItem, SelectItemRole, SelectResolvedItems, SelectState};
use ratatui::{
    buffer::Buffer,
    layout::{Margin, Position, Rect},
    style::{Color, Modifier, Style},
    widgets::{
        Block, BorderType, Borders, Clear, Paragraph, Scrollbar, ScrollbarOrientation,
        ScrollbarState, StatefulWidget, Widget,
    },
};
use std::borrow::Cow;

pub(crate) const SELECT_FIELD_HEIGHT: u16 = 3;
pub(crate) const DEFAULT_MIN_SELECT_FIELD_WIDTH: usize = 12;
pub(crate) const DEFAULT_MAX_SELECT_FIELD_WIDTH: usize = 36;
const SELECT_FIELD_CHROME_WIDTH: usize = 5;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SelectWidgetLayout {
    pub box_area: Rect,
    pub dropdown_area: Option<Rect>,
    pub options_area: Option<Rect>,
    pub first_visible_filtered_index: usize,
    pub visible_option_count: usize,
}

impl SelectWidgetLayout {
    pub(crate) fn option_at(self, position: Position) -> Option<usize> {
        let options_area = self.options_area?;
        if !options_area.contains(position) {
            return None;
        }

        let row = usize::from(position.y.saturating_sub(options_area.y));
        (row < self.visible_option_count).then_some(self.first_visible_filtered_index + row)
    }
}

pub(crate) struct SelectWidget<'a, Id> {
    selected_label: Cow<'a, str>,
    items: &'a [SelectItem<'a, Id>],
    state: Option<&'a SelectState>,
    max_visible_items: usize,
    min_width: usize,
    max_width: usize,
    style: Style,
}

impl<'a, Id> SelectWidget<'a, Id> {
    pub(crate) fn new(
        selected_label: impl Into<Cow<'a, str>>,
        items: &'a [SelectItem<'a, Id>],
        state: Option<&'a SelectState>,
    ) -> Self {
        Self {
            selected_label: selected_label.into(),
            items,
            state,
            max_visible_items: 6,
            min_width: DEFAULT_MIN_SELECT_FIELD_WIDTH,
            max_width: DEFAULT_MAX_SELECT_FIELD_WIDTH,
            style: Style::default(),
        }
    }

    pub(crate) fn with_style(mut self, style: Style) -> Self {
        self.style = style;
        self
    }

    pub(crate) fn max_visible_items(mut self, max_visible_items: usize) -> Self {
        self.max_visible_items = max_visible_items.max(1);
        self
    }

    pub(crate) fn with_min_width(mut self, min_width: usize) -> Self {
        self.min_width = min_width.max(3);
        self
    }

    pub(crate) fn height(&self) -> u16 {
        SELECT_FIELD_HEIGHT
    }

    pub(crate) fn layout(&self, area: Rect) -> SelectWidgetLayout {
        if !self.is_open() {
            return self.field_layout(area);
        }

        self.layout_with_resolved(area, &self.resolved_items())
    }

    pub(crate) fn layout_with_resolved(
        &self,
        area: Rect,
        resolved: &SelectResolvedItems,
    ) -> SelectWidgetLayout {
        let box_area = self.field_area(area);

        let mut layout = SelectWidgetLayout {
            box_area,
            dropdown_area: None,
            options_area: None,
            first_visible_filtered_index: 0,
            visible_option_count: 0,
        };

        let Some(state) = self.state.filter(|state| state.is_open()) else {
            return layout;
        };
        let dropdown_height = self
            .dropdown_height(resolved)
            .min(area.height.saturating_sub(box_area.height));
        if box_area.width < 3 || dropdown_height < 3 {
            return layout;
        }

        let dropdown_area = Rect::new(
            box_area.x,
            box_area.y.saturating_add(box_area.height),
            box_area.width,
            dropdown_height,
        );
        let options_area = dropdown_area.inner(Margin {
            vertical: 1,
            horizontal: 1,
        });
        let filtered_count = resolved.len();
        let visible_option_count = usize::from(options_area.height).min(filtered_count.max(1));
        let max_start = filtered_count.saturating_sub(visible_option_count);
        layout.dropdown_area = Some(dropdown_area);
        layout.options_area = Some(options_area);
        layout.first_visible_filtered_index = state.scroll_offset().min(max_start);
        layout.visible_option_count = visible_option_count;
        layout
    }

    pub(crate) fn resolved_items(&self) -> SelectResolvedItems {
        self.state.map_or_else(
            || SelectState::new().resolve_items(self.items),
            |state| state.resolve_items(self.items),
        )
    }

    pub(crate) fn overlay(self) -> SelectDropdownOverlay<'a, Id> {
        SelectDropdownOverlay { select: self }
    }

    fn dropdown_height(&self, resolved: &SelectResolvedItems) -> u16 {
        if !self.is_open() {
            return 0;
        }
        let visible_count = resolved.len().max(1).min(self.max_visible_items);
        u16::try_from(visible_count.saturating_add(2)).unwrap_or(u16::MAX)
    }

    fn preferred_box_width(&self) -> u16 {
        let max_label_width = self.max_width.saturating_sub(SELECT_FIELD_CHROME_WIDTH);
        let mut label_width = text_width_bounded(self.selected_label.as_ref(), max_label_width);
        for item in self.items {
            if label_width >= max_label_width {
                break;
            }
            label_width = label_width.max(text_width_bounded(item.label.as_ref(), max_label_width));
        }
        usize_to_u16(
            label_width
                .saturating_add(SELECT_FIELD_CHROME_WIDTH)
                .clamp(self.min_width, self.max_width),
        )
    }

    fn is_open(&self) -> bool {
        self.state.is_some_and(SelectState::is_open)
    }

    fn field_area(&self, area: Rect) -> Rect {
        let box_x = area.x;
        let max_box_width = area.right().saturating_sub(box_x);
        let box_width = self.preferred_box_width().min(max_box_width);

        Rect::new(
            box_x,
            area.y,
            box_width,
            SELECT_FIELD_HEIGHT.min(area.height),
        )
    }

    fn field_layout(&self, area: Rect) -> SelectWidgetLayout {
        SelectWidgetLayout {
            box_area: self.field_area(area),
            dropdown_area: None,
            options_area: None,
            first_visible_filtered_index: 0,
            visible_option_count: 0,
        }
    }

    fn field_text(&self) -> &str {
        match self.state {
            Some(state) if state.is_open() && !state.filter().is_empty() => state.filter(),
            _ => self.selected_label.as_ref(),
        }
    }
}

impl<Id> Widget for SelectWidget<'_, Id> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.is_empty() {
            return;
        }

        self.render_field(self.field_area(area), self.style, buf);
    }
}

pub(crate) struct SelectDropdownOverlay<'a, Id> {
    select: SelectWidget<'a, Id>,
}

impl<Id> Widget for SelectDropdownOverlay<'_, Id> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.is_empty() {
            return;
        }
        if !self.select.is_open() {
            return;
        }

        let resolved = self.select.resolved_items();
        let layout = self.select.layout_with_resolved(area, &resolved);
        self.select.render_dropdown(layout, &resolved, buf);
    }
}

impl<Id> SelectWidget<'_, Id> {
    fn render_field(&self, area: Rect, style: Style, buf: &mut Buffer) {
        if area.width < 3 || area.height < SELECT_FIELD_HEIGHT {
            return;
        }

        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(style);
        let inner = block.inner(area);
        block.render(area, buf);
        if inner.is_empty() {
            return;
        }

        let arrow = if self.is_open() { "▴" } else { "▾" };
        let arrow_x = inner.right().saturating_sub(1);
        Paragraph::new(arrow).style(style).render(
            Rect::new(arrow_x, inner.y, usize_to_u16(text_width(arrow).into()), 1),
            buf,
        );

        let text_area_width = inner.width.saturating_sub(2);
        if text_area_width == 0 {
            return;
        }
        render_fitted_text(
            self.field_text(),
            Rect::new(inner.x, inner.y, text_area_width, 1),
            style,
            buf,
        );

        if let Some(state) = self.state.filter(|state| state.is_open()) {
            let cursor =
                usize::from(text_width(state.filter_prefix())).min(usize::from(text_area_width));
            let cursor_x = inner.x.saturating_add(usize_to_u16(cursor));
            if cursor_x < arrow_x {
                buf[(cursor_x, inner.y)].set_style(style.add_modifier(Modifier::REVERSED));
            }
        }
    }

    fn render_dropdown(
        &self,
        layout: SelectWidgetLayout,
        resolved: &SelectResolvedItems,
        buf: &mut Buffer,
    ) {
        let Some(dropdown_area) = layout.dropdown_area else {
            return;
        };
        let Some(options_area) = layout.options_area else {
            return;
        };
        let Some(state) = self.state else {
            return;
        };

        // Dropdown box is floated
        // so we need to clear the area before drawing on the underlying content
        Clear.render(dropdown_area, buf);

        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(Color::Indexed(208)));
        block.render(dropdown_area, buf);
        if options_area.is_empty() {
            return;
        }

        if resolved.is_empty() {
            render_fitted_text(
                "No matches",
                Rect::new(options_area.x, options_area.y, options_area.width, 1),
                Style::default().fg(Color::DarkGray),
                buf,
            );
            return;
        }

        for visible_row in 0..layout.visible_option_count {
            let filtered_index = layout.first_visible_filtered_index + visible_row;
            let Some(item_index) = resolved.item_index(filtered_index) else {
                break;
            };
            let Some(item) = self.items.get(item_index) else {
                break;
            };
            let y = options_area
                .y
                .saturating_add(u16::try_from(visible_row).unwrap_or(u16::MAX));
            if y >= options_area.bottom() {
                break;
            }
            let style = option_style(item.role, filtered_index == state.focused_filtered_index());
            render_fitted_text(
                item.label.as_ref(),
                Rect::new(options_area.x, y, options_area.width, 1),
                style,
                buf,
            );
        }

        if resolved.len() > layout.visible_option_count {
            let mut scrollbar = ScrollbarState::new(resolved.len())
                .position(layout.first_visible_filtered_index)
                .viewport_content_length(layout.visible_option_count);
            StatefulWidget::render(
                Scrollbar::default()
                    .orientation(ScrollbarOrientation::VerticalRight)
                    .begin_symbol(None)
                    .end_symbol(None),
                dropdown_area,
                buf,
                &mut scrollbar,
            );
        }
    }
}

fn option_style(role: SelectItemRole, focused: bool) -> Style {
    if focused {
        Style::default().bg(Color::White).fg(Color::DarkGray)
    } else if role == SelectItemRole::Action {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default()
    }
}

fn render_fitted_text(value: &str, area: Rect, style: Style, buf: &mut Buffer) {
    if area.is_empty() {
        return;
    }

    for x in area.x..area.right() {
        buf[(x, area.y)].set_style(style);
    }

    let text = fit_text(value, usize::from(area.width), BODY_TEXT_TAB_WIDTH);
    Paragraph::new(text.as_ref()).style(style).render(area, buf);
}

fn usize_to_u16(value: usize) -> u16 {
    value.try_into().unwrap_or(u16::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn select_uses_terminal_cells_for_width_and_clipping() {
        let wide_label = "界".repeat(8);
        let items = [SelectItem::value(0, wide_label.as_str())];
        let widget = SelectWidget::new(wide_label.as_str(), &items, None);
        assert_eq!(widget.layout(Rect::new(0, 0, 80, 3)).box_area.width, 21);

        let widget = SelectWidget::new("界ab", &items, None).with_min_width(3);
        let area = Rect::new(0, 0, 7, 3);
        let mut buf = Buffer::empty(area);
        widget.render(area, &mut buf);

        assert_eq!(buf[(1, 1)].symbol(), "界");
        assert_eq!(buf[(3, 1)].symbol(), "…");
    }

    #[test]
    fn select_filter_cursor_uses_rendered_prefix_width() {
        let items = [SelectItem::value(0, "anything")];
        let rendered_cursor_x = |characters: &[char]| {
            let mut state = SelectState::new();
            state.open_with_selected(&items, Some(&0), 4);
            for character in characters {
                state.handle_key(
                    crossterm::event::KeyEvent::new(
                        crossterm::event::KeyCode::Char(*character),
                        crossterm::event::KeyModifiers::NONE,
                    ),
                    &items,
                    4,
                );
            }

            let widget = SelectWidget::new("anything", &items, Some(&state));
            let area = Rect::new(0, 0, 12, 3);
            let mut buf = Buffer::empty(area);
            widget.render(area, &mut buf);
            (1..area.right())
                .find(|x| buf[(*x, 1)].modifier.contains(Modifier::REVERSED))
                .expect("open filter should render its cursor")
        };

        assert_eq!(rendered_cursor_x(&['界']), 3);
        assert_eq!(rendered_cursor_x(&['e', '\u{301}']), 2);
        assert_eq!(rendered_cursor_x(&['👩', '\u{200d}', '💻']), 3);
        assert_eq!(rendered_cursor_x(&['界', '界', '界', '界']), 9);
    }

    #[test]
    fn dropdown_overlay_clears_existing_content() {
        let items = [SelectItem::value(0, "a")];
        let mut state = SelectState::new();
        state.open_with_selected(&items, Some(&0), 4);
        let widget = SelectWidget::new("a", &items, Some(&state));
        let area = Rect::new(0, 0, 30, 8);
        let dropdown_area = widget
            .layout(area)
            .dropdown_area
            .expect("open select should expose a dropdown area");
        let mut buf = Buffer::empty(area);

        for y in dropdown_area.y..dropdown_area.bottom() {
            for x in dropdown_area.x..dropdown_area.right() {
                buf[(x, y)].set_symbol("X");
            }
        }

        widget.overlay().render(area, &mut buf);

        for y in dropdown_area.y..dropdown_area.bottom() {
            for x in dropdown_area.x..dropdown_area.right() {
                assert_ne!(buf[(x, y)].symbol(), "X");
            }
        }
    }
}
