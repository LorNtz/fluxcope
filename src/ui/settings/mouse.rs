use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Position;

use crate::app::{App, SettingsTopic};

use super::content::SettingsTableHit;
use super::hit_regions::SettingsHitRegions;

pub(super) fn handle_settings_popup_mouse(
    mouse: MouseEvent,
    app: &mut App,
    hit_regions: &SettingsHitRegions,
) {
    if handle_proxy_preset_select_mouse(mouse, app, hit_regions) {
        return;
    }
    if handle_prefilter_table_mouse(mouse, app, hit_regions) {
        return;
    }

    if matches!(
        mouse.kind,
        MouseEventKind::ScrollDown | MouseEventKind::ScrollUp
    ) {
        handle_settings_content_scroll_mouse(mouse, app, hit_regions);
    }
}

fn handle_settings_content_scroll_mouse(
    mouse: MouseEvent,
    app: &mut App,
    hit_regions: &SettingsHitRegions,
) {
    let scroll_down = matches!(mouse.kind, MouseEventKind::ScrollDown);
    let viewport = hit_regions.viewport();
    if viewport.is_empty() {
        app.settings_popup.scroll.set_offset(Position::ORIGIN);
        return;
    }

    let position = Position::new(mouse.column, mouse.row);
    let table_hit = viewport
        .contains(position)
        .then(|| hit_regions.table_hit(hit_regions.local_position(mouse)))
        .flatten();

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

    if hit_regions.scrolling_enabled() {
        if scroll_down {
            app.settings_popup.scroll.scroll_down();
        } else {
            app.settings_popup.scroll.scroll_up();
        }
    } else {
        app.settings_popup.scroll.set_offset(Position::ORIGIN);
    }
}

fn handle_prefilter_table_mouse(
    mouse: MouseEvent,
    app: &mut App,
    hit_regions: &SettingsHitRegions,
) -> bool {
    if !matches!(mouse.kind, MouseEventKind::Down(MouseButton::Left))
        || app.settings_popup.topic != SettingsTopic::Recording
        || app.settings_popup.unsaved_dialog().is_some()
        || app.settings_popup.prefilter_pattern_edit().is_some()
    {
        return false;
    }

    let viewport = hit_regions.viewport();
    let position = Position::new(mouse.column, mouse.row);
    if viewport.is_empty() || !viewport.contains(position) {
        return false;
    }

    if let Some((index, toggle)) = hit_regions.prefilter_hit(hit_regions.local_position(mouse)) {
        app.settings_popup.select_prefilter_pattern(index);
        if toggle {
            app.settings_popup.toggle_prefilter_pattern(index);
        }
        return true;
    }
    false
}

fn handle_proxy_preset_select_mouse(
    mouse: MouseEvent,
    app: &mut App,
    hit_regions: &SettingsHitRegions,
) -> bool {
    if app.settings_popup.topic != SettingsTopic::Proxy
        || app.settings_popup.unsaved_dialog().is_some()
        || app.settings_popup.rule_editor().is_some()
    {
        return false;
    }

    let viewport = hit_regions.viewport();
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

    let local_position = hit_regions.local_position(mouse);
    let active_select = hit_regions.active_select();
    let clicked_select = hit_regions.select_at(local_position);
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
