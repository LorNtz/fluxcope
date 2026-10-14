use super::*;

fn close_button(buffer: &Buffer) -> Position {
    let area = settings_popup_area(buffer.area);
    let header = Rect::new(area.x, area.y, area.width, 1);
    find_buffer_text(buffer, header, "[x]").expect("settings close control should be visible")
}

fn click_close(ui: &mut RootView, app: &mut App, buffer: &Buffer) {
    let position = close_button(buffer);
    ui.handle_mouse(
        mouse(
            MouseEventKind::Down(MouseButton::Left),
            position.x,
            position.y,
        ),
        app,
    );
}

#[test]
fn settings_close_button_hits_only_its_cells_and_preserves_corners() {
    for (width, height) in [(100, 28), (36, 12), (12, 8), (5, 5)] {
        for offset in -1..=3 {
            let mut app = App::new(ui_settings(true));
            app.focus_panel(PanelFocus::Detail);
            app.open_settings_popup();
            let (mut ui, buffer) = render_to_buffer_with_size(&mut app, width, height);
            let area = settings_popup_area(buffer.area);
            let close = close_button(&buffer);
            assert_eq!(close, Position::new(area.right() - 4, area.y));
            assert_eq!(buffer[(area.x, area.y)].symbol(), "╭");
            assert_eq!(buffer[(area.right() - 1, area.y)].symbol(), "╮");
            ui.handle_mouse(
                mouse(
                    MouseEventKind::Down(MouseButton::Left),
                    close.x.checked_add_signed(offset).unwrap(),
                    close.y,
                ),
                &mut app,
            );
            let hit = (0..3).contains(&offset);
            assert_eq!(app.settings_popup.visible, !hit);
            assert_eq!(app.is_popup_focused(PopupFocus::Settings), !hit);
            if hit {
                assert!(app.is_panel_focused(PanelFocus::Detail));
            }
            assert!(app.take_settings_save_request().is_none());
        }
    }
}

#[test]
fn settings_close_button_confirms_dirty_draft_then_keeps_editing() {
    let mut app = App::new(ui_settings(true));
    app.open_settings_popup();
    app.settings_popup.draft_mut_for_tests().server.port = 9014;
    let (mut ui, buffer) = render_to_buffer_with_size(&mut app, 100, 28);
    click_close(&mut ui, &mut app, &buffer);
    assert!(app.settings_popup.is_confirming_unsaved());
    assert_eq!(app.settings_popup.draft().server.port, 9014);
    let buffer = redraw(&mut ui, &mut app, 100, 28);
    click_close(&mut ui, &mut app, &buffer);
    assert!(app.settings_popup.visible);
    assert!(!app.settings_popup.is_confirming_unsaved());
    assert_eq!(app.settings_popup.draft().server.port, 9014);
    assert!(app.take_settings_save_request().is_none());
}

#[test]
fn settings_close_button_applies_valid_inline_edit_but_retains_invalid_input() {
    for (input, valid) in [("9014", true), ("0", false)] {
        let mut app = App::new(ui_settings(true));
        app.open_settings_popup();
        let original_port = app.settings_popup.draft().server.port;
        focus_settings_content(&mut app);
        app.handle_key_event(key(KeyCode::Enter));
        for _ in 0..5 {
            app.handle_key_event(key(KeyCode::Backspace));
        }
        app.handle_paste(input);
        let (mut ui, buffer) = render_to_buffer_with_size(&mut app, 100, 28);
        click_close(&mut ui, &mut app, &buffer);
        assert!(app.settings_popup.visible);
        assert_eq!(app.settings_popup.is_confirming_unsaved(), valid);
        if valid {
            assert_eq!(app.settings_popup.draft().server.port, 9014);
        } else {
            assert_eq!(app.settings_popup.draft().server.port, original_port);
            let edit = app
                .settings_popup
                .active_field_edit(crate::app::FieldEditKind::ServerPort)
                .unwrap();
            assert_eq!(edit.value, "0");
            assert!(app.settings_popup.error().is_some());
            app.handle_paste("9014");
            app.handle_key_event(key(KeyCode::Enter));
            assert_eq!(app.settings_popup.draft().server.port, 9014);
        }
        assert!(app.take_settings_save_request().is_none());
    }
}

#[test]
fn settings_close_button_dismisses_only_active_dropdown_or_mapping_editor() {
    for mapping_editor in [false, true] {
        let mut app = App::new(ui_settings(true));
        app.open_settings_popup();
        app.settings_popup.draft_mut_for_tests().proxy =
            Some(proxy_settings_with_rule_counts(1, 0));
        app.settings_popup
            .select_topic_for_tests(SettingsTopic::Proxy);
        if mapping_editor {
            focus_settings_content(&mut app);
            app.settings_popup
                .select_proxy_row_for_tests(ProxyRow::RemoteRule(0));
            app.handle_key_event(key(KeyCode::Enter));
            assert!(app.settings_popup.rule_editor().is_some());
            app.handle_paste("/unfinished");
        } else {
            app.settings_popup.start_proxy_preset_select();
            app.handle_paste("unmatched");
        }
        let draft = app.settings_popup.draft().clone();
        let (mut ui, buffer) = render_to_buffer_with_size(&mut app, 100, 28);
        click_close(&mut ui, &mut app, &buffer);
        assert!(app.settings_popup.visible);
        assert!(!app.settings_popup.is_confirming_unsaved());
        assert!(app.settings_popup.select_state().is_none());
        assert!(app.settings_popup.rule_editor().is_none());
        assert_eq!(app.settings_popup.draft(), &draft);
        assert!(app.take_settings_save_request().is_none());
    }
}

#[test]
fn settings_close_button_respects_pending_transaction_and_nonleft_events() {
    let mut app = App::new(ui_settings(true));
    app.open_settings_popup();
    let (mut ui, buffer) = render_to_buffer_with_size(&mut app, 100, 28);
    let close = close_button(&buffer);
    for kind in [
        MouseEventKind::Down(MouseButton::Right),
        MouseEventKind::Down(MouseButton::Middle),
        MouseEventKind::Up(MouseButton::Left),
        MouseEventKind::Drag(MouseButton::Left),
        MouseEventKind::Moved,
        MouseEventKind::ScrollDown,
    ] {
        ui.handle_mouse(mouse(kind, close.x, close.y), &mut app);
        assert!(app.settings_popup.visible);
    }
    app.set_settings_transaction_pending(true);
    click_close(&mut ui, &mut app, &buffer);
    assert!(app.settings_popup.visible);
    assert!(!app.settings_popup.is_confirming_unsaved());
    assert!(app.take_settings_save_request().is_none());
    app.set_settings_transaction_pending(false);
    let buffer = redraw(&mut ui, &mut app, 36, 12);
    click_close(&mut ui, &mut app, &buffer);
    assert!(!app.settings_popup.visible);
}
