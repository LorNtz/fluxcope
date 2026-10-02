use super::*;

#[test]
fn table_surfaces_focus_empty_tables_for_keyboard_add() {
    for (topic, title) in [
        (SettingsTopic::Recording, "Included URL Patterns"),
        (SettingsTopic::Proxy, "Map Remote Rules"),
        (SettingsTopic::Proxy, "Map Local Rules"),
    ] {
        for surface in ["title", "body", "border"] {
            let mut app = App::new(ui_settings(true));
            app.open_settings_popup();
            app.settings_popup.select_topic_for_tests(topic);
            if title == "Map Local Rules" {
                app.settings_popup
                    .select_proxy_row_for_tests(crate::app::ProxyRow::LocalHeader);
                focus_settings_content(&mut app);
                app.handle_key_event(key(KeyCode::Left));
            }
            let (mut ui, buffer) = render_to_buffer_with_size(&mut app, 100, 28);
            let heading =
                find_buffer_text(&buffer, settings_content_test_area(buffer.area), title).unwrap();
            let position = match surface {
                "body" => Position::new(heading.x + 2, heading.y + 2),
                "border" => Position::new(heading.x - 2, heading.y + 1),
                _ => heading,
            };
            click(&mut ui, &mut app, position);
            assert_eq!(
                app.settings_popup.focus,
                SettingsPaneFocus::Content,
                "{title}: {surface}"
            );
            assert!(!app.settings_popup.is_dirty());
            app.handle_key_event(key(KeyCode::Char('a')));
            match topic {
                SettingsTopic::Recording => assert_eq!(
                    app.settings_popup
                        .draft()
                        .recording
                        .prefilter
                        .include_url_patterns
                        .len(),
                    1
                ),
                _ => {
                    let preset = app.settings_popup.active_proxy_preset().unwrap();
                    let (remote, local) =
                        (preset.map_remote.rules.len(), preset.map_local.rules.len());
                    assert_eq!(
                        (remote, local),
                        if title == "Map Local Rules" {
                            (0, 1)
                        } else {
                            (1, 0)
                        }
                    );
                }
            }
        }
    }
}

#[test]
fn focused_table_background_keeps_selected_row() {
    let mut app = App::new(ui_settings(true));
    app.open_settings_popup();
    app.settings_popup
        .select_topic_for_tests(SettingsTopic::Recording);
    app.settings_popup
        .draft_mut_for_tests()
        .recording
        .prefilter
        .include_url_patterns = vec![
        RecordingPrefilterPatternSettings::new("https://one.test/*"),
        RecordingPrefilterPatternSettings::new("https://two.test/*"),
    ];
    app.settings_popup.select_prefilter_pattern(1);
    let (mut ui, buffer) = render_to_buffer_with_size(&mut app, 100, 28);
    let heading = find_buffer_text(&buffer, buffer.area, "Included URL Patterns").unwrap();
    click(&mut ui, &mut app, heading);
    assert_eq!(app.settings_popup.active_prefilter_pattern(), Some(1));
    assert!(app.settings_popup.prefilter_pattern_edit().is_none());
}

#[test]
fn settings_mouse_double_click_opens_same_prefilter_row() {
    let mut app = App::new(ui_settings(true));
    app.open_settings_popup();
    app.settings_popup
        .select_topic_for_tests(SettingsTopic::Recording);
    app.settings_popup
        .draft_mut_for_tests()
        .recording
        .prefilter
        .include_url_patterns = vec![
        RecordingPrefilterPatternSettings::new("https://one.test/*"),
        RecordingPrefilterPatternSettings::new("https://two.test/*"),
    ];
    let (mut ui, buffer) = render_to_buffer_with_size(&mut app, 100, 28);
    let row = find_buffer_text(&buffer, buffer.area, "https://two.test/*").unwrap();
    click(&mut ui, &mut app, row);
    assert!(app.settings_popup.prefilter_pattern_edit().is_none());
    ui.handle_mouse(
        mouse(MouseEventKind::Up(MouseButton::Left), row.x, row.y),
        &mut app,
    );
    ui.handle_mouse(mouse(MouseEventKind::Moved, row.x, row.y), &mut app);
    redraw(&mut ui, &mut app, 100, 28);
    click(&mut ui, &mut app, row);
    assert!(app.settings_popup.prefilter_pattern_edit().is_some());
}

#[test]
fn settings_mouse_checkbox_and_scroll_interrupt_row_double_click_sequence() {
    for interrupt in ["checkbox", "scroll", "topic", "resize", "rebuild"] {
        let mut app = App::new(ui_settings(true));
        app.open_settings_popup();
        app.settings_popup
            .select_topic_for_tests(SettingsTopic::Recording);
        app.settings_popup
            .draft_mut_for_tests()
            .recording
            .prefilter
            .include_url_patterns = (0..20)
            .map(|i| RecordingPrefilterPatternSettings::new(format!("https://r{i}.test/*")))
            .collect();
        let (mut ui, buffer) = render_to_buffer_with_size(&mut app, 100, 28);
        let row = find_buffer_text(&buffer, buffer.area, "https://r1.test/*").unwrap();
        click(&mut ui, &mut app, row);
        let buffer = redraw(&mut ui, &mut app, 100, 28);
        match interrupt {
            "checkbox" => {
                let checkbox = table_checkbox_on_value_row(
                    &buffer,
                    settings_content_test_area(buffer.area),
                    row,
                );
                click(&mut ui, &mut app, checkbox);
                assert!(app.settings_popup.prefilter_pattern_edit().is_none());
            }
            "scroll" => {
                ui.handle_mouse(mouse(MouseEventKind::ScrollDown, row.x, row.y), &mut app);
                redraw(&mut ui, &mut app, 100, 28);
                ui.handle_mouse(mouse(MouseEventKind::ScrollUp, row.x, row.y), &mut app);
            }
            "topic" => {
                app.settings_popup
                    .select_topic_for_tests(SettingsTopic::Server);
                app.settings_popup
                    .select_topic_for_tests(SettingsTopic::Recording);
            }
            "rebuild" => {
                app.settings_popup
                    .draft_mut_for_tests()
                    .recording
                    .prefilter
                    .include_url_patterns[1]
                    .pattern = "https://replacement.test/*".into()
            }
            _ => {}
        }
        let width = if interrupt == "resize" { 101 } else { 100 };
        let buffer = redraw(&mut ui, &mut app, width, 28);
        let text = if interrupt == "rebuild" {
            "https://replacement.test/*"
        } else {
            "https://r1.test/*"
        };
        let row = find_buffer_text(&buffer, buffer.area, text).unwrap();
        click(&mut ui, &mut app, row);
        assert!(
            app.settings_popup.prefilter_pattern_edit().is_none(),
            "{interrupt}"
        );
    }
}

#[test]
fn settings_mouse_scrolled_local_row_checkbox_does_not_open_editor_or_enable_parent() {
    let mut app = App::new(ui_settings(true));
    app.open_settings_popup();
    let mut proxy = proxy_settings_with_rule_counts(1, 1);
    proxy.enable = false;
    proxy.presets[0].map_local.enable = false;
    proxy.presets[0].map_local.rules[0].from = "https://c.test".into();
    proxy.presets[0].map_local.rules[0].to = "/tmp/mouse.txt".into();
    app.settings_popup.draft_mut_for_tests().proxy = Some(proxy);
    app.settings_popup
        .select_topic_for_tests(SettingsTopic::Proxy);
    app.settings_popup
        .select_proxy_row_for_tests(crate::app::ProxyRow::LocalHeader);
    focus_settings_content(&mut app);
    let (mut ui, buffer) = render_to_buffer_with_size(&mut app, 100, 20);
    let from = find_buffer_text(&buffer, buffer.area, "https://c.test").unwrap();
    let checkbox =
        table_checkbox_on_value_row(&buffer, settings_content_test_area(buffer.area), from);
    click(&mut ui, &mut app, checkbox);
    redraw(&mut ui, &mut app, 100, 20);
    click(&mut ui, &mut app, checkbox);
    assert!(app.settings_popup.rule_editor().is_none());
    let proxy = app.settings_popup.draft().proxy.as_ref().unwrap();
    assert!(!proxy.enable);
    assert!(!proxy.presets[0].map_local.enable);
    assert!(proxy.presets[0].map_local.rules[0].enable);
    redraw(&mut ui, &mut app, 100, 20);
    click(&mut ui, &mut app, from);
    assert!(app.settings_popup.rule_editor().is_none());
    redraw(&mut ui, &mut app, 100, 20);
    click(&mut ui, &mut app, from);
    assert_eq!(
        app.settings_popup.rule_editor().unwrap().active_field,
        crate::app::RuleEditField::From
    );
}

#[test]
fn settings_mouse_double_click_requires_same_row_within_interval() {
    for different_row in [false, true] {
        let mut app = App::new(ui_settings(true));
        app.open_settings_popup();
        app.settings_popup
            .select_topic_for_tests(SettingsTopic::Recording);
        app.settings_popup
            .draft_mut_for_tests()
            .recording
            .prefilter
            .include_url_patterns = vec![
            RecordingPrefilterPatternSettings::new("https://one.test/*"),
            RecordingPrefilterPatternSettings::new("https://two.test/*"),
        ];
        let (mut ui, buffer) = render_to_buffer_with_size(&mut app, 100, 28);
        let first = find_buffer_text(&buffer, buffer.area, "https://one.test/*").unwrap();
        let second = if different_row {
            find_buffer_text(&buffer, buffer.area, "https://two.test/*").unwrap()
        } else {
            first
        };
        let now = Instant::now();
        click_at(&mut ui, &mut app, first, now);
        redraw(&mut ui, &mut app, 100, 28);
        click_at(
            &mut ui,
            &mut app,
            second,
            now + Duration::from_millis(if different_row { 20 } else { 401 }),
        );
        assert!(app.settings_popup.prefilter_pattern_edit().is_none());
        assert_eq!(
            app.settings_popup.active_prefilter_pattern(),
            Some(usize::from(different_row))
        );
    }
}
