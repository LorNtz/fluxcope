use super::*;
use std::time::{Duration, Instant};

fn redraw(ui: &mut RootView, app: &mut App, width: u16, height: u16) -> Buffer {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(|frame| ui.render(frame, app)).unwrap();
    terminal.backend().buffer().clone()
}

fn click(ui: &mut RootView, app: &mut App, position: Position) {
    static CLICK_TIME: std::sync::LazyLock<Instant> = std::sync::LazyLock::new(Instant::now);
    click_at(ui, app, position, *CLICK_TIME);
}

fn click_at(ui: &mut RootView, app: &mut App, position: Position, at: Instant) {
    let area = ui.area();
    ui.settings_popup.handle_mouse_at(
        mouse(
            MouseEventKind::Down(MouseButton::Left),
            position.x,
            position.y,
        ),
        app,
        area,
        at,
    );
}

#[test]
fn settings_mouse_topic_and_checkbox_change_only_draft() {
    let mut app = App::new(ui_settings(true));
    app.open_settings_popup();
    let (mut ui, buffer) = render_to_buffer_with_size(&mut app, 100, 28);
    let topic = find_buffer_text(&buffer, buffer.area, "Interface").unwrap();
    click(&mut ui, &mut app, topic);
    assert_eq!(app.settings_popup.topic, SettingsTopic::Interface);
    assert_eq!(app.settings_popup.focus, SettingsPaneFocus::Topics);
    let buffer = redraw(&mut ui, &mut app, 100, 28);
    let label = find_buffer_text(&buffer, buffer.area, "Auto-expand request tree").unwrap();
    click(&mut ui, &mut app, label);
    assert!(!app.settings_popup.draft().ui.request_list.auto_expand);
    assert!(app.take_settings_save_request().is_none());
}

#[test]
fn settings_mouse_outside_closes_clean_settings_without_write() {
    let mut app = App::new(ui_settings(true));
    app.open_settings_popup();
    let (mut ui, _) = render_to_buffer_with_size(&mut app, 100, 28);
    click(&mut ui, &mut app, Position::ORIGIN);
    assert!(!app.settings_popup.visible);
    assert!(!app.is_popup_focused(PopupFocus::Settings));
    assert!(app.take_settings_save_request().is_none());
}

#[test]
fn settings_mouse_dropdown_outside_consumes_one_layer_and_nonleft_is_inert() {
    let mut app = App::new(ui_settings(true));
    app.open_settings_popup();
    app.settings_popup
        .select_topic_for_tests(SettingsTopic::Proxy);
    app.settings_popup.start_select(SelectTarget::ProxyPreset);
    let (mut ui, _) = render_to_buffer_with_size(&mut app, 100, 28);
    for kind in [
        MouseEventKind::Down(MouseButton::Right),
        MouseEventKind::Down(MouseButton::Middle),
        MouseEventKind::Moved,
        MouseEventKind::Drag(MouseButton::Left),
    ] {
        ui.handle_mouse(mouse(kind, 0, 0), &mut app);
        assert!(app.settings_popup.active_select_target().is_some());
    }
    let offset = app.settings_popup.scroll.offset();
    ui.handle_mouse(mouse(MouseEventKind::ScrollDown, 40, 24), &mut app);
    assert_eq!(app.settings_popup.scroll.offset(), offset);
    click(&mut ui, &mut app, Position::ORIGIN);
    assert!(app.settings_popup.visible);
    assert!(app.settings_popup.active_select_target().is_none());
    assert!(app.settings_popup.draft().proxy.is_none());
    redraw(&mut ui, &mut app, 100, 28);
    click(&mut ui, &mut app, Position::ORIGIN);
    assert!(!app.settings_popup.visible);
    assert!(app.take_settings_save_request().is_none());
}

#[test]
fn settings_mouse_mapping_to_cell_editor_cancels_atomically_without_enabling_gates() {
    let mut app = App::new(ui_settings(true));
    app.open_settings_popup();
    let mut proxy = proxy_settings_with_rule_counts(1, 0);
    proxy.enable = false;
    proxy.presets[0].map_remote.enable = false;
    proxy.presets[0].map_remote.rules[0].from = "https://a.test".into();
    proxy.presets[0].map_remote.rules[0].to = "http://b.test".into();
    app.settings_popup.draft_mut_for_tests().proxy = Some(proxy.clone());
    app.settings_popup
        .select_topic_for_tests(SettingsTopic::Proxy);
    let (mut ui, buffer) = render_to_buffer_with_size(&mut app, 100, 32);
    let to = find_buffer_text(&buffer, buffer.area, "http://b.test").unwrap();
    click(&mut ui, &mut app, to);
    redraw(&mut ui, &mut app, 100, 32);
    click(&mut ui, &mut app, to);
    assert_eq!(
        app.settings_popup.rule_editor().unwrap().active_field,
        crate::app::RuleEditField::To
    );
    app.handle_paste("/edited");
    let buffer = redraw(&mut ui, &mut app, 100, 32);
    let from = find_buffer_text(
        &buffer,
        rule_editor_test_area(buffer.area),
        "https://a.test",
    )
    .unwrap();
    click(&mut ui, &mut app, Position::new(from.x + 8, from.y));
    app.handle_paste("x");
    redraw(&mut ui, &mut app, 100, 32);
    let offset = app.settings_popup.scroll.offset();
    ui.handle_mouse(mouse(MouseEventKind::ScrollDown, 45, 25), &mut app);
    assert_eq!(app.settings_popup.scroll.offset(), offset);
    click(&mut ui, &mut app, Position::ORIGIN);
    assert!(app.settings_popup.visible);
    assert!(app.settings_popup.rule_editor().is_none());
    assert_eq!(app.settings_popup.draft().proxy, Some(proxy));
}

#[test]
fn settings_mouse_filter_click_repositions_without_choosing_preset() {
    let mut app = App::new(ui_settings(true));
    app.open_settings_popup();
    app.settings_popup
        .select_topic_for_tests(SettingsTopic::Proxy);
    app.settings_popup.draft_mut_for_tests().proxy = Some(proxy_settings("dev", &["dev", "qa"]));
    app.settings_popup.start_select(SelectTarget::ProxyPreset);
    app.handle_paste("qa");
    let (mut ui, buffer) = render_to_buffer_with_size(&mut app, 100, 28);
    let filter = find_buffer_text(&buffer, settings_content_test_area(buffer.area), "qa").unwrap();
    click(&mut ui, &mut app, Position::new(filter.x + 1, filter.y));
    app.handle_paste("x");
    assert_eq!(
        app.settings_popup
            .select_state(SelectTarget::ProxyPreset)
            .unwrap()
            .filter(),
        "qxa"
    );
    assert_eq!(
        app.settings_popup
            .draft()
            .proxy
            .as_ref()
            .unwrap()
            .active_preset
            .as_deref(),
        Some("dev")
    );
}

mod confirmation;
mod inputs;
mod tables;
