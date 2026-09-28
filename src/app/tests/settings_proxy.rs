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
fn settings_popup_resolves_active_preset_by_trimmed_identity() {
    let mut settings = settings_with_proxy_presets("dev");
    settings.proxy.as_mut().expect("proxy should exist").presets[0].name = " dev ".to_string();
    let mut app = App::with_settings(settings, RecordingState::default());
    app.handle_key_event(key(KeyCode::Char('m')));
    app.settings_popup
        .select_topic_for_tests(SettingsTopic::Proxy);

    assert_eq!(
        app.settings_popup
            .active_proxy_preset()
            .map(|preset| preset.name.as_str()),
        Some(" dev ")
    );
    assert!(
        app.settings_popup
            .visible_proxy_widgets()
            .contains(&ProxyWidget::MapRemoteEnabled)
    );
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

fn settings_without_proxy_presets() -> [AppSettings; 3] {
    [
        AppSettings::default(),
        AppSettings {
            proxy: Some(crate::settings::ProxySettings::default()),
            ..AppSettings::default()
        },
        AppSettings {
            proxy: Some(crate::settings::ProxySettings {
                enable: false,
                active_preset: Some(String::new()),
                presets: vec![],
            }),
            ..AppSettings::default()
        },
    ]
}

#[test]
fn empty_default_preset_browsing_and_selection_preserve_settings() {
    for settings in settings_without_proxy_presets() {
        let mut app = App::with_settings(settings.clone(), RecordingState::default());
        app.open_settings_popup();
        app.settings_popup
            .select_topic_for_tests(SettingsTopic::Proxy);
        focus_settings_content(&mut app);
        app.handle_key_event(key(KeyCode::Enter));
        assert_eq!(
            app.settings_popup.active_select_target(),
            Some(SelectTarget::ProxyPreset)
        );
        app.handle_key_event(key(KeyCode::Esc));
        app.handle_key_event(key(KeyCode::Enter));
        assert!(app.settings_popup.commit_active_select_filtered_index(0));
        assert_eq!(app.settings_popup.draft(), &settings);
        assert!(!app.settings_popup.is_dirty());

        app.handle_key_event(key(KeyCode::Esc));
        assert!(!app.settings_popup.visible);
        assert!(app.take_settings_save_request().is_none());
        app.open_settings_popup();
        assert_eq!(app.settings_popup.draft(), &settings);
    }
}

#[test]
fn empty_default_preset_cancelled_noop_and_invalid_rename_stay_clean() {
    for settings in settings_without_proxy_presets() {
        let mut app = App::with_settings(settings.clone(), RecordingState::default());
        app.open_settings_popup();
        app.settings_popup
            .select_topic_for_tests(SettingsTopic::Proxy);
        app.settings_popup
            .select_proxy_row_for_tests(ProxyRow::PresetName);
        focus_settings_content(&mut app);
        app.handle_key_event(key(KeyCode::Enter));
        app.handle_key_event(key(KeyCode::Char('x')));
        app.handle_key_event(key(KeyCode::Esc));
        assert_eq!(app.settings_popup.draft(), &settings);
        assert!(!app.settings_popup.is_dirty());

        app.handle_key_event(key(KeyCode::Enter));
        app.handle_key_event(key(KeyCode::Char(' ')));
        app.handle_key_event(key(KeyCode::Enter));
        assert!(
            app.settings_popup
                .active_field_edit(FieldEditKind::ProxyPresetName)
                .is_none()
        );
        assert_eq!(app.settings_popup.draft(), &settings);
        assert!(!app.settings_popup.is_dirty());

        app.handle_key_event(key(KeyCode::Enter));
        for _ in 0.."default".len() {
            app.handle_key_event(key(KeyCode::Backspace));
        }
        app.handle_key_event(key(KeyCode::Enter));
        assert!(
            app.settings_popup
                .active_field_edit(FieldEditKind::ProxyPresetName)
                .is_some()
        );
        assert!(
            app.settings_popup
                .field_hint(FieldEditKind::ProxyPresetName)
                .is_some()
        );
        assert_eq!(app.settings_popup.draft(), &settings);
        assert!(!app.settings_popup.is_dirty());
    }
}

#[test]
fn empty_default_preset_unrelated_save_preserves_proxy_configuration() {
    for settings in settings_without_proxy_presets() {
        let mut app = App::with_settings(settings.clone(), RecordingState::default());
        app.open_settings_popup();
        app.settings_popup
            .select_topic_for_tests(SettingsTopic::Proxy);
        focus_settings_content(&mut app);
        app.handle_key_event(key(KeyCode::Enter));
        assert!(app.settings_popup.commit_active_select_filtered_index(0));
        app.settings_popup
            .select_topic_for_tests(SettingsTopic::Interface);
        app.handle_key_event(key(KeyCode::Char(' ')));
        app.handle_key_event(key(KeyCode::Char('s')));
        let saved = app.take_settings_save_request().expect("unrelated save");
        assert_eq!(saved.proxy, settings.proxy);
        assert_eq!(
            saved.ui.request_list.auto_expand,
            !settings.ui.request_list.auto_expand
        );
        app.finish_settings_save(saved, crate::runtime::settings::SettingsRevision::INITIAL);
        app.open_settings_popup();
        assert_eq!(app.settings_popup.draft().proxy, settings.proxy);
        assert!(!app.settings_popup.is_dirty());
    }
}

#[test]
fn empty_default_preset_add_edit_save_keeps_mapping_gates_disabled() {
    for settings in settings_without_proxy_presets() {
        for (header, row, table, old_to, new_to) in [
            (
                ProxyRow::RemoteHeader,
                ProxyRow::RemoteRule(0),
                ProxyRuleTable::Remote,
                "http://localhost:3000",
                "http://localhost:8080",
            ),
            (
                ProxyRow::LocalHeader,
                ProxyRow::LocalRule(0),
                ProxyRuleTable::Local,
                "~/mock-response.json",
                "/tmp/response.json",
            ),
        ] {
            let mut app = App::with_settings(settings.clone(), RecordingState::default());
            app.open_settings_popup();
            app.settings_popup
                .select_topic_for_tests(SettingsTopic::Proxy);
            app.settings_popup.select_proxy_row_for_tests(header);
            focus_settings_content(&mut app);
            app.handle_key_event(key(KeyCode::Char('a')));
            assert!(app.settings_popup.is_dirty());
            app.settings_popup.select_proxy_row_for_tests(row);
            app.handle_key_event(key(KeyCode::Enter));
            for _ in 0.."https://example.com".len() {
                app.handle_key_event(key(KeyCode::Backspace));
            }
            for ch in "https://api.example.com/v1".chars() {
                app.handle_key_event(key(KeyCode::Char(ch)));
            }
            app.handle_key_event(key(KeyCode::Tab));
            for _ in 0..old_to.len() {
                app.handle_key_event(key(KeyCode::Backspace));
            }
            for ch in new_to.chars() {
                app.handle_key_event(key(KeyCode::Char(ch)));
            }
            app.handle_key_event(key(KeyCode::Enter));
            app.handle_key_event(key(KeyCode::Esc));
            app.handle_key_event(key(KeyCode::Char('s')));
            let saved = app.take_settings_save_request().expect("rule save");
            let proxy = saved.proxy.as_ref().expect("materialized proxy");
            assert!(!proxy.enable);
            assert_eq!(proxy.active_preset.as_deref(), Some("default"));
            assert_eq!(proxy.presets.len(), 1);
            let preset = &proxy.presets[0];
            assert!(!preset.map_remote.enable);
            assert!(!preset.map_local.enable);
            let (from, to) = match table {
                ProxyRuleTable::Remote => {
                    assert!(preset.map_local.rules.is_empty());
                    assert_eq!(preset.map_remote.rules.len(), 1);
                    (
                        &preset.map_remote.rules[0].from,
                        &preset.map_remote.rules[0].to,
                    )
                }
                ProxyRuleTable::Local => {
                    assert!(preset.map_remote.rules.is_empty());
                    assert_eq!(preset.map_local.rules.len(), 1);
                    (
                        &preset.map_local.rules[0].from,
                        &preset.map_local.rules[0].to,
                    )
                }
            };
            assert_eq!(from, "https://api.example.com/v1");
            assert_eq!(to, new_to);
            let expected = saved.clone();
            app.finish_settings_save(saved, crate::runtime::settings::SettingsRevision::INITIAL);
            app.open_settings_popup();
            assert_eq!(app.settings_popup.draft(), expected.as_ref());
            assert!(!app.settings_popup.is_dirty());
        }
    }
}

#[test]
fn empty_default_preset_proxy_edits_discard_to_original_configuration() {
    for settings in settings_without_proxy_presets() {
        for row in [
            ProxyRow::MappingEnabled,
            ProxyRow::MapRemoteEnabled,
            ProxyRow::MapLocalEnabled,
            ProxyRow::PresetName,
            ProxyRow::RemoteHeader,
        ] {
            let mut app = App::with_settings(settings.clone(), RecordingState::default());
            app.open_settings_popup();
            app.settings_popup
                .select_topic_for_tests(SettingsTopic::Proxy);
            app.settings_popup.select_proxy_row_for_tests(row);
            focus_settings_content(&mut app);
            match row {
                ProxyRow::PresetName => {
                    app.handle_key_event(key(KeyCode::Enter));
                    app.handle_key_event(key(KeyCode::Char('x')));
                    app.handle_key_event(key(KeyCode::Enter));
                }
                ProxyRow::RemoteHeader => {
                    app.handle_key_event(key(KeyCode::Char('a')));
                }
                _ => {
                    app.handle_key_event(key(KeyCode::Char(' ')));
                }
            }
            let proxy = app
                .settings_popup
                .draft()
                .proxy
                .as_ref()
                .expect("proxy edit");
            assert_eq!(proxy.enable, matches!(row, ProxyRow::MappingEnabled));
            assert_eq!(
                proxy.presets[0].map_remote.enable,
                matches!(row, ProxyRow::MapRemoteEnabled)
            );
            assert_eq!(
                proxy.presets[0].map_local.enable,
                matches!(row, ProxyRow::MapLocalEnabled)
            );
            assert_eq!(
                proxy.active_preset.as_deref(),
                Some(if matches!(row, ProxyRow::PresetName) {
                    "defaultx"
                } else {
                    "default"
                })
            );
            assert!(app.settings_popup.is_dirty());
            app.handle_key_event(key(KeyCode::Esc));
            assert!(app.settings_popup.is_confirming_unsaved());
            app.handle_key_event(key(KeyCode::Esc));
            assert!(app.take_settings_save_request().is_none());
            app.open_settings_popup();
            assert_eq!(app.settings_popup.draft(), &settings);
            assert!(!app.settings_popup.is_dirty());
        }
    }
}

#[test]
fn settings_popup_create_preset_from_absent_proxy_is_disabled() {
    let mut app = App::new(ui_settings(true));
    app.open_settings_popup();
    app.settings_popup
        .select_topic_for_tests(SettingsTopic::Proxy);
    focus_settings_content(&mut app);
    app.handle_key_event(key(KeyCode::Enter));
    app.handle_key_event(key(KeyCode::Down));
    app.handle_key_event(key(KeyCode::Enter));

    let proxy = app
        .settings_popup
        .draft()
        .proxy
        .as_ref()
        .expect("created proxy");
    assert!(!proxy.enable);
    assert_eq!(proxy.active_preset.as_deref(), Some("Preset 1"));
    assert_eq!(proxy.presets.len(), 1);
    let preset = &proxy.presets[0];
    assert_eq!(preset.name, "Preset 1");
    assert!(!preset.map_remote.enable);
    assert!(!preset.map_local.enable);
    assert!(preset.map_remote.rules.is_empty());
    assert!(preset.map_local.rules.is_empty());
    assert_eq!(app.settings_popup.active_select_target(), None);
    assert!(app.settings_popup.is_dirty());
}

#[test]
fn settings_popup_create_preset_preserves_empty_proxy_global_gate() {
    for enabled in [false, true] {
        let mut settings = settings_with_proxy_presets("missing");
        let proxy = settings.proxy.as_mut().unwrap();
        proxy.enable = enabled;
        proxy.presets.clear();
        let mut app = App::with_settings(settings, RecordingState::default());
        app.open_settings_popup();
        app.settings_popup
            .select_topic_for_tests(SettingsTopic::Proxy);
        focus_settings_content(&mut app);
        app.handle_key_event(key(KeyCode::Enter));
        app.handle_key_event(key(KeyCode::Down));
        app.handle_key_event(key(KeyCode::Enter));

        let proxy = app.settings_popup.draft().proxy.as_ref().unwrap();
        assert_eq!(proxy.enable, enabled);
        assert_eq!(proxy.active_preset.as_deref(), Some("Preset 1"));
        assert_eq!(proxy.presets.len(), 1);
        assert!(!proxy.presets[0].map_remote.enable);
        assert!(!proxy.presets[0].map_local.enable);
    }
}

#[test]
fn settings_popup_create_preset_uses_smallest_unused_trimmed_identity() {
    let mut settings = settings_with_proxy_presets("Preset 1");
    let proxy = settings.proxy.as_mut().unwrap();
    proxy.presets[0].name = " Preset 1 ".to_string();
    proxy.presets[1].name = "Preset 3".to_string();
    proxy.presets[0]
        .map_remote
        .rules
        .push(crate::settings::ProxyMapRemoteRule {
            from: "https://example.com".to_string(),
            to: "http://localhost:3000".to_string(),
            enable: true,
        });
    let original = proxy.presets.clone();
    let mut app = App::with_settings(settings, RecordingState::default());
    app.open_settings_popup();
    app.settings_popup
        .select_topic_for_tests(SettingsTopic::Proxy);
    focus_settings_content(&mut app);

    for expected in ["Preset 2", "Preset 4"] {
        app.handle_key_event(key(KeyCode::Enter));
        for ch in "unmatched".chars() {
            app.handle_key_event(key(KeyCode::Char(ch)));
        }
        app.handle_key_event(key(KeyCode::Enter));
        let proxy = app.settings_popup.draft().proxy.as_ref().unwrap();
        assert_eq!(proxy.active_preset.as_deref(), Some(expected));
        assert_eq!(&proxy.presets[..original.len()], original.as_slice());
        assert!(proxy.enable);
        let created = proxy.presets.last().unwrap();
        assert_eq!(created.name, expected);
        assert!(!created.map_remote.enable);
        assert!(!created.map_local.enable);
        assert!(created.map_remote.rules.is_empty());
        assert!(created.map_local.rules.is_empty());
        assert_eq!(app.settings_popup.active_select_target(), None);
    }
    let proxy = app.settings_popup.draft().proxy.as_ref().unwrap();
    assert_eq!(proxy.presets.len(), original.len() + 2);
}

#[test]
fn settings_popup_create_preset_rename_survives_save_roundtrip() {
    let mut app = App::new(ui_settings(true));
    app.open_settings_popup();
    app.settings_popup
        .select_topic_for_tests(SettingsTopic::Proxy);
    focus_settings_content(&mut app);
    app.handle_key_event(key(KeyCode::Enter));
    app.handle_key_event(key(KeyCode::Down));
    app.handle_key_event(key(KeyCode::Enter));
    app.settings_popup
        .select_proxy_row_for_tests(ProxyRow::PresetName);
    app.handle_key_event(key(KeyCode::Enter));
    for _ in 0.."Preset 1".len() {
        app.handle_key_event(key(KeyCode::Backspace));
    }
    for ch in "local dev".chars() {
        app.handle_key_event(key(KeyCode::Char(ch)));
    }
    app.handle_key_event(key(KeyCode::Enter));
    app.handle_key_event(key(KeyCode::Char('s')));
    let saved = app.take_settings_save_request().expect("save new preset");
    let yaml = serde_yaml::to_string(&saved).expect("serialize settings");
    let reloaded: AppSettings = serde_yaml::from_str(&yaml).expect("reload settings");
    let proxy = reloaded.proxy.as_ref().expect("persisted proxy");
    assert_eq!(proxy.active_preset.as_deref(), Some("local dev"));
    assert_eq!(proxy.presets[0].name, "local dev");
    assert_eq!(proxy.presets.len(), 1);
    assert!(!proxy.enable);
    assert!(!proxy.presets[0].map_remote.enable);
    assert!(!proxy.presets[0].map_local.enable);
    app.finish_settings_save(
        reloaded,
        crate::runtime::settings::SettingsRevision::INITIAL,
    );
    app.open_settings_popup();
    assert_eq!(
        app.settings_popup.active_proxy_preset().unwrap().name,
        "local dev"
    );
    assert!(!app.settings_popup.is_dirty());
}

#[test]
fn settings_popup_create_preset_discard_restores_original_settings() {
    let mut app = App::new(ui_settings(true));
    app.open_settings_popup();
    app.settings_popup
        .select_topic_for_tests(SettingsTopic::Proxy);
    focus_settings_content(&mut app);
    app.handle_key_event(key(KeyCode::Enter));
    app.handle_key_event(key(KeyCode::Down));
    app.handle_key_event(key(KeyCode::Enter));
    assert!(app.settings_popup.is_dirty());
    app.handle_key_event(key(KeyCode::Esc));
    app.handle_key_event(key(KeyCode::Esc));
    assert!(app.take_settings_save_request().is_none());
    app.open_settings_popup();
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
fn settings_popup_rule_toggle_keeps_remote_and_local_tables_separate() {
    let mut app = app_with_proxy_presets("dev");
    app.handle_key_event(key(KeyCode::Char('m')));
    app.settings_popup
        .select_topic_for_tests(SettingsTopic::Proxy);
    focus_settings_content(&mut app);

    app.settings_popup
        .select_proxy_row_for_tests(ProxyRow::RemoteHeader);
    app.handle_key_event(key(KeyCode::Char('a')));
    app.settings_popup
        .select_proxy_row_for_tests(ProxyRow::LocalHeader);
    app.handle_key_event(key(KeyCode::Char('a')));
    app.settings_popup
        .select_proxy_row_for_tests(ProxyRow::RemoteRule(0));
    app.handle_key_event(key(KeyCode::Char(' ')));

    let preset = &app.settings_popup.draft().proxy.as_ref().unwrap().presets[0];
    assert!(!preset.map_remote.rules[0].enable);
    assert!(preset.map_local.rules[0].enable);
}

#[test]
fn settings_popup_invalid_rule_edit_is_atomic_and_reports_inline_error() {
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
    for ch in "not a url".chars() {
        app.handle_key_event(key(KeyCode::Char(ch)));
    }
    app.handle_key_event(key(KeyCode::Enter));

    let rule = &app.settings_popup.draft().proxy.as_ref().unwrap().presets[0]
        .map_remote
        .rules[0];
    assert_eq!(rule.from, "https://example.com");
    assert_eq!(rule.to, "http://localhost:3000");
    assert!(app.settings_popup.error().is_some());
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
