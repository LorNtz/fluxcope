use super::*;

#[test]
fn settings_popup_renders_topics_and_selected_content() {
    let mut app = App::new(ui_settings(true));
    app.open_settings_popup();

    let (_ui, buffer) = render_to_buffer(&mut app);
    let rendered = (0..buffer.area.height)
        .map(|row| buffer_row(&buffer, row, 0, buffer.area.width))
        .collect::<Vec<_>>()
        .join("\n");

    assert!(rendered.contains("Settings"));
    assert!(rendered.contains("Server"));
    assert!(rendered.contains("Proxy port"));
}

#[test]
fn settings_popup_renders_content_column_border() {
    let mut app = App::new(ui_settings(true));
    app.open_settings_popup();

    let (_ui, buffer) = render_to_buffer(&mut app);
    let popup_area = settings_popup_area(buffer.area);
    let inner = popup_area.inner(Margin {
        vertical: 1,
        horizontal: 1,
    });
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(18), Constraint::Min(20)])
        .split(inner);
    let content_area = chunks[1];

    assert_eq!(
        buffer[(content_area.x, content_area.y)].symbol(),
        symbols::border::ROUNDED.top_left
    );
    assert_eq!(
        buffer[(content_area.right() - 1, content_area.y)].symbol(),
        symbols::border::ROUNDED.top_right
    );
}

#[test]
fn settings_popup_footer_uses_topic_browse_key_hints() {
    let mut app = App::new(ui_settings(true));
    app.open_settings_popup();

    let (_ui, buffer) = render_to_buffer(&mut app);
    let footer = settings_popup_footer_row(&buffer);

    assert!(footer.contains("Save [s]"), "{footer}");
    assert!(footer.contains("Close [Esc]"), "{footer}");
    assert!(footer.contains("Pane [h/l]"), "{footer}");
    assert!(footer.contains("Move [j/k]"), "{footer}");
    assert!(footer.contains("Open [Enter]"), "{footer}");
}

#[test]
fn settings_popup_footer_switches_to_text_edit_key_hints() {
    let mut app = App::new(ui_settings(true));
    app.open_settings_popup();
    focus_settings_content(&mut app);
    app.handle_key_event(key(KeyCode::Enter));

    let (_ui, buffer) = render_to_buffer(&mut app);
    let footer = settings_popup_footer_row(&buffer);

    assert!(footer.contains("Apply [Enter]"), "{footer}");
    assert!(footer.contains("Cancel [Esc]"), "{footer}");
    assert!(footer.contains("Cursor [Left/Right]"), "{footer}");
    assert!(footer.contains("Type [text]"), "{footer}");
    assert!(!footer.contains("Save [s]"), "{footer}");
}

#[test]
fn settings_popup_footer_switches_to_select_key_hints() {
    let mut app = App::new(ui_settings(true));
    app.open_settings_popup();
    app.settings_popup
        .select_topic_for_tests(SettingsTopic::Proxy);
    app.settings_popup.draft_mut_for_tests().proxy =
        Some(proxy_settings("dev", &["dev", "qa", "prod"]));
    focus_settings_content(&mut app);
    app.handle_key_event(key(KeyCode::Enter));

    let (_ui, buffer) = render_to_buffer(&mut app);
    let footer = settings_popup_footer_row(&buffer);

    assert!(footer.contains("Choose [Enter]"), "{footer}");
    assert!(footer.contains("Cancel [Esc]"), "{footer}");
    assert!(footer.contains("Move [Up/Down]"), "{footer}");
    assert!(footer.contains("Filter [type]"), "{footer}");
    assert!(!footer.contains("Save [s]"), "{footer}");
}

#[test]
fn settings_popup_footer_switches_to_rule_table_key_hints() {
    let mut app = App::new(ui_settings(true));
    app.open_settings_popup();
    app.settings_popup
        .select_topic_for_tests(SettingsTopic::Proxy);
    app.settings_popup.draft_mut_for_tests().proxy = Some(proxy_settings_with_rule_counts(2, 0));
    app.settings_popup
        .select_proxy_row_for_tests(ProxyRow::RemoteHeader);
    focus_settings_content(&mut app);
    app.handle_key_event(key(KeyCode::Enter));

    let (_ui, buffer) = render_to_buffer(&mut app);
    let footer = settings_popup_footer_row(&buffer);

    assert!(footer.contains("Back [Esc]"), "{footer}");
    assert!(footer.contains("Edit [Enter/e]"), "{footer}");
    assert!(footer.contains("Toggle [Space]"), "{footer}");
    assert!(footer.contains("Add/del [a/d]"), "{footer}");
    assert!(footer.contains("Move [j/k/Pg/J/K]"), "{footer}");
    assert!(!footer.contains("Save [s]"), "{footer}");
}

#[test]
fn settings_popup_footer_uses_empty_rule_table_key_hints() {
    let mut app = App::new(ui_settings(true));
    app.open_settings_popup();
    app.settings_popup
        .select_topic_for_tests(SettingsTopic::Proxy);
    app.settings_popup.draft_mut_for_tests().proxy = Some(proxy_settings_with_rule_counts(0, 0));
    app.settings_popup
        .select_proxy_row_for_tests(ProxyRow::RemoteHeader);
    focus_settings_content(&mut app);
    app.handle_key_event(key(KeyCode::Enter));

    let (_ui, buffer) = render_to_buffer(&mut app);
    let footer = settings_popup_footer_row(&buffer);

    assert!(footer.contains("Back [Esc]"), "{footer}");
    assert!(footer.contains("Add [a]"), "{footer}");
    assert!(!footer.contains("Edit [Enter/e]"), "{footer}");
    assert!(!footer.contains("Toggle [Space]"), "{footer}");
    assert!(!footer.contains("Add/del [a/d]"), "{footer}");
    assert!(!footer.contains("Move [j/k/Pg/J/K]"), "{footer}");
}

#[test]
fn settings_popup_input_field_renders_boxed_and_selected_green() {
    let mut app = App::new(ui_settings(true));
    app.open_settings_popup();
    focus_settings_content(&mut app);

    let (_ui, buffer) = render_to_buffer(&mut app);
    let content_area = settings_content_test_area(buffer.area);
    let label =
        find_buffer_text(&buffer, content_area, "Proxy port").expect("input label should render");
    let top_left = find_buffer_text(&buffer, content_area, "╭").expect("input box should render");
    let bottom_left =
        find_buffer_text(&buffer, content_area, "╰").expect("input bottom border should render");

    assert_eq!(buffer[label].fg, Color::Green);
    assert_eq!(buffer[top_left].fg, Color::Green);
    assert_eq!(buffer[bottom_left].fg, Color::Green);
}

#[test]
fn settings_popup_input_field_uses_orange_while_editing() {
    let mut app = App::new(ui_settings(true));
    app.open_settings_popup();
    focus_settings_content(&mut app);
    app.handle_key_event(key(KeyCode::Enter));

    let (_ui, buffer) = render_to_buffer(&mut app);
    let content_area = settings_content_test_area(buffer.area);
    let label =
        find_buffer_text(&buffer, content_area, "Proxy port").expect("input label should render");
    let top_left = find_buffer_text(&buffer, content_area, "╭").expect("input box should render");

    assert_eq!(buffer[label].fg, Color::Indexed(208));
    assert_eq!(buffer[top_left].fg, Color::Indexed(208));
}

#[test]
fn settings_popup_input_field_centers_label_next_to_textarea() {
    let mut app = App::new(ui_settings(true));
    app.open_settings_popup();

    let (_ui, buffer) = render_to_buffer(&mut app);
    let content_area = settings_content_test_area(buffer.area);
    let label =
        find_buffer_text(&buffer, content_area, "Proxy port").expect("input label should render");
    let top_left = find_buffer_text(&buffer, content_area, "╭").expect("input box should render");

    assert_eq!(label.y, top_left.y + 1);
}

#[test]
fn settings_popup_port_input_uses_fixed_width_in_idle_and_edit_modes() {
    let mut app = App::new(ui_settings(true));
    app.open_settings_popup();

    let (_ui, buffer) = render_to_buffer(&mut app);
    let content_area = settings_content_test_area(buffer.area);
    let idle_width =
        field_box_width(&buffer, content_area, "Proxy port").expect("port field box should render");

    assert_eq!(
        idle_width,
        PORT_INPUT_WIDTH_COLS.saturating_add(TEXT_INPUT_CHROME_WIDTH)
    );

    focus_settings_content(&mut app);
    app.handle_key_event(key(KeyCode::Enter));
    for ch in "12345678901234567890".chars() {
        app.handle_key_event(key(KeyCode::Char(ch)));
    }

    let (_ui, buffer) = render_to_buffer(&mut app);
    let content_area = settings_content_test_area(buffer.area);
    let edited_width = field_box_width(&buffer, content_area, "Proxy port")
        .expect("edited port field box should render");
    let top_left = field_box_top_left(&buffer, content_area, "Proxy port")
        .expect("edited port field box should have a left border");
    let top_right = field_box_top_right(&buffer, content_area, "Proxy port")
        .expect("edited port field box should have a right border");
    let cursor_inside_box = (top_left.x.saturating_add(1)..top_right.x).any(|x| {
        buffer[(x, top_left.y.saturating_add(1))]
            .modifier
            .contains(Modifier::REVERSED)
    });

    assert_eq!(edited_width, idle_width);
    assert!(cursor_inside_box);
}

#[test]
fn settings_popup_pem_filename_input_uses_fixed_width() {
    let mut app = App::new(ui_settings(true));
    app.open_settings_popup();
    app.settings_popup
        .select_topic_for_tests(SettingsTopic::Certificate);

    let (_ui, buffer) = render_to_buffer(&mut app);
    let content_area = settings_content_test_area(buffer.area);
    let width = field_box_width(&buffer, content_area, "CA PEM filename")
        .expect("PEM filename field box should render");

    assert_eq!(
        width,
        PEM_FILENAME_INPUT_WIDTH_COLS.saturating_add(TEXT_INPUT_CHROME_WIDTH)
    );
}

#[test]
fn settings_text_input_fixed_width_clamps_to_available_control_width() {
    let input = SettingsTextInputControl {
        value: Cow::Borrowed("8989"),
        cursor: None,
        fixed_edit_width_cols: Some(PORT_INPUT_WIDTH_COLS),
        hint: None,
    };

    assert_eq!(input.render_width(8), 8);
}

#[test]
fn settings_popup_input_field_renders_textarea_cursor_at_edit_position() {
    let mut app = App::new(ui_settings(true));
    app.open_settings_popup();
    focus_settings_content(&mut app);
    app.handle_key_event(key(KeyCode::Enter));
    app.handle_key_event(key(KeyCode::Left));

    let (_ui, buffer) = render_to_buffer(&mut app);
    let content_area = settings_content_test_area(buffer.area);
    let value = find_buffer_text(&buffer, content_area, "8989").expect("field value should render");

    assert!(
        buffer[(value.x + 3, value.y)]
            .modifier
            .contains(Modifier::REVERSED)
    );
    assert!(
        !buffer[(value.x + 4, value.y)]
            .modifier
            .contains(Modifier::REVERSED)
    );
}

#[test]
fn settings_popup_proxy_rule_edit_opens_secondary_textarea_popup() {
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
    let popup_area = settings_popup_area(buffer.area);
    let label =
        find_buffer_text(&buffer, popup_area, "From").expect("rule edit label should render");
    let top_left = find_buffer_text(
        &buffer,
        Rect::new(
            label.x,
            label.y.saturating_sub(1),
            popup_area.right().saturating_sub(label.x),
            1,
        ),
        "╭",
    )
    .expect("rule edit box should render");
    let title = find_buffer_text(&buffer, popup_area, "Edit Map Remote Rule")
        .expect("rule editor title should render");

    assert!(title.y < label.y);
    assert_eq!(buffer[label].fg, Color::Indexed(208));
    assert_eq!(buffer[top_left].fg, Color::Indexed(208));
}

#[test]
fn settings_popup_long_input_field_keeps_box_border_visible() {
    let mut app = App::new(ui_settings(true));
    app.open_settings_popup();
    app.settings_popup
        .select_topic_for_tests(SettingsTopic::Certificate);
    app.settings_popup
        .draft_mut_for_tests()
        .certificate
        .store_dir =
        "/very/long/path/that/would/otherwise/stretch/the/settings/popup/content/column"
            .to_string();

    let (_ui, buffer) = render_to_buffer(&mut app);
    let content_area = settings_content_test_area(buffer.area);
    let label =
        find_buffer_text(&buffer, content_area, "CA store dir").expect("input label should render");
    let top_right = find_buffer_text(
        &buffer,
        Rect::new(
            label.x,
            label.y.saturating_sub(1),
            content_area.right().saturating_sub(label.x),
            1,
        ),
        "╮",
    )
    .expect("top input border should fit above the centered label row");

    assert_eq!(top_right.x, content_area.right().saturating_sub(1));
}

#[test]
fn settings_popup_content_fields_share_control_column_within_topic() {
    let mut app = App::new(ui_settings(true));
    app.open_settings_popup();
    app.settings_popup
        .select_topic_for_tests(SettingsTopic::Certificate);

    let (_ui, buffer) = render_to_buffer(&mut app);
    let content_area = settings_content_test_area(buffer.area);
    let store_box = field_box_top_left(&buffer, content_area, "CA store dir")
        .expect("store dir field box should render");
    let pem_box = field_box_top_left(&buffer, content_area, "CA PEM filename")
        .expect("pem filename field box should render");

    assert_eq!(store_box.x, pem_box.x);
}

#[test]
fn settings_popup_content_fields_do_not_render_visible_selection_marker() {
    let mut app = App::new(ui_settings(true));
    app.open_settings_popup();

    let (_ui, buffer) = render_to_buffer(&mut app);
    let content_area = settings_content_test_area(buffer.area);
    let label = find_buffer_text(&buffer, content_area, "Proxy port")
        .expect("selected content field should render");
    let row = buffer_row(&buffer, label.y, content_area.x, content_area.width);

    assert!(!row.contains("> Proxy port"), "{row}");
}

#[test]
fn settings_popup_checkbox_field_renders_label_then_checkbox_with_green_selection() {
    let mut app = App::new(ui_settings(true));
    app.open_settings_popup();
    app.settings_popup
        .select_topic_for_tests(SettingsTopic::Recording);
    focus_settings_content(&mut app);

    let (_ui, buffer) = render_to_buffer(&mut app);
    let content_area = settings_content_test_area(buffer.area);
    let label = find_buffer_text(&buffer, content_area, "Start recording on launch")
        .expect("checkbox label should render");
    let checkbox = find_buffer_text(&buffer, content_area, "[✓]").expect("checkbox should render");
    let row = buffer_row(&buffer, label.y, content_area.x, content_area.width);

    assert!(row.contains("Start recording on launch  [✓]"));
    assert!(checkbox.x > label.x);
    assert_eq!(buffer[label].fg, Color::Green);
    assert_eq!(buffer[checkbox].fg, Color::Green);
    assert_eq!(buffer[label].bg, Color::Reset);
    assert_eq!(buffer[checkbox].bg, Color::Reset);
}

#[test]
fn settings_popup_short_content_ignores_mouse_scroll() {
    let mut app = App::new(ui_settings(true));
    app.open_settings_popup();
    let (ui, buffer) = render_to_buffer(&mut app);
    let content_area = settings_content_test_area(buffer.area);

    ui.handle_mouse(
        mouse(MouseEventKind::ScrollDown, content_area.x, content_area.y),
        &mut app,
    );

    assert_eq!(app.settings_popup.scroll.offset().y, 0);
}

#[test]
fn settings_popup_overflowing_content_renders_vertical_scrollbar() {
    let mut app = App::new(ui_settings(true));
    app.open_settings_popup();
    app.settings_popup
        .select_topic_for_tests(SettingsTopic::Proxy);
    app.settings_popup.draft_mut_for_tests().proxy = Some(proxy_settings_with_rule_counts(3, 3));

    let (_ui, buffer) = render_to_buffer_with_size(&mut app, 100, 12);
    let content_area = settings_content_test_area(buffer.area);
    let scrollbar_column = (content_area.y..content_area.bottom())
        .map(|row| buffer[(content_area.right() - 1, row)].symbol())
        .collect::<String>();

    assert!(
        scrollbar_column.contains('▲')
            || scrollbar_column.contains('▼')
            || scrollbar_column.contains('█')
            || scrollbar_column.contains('║'),
        "{scrollbar_column:?}"
    );
}

#[test]
fn settings_popup_keyboard_navigation_scrolls_multi_row_widget_into_view() {
    let mut app = App::new(ui_settings(true));
    app.open_settings_popup();
    app.settings_popup
        .select_topic_for_tests(SettingsTopic::Proxy);
    app.settings_popup.draft_mut_for_tests().proxy = Some(proxy_settings_with_rule_counts(3, 1));
    app.handle_key_event(key(KeyCode::Enter));

    let (_ui, _buffer) = render_to_buffer_with_size(&mut app, 100, 12);
    for _ in 0..4 {
        app.handle_key_event(key(KeyCode::Char('j')));
        let (_ui, _buffer) = render_to_buffer_with_size(&mut app, 100, 12);
    }

    assert!(app.settings_popup.scroll.offset().y > 0);
}

#[test]
fn settings_popup_rule_table_navigation_scrolls_active_rule_into_view() {
    let mut app = App::new(ui_settings(true));
    app.open_settings_popup();
    app.settings_popup
        .select_topic_for_tests(SettingsTopic::Proxy);
    app.settings_popup.draft_mut_for_tests().proxy = Some(proxy_settings_with_rule_counts(8, 0));
    app.settings_popup
        .select_proxy_row_for_tests(ProxyRow::RemoteRule(0));

    let (_ui, _buffer) = render_to_buffer_with_size(&mut app, 100, 12);
    for _ in 0..5 {
        app.handle_key_event(key(KeyCode::Char('j')));
        let (_ui, _buffer) = render_to_buffer_with_size(&mut app, 100, 12);
    }

    assert_eq!(app.settings_popup.scroll.offset().y, 0);
    assert_eq!(
        app.settings_popup
            .rule_table_scroll_offset_for_tests(ProxyRuleTable::Remote),
        5
    );
}

#[test]
fn settings_popup_rule_tables_cap_height_from_viewport_height() {
    let mut app = App::new(ui_settings(true));
    app.open_settings_popup();
    app.settings_popup
        .select_topic_for_tests(SettingsTopic::Proxy);
    app.settings_popup.draft_mut_for_tests().proxy = Some(proxy_settings_with_rule_counts(10, 10));

    let (_ui, buffer) = render_to_buffer_with_size(&mut app, 100, 28);
    let content_area = settings_content_test_area(buffer.area);
    let max_height = settings_table_max_height(content_area.height);
    find_buffer_text(&buffer, content_area, "Map Remote Rules")
        .expect("remote rule table should render");
    let preset = app
        .settings_popup
        .active_proxy_preset()
        .expect("active preset should exist");
    let table = proxy_rule_table_widget(
        &app.settings_popup,
        ProxyRuleTable::Remote,
        Some(preset),
        max_height,
    );

    assert_eq!(max_height, content_area.height / 2);
    assert_eq!(table.height(), max_height);
}

#[test]
fn settings_popup_rule_table_hides_overflowing_rows_and_renders_scrollbar() {
    let mut app = App::new(ui_settings(true));
    app.open_settings_popup();
    app.settings_popup
        .select_topic_for_tests(SettingsTopic::Proxy);
    app.settings_popup.draft_mut_for_tests().proxy = Some(proxy_settings_with_rule_counts(10, 0));

    let (_ui, buffer) = render_to_buffer_with_size(&mut app, 100, 28);
    let content_area = settings_content_test_area(buffer.area);
    let remote_title = find_buffer_text(&buffer, content_area, "Map Remote Rules")
        .expect("remote rule table should render");
    let max_height = settings_table_max_height(settings_content_test_area(buffer.area).height);
    let rendered = (0..buffer.area.height)
        .map(|row| buffer_row(&buffer, row, 0, buffer.area.width))
        .collect::<Vec<_>>()
        .join("\n");
    let scrollbar_x = rule_table_scrollbar_column(&buffer, content_area, "Map Remote Rules")
        .expect("remote rule table scrollbar column should render");
    let scrollbar_column = (remote_title.y + 2..remote_title.y + max_height - 1)
        .map(|row| buffer[(scrollbar_x, row)].symbol())
        .collect::<String>();

    assert!(rendered.contains("https://api.example.com/v0"));
    assert!(rendered.contains("https://api.example.com/v6"));
    assert!(!rendered.contains("https://api.example.com/v7"));
    assert!(
        scrollbar_column.contains('█') || scrollbar_column.contains('║'),
        "{scrollbar_column:?}"
    );
}

#[test]
fn settings_popup_mouse_wheel_scrolls_overflowing_rule_table() {
    let mut app = App::new(ui_settings(true));
    app.open_settings_popup();
    app.settings_popup
        .select_topic_for_tests(SettingsTopic::Proxy);
    app.settings_popup.draft_mut_for_tests().proxy = Some(proxy_settings_with_rule_counts(10, 0));

    let (ui, buffer) = render_to_buffer_with_size(&mut app, 100, 28);
    let content_area = settings_content_test_area(buffer.area);
    let remote_title = find_buffer_text(&buffer, content_area, "Map Remote Rules")
        .expect("remote rule table should render");

    ui.handle_mouse(
        mouse(
            MouseEventKind::ScrollDown,
            content_area.x.saturating_add(2),
            remote_title.y.saturating_add(2),
        ),
        &mut app,
    );

    assert_eq!(app.settings_popup.scroll.offset().y, 0);
    assert_eq!(
        app.settings_popup
            .rule_table_scroll_offset_for_tests(ProxyRuleTable::Remote),
        1
    );
}

#[test]
fn settings_popup_rule_table_scrollbar_reaches_bottom_on_last_page() {
    let mut app = App::new(ui_settings(true));
    app.open_settings_popup();
    app.settings_popup
        .select_topic_for_tests(SettingsTopic::Proxy);
    app.settings_popup.draft_mut_for_tests().proxy = Some(proxy_settings_with_rule_counts(20, 0));
    app.settings_popup
        .select_proxy_row_for_tests(ProxyRow::RemoteRule(19));

    let (_ui, buffer) = render_to_buffer_with_size(&mut app, 100, 28);
    let content_area = settings_content_test_area(buffer.area);
    let remote_title = find_buffer_text(&buffer, content_area, "Map Remote Rules")
        .expect("remote rule table should render");
    let max_height = settings_table_max_height(content_area.height);
    let scrollbar_column = rule_table_scrollbar_column(&buffer, content_area, "Map Remote Rules")
        .expect("remote rule table scrollbar column should render");

    assert_eq!(
        buffer[(scrollbar_column, remote_title.y + max_height - 2)].symbol(),
        "█"
    );
}

#[test]
fn settings_popup_preset_switch_resets_rule_table_scroll_offsets() {
    let mut app = App::new(ui_settings(true));
    app.open_settings_popup();
    app.settings_popup
        .select_topic_for_tests(SettingsTopic::Proxy);
    let mut proxy = proxy_settings_with_rule_counts(10, 0);
    proxy.presets.push(ProxyPresetSettings {
        name: "qa".to_string(),
        ..ProxyPresetSettings::default()
    });
    app.settings_popup.draft_mut_for_tests().proxy = Some(proxy);
    app.settings_popup
        .select_proxy_row_for_tests(ProxyRow::RemoteRule(0));
    let (_ui, _buffer) = render_to_buffer_with_size(&mut app, 100, 28);
    for _ in 0..7 {
        app.handle_key_event(key(KeyCode::Char('j')));
        let (_ui, _buffer) = render_to_buffer_with_size(&mut app, 100, 28);
    }
    assert!(
        app.settings_popup
            .rule_table_scroll_offset_for_tests(ProxyRuleTable::Remote)
            > 0
    );

    app.settings_popup.start_proxy_preset_select();
    app.handle_key_event(key(KeyCode::Down));
    app.handle_key_event(key(KeyCode::Enter));

    assert_eq!(
        app.settings_popup
            .draft()
            .proxy
            .as_ref()
            .and_then(|proxy| proxy.active_preset.as_deref()),
        Some("qa")
    );
    assert_eq!(
        app.settings_popup
            .rule_table_scroll_offset_for_tests(ProxyRuleTable::Remote),
        0
    );
}

#[test]
fn settings_popup_mouse_scroll_does_not_snap_back_to_selected_row() {
    let mut app = App::new(ui_settings(true));
    app.open_settings_popup();
    app.settings_popup
        .select_topic_for_tests(SettingsTopic::Proxy);
    app.settings_popup.draft_mut_for_tests().proxy = Some(proxy_settings_with_rule_counts(3, 3));
    let (ui, _buffer) = render_to_buffer_with_size(&mut app, 100, 12);
    let content_area = settings_content_test_area(Rect::new(0, 0, 100, 12));

    ui.handle_mouse(
        mouse(MouseEventKind::ScrollDown, content_area.x, content_area.y),
        &mut app,
    );
    let (_ui, _buffer) = render_to_buffer_with_size(&mut app, 100, 12);

    assert_eq!(app.settings_popup.scroll.offset().y, 1);
}
