use super::*;

#[test]
fn settings_popup_recording_topic_renders_prefilter_controls_and_patterns() {
    let mut app = App::new(ui_settings(true));
    app.open_settings_popup();
    app.settings_popup
        .select_topic_for_tests(SettingsTopic::Recording);
    app.settings_popup
        .draft_mut_for_tests()
        .recording
        .prefilter
        .include_url_patterns = vec![RecordingPrefilterPatternSettings::new(
        "https://api.example.com/*?client=*",
    )];

    let (_ui, buffer) = render_to_buffer(&mut app);
    let rendered = (0..buffer.area.height)
        .map(|row| buffer_row(&buffer, row, 0, buffer.area.width))
        .collect::<Vec<_>>()
        .join("\n");

    assert!(rendered.contains("URL Prefilter"), "{rendered}");
    assert!(rendered.contains("URL prefilter enabled"), "{rendered}");
    assert!(rendered.contains("Included URL Patterns"), "{rendered}");
    assert!(rendered.contains("On"), "{rendered}");
    assert!(rendered.contains("[✓]"), "{rendered}");
    assert!(
        rendered.contains("https://api.example.com/*?client=*"),
        "{rendered}"
    );
}

#[test]
fn prefilter_checkbox_muting_tracks_prefilter_instead_of_launch_recording() {
    let mut app = App::new(ui_settings(true));
    app.open_settings_popup();
    app.settings_popup
        .select_topic_for_tests(SettingsTopic::Recording);
    let recording = &mut app.settings_popup.draft_mut_for_tests().recording;
    recording.start_record_on_launch = false;
    recording.prefilter.enable = true;
    recording.prefilter.include_url_patterns = vec![RecordingPrefilterPatternSettings::new(
        "https://api.example.com/*",
    )];

    let (_ui, enabled_buffer) = render_to_buffer_with_size(&mut app, 100, 28);
    let enabled_area = settings_content_test_area(enabled_buffer.area);
    let enabled_pattern =
        find_buffer_text(&enabled_buffer, enabled_area, "https://api.example.com/*")
            .expect("prefilter pattern should render");
    let enabled_checkbox =
        table_checkbox_on_value_row(&enabled_buffer, enabled_area, enabled_pattern);

    assert_eq!(enabled_buffer[enabled_checkbox].fg, Color::Reset);
    assert_eq!(enabled_buffer[enabled_pattern].fg, Color::Reset);

    app.settings_popup
        .draft_mut_for_tests()
        .recording
        .prefilter
        .enable = false;
    let (_ui, disabled_buffer) = render_to_buffer_with_size(&mut app, 100, 28);
    let disabled_area = settings_content_test_area(disabled_buffer.area);
    let disabled_pattern =
        find_buffer_text(&disabled_buffer, disabled_area, "https://api.example.com/*")
            .expect("prefilter pattern should render");
    let disabled_checkbox =
        table_checkbox_on_value_row(&disabled_buffer, disabled_area, disabled_pattern);

    assert_eq!(disabled_buffer[disabled_checkbox].fg, Color::DarkGray);
    assert_eq!(disabled_buffer[disabled_pattern].fg, Color::Reset);
}

#[test]
fn selected_and_edited_suppressed_prefilter_checkbox_stays_distinct() {
    let mut app = App::new(ui_settings(true));
    app.open_settings_popup();
    app.settings_popup
        .select_topic_for_tests(SettingsTopic::Recording);
    app.settings_popup
        .draft_mut_for_tests()
        .recording
        .prefilter
        .enable = false;
    let mut pattern = RecordingPrefilterPatternSettings::new("https://api.example.com/*");
    pattern.enable = false;
    app.settings_popup
        .draft_mut_for_tests()
        .recording
        .prefilter
        .include_url_patterns = vec![pattern];
    app.settings_popup.select_prefilter_pattern(0);

    let (_ui, selected_buffer) = render_to_buffer_with_size(&mut app, 100, 28);
    let selected_area = settings_content_test_area(selected_buffer.area);
    let selected_pattern =
        find_buffer_text(&selected_buffer, selected_area, "https://api.example.com/*")
            .expect("selected prefilter pattern should render");
    let selected_checkbox =
        table_checkbox_on_value_row(&selected_buffer, selected_area, selected_pattern);

    assert_eq!(
        buffer_row(
            &selected_buffer,
            selected_checkbox.y,
            selected_checkbox.x,
            3
        ),
        "[ ]"
    );
    assert_eq!(selected_buffer[selected_checkbox].fg, Color::Gray);
    assert_eq!(selected_buffer[selected_checkbox].bg, Color::White);
    assert_eq!(selected_buffer[selected_pattern].fg, Color::DarkGray);
    assert_eq!(selected_buffer[selected_pattern].bg, Color::White);

    app.handle_key_event(key(KeyCode::Enter));
    assert!(app.settings_popup.prefilter_pattern_edit().is_some());
    let (_ui, edited_buffer) = render_to_buffer_with_size(&mut app, 100, 28);
    let edited_area = settings_content_test_area(edited_buffer.area);
    let edited_pattern = find_buffer_text(&edited_buffer, edited_area, "https://api.example.com/*")
        .expect("edited prefilter pattern should render");
    let edited_checkbox = table_checkbox_on_value_row(&edited_buffer, edited_area, edited_pattern);

    assert_eq!(edited_buffer[edited_checkbox].fg, Color::Gray);
    assert_eq!(edited_buffer[edited_checkbox].bg, Color::White);
}

#[test]
fn settings_popup_mouse_selects_prefilter_pattern_row() {
    let mut app = App::new(ui_settings(true));
    app.open_settings_popup();
    app.settings_popup
        .select_topic_for_tests(SettingsTopic::Recording);
    app.settings_popup
        .draft_mut_for_tests()
        .recording
        .prefilter
        .include_url_patterns = vec![
        RecordingPrefilterPatternSettings::new("https://first.example.com/*"),
        RecordingPrefilterPatternSettings::new("https://second.example.com/*"),
    ];
    let (ui, buffer) = render_to_buffer_with_size(&mut app, 100, 28);
    let content_area = settings_content_test_area(buffer.area);
    let second_pattern = find_buffer_text(&buffer, content_area, "https://second.example.com/*")
        .expect("second prefilter pattern should render");

    ui.handle_mouse(
        mouse(
            MouseEventKind::Down(MouseButton::Left),
            second_pattern.x,
            second_pattern.y,
        ),
        &mut app,
    );

    assert_eq!(app.settings_popup.active_prefilter_pattern(), Some(1));
    assert!(
        app.settings_popup
            .draft()
            .recording
            .prefilter
            .include_url_patterns[1]
            .enable
    );
}

#[test]
fn settings_popup_mouse_toggles_prefilter_pattern_checkbox() {
    let mut app = App::new(ui_settings(true));
    app.open_settings_popup();
    app.settings_popup
        .select_topic_for_tests(SettingsTopic::Recording);
    app.settings_popup
        .draft_mut_for_tests()
        .recording
        .prefilter
        .include_url_patterns = vec![RecordingPrefilterPatternSettings::new(
        "https://api.example.com/*",
    )];
    let (ui, buffer) = render_to_buffer_with_size(&mut app, 100, 28);
    let content_area = settings_content_test_area(buffer.area);
    let pattern = find_buffer_text(&buffer, content_area, "https://api.example.com/*")
        .expect("prefilter pattern should render");

    ui.handle_mouse(
        mouse(
            MouseEventKind::Down(MouseButton::Left),
            pattern.x.saturating_sub(5),
            pattern.y,
        ),
        &mut app,
    );

    assert_eq!(app.settings_popup.active_prefilter_pattern(), Some(0));
    assert!(
        !app.settings_popup
            .draft()
            .recording
            .prefilter
            .include_url_patterns[0]
            .enable
    );
}

#[test]
fn settings_popup_mouse_wheel_scrolls_overflowing_prefilter_table() {
    let mut app = App::new(ui_settings(true));
    app.open_settings_popup();
    app.settings_popup
        .select_topic_for_tests(SettingsTopic::Recording);
    app.settings_popup
        .draft_mut_for_tests()
        .recording
        .prefilter
        .include_url_patterns = (0..10)
        .map(|index| {
            RecordingPrefilterPatternSettings::new(format!("https://api.example.com/v{index}/*"))
        })
        .collect();
    let (ui, buffer) = render_to_buffer_with_size(&mut app, 100, 28);
    let content_area = settings_content_test_area(buffer.area);
    let title = find_buffer_text(&buffer, content_area, "Included URL Patterns")
        .expect("prefilter table should render");

    ui.handle_mouse(
        mouse(
            MouseEventKind::ScrollDown,
            content_area.x.saturating_add(2),
            title.y.saturating_add(2),
        ),
        &mut app,
    );

    assert_eq!(app.settings_popup.prefilter_table_scroll_offset(), 1);
}

#[test]
fn settings_popup_mouse_rejects_stale_prefilter_table_scroll_mapping() {
    let mut app = App::new(ui_settings(true));
    app.open_settings_popup();
    app.settings_popup
        .select_topic_for_tests(SettingsTopic::Recording);
    app.settings_popup
        .draft_mut_for_tests()
        .recording
        .prefilter
        .include_url_patterns = (0..10)
        .map(|index| {
            RecordingPrefilterPatternSettings::new(format!("https://api.example.com/v{index}/*"))
        })
        .collect();
    let (ui, buffer) = render_to_buffer_with_size(&mut app, 100, 28);
    let content_area = settings_content_test_area(buffer.area);
    let first_pattern = find_buffer_text(&buffer, content_area, "https://api.example.com/v0/*")
        .expect("first prefilter pattern should render");

    assert!(app.settings_popup.scroll_prefilter_table_down());
    ui.handle_mouse(
        mouse(
            MouseEventKind::Down(MouseButton::Left),
            first_pattern.x,
            first_pattern.y,
        ),
        &mut app,
    );

    assert_eq!(app.settings_popup.active_prefilter_pattern(), None);
}
