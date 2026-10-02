use super::*;

#[test]
fn settings_mouse_confirmation_initial_enter_keeps_draft() {
    let mut app = App::new(ui_settings(true));
    app.open_settings_popup();
    app.settings_popup.draft_mut_for_tests().server.port = 9014;
    app.handle_key_event(key(KeyCode::Esc));
    app.handle_key_event(key(KeyCode::Enter));
    assert!(app.settings_popup.visible);
    assert!(!app.settings_popup.is_confirming_unsaved());
    assert_eq!(app.settings_popup.draft().server.port, 9014);
    assert!(app.take_settings_save_request().is_none());
}

#[test]
fn settings_mouse_confirmation_buttons_route_save_discard_and_keep_editing() {
    for ephemeral in [false, true] {
        for action in ["commit", "Discard", "[x]"] {
            let mut app = if ephemeral {
                app_with_settings_context(SettingsUiContext {
                    config_mode: ConfigMode::Temporary,
                    persistence: PersistenceMode::Ephemeral,
                })
            } else {
                App::new(ui_settings(true))
            };
            app.open_settings_popup();
            app.settings_popup.draft_mut_for_tests().server.port = 9014;
            app.handle_key_event(key(KeyCode::Esc));
            let (mut ui, buffer) = render_to_buffer_with_size(&mut app, 100, 28);
            let label = if action == "commit" {
                if ephemeral { "Apply" } else { "Save" }
            } else {
                action
            };
            let button =
                find_buffer_text(&buffer, action_dialog_test_area(buffer.area, &app), label)
                    .unwrap();
            click(&mut ui, &mut app, button);
            match action {
                "commit" => assert_eq!(app.take_settings_save_request().unwrap().server.port, 9014),
                "Discard" => {
                    assert!(!app.settings_popup.visible);
                    app.open_settings_popup();
                    assert_ne!(app.settings_popup.draft().server.port, 9014);
                }
                _ => {
                    assert!(app.settings_popup.visible);
                    assert!(!app.settings_popup.is_confirming_unsaved());
                    assert_eq!(app.settings_popup.draft().server.port, 9014);
                    assert!(app.take_settings_save_request().is_none());
                }
            }
        }
    }
}

#[test]
fn settings_mouse_outside_confirmation_keeps_draft_and_pending_blocks_discard() {
    let mut app = App::new(ui_settings(true));
    app.open_settings_popup();
    app.settings_popup.draft_mut_for_tests().server.port = 9014;
    app.handle_key_event(key(KeyCode::Esc));
    let (mut ui, _) = render_to_buffer_with_size(&mut app, 100, 28);
    click(&mut ui, &mut app, Position::ORIGIN);
    assert!(app.settings_popup.visible);
    assert!(!app.settings_popup.is_confirming_unsaved());
    assert_eq!(app.settings_popup.draft().server.port, 9014);
    app.handle_key_event(key(KeyCode::Esc));
    app.set_settings_transaction_pending(true);
    let buffer = redraw(&mut ui, &mut app, 100, 28);
    let discard = find_buffer_text(
        &buffer,
        action_dialog_test_area(buffer.area, &app),
        "Discard",
    )
    .unwrap();
    click(&mut ui, &mut app, discard);
    assert!(app.settings_popup.visible);
    assert_eq!(app.settings_popup.draft().server.port, 9014);
    assert!(app.take_settings_save_request().is_none());
}

#[test]
fn settings_mouse_failed_save_preserves_draft_and_pending_click_preserves_inline_text() {
    let mut app = App::new(ui_settings(true));
    app.open_settings_popup();
    app.settings_popup.draft_mut_for_tests().server.port = 0;
    app.handle_key_event(key(KeyCode::Esc));
    let (mut ui, buffer) = render_to_buffer_with_size(&mut app, 100, 28);
    let save =
        find_buffer_text(&buffer, action_dialog_test_area(buffer.area, &app), "Save").unwrap();
    click(&mut ui, &mut app, save);
    assert!(app.settings_popup.visible);
    assert!(!app.settings_popup.is_confirming_unsaved());
    assert!(app.settings_popup.error().is_some());
    assert_eq!(app.settings_popup.draft().server.port, 0);
    assert!(app.take_settings_save_request().is_none());
    focus_settings_content(&mut app);
    app.handle_key_event(key(KeyCode::Enter));
    app.handle_key_event(key(KeyCode::Backspace));
    app.handle_paste("9014");
    app.set_settings_transaction_pending(true);
    let buffer = redraw(&mut ui, &mut app, 100, 28);
    let topic = find_buffer_text(&buffer, buffer.area, "Certificate").unwrap();
    click(&mut ui, &mut app, topic);
    app.set_settings_transaction_pending(false);
    app.handle_key_event(key(KeyCode::Enter));
    assert_eq!(app.settings_popup.draft().server.port, 9014);
}

#[test]
fn confirmation_close_control_keeps_draft_in_normal_and_narrow_terminals() {
    for (width, height) in [(100, 28), (36, 12)] {
        let mut app = App::new(ui_settings(true));
        app.open_settings_popup();
        app.settings_popup.draft_mut_for_tests().server.port = 9014;
        app.handle_key_event(key(KeyCode::Esc));
        let (mut ui, buffer) = render_to_buffer_with_size(&mut app, width, height);
        let area = action_dialog_test_area(buffer.area, &app);
        let close = find_buffer_text(&buffer, area, "[x]").expect("visible close control");
        assert_eq!(close.y, area.y);
        assert!(close.x > area.x + area.width / 2);
        click(&mut ui, &mut app, close);
        assert!(app.settings_popup.visible);
        assert!(!app.settings_popup.is_confirming_unsaved());
        assert_eq!(app.settings_popup.draft().server.port, 9014);
        assert!(app.take_settings_save_request().is_none());
    }
}

#[test]
fn confirmation_advertised_shortcuts_commit_or_discard_through_existing_boundary() {
    for ephemeral in [false, true] {
        for discard in [false, true] {
            let mut app = if ephemeral {
                app_with_settings_context(SettingsUiContext {
                    config_mode: ConfigMode::Temporary,
                    persistence: PersistenceMode::Ephemeral,
                })
            } else {
                App::new(ui_settings(true))
            };
            app.open_settings_popup();
            app.settings_popup.draft_mut_for_tests().server.port = 9014;
            app.handle_key_event(key(KeyCode::Esc));
            let (_, buffer) = render_to_buffer_with_size(&mut app, 100, 28);
            let area = action_dialog_test_area(buffer.area, &app);
            let label = if discard {
                "Discard [d]"
            } else if ephemeral {
                "Apply [a]"
            } else {
                "Save [s]"
            };
            assert!(find_buffer_text(&buffer, area, label).is_some());
            app.handle_key_event(key(KeyCode::Char(if discard {
                'd'
            } else if ephemeral {
                'a'
            } else {
                's'
            })));
            if discard {
                assert!(!app.settings_popup.visible);
                assert!(app.take_settings_save_request().is_none());
            } else {
                assert_eq!(app.take_settings_save_request().unwrap().server.port, 9014);
            }
        }
    }
}

#[test]
fn confirmation_shortcuts_respect_validation_and_pending_transactions() {
    let mut app = App::new(ui_settings(true));
    app.open_settings_popup();
    app.settings_popup.draft_mut_for_tests().server.port = 0;
    app.handle_key_event(key(KeyCode::Esc));
    app.handle_key_event(key(KeyCode::Char('s')));
    assert!(!app.settings_popup.is_confirming_unsaved());
    assert!(app.settings_popup.error().is_some());
    assert!(app.take_settings_save_request().is_none());
    app.settings_popup.draft_mut_for_tests().server.port = 9014;
    app.handle_key_event(key(KeyCode::Esc));
    app.set_settings_transaction_pending(true);
    for code in ['s', 'd'] {
        app.handle_key_event(key(KeyCode::Char(code)));
        assert!(app.settings_popup.is_confirming_unsaved());
        assert_eq!(app.settings_popup.draft().server.port, 9014);
        assert!(app.take_settings_save_request().is_none());
    }
}
