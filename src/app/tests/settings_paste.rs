use super::*;

fn app_with_mapping_paste_editor(table: ProxyRuleTable) -> App {
    let mut settings = settings_with_proxy_presets("dev");
    let preset = &mut settings.proxy.as_mut().unwrap().presets[0];
    preset
        .map_remote
        .rules
        .push(crate::settings::ProxyMapRemoteRule {
            from: "https://example.com/α終".to_string(),
            to: "http://localhost/β終".to_string(),
            enable: true,
        });
    preset
        .map_local
        .rules
        .push(crate::settings::ProxyMapLocalRule {
            from: "https://example.com/α終".to_string(),
            to: "/tmp/β終".to_string(),
            enable: true,
        });
    let mut app = App::with_settings(settings, RecordingState::default());
    app.handle_key_event(key(KeyCode::Char('m')));
    app.settings_popup
        .select_topic_for_tests(SettingsTopic::Proxy);
    focus_settings_content(&mut app);
    app.settings_popup.select_proxy_row_for_tests(match table {
        ProxyRuleTable::Remote => ProxyRow::RemoteRule(0),
        ProxyRuleTable::Local => ProxyRow::LocalRule(0),
    });
    app.handle_key_event(key(KeyCode::Enter));
    app
}

#[test]
fn mapping_paste_inserts_unicode_before_the_cursor_in_both_fields_and_tables() {
    for table in [ProxyRuleTable::Remote, ProxyRuleTable::Local] {
        let mut app = app_with_mapping_paste_editor(table);
        let original_to = match table {
            ProxyRuleTable::Remote => "http://localhost/β終",
            ProxyRuleTable::Local => "/tmp/β終",
        };
        for field in [RuleEditField::From, RuleEditField::To] {
            app.handle_key_event(key(KeyCode::Left));
            let before = app.settings_popup.presentation_revision();
            assert!(app.handle_paste("雪🙂/片"));
            assert_eq!(app.settings_popup.presentation_revision(), before + 1);
            let editor = app.settings_popup.rule_editor().unwrap();
            let (value, original) = match field {
                RuleEditField::From => (editor.from, "https://example.com/α終"),
                RuleEditField::To => (editor.to, original_to),
            };
            assert_eq!(
                value,
                format!("{}雪🙂/片終", original.trim_end_matches('終'))
            );
            assert_eq!(editor.table, table);
            assert_eq!(editor.active_field, field);
            app.handle_key_event(key(KeyCode::Backspace));
            if field == RuleEditField::From {
                assert_eq!(app.settings_popup.rule_editor().unwrap().to, original_to);
                app.handle_key_event(key(KeyCode::Tab));
            }
        }
        assert!(!app.settings_popup.is_dirty());
        app.handle_key_event(key(KeyCode::Enter));
        let preset = app.settings_popup.active_proxy_preset().unwrap();
        let (from, to) = match table {
            ProxyRuleTable::Remote => (
                &preset.map_remote.rules[0].from,
                &preset.map_remote.rules[0].to,
            ),
            ProxyRuleTable::Local => (
                &preset.map_local.rules[0].from,
                &preset.map_local.rules[0].to,
            ),
        };
        assert_eq!(from, "https://example.com/α雪🙂/終");
        assert_eq!(
            to,
            &format!("{}雪🙂/終", original_to.trim_end_matches('終'))
        );
        assert!(app.settings_popup.is_dirty());
        assert!(app.settings_popup.rule_editor().is_none());
    }
}

#[test]
fn mapping_paste_filters_controls_without_commands_or_implicit_submit_and_cancel_discards() {
    let mut app = app_with_mapping_paste_editor(ProxyRuleTable::Local);
    app.handle_key_event(key(KeyCode::Tab));
    let recording = app.is_recording();
    let before = app.settings_popup.presentation_revision();
    app.handle_paste("\r\n\t\u{1b}\0\u{7f}\u{85}");
    app.handle_paste("");
    assert_eq!(app.settings_popup.presentation_revision(), before);
    assert_eq!(app.settings_popup.rule_editor().unwrap().to, "/tmp/β終");

    assert!(app.handle_paste("\r\nq\t r\u{1b}m\0/文件 ?#%~"));
    let editor = app.settings_popup.rule_editor().unwrap();
    assert_eq!(editor.to, "/tmp/β終q rm/文件 ?#%~");
    assert_eq!(editor.active_field, RuleEditField::To);
    assert_eq!(app.is_recording(), recording);
    assert!(app.is_popup_focused(PopupFocus::Settings));
    assert!(app.take_settings_save_request().is_none());
    assert!(!app.settings_popup.is_dirty());

    app.handle_key_event(key(KeyCode::Esc));
    assert!(app.settings_popup.rule_editor().is_none());
    assert!(!app.settings_popup.is_dirty());
    app.handle_key_event(key(KeyCode::Enter));
    assert_eq!(app.settings_popup.rule_editor().unwrap().to, "/tmp/β終");
}

#[test]
fn mapping_paste_respects_popup_focus_and_pending_transactions_before_search() {
    let mut app = app_with_mapping_paste_editor(ProxyRuleTable::Remote);
    app.focus.close_popup();
    let before = app.settings_popup.presentation_revision();
    for panel in [PanelFocus::RequestList, PanelFocus::Detail, PanelFocus::Log] {
        app.focus.focus_panel(panel);
        assert!(!app.handle_paste("unfocused"));
        assert_eq!(app.settings_popup.presentation_revision(), before);
    }
    app.focus.open_popup(PopupFocus::Settings);
    app.begin_request_search();
    app.set_settings_transaction_pending(true);
    let before = app.settings_popup.presentation_revision();
    assert!(!app.handle_paste("blocked"));
    assert_eq!(app.settings_popup.presentation_revision(), before);
    assert_eq!(app.request_search_query(), None);
    assert_eq!(
        app.settings_popup.rule_editor().unwrap().from,
        "https://example.com/α終"
    );

    app.set_settings_transaction_pending(false);
    app.focus.open_popup(PopupFocus::Certificate);
    assert!(!app.handle_paste("certificate"));
    assert_eq!(app.request_search_query(), None);
    assert_eq!(app.settings_popup.presentation_revision(), before);
    app.focus.open_popup(PopupFocus::Settings);
    assert!(app.handle_paste("/allowed"));
    assert_eq!(app.request_search_query(), None);
    assert_eq!(
        app.settings_popup.rule_editor().unwrap().from,
        "https://example.com/α終/allowed"
    );
}

#[test]
fn settings_paste_ignores_non_input_modes_without_leaking_into_search() {
    let mut app = app_with_mapping_paste_editor(ProxyRuleTable::Remote);
    app.begin_request_search();
    app.handle_key_event(key(KeyCode::Esc));
    let original = app.settings_popup.draft().clone();
    assert!(!app.handle_paste("table"));
    app.handle_key_event(key(KeyCode::Esc));
    assert!(!app.handle_paste("browse"));
    app.settings_popup
        .select_topic_for_tests(SettingsTopic::Recording);
    app.handle_key_event(key(KeyCode::Down));
    app.handle_key_event(key(KeyCode::Down));
    app.handle_key_event(key(KeyCode::Enter));
    assert!(!app.handle_paste("prefilter table"));
    assert_eq!(app.settings_popup.draft(), &original);
    app.handle_key_event(key(KeyCode::Esc));
    app.settings_popup
        .draft_mut_for_tests()
        .recording
        .start_record_on_launch = !original.recording.start_record_on_launch;
    app.handle_key_event(key(KeyCode::Esc));
    assert!(app.settings_popup.is_confirming_unsaved());
    assert!(!app.handle_paste("s\r\n"));
    assert!(app.settings_popup.is_confirming_unsaved());
    assert!(app.take_settings_save_request().is_none());
    assert_eq!(app.request_search_query(), None);
}

#[test]
fn settings_paste_targets_each_active_field_without_applying() {
    for (kind, topic, row, original, left, pasted, expected) in [
        (
            FieldEditKind::ServerPort,
            SettingsTopic::Server,
            0,
            "80",
            1,
            "9\r\n",
            "890",
        ),
        (
            FieldEditKind::CertificateStoreDir,
            SettingsTopic::Certificate,
            0,
            "/tmp/終",
            1,
            "雪\t",
            "/tmp/雪終",
        ),
        (
            FieldEditKind::CertificatePemFilename,
            SettingsTopic::Certificate,
            1,
            "ca.pem",
            4,
            "雪\n",
            "ca雪.pem",
        ),
        (
            FieldEditKind::ProxyPresetName,
            SettingsTopic::Proxy,
            1,
            "dev",
            1,
            "雪\r",
            "de雪v",
        ),
    ] {
        let mut settings = settings_with_proxy_presets("dev");
        settings.server.port = 80;
        settings.certificate.store_dir = "/tmp/終".to_string();
        settings.certificate.pem_filename = "ca.pem".to_string();
        let mut app = App::with_settings(settings.clone(), RecordingState::default());
        app.open_settings_popup();
        app.settings_popup.select_topic_for_tests(topic);
        focus_settings_content(&mut app);
        for _ in 0..row {
            app.handle_key_event(key(KeyCode::Down));
        }
        app.handle_key_event(key(KeyCode::Enter));
        app.begin_request_search();
        app.set_settings_transaction_pending(true);
        assert!(!app.handle_paste("blocked"));
        assert_eq!(
            app.settings_popup.active_field_edit(kind).unwrap().value,
            original
        );
        app.set_settings_transaction_pending(false);
        for _ in 0..left {
            app.handle_key_event(key(KeyCode::Left));
        }
        assert!(app.handle_paste(pasted));
        let edit = app.settings_popup.active_field_edit(kind).unwrap();
        assert_eq!(edit.value, expected);
        app.handle_paste("\r\n\t\u{1b}\0");
        assert_eq!(
            app.settings_popup.active_field_edit(kind).unwrap().value,
            expected
        );
        assert_eq!(app.settings_popup.draft(), &settings);
        assert_eq!(app.request_search_query(), None);
        assert!(app.take_settings_save_request().is_none());

        app.handle_key_event(key(KeyCode::Enter));
        let draft = app.settings_popup.draft();
        match kind {
            FieldEditKind::ServerPort => assert_eq!(draft.server.port, 890),
            FieldEditKind::CertificateStoreDir => assert_eq!(draft.certificate.store_dir, expected),
            FieldEditKind::CertificatePemFilename => {
                assert_eq!(draft.certificate.pem_filename, expected)
            }
            FieldEditKind::ProxyPresetName => {
                let proxy = draft.proxy.as_ref().unwrap();
                assert_eq!(proxy.presets[0].name, expected);
                assert_eq!(proxy.active_preset.as_deref(), Some(expected));
            }
        }
        assert!(app.settings_popup.active_field_edit(kind).is_none());
    }
}

#[test]
fn settings_paste_preserves_port_validation_and_field_cancellation() {
    let mut settings = AppSettings::default();
    settings.server.port = 8080;
    let mut app = App::with_settings(settings, RecordingState::default());
    app.open_settings_popup();
    focus_settings_content(&mut app);
    app.handle_key_event(key(KeyCode::Enter));
    app.handle_paste("1");
    assert_eq!(
        app.settings_popup
            .active_field_edit(FieldEditKind::ServerPort)
            .unwrap()
            .value,
        "80801"
    );
    app.handle_key_event(key(KeyCode::Esc));
    assert_eq!(app.settings_popup.draft().server.port, 8080);
    assert!(!app.settings_popup.is_dirty());

    app.handle_key_event(key(KeyCode::Enter));
    for _ in 0..4 {
        app.handle_key_event(key(KeyCode::Backspace));
    }
    app.handle_paste("65536\n");
    app.handle_key_event(key(KeyCode::Enter));
    assert_eq!(app.settings_popup.draft().server.port, 8080);
    assert!(app.settings_popup.error().is_some());
    assert_eq!(
        app.settings_popup
            .active_field_edit(FieldEditKind::ServerPort)
            .unwrap()
            .value,
        "65536"
    );
    for _ in 0..5 {
        app.handle_key_event(key(KeyCode::Backspace));
    }
    app.handle_paste("9090\r\n");
    assert_eq!(app.settings_popup.draft().server.port, 8080);
    app.handle_key_event(key(KeyCode::Enter));
    assert_eq!(app.settings_popup.draft().server.port, 9090);
    assert!(app.settings_popup.error().is_none());
}

#[test]
fn settings_paste_corrects_a_rejected_preset_name_without_applying_early() {
    let mut app = app_with_proxy_presets("dev");
    app.open_settings_popup();
    app.settings_popup
        .select_topic_for_tests(SettingsTopic::Proxy);
    app.settings_popup
        .select_proxy_row_for_tests(ProxyRow::PresetName);
    focus_settings_content(&mut app);
    app.handle_key_event(key(KeyCode::Enter));
    for _ in 0.."dev".len() {
        app.handle_key_event(key(KeyCode::Backspace));
    }
    app.handle_paste("qa");
    assert_eq!(
        app.settings_popup
            .active_field_edit(FieldEditKind::ProxyPresetName)
            .unwrap()
            .value,
        "qa"
    );
    app.handle_key_event(key(KeyCode::Enter));
    assert!(
        app.settings_popup
            .field_hint(FieldEditKind::ProxyPresetName)
            .is_some()
    );
    assert_eq!(
        app.settings_popup.active_proxy_preset().unwrap().name,
        "dev"
    );
    app.handle_paste("\r\n\t");
    assert!(
        app.settings_popup
            .field_hint(FieldEditKind::ProxyPresetName)
            .is_some()
    );
    app.handle_paste("-雪");
    assert!(
        app.settings_popup
            .field_hint(FieldEditKind::ProxyPresetName)
            .is_none()
    );
    assert_eq!(
        app.settings_popup.active_proxy_preset().unwrap().name,
        "dev"
    );
    app.handle_key_event(key(KeyCode::Enter));
    assert_eq!(
        app.settings_popup.active_proxy_preset().unwrap().name,
        "qa-雪"
    );
}

#[test]
fn settings_paste_edits_only_the_active_prefilter_and_cancel_keeps_applied_value() {
    let mut settings = AppSettings::default();
    settings.recording.prefilter.include_url_patterns = vec![
        RecordingPrefilterPatternSettings::new("https://example.com/終"),
        RecordingPrefilterPatternSettings::new("https://other.example/*"),
    ];
    let mut app = App::with_settings(settings.clone(), RecordingState::default());
    app.open_settings_popup();
    app.settings_popup
        .select_topic_for_tests(SettingsTopic::Recording);
    focus_settings_content(&mut app);
    app.handle_key_event(key(KeyCode::Down));
    app.handle_key_event(key(KeyCode::Down));
    app.handle_key_event(key(KeyCode::Enter));
    app.handle_key_event(key(KeyCode::Enter));
    app.handle_key_event(key(KeyCode::Left));
    app.handle_paste("雪\r\n");
    let edit = app.settings_popup.prefilter_pattern_edit().unwrap();
    assert_eq!(edit.value, "https://example.com/雪終");
    assert_eq!(app.settings_popup.draft(), &settings);
    app.handle_key_event(key(KeyCode::Enter));
    let expected = vec![
        RecordingPrefilterPatternSettings::new("https://example.com/雪終"),
        settings.recording.prefilter.include_url_patterns[1].clone(),
    ];
    assert_eq!(
        app.settings_popup
            .draft()
            .recording
            .prefilter
            .include_url_patterns,
        expected
    );
    app.handle_key_event(key(KeyCode::Enter));
    app.handle_paste("/discarded");
    app.handle_key_event(key(KeyCode::Esc));
    assert_eq!(
        app.settings_popup
            .draft()
            .recording
            .prefilter
            .include_url_patterns,
        expected
    );
}

#[test]
fn settings_paste_filters_presets_at_unicode_cursor_without_committing() {
    let mut settings = settings_with_proxy_presets("其他");
    for (preset, name) in
        settings
            .proxy
            .as_mut()
            .unwrap()
            .presets
            .iter_mut()
            .zip(["前猫", "前雪猫", "其他"])
    {
        preset.name = name.to_string();
    }
    let mut app = App::with_settings(settings.clone(), RecordingState::default());
    app.open_settings_popup();
    app.settings_popup
        .select_topic_for_tests(SettingsTopic::Proxy);
    focus_settings_content(&mut app);
    app.handle_key_event(key(KeyCode::Enter));
    app.begin_request_search();
    let target = app.settings_popup.active_select_target().unwrap();
    let focused = app
        .settings_popup
        .select_state(target)
        .unwrap()
        .focused_filtered_index();
    app.handle_paste("\r\n\t");
    assert_eq!(
        app.settings_popup
            .select_state(target)
            .unwrap()
            .focused_filtered_index(),
        focused
    );
    app.handle_paste("前猫");
    app.handle_key_event(key(KeyCode::Left));
    app.handle_paste("雪\n");
    let state = app.settings_popup.select_state(target).unwrap();
    assert_eq!(state.filter(), "前雪猫");
    app.handle_key_event(key(KeyCode::Backspace));
    assert_eq!(
        app.settings_popup.select_state(target).unwrap().filter(),
        "前猫"
    );
    assert_eq!(app.settings_popup.draft(), &settings);
    assert_eq!(app.request_search_query(), None);
    app.handle_key_event(key(KeyCode::Enter));
    assert_eq!(
        app.settings_popup.active_proxy_preset().unwrap().name,
        "前猫"
    );
}

#[test]
fn settings_paste_keeps_creation_available_without_an_implicit_commit() {
    let mut app = App::new(ui_settings(true));
    app.open_settings_popup();
    app.settings_popup
        .select_topic_for_tests(SettingsTopic::Proxy);
    focus_settings_content(&mut app);
    app.handle_key_event(key(KeyCode::Enter));
    app.handle_paste("unmatched\n");
    let target = app.settings_popup.active_select_target().unwrap();
    assert_eq!(
        app.settings_popup.select_state(target).unwrap().filter(),
        "unmatched"
    );
    assert!(app.settings_popup.draft().proxy.is_none());
    app.handle_key_event(key(KeyCode::Enter));
    let proxy = app.settings_popup.draft().proxy.as_ref().unwrap();
    assert_eq!(proxy.presets.len(), 1);
    assert_eq!(proxy.active_preset.as_deref(), Some("Preset 1"));
    assert_eq!(proxy.presets[0].name, "Preset 1");
}

#[test]
fn settings_fields_delete_whole_graphemes_without_truncating_long_values() {
    for (kind, topic, row) in [
        (
            FieldEditKind::CertificateStoreDir,
            SettingsTopic::Certificate,
            0,
        ),
        (
            FieldEditKind::CertificatePemFilename,
            SettingsTopic::Certificate,
            1,
        ),
        (FieldEditKind::ProxyPresetName, SettingsTopic::Proxy, 1),
    ] {
        let mut app = App::with_settings(
            settings_with_proxy_presets("dev"),
            RecordingState::default(),
        );
        app.open_settings_popup();
        app.settings_popup.select_topic_for_tests(topic);
        focus_settings_content(&mut app);
        for _ in 0..row {
            app.handle_key_event(key(KeyCode::Down));
        }
        app.handle_key_event(key(KeyCode::Enter));
        let original = app
            .settings_popup
            .active_field_edit(kind)
            .unwrap()
            .value
            .to_owned();
        let prefix = "x".repeat(520);
        app.handle_paste(&format!("{prefix}a👨‍👩‍👧e\u{301}z"));
        app.handle_key_event(key(KeyCode::Left));
        app.handle_key_event(key(KeyCode::Backspace));
        app.handle_key_event(key(KeyCode::Backspace));
        app.handle_paste("X");
        assert_eq!(
            app.settings_popup.active_field_edit(kind).unwrap().value,
            format!("{original}{prefix}aXz"),
        );
        assert!(app.take_settings_save_request().is_none());
    }
}

#[test]
fn mapping_fields_delete_whole_graphemes() {
    for table in [ProxyRuleTable::Remote, ProxyRuleTable::Local] {
        let mut app = app_with_mapping_paste_editor(table);
        for field in [RuleEditField::From, RuleEditField::To] {
            let original = match field {
                RuleEditField::From => app.settings_popup.rule_editor().unwrap().from,
                RuleEditField::To => app.settings_popup.rule_editor().unwrap().to,
            }
            .to_owned();
            app.handle_paste("/a👨‍👩‍👧e\u{301}z");
            app.handle_key_event(key(KeyCode::Left));
            app.handle_key_event(key(KeyCode::Backspace));
            app.handle_key_event(key(KeyCode::Backspace));
            app.handle_paste("X");
            let editor = app.settings_popup.rule_editor().unwrap();
            assert_eq!(
                match field {
                    RuleEditField::From => editor.from,
                    RuleEditField::To => editor.to,
                },
                format!("{original}/aXz"),
            );
            app.handle_key_event(key(KeyCode::Tab));
        }
        assert!(!app.settings_popup.is_dirty());
        app.handle_key_event(key(KeyCode::Esc));
        assert!(!app.settings_popup.is_dirty());
    }
}

#[test]
fn prefilter_input_deletes_whole_graphemes_before_apply() {
    let mut app = App::new(ui_settings(true));
    app.open_settings_popup();
    app.settings_popup
        .draft_mut_for_tests()
        .recording
        .prefilter
        .include_url_patterns = vec![RecordingPrefilterPatternSettings::new(
        "https://example.com/",
    )];
    app.settings_popup
        .select_topic_for_tests(SettingsTopic::Recording);
    app.settings_popup.select_prefilter_pattern(0);
    app.handle_key_event(key(KeyCode::Enter));
    app.handle_paste("a👨‍👩‍👧e\u{301}z");
    app.handle_key_event(key(KeyCode::Left));
    app.handle_key_event(key(KeyCode::Backspace));
    app.handle_key_event(key(KeyCode::Backspace));
    app.handle_paste("X");
    app.handle_key_event(key(KeyCode::Enter));
    assert_eq!(
        app.settings_popup
            .draft()
            .recording
            .prefilter
            .include_url_patterns[0]
            .pattern,
        "https://example.com/aXz",
    );
}
