use super::*;

#[test]
fn settings_popup_proxy_page_renders_preset_dividers_and_rule_tables() {
    let mut app = App::new(ui_settings(true));
    app.open_settings_popup();
    app.settings_popup
        .select_topic_for_tests(SettingsTopic::Proxy);
    app.settings_popup.draft_mut_for_tests().proxy = Some(crate::settings::ProxySettings {
        enable: true,
        active_preset: Some("dev".to_string()),
        presets: vec![crate::settings::ProxyPresetSettings {
            name: "dev".to_string(),
            map_remote: crate::settings::ProxyMapRemoteSettings {
                enable: false,
                rules: vec![crate::settings::ProxyMapRemoteRule {
                    from: "https://api.example.com".to_string(),
                    to: "http://localhost:3000".to_string(),
                    enable: true,
                }],
            },
            map_local: crate::settings::ProxyMapLocalSettings {
                enable: false,
                rules: vec![crate::settings::ProxyMapLocalRule {
                    from: "https://static.example.com".to_string(),
                    to: "~/fixtures/app.js".to_string(),
                    enable: false,
                }],
            },
        }],
    });

    let (_ui, buffer) = render_to_buffer_with_size(&mut app, 100, 28);
    let rendered = (0..buffer.area.height)
        .map(|row| buffer_row(&buffer, row, 0, buffer.area.width))
        .collect::<Vec<_>>()
        .join("\n");

    assert!(rendered.contains("Preset"));
    assert!(rendered.contains("Preset name"));
    assert!(rendered.contains("Mapping enabled"));
    assert!(rendered.contains("─ Map Remote ─"));
    assert!(rendered.contains("Map remote enabled"));
    assert!(rendered.contains("─ Map Local ─"));
    assert!(rendered.contains("Map local enabled"));
    assert!(rendered.contains("dev"));
    assert!(rendered.contains("Map Remote Rules"));
    assert!(rendered.contains("Map Local Rules"));
    assert!(rendered.contains("On"));
    assert!(rendered.contains("From"));
    assert!(rendered.contains("To"));
    assert!(rendered.contains("https://api.example.com"));
    assert!(rendered.contains("http://localhost:3000"));
}

#[test]
fn mapping_disabled_mutes_remote_and_local_rule_checkboxes_only() {
    let mut app = App::new(ui_settings(true));
    app.open_settings_popup();
    app.settings_popup
        .select_topic_for_tests(SettingsTopic::Proxy);
    let mut proxy = proxy_settings_with_rule_counts(1, 1);
    proxy.enable = false;
    app.settings_popup.draft_mut_for_tests().proxy = Some(proxy);

    let (_ui, remote_buffer) = render_to_buffer_with_size(&mut app, 100, 28);
    let remote_area = settings_content_test_area(remote_buffer.area);
    let remote_value = find_buffer_text(&remote_buffer, remote_area, "http://localhost:3000")
        .expect("remote mapping rule should render");
    let remote_checkbox = table_checkbox_on_value_row(&remote_buffer, remote_area, remote_value);

    assert_eq!(remote_buffer[remote_checkbox].fg, Color::DarkGray);
    assert_eq!(remote_buffer[remote_value].fg, Color::Reset);

    app.settings_popup
        .select_proxy_row_for_tests(ProxyRow::LocalHeader);
    focus_settings_content(&mut app);
    let (_ui, local_buffer) = render_to_buffer_with_size(&mut app, 100, 28);
    let local_area = settings_content_test_area(local_buffer.area);
    let local_value = find_buffer_text(&local_buffer, local_area, "~/fixtures/app0.js")
        .expect("local mapping rule should render");
    let local_checkbox = table_checkbox_on_value_row(&local_buffer, local_area, local_value);

    assert_eq!(local_buffer[local_checkbox].fg, Color::DarkGray);
    assert_eq!(local_buffer[local_value].fg, Color::Reset);
}

#[test]
fn mapping_section_toggle_mutes_only_its_own_rule_checkboxes() {
    let mut app = App::new(ui_settings(true));
    app.open_settings_popup();
    app.settings_popup
        .select_topic_for_tests(SettingsTopic::Proxy);
    let mut proxy = proxy_settings_with_rule_counts(1, 1);
    proxy.presets[0].map_remote.enable = false;
    app.settings_popup.draft_mut_for_tests().proxy = Some(proxy);

    let (_ui, remote_buffer) = render_to_buffer_with_size(&mut app, 100, 28);
    let remote_area = settings_content_test_area(remote_buffer.area);
    let remote_value = find_buffer_text(&remote_buffer, remote_area, "http://localhost:3000")
        .expect("remote mapping rule should render");
    let remote_checkbox = table_checkbox_on_value_row(&remote_buffer, remote_area, remote_value);

    assert_eq!(remote_buffer[remote_checkbox].fg, Color::DarkGray);

    app.settings_popup
        .select_proxy_row_for_tests(ProxyRow::LocalHeader);
    focus_settings_content(&mut app);
    let (_ui, local_buffer) = render_to_buffer_with_size(&mut app, 100, 28);
    let local_area = settings_content_test_area(local_buffer.area);
    let local_value = find_buffer_text(&local_buffer, local_area, "~/fixtures/app0.js")
        .expect("local mapping rule should render");
    let local_checkbox = table_checkbox_on_value_row(&local_buffer, local_area, local_value);

    assert_eq!(local_buffer[local_checkbox].fg, Color::Reset);
}

#[test]
fn local_mapping_section_toggle_mutes_only_local_rule_checkboxes() {
    let mut app = App::new(ui_settings(true));
    app.open_settings_popup();
    app.settings_popup
        .select_topic_for_tests(SettingsTopic::Proxy);
    let mut proxy = proxy_settings_with_rule_counts(1, 1);
    proxy.presets[0].map_local.enable = false;
    app.settings_popup.draft_mut_for_tests().proxy = Some(proxy);

    let (_ui, remote_buffer) = render_to_buffer_with_size(&mut app, 100, 28);
    let remote_area = settings_content_test_area(remote_buffer.area);
    let remote_value = find_buffer_text(&remote_buffer, remote_area, "http://localhost:3000")
        .expect("remote mapping rule should render");
    let remote_checkbox = table_checkbox_on_value_row(&remote_buffer, remote_area, remote_value);

    assert_eq!(remote_buffer[remote_checkbox].fg, Color::Reset);

    app.settings_popup
        .select_proxy_row_for_tests(ProxyRow::LocalHeader);
    focus_settings_content(&mut app);
    let (_ui, local_buffer) = render_to_buffer_with_size(&mut app, 100, 28);
    let local_area = settings_content_test_area(local_buffer.area);
    let local_value = find_buffer_text(&local_buffer, local_area, "~/fixtures/app0.js")
        .expect("local mapping rule should render");
    let local_checkbox = table_checkbox_on_value_row(&local_buffer, local_area, local_value);

    assert_eq!(local_buffer[local_checkbox].fg, Color::DarkGray);
}

#[test]
fn selected_suppressed_mapping_checkbox_stays_distinct() {
    let mut app = App::new(ui_settings(true));
    app.open_settings_popup();
    app.settings_popup
        .select_topic_for_tests(SettingsTopic::Proxy);
    let mut proxy = proxy_settings_with_rule_counts(1, 1);
    proxy.enable = false;
    app.settings_popup.draft_mut_for_tests().proxy = Some(proxy);
    focus_settings_content(&mut app);
    app.settings_popup
        .select_proxy_row_for_tests(ProxyRow::RemoteRule(0));

    let (_ui, buffer) = render_to_buffer_with_size(&mut app, 100, 28);
    let content_area = settings_content_test_area(buffer.area);
    let value = find_buffer_text(&buffer, content_area, "http://localhost:3000")
        .expect("selected remote mapping rule should render");
    let checkbox = table_checkbox_on_value_row(&buffer, content_area, value);

    assert_eq!(buffer[checkbox].fg, Color::Gray);
    assert_eq!(buffer[checkbox].bg, Color::White);
    assert_eq!(buffer[value].fg, Color::DarkGray);
    assert_eq!(buffer[value].bg, Color::White);
}

#[test]
fn settings_popup_proxy_page_without_presets_renders_empty_state_only() {
    let mut app = App::new(ui_settings(true));
    app.open_settings_popup();
    app.settings_popup
        .select_topic_for_tests(SettingsTopic::Proxy);

    let (_ui, buffer) = render_to_buffer_with_size(&mut app, 100, 20);
    let rendered = (0..buffer.area.height)
        .map(|row| buffer_row(&buffer, row, 0, buffer.area.width))
        .collect::<Vec<_>>()
        .join("\n");

    assert!(rendered.contains("No proxy preset configured."));
    assert!(!rendered.contains("Preset name"));
    assert!(!rendered.contains("Mapping enabled"));
    assert!(!rendered.contains("─ Map Remote ─"));
    assert!(!rendered.contains("─ Map Local ─"));
    assert!(!rendered.contains("Map Remote Rules"));
    assert!(!rendered.contains("Map Local Rules"));
}

#[test]
fn settings_popup_proxy_page_with_stale_active_preset_renders_only_select() {
    let mut app = App::new(ui_settings(true));
    app.open_settings_popup();
    app.settings_popup
        .select_topic_for_tests(SettingsTopic::Proxy);
    app.settings_popup.draft_mut_for_tests().proxy =
        Some(proxy_settings("missing", &["dev", "qa"]));

    let (_ui, buffer) = render_to_buffer_with_size(&mut app, 100, 20);
    let rendered = (0..buffer.area.height)
        .map(|row| buffer_row(&buffer, row, 0, buffer.area.width))
        .collect::<Vec<_>>()
        .join("\n");

    assert!(rendered.contains("Preset"));
    assert!(rendered.contains("(none)"));
    assert!(!rendered.contains("Preset name"));
    assert!(!rendered.contains("Mapping enabled"));
    assert!(!rendered.contains("─ Map Remote ─"));
    assert!(!rendered.contains("─ Map Local ─"));
    assert!(!rendered.contains("Map Remote Rules"));
    assert!(!rendered.contains("Map Local Rules"));
}

#[test]
fn settings_popup_proxy_page_renders_active_preset_controls_in_order() {
    let mut app = App::new(ui_settings(true));
    app.open_settings_popup();
    app.settings_popup
        .select_topic_for_tests(SettingsTopic::Proxy);
    app.settings_popup.draft_mut_for_tests().proxy = Some(proxy_settings_with_rule_counts(1, 1));

    let (_ui, buffer) = render_to_buffer_with_size(&mut app, 100, 28);
    let content_area = settings_content_test_area(buffer.area);
    let preset_name = find_buffer_text(&buffer, content_area, "Preset name")
        .expect("preset name input should render");
    let mapping_enabled = find_buffer_text(&buffer, content_area, "Mapping enabled")
        .expect("mapping checkbox should render");
    let remote_divider = find_buffer_text(&buffer, content_area, "─ Map Remote ─")
        .expect("remote divider should render");
    let remote_enabled = find_buffer_text(&buffer, content_area, "Map remote enabled")
        .expect("remote checkbox should render");
    let remote_rules = find_buffer_text(&buffer, content_area, "Map Remote Rules")
        .expect("remote table should render");
    let local_divider = find_buffer_text(&buffer, content_area, "─ Map Local ─")
        .expect("local divider should render");
    let local_enabled = find_buffer_text(&buffer, content_area, "Map local enabled")
        .expect("local checkbox should render");
    let local_rules = find_buffer_text(&buffer, content_area, "Map Local Rules")
        .expect("local table should render");

    assert_centered_divider_with_padding(&buffer, content_area, remote_divider, "Map Remote");
    assert_centered_divider_with_padding(&buffer, content_area, local_divider, "Map Local");
    assert!(preset_name.y < mapping_enabled.y);
    assert!(mapping_enabled.y < remote_divider.y);
    assert!(remote_divider.y < remote_enabled.y);
    assert!(remote_enabled.y < remote_rules.y);
    assert!(remote_rules.y < local_divider.y);
    assert!(local_divider.y < local_enabled.y);
    assert!(local_enabled.y < local_rules.y);
}

#[test]
fn settings_popup_duplicate_preset_name_hint_renders_below_input() {
    let mut app = App::new(ui_settings(true));
    app.open_settings_popup();
    app.settings_popup
        .select_topic_for_tests(SettingsTopic::Proxy);
    app.settings_popup.draft_mut_for_tests().proxy = Some(proxy_settings("dev", &["dev", "qa"]));
    app.settings_popup
        .select_proxy_row_for_tests(ProxyRow::PresetName);
    focus_settings_content(&mut app);

    app.handle_key_event(key(KeyCode::Enter));
    for _ in 0.."dev".chars().count() {
        app.handle_key_event(key(KeyCode::Backspace));
    }
    for ch in "qa".chars() {
        app.handle_key_event(key(KeyCode::Char(ch)));
    }
    app.handle_key_event(key(KeyCode::Enter));

    let (_ui, buffer) = render_to_buffer_with_size(&mut app, 100, 28);
    let content_area = settings_content_test_area(buffer.area);
    let input_box = field_box_bounds(&buffer, content_area, "Preset name")
        .expect("preset name input should render");
    let hint = find_buffer_text(&buffer, content_area, "preset name already exists")
        .expect("duplicate hint should render");

    assert_eq!(
        hint.y,
        input_box.0.y.saturating_add(SETTING_TEXT_FIELD_HEIGHT)
    );
    assert_eq!(buffer[hint].fg, Color::Red);
}

#[test]
fn settings_popup_proxy_preset_select_renders_dropdown_options() {
    let mut app = App::new(ui_settings(true));
    app.open_settings_popup();
    app.settings_popup
        .select_topic_for_tests(SettingsTopic::Proxy);
    app.settings_popup.draft_mut_for_tests().proxy =
        Some(proxy_settings("dev", &["dev", "qa", "prod"]));
    focus_settings_content(&mut app);
    app.handle_key_event(key(KeyCode::Enter));

    let (_ui, buffer) = render_to_buffer(&mut app);
    let rendered = (0..buffer.area.height)
        .map(|row| buffer_row(&buffer, row, 0, buffer.area.width))
        .collect::<Vec<_>>()
        .join("\n");

    assert!(rendered.contains("Preset"));
    assert!(rendered.contains("▴"));
    assert!(rendered.contains("qa"));
    assert!(rendered.contains("prod"));
}

#[test]
fn settings_popup_proxy_preset_select_uses_ellipsis_for_overflow() {
    let long_name = "very-long-proxy-preset-name-that-needs-truncation";
    let mut app = App::new(ui_settings(true));
    app.open_settings_popup();
    app.settings_popup
        .select_topic_for_tests(SettingsTopic::Proxy);
    app.settings_popup.draft_mut_for_tests().proxy = Some(proxy_settings(long_name, &[long_name]));

    let (_ui, buffer) = render_to_buffer(&mut app);
    let content_area = settings_content_test_area(buffer.area);
    let row = buffer_row(
        &buffer,
        content_area.y + 1,
        content_area.x,
        content_area.width,
    );

    assert!(row.contains("…"));
}

#[test]
fn settings_popup_proxy_preset_select_handles_mouse_selection() {
    let mut app = App::new(ui_settings(true));
    app.open_settings_popup();
    app.settings_popup
        .select_topic_for_tests(SettingsTopic::Proxy);
    app.settings_popup.draft_mut_for_tests().proxy =
        Some(proxy_settings("dev", &["dev", "qa", "prod"]));

    let (ui, _buffer) = render_to_buffer(&mut app);
    let root_area = Rect::new(0, 0, 100, 12);
    let content_area = settings_content_test_area(root_area);
    let content_width = content_area.width.saturating_sub(1).max(1);
    let items = settings_content_items_for_test(&app.settings_popup, root_area);
    let field_layout = settings_field_layout(&items);
    let content_height =
        settings_content_height(&items, content_area.height, field_layout, content_width);
    let layout = settings_select_layout(
        &items,
        Some(SelectTarget::ProxyPreset),
        field_layout,
        content_width,
        content_height,
    )
    .expect("proxy preset select layout should exist")
    .layout;
    let box_click = Position::new(
        content_area.x + layout.box_area.x + 1,
        content_area.y + layout.box_area.y + 1,
    );
    drop(items);

    ui.handle_mouse(
        mouse(
            MouseEventKind::Down(MouseButton::Left),
            box_click.x,
            box_click.y,
        ),
        &mut app,
    );

    let (ui, _buffer) = render_to_buffer(&mut app);
    let items = settings_content_items_for_test(&app.settings_popup, root_area);
    let field_layout = settings_field_layout(&items);
    let content_height =
        settings_content_height(&items, content_area.height, field_layout, content_width);
    let layout = settings_select_layout(
        &items,
        Some(SelectTarget::ProxyPreset),
        field_layout,
        content_width,
        content_height,
    )
    .expect("proxy preset select layout should exist")
    .layout;
    let options_area = layout
        .options_area
        .expect("open preset select should expose option rows");
    let qa_click = Position::new(
        content_area.x + options_area.x + 1,
        content_area.y + options_area.y + 1,
    );
    drop(items);

    ui.handle_mouse(
        mouse(
            MouseEventKind::Down(MouseButton::Left),
            qa_click.x,
            qa_click.y,
        ),
        &mut app,
    );

    assert_eq!(
        app.settings_popup
            .draft()
            .proxy
            .as_ref()
            .and_then(|proxy| proxy.active_preset.as_deref()),
        Some("qa")
    );
}

#[test]
fn settings_popup_mouse_rejects_hit_regions_after_presentation_changes() {
    let mut app = App::new(ui_settings(true));
    app.open_settings_popup();
    app.settings_popup
        .select_topic_for_tests(SettingsTopic::Proxy);
    app.settings_popup.draft_mut_for_tests().proxy = Some(proxy_settings(
        "dev",
        &["dev", "qa", "prod", "test", "staging", "local", "canary"],
    ));

    let (ui, buffer) = render_to_buffer(&mut app);
    let content_area = settings_content_test_area(buffer.area);
    let preset =
        find_buffer_text(&buffer, content_area, "dev").expect("active proxy preset should render");
    app.settings_popup.start_select(SelectTarget::ProxyPreset);
    assert!(app.settings_popup.scroll_active_select_down());
    assert_eq!(
        app.settings_popup
            .select_state(SelectTarget::ProxyPreset)
            .expect("proxy preset select should be open")
            .scroll_offset(),
        1
    );

    ui.handle_mouse(
        mouse(MouseEventKind::Down(MouseButton::Left), preset.x, preset.y),
        &mut app,
    );

    assert_eq!(
        app.settings_popup
            .select_state(SelectTarget::ProxyPreset)
            .expect("stale hit regions should not reset the open select")
            .scroll_offset(),
        1
    );
}

#[test]
fn settings_popup_mouse_movement_preserves_rendered_hit_regions() {
    let mut app = App::new(ui_settings(true));
    app.open_settings_popup();
    app.settings_popup
        .select_topic_for_tests(SettingsTopic::Proxy);
    app.settings_popup.draft_mut_for_tests().proxy = Some(proxy_settings("dev", &["dev", "qa"]));

    let (ui, buffer) = render_to_buffer(&mut app);
    let content_area = settings_content_test_area(buffer.area);
    let preset =
        find_buffer_text(&buffer, content_area, "dev").expect("active proxy preset should render");

    ui.handle_mouse(mouse(MouseEventKind::Moved, preset.x, preset.y), &mut app);
    ui.handle_mouse(
        mouse(MouseEventKind::Down(MouseButton::Left), preset.x, preset.y),
        &mut app,
    );

    assert_eq!(
        app.settings_popup.active_select_target(),
        Some(SelectTarget::ProxyPreset)
    );
}

#[test]
fn settings_popup_proxy_preset_dropdown_does_not_change_content_height() {
    let mut app = App::new(ui_settings(true));
    app.open_settings_popup();
    app.settings_popup
        .select_topic_for_tests(SettingsTopic::Proxy);
    app.settings_popup.draft_mut_for_tests().proxy =
        Some(proxy_settings("dev", &["dev", "qa", "prod"]));
    let content_width = 60;
    let root_area = Rect::new(0, 0, 100, 12);
    let closed_items = settings_content_items_for_test(&app.settings_popup, root_area);
    let closed_layout = settings_field_layout(&closed_items);
    let closed_height = settings_content_height(&closed_items, 0, closed_layout, content_width);
    drop(closed_items);

    focus_settings_content(&mut app);
    app.handle_key_event(key(KeyCode::Enter));
    assert_eq!(
        app.settings_popup.active_select_target(),
        Some(SelectTarget::ProxyPreset)
    );

    let open_items = settings_content_items_for_test(&app.settings_popup, root_area);
    let open_layout = settings_field_layout(&open_items);
    let open_height = settings_content_height(&open_items, 0, open_layout, content_width);

    assert_eq!(closed_height, open_height);
    assert_eq!(
        settings_select_control(&app.settings_popup, SelectTarget::ProxyPreset)
            .height_for_width(content_width),
        crate::select_widget::SELECT_FIELD_HEIGHT
    );
}

#[test]
fn settings_popup_proxy_select_overlay_anchors_to_shared_control_area() {
    let mut app = App::new(ui_settings(true));
    app.open_settings_popup();
    app.settings_popup
        .select_topic_for_tests(SettingsTopic::Proxy);
    app.settings_popup.draft_mut_for_tests().proxy =
        Some(proxy_settings("dev", &["dev", "qa", "prod"]));
    focus_settings_content(&mut app);
    app.handle_key_event(key(KeyCode::Enter));

    let root_area = Rect::new(0, 0, 100, 12);
    let content_area = settings_content_test_area(root_area);
    let content_width = content_area.width.saturating_sub(1).max(1);
    let items = settings_content_items_for_test(&app.settings_popup, root_area);
    let field_layout = settings_field_layout(&items);
    let content_height =
        settings_content_height(&items, content_area.height, field_layout, content_width);
    let row_area = Rect::new(
        0,
        0,
        content_width,
        items[0].height(field_layout, content_width),
    );
    let row_areas = field_layout.areas(row_area);
    let select_layout = settings_select_layout(
        &items,
        Some(SelectTarget::ProxyPreset),
        field_layout,
        content_width,
        content_height,
    )
    .expect("proxy preset select layout should exist")
    .layout;

    assert_eq!(select_layout.box_area.x, row_areas.control.x);
    assert_eq!(
        select_layout
            .dropdown_area
            .expect("dropdown should render")
            .x,
        row_areas.control.x
    );
}

#[test]
fn settings_popup_proxy_rule_tables_render_as_bordered_widgets() {
    let mut app = App::new(ui_settings(true));
    app.open_settings_popup();
    app.settings_popup
        .select_topic_for_tests(SettingsTopic::Proxy);
    app.settings_popup.draft_mut_for_tests().proxy = Some(crate::settings::ProxySettings {
        enable: true,
        active_preset: Some("dev".to_string()),
        presets: vec![crate::settings::ProxyPresetSettings {
            name: "dev".to_string(),
            map_remote: crate::settings::ProxyMapRemoteSettings {
                enable: true,
                rules: vec![crate::settings::ProxyMapRemoteRule {
                    from: "https://api.example.com".to_string(),
                    to: "http://localhost:3000".to_string(),
                    enable: true,
                }],
            },
            map_local: crate::settings::ProxyMapLocalSettings::default(),
        }],
    });

    let (_ui, buffer) = render_to_buffer_with_size(&mut app, 100, 28);
    let content_area = settings_content_test_area(buffer.area);
    let title = find_buffer_text(&buffer, content_area, "Map Remote Rules")
        .expect("remote rule table title should render");

    assert_eq!(buffer[(content_area.x, title.y)].symbol(), "╭");
    assert_eq!(buffer[(content_area.x, title.y + 1)].symbol(), "│");
    assert_eq!(buffer[(content_area.x, title.y + 3)].symbol(), "╰");
}

#[test]
fn settings_popup_rule_editor_popup_renders_two_textareas() {
    let mut app = App::new(ui_settings(true));
    app.open_settings_popup();
    app.settings_popup
        .select_topic_for_tests(SettingsTopic::Proxy);
    app.settings_popup.draft_mut_for_tests().proxy = Some(crate::settings::ProxySettings {
        enable: true,
        active_preset: Some("dev".to_string()),
        presets: vec![crate::settings::ProxyPresetSettings {
            name: "dev".to_string(),
            map_remote: crate::settings::ProxyMapRemoteSettings {
                enable: true,
                rules: vec![crate::settings::ProxyMapRemoteRule {
                    from: "https://api.example.com".to_string(),
                    to: "http://localhost:3000".to_string(),
                    enable: true,
                }],
            },
            map_local: crate::settings::ProxyMapLocalSettings::default(),
        }],
    });
    app.settings_popup
        .select_proxy_row_for_tests(ProxyRow::RemoteRule(0));
    app.handle_key_event(key(KeyCode::Enter));

    let (_ui, buffer) = render_to_buffer(&mut app);
    let rendered = (0..buffer.area.height)
        .map(|row| buffer_row(&buffer, row, 0, buffer.area.width))
        .collect::<Vec<_>>()
        .join("\n");

    assert!(rendered.contains("Edit Map Remote Rule"));
    assert!(rendered.contains("From"));
    assert!(rendered.contains("To"));
    assert!(rendered.contains("https://api.example.com"));
    assert!(rendered.contains("http://localhost:3000"));
    assert!(rendered.contains("Apply [Enter]"));
}

#[test]
fn settings_popup_rule_editor_uses_its_own_footer_key_hints() {
    let mut app = App::new(ui_settings(true));
    app.open_settings_popup();
    app.settings_popup
        .select_topic_for_tests(SettingsTopic::Proxy);
    app.settings_popup.draft_mut_for_tests().proxy = Some(crate::settings::ProxySettings {
        enable: true,
        active_preset: Some("dev".to_string()),
        presets: vec![crate::settings::ProxyPresetSettings {
            name: "dev".to_string(),
            map_remote: crate::settings::ProxyMapRemoteSettings {
                enable: true,
                rules: vec![crate::settings::ProxyMapRemoteRule {
                    from: "https://api.example.com".to_string(),
                    to: "http://localhost:3000".to_string(),
                    enable: true,
                }],
            },
            map_local: crate::settings::ProxyMapLocalSettings::default(),
        }],
    });
    app.settings_popup
        .select_proxy_row_for_tests(ProxyRow::RemoteRule(0));
    app.handle_key_event(key(KeyCode::Enter));

    let (_ui, buffer) = render_to_buffer(&mut app);
    let editor_footer = rule_editor_footer_row(&buffer);
    let settings_footer = settings_popup_footer_row(&buffer);

    assert!(editor_footer.contains("Apply [Enter]"), "{editor_footer}");
    assert!(editor_footer.contains("Cancel [Esc]"), "{editor_footer}");
    assert!(editor_footer.contains("Switch [Tab]"), "{editor_footer}");
    assert!(
        editor_footer.contains("Cursor [Left/Right]"),
        "{editor_footer}"
    );
    assert!(settings_footer.contains("Back [Esc]"), "{settings_footer}");
    assert!(
        settings_footer.contains("Edit [Enter/e]"),
        "{settings_footer}"
    );
    assert!(
        !settings_footer.contains("Apply [Enter]"),
        "{settings_footer}"
    );
    assert!(
        !settings_footer.contains("Cancel [Esc]"),
        "{settings_footer}"
    );
}

#[test]
fn settings_popup_rule_editor_fields_share_local_control_column() {
    let mut app = App::new(ui_settings(true));
    app.open_settings_popup();
    app.settings_popup
        .select_topic_for_tests(SettingsTopic::Proxy);
    app.settings_popup.draft_mut_for_tests().proxy = Some(crate::settings::ProxySettings {
        enable: true,
        active_preset: Some("dev".to_string()),
        presets: vec![crate::settings::ProxyPresetSettings {
            name: "dev".to_string(),
            map_remote: crate::settings::ProxyMapRemoteSettings {
                enable: true,
                rules: vec![crate::settings::ProxyMapRemoteRule {
                    from: "https://api.example.com".to_string(),
                    to: "http://localhost:3000".to_string(),
                    enable: true,
                }],
            },
            map_local: crate::settings::ProxyMapLocalSettings::default(),
        }],
    });
    app.settings_popup
        .select_proxy_row_for_tests(ProxyRow::RemoteRule(0));
    app.handle_key_event(key(KeyCode::Enter));

    let (_ui, buffer) = render_to_buffer(&mut app);
    let editor_area = rule_editor_test_area(buffer.area);
    let from_box =
        field_box_bounds(&buffer, editor_area, "From").expect("from field box should render");
    let to_box = field_box_bounds(&buffer, editor_area, "To").expect("to field box should render");
    let editor_inner = editor_area.inner(Margin {
        vertical: 1,
        horizontal: 1,
    });
    let expected_right = editor_inner.right().saturating_sub(2);

    assert_eq!(from_box.0.x, to_box.0.x);
    assert_eq!(from_box.1.x, expected_right);
    assert_eq!(to_box.1.x, expected_right);
}
