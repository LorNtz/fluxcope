use crate::app::{
    App, BODY_TEXT_TAB_WIDTH, PopupFocus, ProxyRuleTable, SettingsKeyHint, SettingsPaneFocus,
    SettingsPopup, SettingsScrollRequest, SettingsTopic,
};
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout, Margin, Position, Rect, Size},
    style::{Color, Style},
    text::Line,
    widgets::{Block, BorderType, Borders, Clear, Paragraph},
};
use tui_scrollview::{ScrollView, ScrollbarVisibility};

use super::chrome::{base_panel_block, centered_rect, panel_block};
use super::terminal_text::{fit_text_to_width, text_width};

mod content;
use content::{
    SettingsContentLayout, settings_content_width_for_viewport,
    settings_field_layout as calculate_settings_field_layout,
};
mod controls;
mod dialogs;
use dialogs::{render_action_dialog, render_rule_editor_popup};
mod hit_regions;
use hit_regions::SettingsHitRegions;
mod mouse;
mod pages;
use pages::settings_content_items_with_error as build_settings_content_items;
mod prefilter;
mod tables;
use tables::{settings_table_height, settings_table_visible_rows};

#[cfg(test)]
pub(in crate::ui) use content::{
    SettingsContentItem, settings_content_height, settings_field_layout, settings_select_layout,
};
#[cfg(test)]
pub(in crate::ui) use controls::{
    PEM_FILENAME_INPUT_WIDTH_COLS, PORT_INPUT_WIDTH_COLS, SETTING_TEXT_FIELD_HEIGHT,
    SettingsTextInputControl, TEXT_INPUT_CHROME_WIDTH, settings_select_control,
};
#[cfg(test)]
pub(in crate::ui) use dialogs::{action_dialog_area, rule_editor_area};
#[cfg(test)]
pub(in crate::ui) use pages::settings_content_items_with_error;
#[cfg(test)]
pub(in crate::ui) use tables::proxy_rule_table_widget;

pub(super) fn settings_popup_area(area: Rect) -> Rect {
    let width = area.width.saturating_sub(4).clamp(40, 96);
    let height = area.height.saturating_sub(4).clamp(12, 28);
    centered_rect(width, height, area)
}

const SETTINGS_TABLE_MAX_HEIGHT_PERCENT: u16 = 50;
const SETTINGS_TABLE_MIN_HEIGHT: u16 = 4;

pub(super) fn settings_table_max_height(viewport_height: u16) -> u16 {
    let proportional = u16::try_from(
        u32::from(viewport_height) * u32::from(SETTINGS_TABLE_MAX_HEIGHT_PERCENT) / 100,
    )
    .unwrap_or(u16::MAX);
    proportional.max(SETTINGS_TABLE_MIN_HEIGHT)
}

#[derive(Clone, Copy)]
pub(super) struct SettingsPopupLayout {
    area: Rect,
    topics: Rect,
    content_panel: Rect,
    #[cfg(test)]
    pub(super) content_viewport: Rect,
}

pub(super) fn settings_popup_layout(area: Rect) -> SettingsPopupLayout {
    let popup_area = settings_popup_area(area);
    let inner = popup_area.inner(Margin {
        vertical: 1,
        horizontal: 1,
    });
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(18), Constraint::Min(20)])
        .split(inner);
    #[cfg(test)]
    let content_viewport = chunks[1].inner(Margin {
        vertical: 1,
        horizontal: 1,
    });

    SettingsPopupLayout {
        area: popup_area,
        topics: chunks[0],
        content_panel: chunks[1],
        #[cfg(test)]
        content_viewport,
    }
}

pub(super) struct SettingsPopupView {
    hit_regions: Option<SettingsHitRegions>,
}

impl SettingsPopupView {
    pub(super) fn new() -> Self {
        Self { hit_regions: None }
    }

    pub(super) fn render(&mut self, frame: &mut Frame, app: &mut App) {
        self.hit_regions = render_settings_popup(frame, app);
    }

    pub(super) fn clear(&mut self) {
        self.hit_regions = None;
    }

    pub(super) fn handle_mouse(
        &mut self,
        mouse: crossterm::event::MouseEvent,
        app: &mut App,
        root_area: Rect,
    ) -> bool {
        let Some(hit_regions) = self
            .hit_regions
            .as_ref()
            .filter(|regions| regions.matches(root_area, &app.settings_popup))
        else {
            return true;
        };

        mouse::handle_settings_popup_mouse(mouse, app, hit_regions);
        true
    }
}

fn render_settings_popup(frame: &mut Frame, app: &mut App) -> Option<SettingsHitRegions> {
    let root_area = frame.area();
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
    let hit_regions = render_settings_content(
        frame,
        &mut app.settings_popup,
        layout.content_panel,
        root_area,
    );

    if let Some(dialog) = app.settings_popup.unsaved_dialog() {
        render_action_dialog(frame, &dialog, area);
    }

    if let Some(editor) = app.settings_popup.rule_editor() {
        render_rule_editor_popup(frame, editor, area);
    }

    hit_regions
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
        fit_text_to_width(
            &format!("{} [{}]", hint.label, hint.key),
            max_width,
            BODY_TEXT_TAB_WIDTH,
        )
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

fn render_settings_content(
    frame: &mut Frame,
    popup: &mut SettingsPopup,
    area: Rect,
    root_area: Rect,
) -> Option<SettingsHitRegions> {
    frame.render_widget(
        base_panel_block(popup.focus == SettingsPaneFocus::Content),
        area,
    );
    let area = area.inner(Margin {
        vertical: 1,
        horizontal: 1,
    });
    if area.is_empty() {
        return None;
    }

    let scroll_request = popup.take_scroll_request();
    let table_max_height = settings_table_max_height(area.height);
    match popup.topic {
        SettingsTopic::Recording => sync_prefilter_table_view(popup, table_max_height),
        SettingsTopic::Proxy => sync_settings_rule_table_views(popup, table_max_height),
        _ => {}
    }
    let (scroll_target, scrolling_enabled, scroll_view, mut hit_regions) = {
        let items = build_settings_content_items(popup, table_max_height);
        let field_layout = calculate_settings_field_layout(&items);
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
        let hit_regions =
            SettingsHitRegions::from_rendered(root_area, popup, area, &content_layout);

        (scroll_target, scrolling_enabled, scroll_view, hit_regions)
    };
    if let Some(y) = scroll_target {
        let offset = popup.scroll.offset();
        popup.scroll.set_offset(Position::new(offset.x, y));
    } else if !scrolling_enabled {
        popup.scroll.set_offset(Position::ORIGIN);
    }
    frame.render_stateful_widget(scroll_view, area, &mut popup.scroll);
    hit_regions.set_scroll_y(if scrolling_enabled {
        popup.scroll.offset().y
    } else {
        0
    });
    Some(hit_regions)
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
