use super::*;

#[test]
fn unsaved_dialog_visibly_tracks_the_action_enter_will_activate() {
    let mut app = App::new(ui_settings(true));
    app.open_settings_popup();
    app.settings_popup
        .draft_mut_for_tests()
        .recording
        .start_record_on_launch = false;
    app.handle_key_event(key(KeyCode::Esc));

    let (_, buffer) = render_to_buffer(&mut app);
    let area = action_dialog_test_area(buffer.area, &app);
    let keep = find_buffer_text(&buffer, area, "[x]").expect("close control");
    let save = find_buffer_text(&buffer, area, "Save").expect("save action");
    let discard = find_buffer_text(&buffer, area, "Discard").expect("discard action");
    assert_eq!(buffer[keep].fg, Color::Green);
    assert_ne!(buffer[save].fg, Color::Green);
    assert_ne!(buffer[discard].fg, Color::Green);

    app.handle_key_event(key(KeyCode::Right));
    let (_, buffer) = render_to_buffer(&mut app);
    let save = find_buffer_text(&buffer, area, "Save").expect("save action");
    let keep = find_buffer_text(&buffer, area, "[x]").expect("close control");
    assert_eq!(buffer[save].fg, Color::Green);
    assert_ne!(buffer[keep].fg, Color::Green);
}

#[test]
fn ephemeral_unsaved_dialog_distinguishes_apply_from_persistent_save() {
    let mut app = app_with_settings_context(SettingsUiContext {
        config_mode: ConfigMode::Temporary,
        persistence: PersistenceMode::Ephemeral,
    });
    app.open_settings_popup();
    app.settings_popup
        .draft_mut_for_tests()
        .recording
        .start_record_on_launch = false;
    app.handle_key_event(key(KeyCode::Esc));
    let (_, buffer) = render_to_buffer(&mut app);
    let area = action_dialog_test_area(buffer.area, &app);
    assert!(find_buffer_text(&buffer, area, "Apply").is_some());
    assert!(find_buffer_text(&buffer, area, "Save").is_none());
}

#[test]
fn unsaved_settings_dialog_buttons_do_not_overwrite_narrow_dialog_border() {
    let mut app = App::new(ui_settings(true));
    app.open_settings_popup();
    app.settings_popup
        .draft_mut_for_tests()
        .recording
        .start_record_on_launch = false;
    app.handle_key_event(key(KeyCode::Esc));

    let (_, buffer) = render_to_buffer_with_size(&mut app, 36, 12);
    let dialog_area = action_dialog_test_area(buffer.area, &app);
    let button_label_row = dialog_area.bottom().saturating_sub(3);
    assert_eq!(
        buffer[(dialog_area.right() - 1, button_label_row)].symbol(),
        "│"
    );
}
