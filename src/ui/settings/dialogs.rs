use std::borrow::Cow;

use ratatui::{
    Frame,
    layout::{Alignment, Margin, Rect},
    style::{Color, Style},
    text::Line,
    widgets::{Block, BorderType, Borders, Clear, Paragraph},
};

use crate::app::{
    ActionDialog, BODY_TEXT_TAB_WIDTH, RULE_EDITOR_KEY_HINTS, RuleEditField, RuleEditorState,
};

use super::content::{
    SettingsFieldLayoutConfig, SettingsFieldRow, SettingsFieldRowWidget,
    settings_field_layout_for_rows,
};
use super::controls::{SETTING_TEXT_FIELD_HEIGHT, SettingsTextInputControl};
use super::settings_key_hint_text;
use crate::ui::chrome::centered_rect;
use crate::ui::terminal_text::{fit_text_to_width, text_width};

pub(super) fn render_rule_editor_popup(
    frame: &mut Frame,
    editor: RuleEditorState<'_>,
    parent: Rect,
) {
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

pub(in crate::ui) fn rule_editor_area(parent: Rect) -> Rect {
    let max_width = parent.width.saturating_sub(4).max(1);
    let min_width = 44.min(max_width);
    let width = max_width.min(76).max(min_width);
    let height = 11.min(parent.height.saturating_sub(2)).max(9);

    centered_rect(width, height, parent)
}

pub(in crate::ui) fn action_dialog_area(dialog: &ActionDialog, parent: Rect) -> Rect {
    let button_width = action_buttons_full_width(dialog).saturating_add(2);
    let width = 58
        .min(parent.width.saturating_sub(4))
        .max(32)
        .max(button_width);
    let height = 9.min(parent.height.saturating_sub(2)).max(7);
    centered_rect(width, height, parent)
}

pub(super) fn render_action_dialog(frame: &mut Frame, dialog: &ActionDialog, parent: Rect) {
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
            fit_text_to_width(label, usize::from(label_area.width), BODY_TEXT_TAB_WIDTH)
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
