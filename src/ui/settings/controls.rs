use std::borrow::Cow;

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::Line,
    widgets::{Block, BorderType, Borders, Paragraph, Widget},
};
use tui_textarea::{CursorMove, TextArea};

use crate::app::{
    PROXY_PRESET_SELECT_MAX_VISIBLE_ITEMS, SelectTarget, SettingsPopup, SettingsSelectId,
};
use crate::select::{SelectItem, SelectState};
use crate::ui::select_widget::SelectWidget;

use super::content::{
    SettingsControlActivity, SettingsControlView, SettingsFieldStyle, SettingsSelectLayout,
};

pub(in crate::ui) const SETTING_TEXT_FIELD_HEIGHT: u16 = 3;
// Border columns plus the explicit leading/trailing spaces in the rendered textarea text.
pub(in crate::ui) const TEXT_INPUT_CHROME_WIDTH: u16 = 4;
pub(in crate::ui) const PORT_INPUT_WIDTH_COLS: u16 = 10;
pub(in crate::ui) const PEM_FILENAME_INPUT_WIDTH_COLS: u16 = 32;

pub(in crate::ui) struct SettingsTextInputControl<'a> {
    pub(in crate::ui) value: Cow<'a, str>,
    pub(in crate::ui) cursor: Option<usize>,
    pub(in crate::ui) fixed_edit_width_cols: Option<u16>,
    pub(in crate::ui) hint: Option<&'a str>,
}

impl SettingsTextInputControl<'_> {
    pub(in crate::ui) fn render_width(&self, available_width: u16) -> u16 {
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

pub(super) struct SettingsCheckboxControl {
    pub(super) checked: bool,
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

pub(in crate::ui) fn settings_select_control<'a>(
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

pub(in crate::ui) struct SettingsSelectControl<'a> {
    target: SelectTarget,
    selected_label: &'a str,
    items: Vec<SelectItem<'a, SettingsSelectId>>,
    state: Option<&'a SelectState>,
    max_visible_items: usize,
}

impl SettingsSelectControl<'_> {
    #[cfg(test)]
    pub(in crate::ui) fn height_for_width(&self, width: u16) -> u16 {
        <Self as SettingsControlView>::height_for_width(self, width)
    }

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
    ) -> crate::ui::select_widget::SelectWidgetLayout {
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
