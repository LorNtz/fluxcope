use super::*;

#[test]
fn settings_popup_selects_proxy_preset_from_keyboard() {
    let settings = settings_with_proxy_presets("dev");
    let mut app = App::with_settings(settings, RecordingState::default());
    app.handle_key_event(key(KeyCode::Char('m')));
    app.settings_popup
        .select_topic_for_tests(SettingsTopic::Proxy);
    app.settings_popup
        .select_proxy_row_for_tests(ProxyRow::Preset);

    focus_settings_content(&mut app);
    app.handle_key_event(key(KeyCode::Enter));
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
    assert!(app.settings_popup.is_dirty());
}

#[test]
fn settings_popup_filters_proxy_preset_select() {
    let settings = settings_with_proxy_presets("dev");
    let mut app = App::with_settings(settings, RecordingState::default());
    app.handle_key_event(key(KeyCode::Char('m')));
    app.settings_popup
        .select_topic_for_tests(SettingsTopic::Proxy);

    focus_settings_content(&mut app);
    app.handle_key_event(key(KeyCode::Enter));
    app.handle_key_event(key(KeyCode::Char('p')));
    app.handle_key_event(key(KeyCode::Enter));

    assert_eq!(
        app.settings_popup
            .draft()
            .proxy
            .as_ref()
            .and_then(|proxy| proxy.active_preset.as_deref()),
        Some("prod")
    );
}

#[test]
fn settings_popup_esc_closes_proxy_preset_select_without_changing_value() {
    let settings = settings_with_proxy_presets("dev");
    let mut app = App::with_settings(settings, RecordingState::default());
    app.handle_key_event(key(KeyCode::Char('m')));
    app.settings_popup
        .select_topic_for_tests(SettingsTopic::Proxy);

    focus_settings_content(&mut app);
    app.handle_key_event(key(KeyCode::Enter));
    app.handle_key_event(key(KeyCode::Down));
    app.handle_key_event(key(KeyCode::Esc));

    assert_eq!(
        app.settings_popup
            .draft()
            .proxy
            .as_ref()
            .and_then(|proxy| proxy.active_preset.as_deref()),
        Some("dev")
    );
    assert!(!app.settings_popup.is_dirty());
}

#[test]
fn settings_popup_opening_empty_proxy_preset_select_does_not_create_proxy_settings() {
    let mut app = App::new(ui_settings(true));
    app.handle_key_event(key(KeyCode::Char('m')));
    app.settings_popup
        .select_topic_for_tests(SettingsTopic::Proxy);

    focus_settings_content(&mut app);
    app.handle_key_event(key(KeyCode::Enter));

    assert_eq!(app.settings_popup.active_select_target(), None);
    assert!(app.settings_popup.draft().proxy.is_none());
    assert!(!app.settings_popup.is_dirty());
}

#[test]
fn settings_popup_stale_active_preset_can_select_existing_preset() {
    let mut app = app_with_proxy_presets("missing");
    app.handle_key_event(key(KeyCode::Char('m')));
    app.settings_popup
        .select_topic_for_tests(SettingsTopic::Proxy);

    focus_settings_content(&mut app);
    app.handle_key_event(key(KeyCode::Enter));
    assert_eq!(
        app.settings_popup.active_select_target(),
        Some(SelectTarget::ProxyPreset)
    );
    assert!(app.settings_popup.commit_active_select_filtered_index(1));

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
fn settings_popup_clamps_stale_proxy_row_before_content_action() {
    let mut app = app_with_proxy_presets("dev");
    app.handle_key_event(key(KeyCode::Char('m')));
    app.settings_popup
        .select_topic_for_tests(SettingsTopic::Proxy);
    app.settings_popup
        .select_proxy_row_for_tests(ProxyRow::MapLocalEnabled);
    app.settings_popup
        .draft_mut_for_tests()
        .proxy
        .as_mut()
        .expect("proxy should exist")
        .active_preset = Some("missing".to_string());
    focus_settings_content(&mut app);

    app.handle_key_event(key(KeyCode::Char(' ')));

    assert_eq!(app.settings_popup.selected_row, 0);
    assert!(
        app.settings_popup
            .draft()
            .proxy
            .as_ref()
            .expect("proxy should exist")
            .enable
    );
}

#[test]
fn settings_popup_space_toggles_proxy_mapping_enabled() {
    let mut app = app_with_proxy_presets("dev");
    app.handle_key_event(key(KeyCode::Char('m')));
    app.settings_popup
        .select_topic_for_tests(SettingsTopic::Proxy);
    app.settings_popup
        .select_proxy_row_for_tests(ProxyRow::MappingEnabled);
    focus_settings_content(&mut app);

    app.handle_key_event(key(KeyCode::Char(' ')));

    assert!(
        !app.settings_popup
            .draft()
            .proxy
            .as_ref()
            .expect("proxy should exist")
            .enable
    );
}

#[test]
fn settings_popup_space_toggles_map_remote_enabled() {
    let mut app = app_with_proxy_presets("dev");
    app.handle_key_event(key(KeyCode::Char('m')));
    app.settings_popup
        .select_topic_for_tests(SettingsTopic::Proxy);
    app.settings_popup
        .select_proxy_row_for_tests(ProxyRow::MapRemoteEnabled);
    focus_settings_content(&mut app);

    app.handle_key_event(key(KeyCode::Char(' ')));

    assert!(
        !app.settings_popup
            .draft()
            .proxy
            .as_ref()
            .expect("proxy should exist")
            .presets[0]
            .map_remote
            .enable
    );
}

#[test]
fn settings_popup_space_toggles_map_local_enabled() {
    let mut app = app_with_proxy_presets("dev");
    app.handle_key_event(key(KeyCode::Char('m')));
    app.settings_popup
        .select_topic_for_tests(SettingsTopic::Proxy);
    app.settings_popup
        .select_proxy_row_for_tests(ProxyRow::MapLocalEnabled);
    focus_settings_content(&mut app);

    app.handle_key_event(key(KeyCode::Char(' ')));

    assert!(
        !app.settings_popup
            .draft()
            .proxy
            .as_ref()
            .expect("proxy should exist")
            .presets[0]
            .map_local
            .enable
    );
}

#[test]
fn settings_popup_renames_active_proxy_preset() {
    let mut app = app_with_proxy_presets("dev");
    app.handle_key_event(key(KeyCode::Char('m')));
    app.settings_popup
        .select_topic_for_tests(SettingsTopic::Proxy);
    app.settings_popup
        .select_proxy_row_for_tests(ProxyRow::PresetName);
    focus_settings_content(&mut app);

    app.handle_key_event(key(KeyCode::Enter));
    for _ in 0.."dev".chars().count() {
        app.handle_key_event(key(KeyCode::Backspace));
    }
    for ch in "local dev".chars() {
        app.handle_key_event(key(KeyCode::Char(ch)));
    }
    app.handle_key_event(key(KeyCode::Enter));

    let proxy = app
        .settings_popup
        .draft()
        .proxy
        .as_ref()
        .expect("proxy should exist");
    assert_eq!(proxy.presets[0].name, "local dev");
    assert_eq!(proxy.active_preset.as_deref(), Some("local dev"));
}

#[test]
fn settings_popup_rejects_duplicate_proxy_preset_name_inline() {
    let mut app = app_with_proxy_presets("dev");
    app.handle_key_event(key(KeyCode::Char('m')));
    app.settings_popup
        .select_topic_for_tests(SettingsTopic::Proxy);
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

    let proxy = app
        .settings_popup
        .draft()
        .proxy
        .as_ref()
        .expect("proxy should exist");
    assert_eq!(proxy.presets[0].name, "dev");
    assert_eq!(
        app.settings_popup
            .field_hint(FieldEditKind::ProxyPresetName),
        Some("preset name already exists")
    );
    assert!(
        app.settings_popup
            .active_field_edit(FieldEditKind::ProxyPresetName)
            .is_some()
    );
}

#[test]
fn settings_popup_rejects_empty_proxy_preset_name_inline() {
    let mut app = app_with_proxy_presets("dev");
    app.handle_key_event(key(KeyCode::Char('m')));
    app.settings_popup
        .select_topic_for_tests(SettingsTopic::Proxy);
    app.settings_popup
        .select_proxy_row_for_tests(ProxyRow::PresetName);
    focus_settings_content(&mut app);

    app.handle_key_event(key(KeyCode::Enter));
    for _ in 0.."dev".chars().count() {
        app.handle_key_event(key(KeyCode::Backspace));
    }
    app.handle_key_event(key(KeyCode::Enter));

    assert_eq!(
        app.settings_popup
            .field_hint(FieldEditKind::ProxyPresetName),
        Some("preset name cannot be empty")
    );
    assert_eq!(
        app.settings_popup.draft().proxy.as_ref().unwrap().presets[0].name,
        "dev"
    );
}

#[test]
fn settings_popup_proxy_mapping_topics_are_merged() {
    let topics = SettingsTopic::all()
        .iter()
        .map(|topic| topic.title())
        .collect::<Vec<_>>();

    assert_eq!(
        topics,
        vec!["Server", "Certificate", "Recording", "Interface", "Proxy"]
    );
}

#[test]
fn settings_popup_adds_remote_rule_from_keyboard() {
    let mut app = app_with_proxy_presets("dev");
    app.handle_key_event(key(KeyCode::Char('m')));
    app.settings_popup
        .select_topic_for_tests(SettingsTopic::Proxy);
    app.settings_popup
        .select_proxy_row_for_tests(ProxyRow::RemoteHeader);
    focus_settings_content(&mut app);

    app.handle_key_event(key(KeyCode::Char('a')));

    assert_eq!(
        app.settings_popup.draft().proxy.as_ref().unwrap().presets[0]
            .map_remote
            .rules
            .len(),
        1
    );
}

#[test]
fn settings_popup_edits_new_remote_rule_from_keyboard() {
    let mut app = app_with_proxy_presets("dev");
    app.handle_key_event(key(KeyCode::Char('m')));
    app.settings_popup
        .select_topic_for_tests(SettingsTopic::Proxy);
    app.settings_popup
        .select_proxy_row_for_tests(ProxyRow::RemoteHeader);
    focus_settings_content(&mut app);
    app.handle_key_event(key(KeyCode::Char('a')));
    app.settings_popup
        .select_proxy_row_for_tests(ProxyRow::RemoteRule(0));

    app.handle_key_event(key(KeyCode::Enter));
    for _ in 0.."https://example.com".chars().count() {
        app.handle_key_event(key(KeyCode::Backspace));
    }
    for ch in "https://api.example.com/v1".chars() {
        app.handle_key_event(key(KeyCode::Char(ch)));
    }
    app.handle_key_event(key(KeyCode::Tab));
    for _ in 0.."http://localhost:3000".chars().count() {
        app.handle_key_event(key(KeyCode::Backspace));
    }
    for ch in "http://localhost:8080".chars() {
        app.handle_key_event(key(KeyCode::Char(ch)));
    }
    app.handle_key_event(key(KeyCode::Enter));

    assert_eq!(
        app.settings_popup.draft().proxy.as_ref().unwrap().presets[0]
            .map_remote
            .rules[0]
            .from,
        "https://api.example.com/v1"
    );
    assert_eq!(
        app.settings_popup.draft().proxy.as_ref().unwrap().presets[0]
            .map_remote
            .rules[0]
            .to,
        "http://localhost:8080"
    );
}

#[test]
fn settings_popup_enter_in_proxy_rule_table_opens_rule_editor_popup() {
    let mut app = app_with_proxy_presets("dev");
    app.handle_key_event(key(KeyCode::Char('m')));
    app.settings_popup
        .select_topic_for_tests(SettingsTopic::Proxy);
    app.settings_popup
        .select_proxy_row_for_tests(ProxyRow::RemoteHeader);
    focus_settings_content(&mut app);
    app.handle_key_event(key(KeyCode::Char('a')));
    app.settings_popup
        .select_proxy_row_for_tests(ProxyRow::RemoteRule(0));

    app.handle_key_event(key(KeyCode::Enter));

    assert!(app.settings_popup.rule_editor_for_tests().is_some());
}

#[test]
fn settings_popup_rule_editor_updates_from_and_to_together() {
    let mut app = app_with_proxy_presets("dev");
    app.handle_key_event(key(KeyCode::Char('m')));
    app.settings_popup
        .select_topic_for_tests(SettingsTopic::Proxy);
    app.settings_popup
        .select_proxy_row_for_tests(ProxyRow::RemoteHeader);
    focus_settings_content(&mut app);
    app.handle_key_event(key(KeyCode::Char('a')));
    app.settings_popup
        .select_proxy_row_for_tests(ProxyRow::RemoteRule(0));
    app.handle_key_event(key(KeyCode::Enter));

    for _ in 0.."https://example.com".chars().count() {
        app.handle_key_event(key(KeyCode::Backspace));
    }
    for ch in "https://api.example.com/v1".chars() {
        app.handle_key_event(key(KeyCode::Char(ch)));
    }
    app.handle_key_event(key(KeyCode::Tab));
    for _ in 0.."http://localhost:3000".chars().count() {
        app.handle_key_event(key(KeyCode::Backspace));
    }
    for ch in "http://localhost:8080".chars() {
        app.handle_key_event(key(KeyCode::Char(ch)));
    }
    app.handle_key_event(key(KeyCode::Enter));

    let rule = &app.settings_popup.draft().proxy.as_ref().unwrap().presets[0]
        .map_remote
        .rules[0];
    assert_eq!(rule.from, "https://api.example.com/v1");
    assert_eq!(rule.to, "http://localhost:8080");
    assert!(app.settings_popup.rule_editor_for_tests().is_none());
}

#[test]
fn settings_popup_enter_in_local_rule_table_opens_local_rule_editor_popup() {
    let mut app = app_with_proxy_presets("dev");
    app.handle_key_event(key(KeyCode::Char('m')));
    app.settings_popup
        .select_topic_for_tests(SettingsTopic::Proxy);
    app.settings_popup
        .select_proxy_row_for_tests(ProxyRow::LocalHeader);
    focus_settings_content(&mut app);
    app.handle_key_event(key(KeyCode::Char('a')));
    app.settings_popup
        .select_proxy_row_for_tests(ProxyRow::LocalRule(0));

    app.handle_key_event(key(KeyCode::Enter));

    let editor = app
        .settings_popup
        .rule_editor_for_tests()
        .expect("local rule editor should open");
    assert_eq!(editor.title, "Edit Map Local Rule");
}

#[test]
fn settings_popup_adds_local_rule_from_combined_proxy_keyboard() {
    let mut app = app_with_proxy_presets("dev");
    app.handle_key_event(key(KeyCode::Char('m')));
    app.settings_popup
        .select_topic_for_tests(SettingsTopic::Proxy);
    app.settings_popup
        .select_proxy_row_for_tests(ProxyRow::LocalHeader);
    focus_settings_content(&mut app);

    app.handle_key_event(key(KeyCode::Char('a')));

    assert_eq!(
        app.settings_popup.draft().proxy.as_ref().unwrap().presets[0]
            .map_local
            .rules
            .len(),
        1
    );
}

#[test]
fn settings_popup_rejects_mapping_rule_url_query() {
    let mut settings = crate::settings::AppSettings::default();
    settings.proxy = Some(crate::settings::ProxySettings {
        presets: vec![crate::settings::ProxyPresetSettings {
            name: "dev".to_string(),
            map_remote: crate::settings::ProxyMapRemoteSettings {
                enable: true,
                rules: vec![crate::settings::ProxyMapRemoteRule {
                    from: "https://example.com/api?q=1".to_string(),
                    to: "http://localhost:3000".to_string(),
                    enable: true,
                }],
            },
            map_local: crate::settings::ProxyMapLocalSettings::default(),
        }],
        active_preset: Some("dev".to_string()),
        enable: true,
    });
    let mut popup = SettingsPopup::new();

    popup.open(settings);

    assert!(popup.validate().is_err());
}

#[test]
fn unsaved_confirm_enter_validation_error_returns_to_settings() {
    let mut app = App::new(ui_settings(true));
    app.handle_key_event(key(KeyCode::Char('m')));
    app.settings_popup.draft_mut_for_tests().server.port = 0;
    app.handle_key_event(key(KeyCode::Esc));

    app.handle_key_event(key(KeyCode::Enter));

    assert!(app.settings_popup.visible);
    assert!(!app.settings_popup.is_confirming_unsaved());
    assert!(app.settings_popup.error().is_some());
    assert!(app.take_settings_save_request().is_none());
}
