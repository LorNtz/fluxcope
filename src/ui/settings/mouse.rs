use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};

use crate::app::{App, SettingsPopup, SettingsTopic};

use super::content::{
    SettingsContentItem, SettingsContentLayout, SettingsFullWidthTable, SettingsTableHit,
    settings_content_width_for_viewport, settings_field_layout, settings_select_layout,
    settings_select_layout_at_position,
};
use super::pages::settings_content_items_with_error;
use super::{settings_popup_layout, settings_table_max_height};

pub(in crate::ui) fn handle_settings_popup_mouse(
    mouse: MouseEvent,
    app: &mut App,
    root_area: Rect,
) -> bool {
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
