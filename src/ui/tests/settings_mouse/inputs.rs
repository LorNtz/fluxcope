use super::*;

#[test]
fn settings_mouse_field_text_positions_cursor_before_insertion() {
    let mut app = App::new(ui_settings(true));
    app.open_settings_popup();
    app.settings_popup
        .draft_mut_for_tests()
        .certificate
        .pem_filename = "abc.pem".to_string();
    app.settings_popup
        .select_topic_for_tests(SettingsTopic::Certificate);
    let (mut ui, buffer) = render_to_buffer_with_size(&mut app, 100, 28);
    let value = find_buffer_text(&buffer, buffer.area, "abc.pem").unwrap();
    click(&mut ui, &mut app, Position::new(value.x + 1, value.y));
    app.handle_paste("X");
    app.handle_key_event(key(KeyCode::Enter));
    assert_eq!(
        app.settings_popup.draft().certificate.pem_filename,
        "aXbc.pem"
    );
}

#[test]
fn settings_mouse_invalid_inline_port_blocks_topic_and_outside_close() {
    let mut app = App::new(ui_settings(true));
    app.open_settings_popup();
    focus_settings_content(&mut app);
    app.handle_key_event(key(KeyCode::Enter));
    for _ in 0..5 {
        app.handle_key_event(key(KeyCode::Backspace));
    }
    app.handle_paste("0");
    app.handle_key_event(key(KeyCode::Enter));
    let (mut ui, buffer) = render_to_buffer_with_size(&mut app, 100, 28);
    let topic = find_buffer_text(&buffer, buffer.area, "Certificate").unwrap();
    click(&mut ui, &mut app, topic);
    assert_eq!(app.settings_popup.topic, SettingsTopic::Server);
    redraw(&mut ui, &mut app, 100, 28);
    click(&mut ui, &mut app, Position::ORIGIN);
    assert!(app.settings_popup.visible);
    app.handle_paste("9014");
    app.handle_key_event(key(KeyCode::Enter));
    assert_eq!(app.settings_popup.draft().server.port, 9014);
}

#[test]
fn settings_mouse_input_chrome_preserves_cursor_and_switch_applies_inline_value() {
    let mut app = App::new(ui_settings(true));
    app.open_settings_popup();
    app.settings_popup
        .draft_mut_for_tests()
        .certificate
        .pem_filename = "abc.pem".into();
    app.settings_popup
        .select_topic_for_tests(SettingsTopic::Certificate);
    let (mut ui, buffer) = render_to_buffer_with_size(&mut app, 100, 28);
    let value = find_buffer_text(&buffer, buffer.area, "abc.pem").unwrap();
    click(&mut ui, &mut app, Position::new(value.x + 1, value.y));
    let buffer = redraw(&mut ui, &mut app, 100, 28);
    let label = find_buffer_text(&buffer, buffer.area, "CA PEM filename").unwrap();
    click(&mut ui, &mut app, label);
    app.handle_paste("X");
    let buffer = redraw(&mut ui, &mut app, 100, 28);
    let topic = find_buffer_text(&buffer, settings_popup_area(buffer.area), "Recording").unwrap();
    click(&mut ui, &mut app, topic);
    assert_eq!(app.settings_popup.topic, SettingsTopic::Recording);
    assert_eq!(
        app.settings_popup.draft().certificate.pem_filename,
        "aXbc.pem"
    );
    assert!(app.take_settings_save_request().is_none());
}

#[test]
fn settings_mouse_wide_character_and_scrolled_text_insert_at_displayed_boundaries() {
    for long in [false, true] {
        let mut app = App::new(ui_settings(true));
        app.open_settings_popup();
        let prefix = if long {
            "long-directory/".repeat(12)
        } else {
            String::new()
        };
        let value = format!("{prefix}a界e\u{301}z");
        app.settings_popup
            .draft_mut_for_tests()
            .certificate
            .store_dir = value.clone();
        app.settings_popup
            .select_topic_for_tests(SettingsTopic::Certificate);
        focus_settings_content(&mut app);
        app.handle_key_event(key(KeyCode::Enter));
        let (mut ui, buffer) = render_to_buffer_with_size(&mut app, 100, 28);
        let position = buffer
            .content
            .iter()
            .enumerate()
            .find(|(_, cell)| cell.symbol() == "界")
            .map(|(i, _)| Position::new(i as u16 % buffer.area.width, i as u16 / buffer.area.width))
            .expect("wide character should be visible near active cursor");
        click(&mut ui, &mut app, Position::new(position.x + 1, position.y));
        app.handle_paste("X");
        app.handle_key_event(key(KeyCode::Enter));
        assert_eq!(
            app.settings_popup.draft().certificate.store_dir,
            format!("{prefix}aX界e\u{301}z")
        );
    }
}

#[test]
fn settings_mouse_prefilter_editor_click_repositions_and_switch_applies_without_validation() {
    let mut app = App::new(ui_settings(true));
    app.open_settings_popup();
    app.settings_popup
        .select_topic_for_tests(SettingsTopic::Recording);
    app.settings_popup
        .draft_mut_for_tests()
        .recording
        .prefilter
        .include_url_patterns = vec![RecordingPrefilterPatternSettings::new("https://api.test/*")];
    app.settings_popup.select_prefilter_pattern(0);
    app.handle_key_event(key(KeyCode::Enter));
    let (mut ui, buffer) = render_to_buffer_with_size(&mut app, 100, 28);
    let value = find_buffer_text(&buffer, buffer.area, "https://api.test/*").unwrap();
    click(&mut ui, &mut app, Position::new(value.x + 8, value.y));
    app.handle_paste("new.");
    let buffer = redraw(&mut ui, &mut app, 100, 28);
    let checkbox = find_buffer_text(&buffer, buffer.area, "URL prefilter enabled").unwrap();
    click(&mut ui, &mut app, checkbox);
    assert_eq!(
        app.settings_popup
            .draft()
            .recording
            .prefilter
            .include_url_patterns[0]
            .pattern,
        "https://new.api.test/*"
    );
    assert!(!app.settings_popup.draft().recording.prefilter.enable);
}

#[test]
fn blank_settings_space_finishes_inline_edits_without_saving() {
    let mut app = App::new(ui_settings(true));
    app.open_settings_popup();
    focus_settings_content(&mut app);
    app.handle_key_event(key(KeyCode::Enter));
    for _ in 0..5 {
        app.handle_key_event(key(KeyCode::Backspace));
    }
    app.handle_paste("9014");
    let (mut ui, buffer) = render_to_buffer_with_size(&mut app, 100, 28);
    let viewport = settings_content_test_area(buffer.area);
    click(
        &mut ui,
        &mut app,
        Position::new(viewport.x + 2, viewport.bottom() - 2),
    );
    assert_eq!(app.settings_popup.draft().server.port, 9014);
    assert!(
        app.settings_popup
            .active_field_edit(crate::app::FieldEditKind::ServerPort)
            .is_none()
    );
    assert!(app.take_settings_save_request().is_none());

    app.settings_popup
        .select_topic_for_tests(SettingsTopic::Recording);
    app.settings_popup
        .draft_mut_for_tests()
        .recording
        .prefilter
        .include_url_patterns = vec![RecordingPrefilterPatternSettings::new("https://one.test/*")];
    app.settings_popup.select_prefilter_pattern(0);
    app.handle_key_event(key(KeyCode::Enter));
    app.handle_paste("suffix");
    redraw(&mut ui, &mut app, 100, 28);
    click(
        &mut ui,
        &mut app,
        Position::new(viewport.x + 2, viewport.bottom() - 2),
    );
    assert!(app.settings_popup.prefilter_pattern_edit().is_none());
    assert_eq!(
        app.settings_popup
            .draft()
            .recording
            .prefilter
            .include_url_patterns[0]
            .pattern,
        "https://one.test/*suffix"
    );
    assert!(app.take_settings_save_request().is_none());
}

#[test]
fn rejected_inline_blur_keeps_text_until_cancelled() {
    let mut app = App::new(ui_settings(true));
    app.open_settings_popup();
    let original = app.settings_popup.draft().server.port;
    focus_settings_content(&mut app);
    app.handle_key_event(key(KeyCode::Enter));
    for _ in 0..5 {
        app.handle_key_event(key(KeyCode::Backspace));
    }
    app.handle_paste("0");
    let (mut ui, buffer) = render_to_buffer_with_size(&mut app, 100, 28);
    let viewport = settings_content_test_area(buffer.area);
    click(
        &mut ui,
        &mut app,
        Position::new(viewport.x + 2, viewport.bottom() - 2),
    );
    let edit = app
        .settings_popup
        .active_field_edit(crate::app::FieldEditKind::ServerPort)
        .unwrap();
    assert_eq!(edit.value, "0");
    assert!(app.settings_popup.error().is_some());
    assert_eq!(app.settings_popup.draft().server.port, original);
    app.handle_key_event(key(KeyCode::Esc));
    assert!(
        app.settings_popup
            .active_field_edit(crate::app::FieldEditKind::ServerPort)
            .is_none()
    );
}
