use super::*;

#[test]
fn settings_popup_tracks_dirty_draft() {
    let settings = crate::settings::AppSettings::default();
    let mut popup = SettingsPopup::new();

    popup.open(settings);
    assert!(!popup.is_dirty());

    popup.draft_mut_for_tests().recording.start_record_on_launch = false;
    assert!(popup.is_dirty());
}
#[test]
fn persistent_settings_context_uses_save_commit_label() {
    let app = app_with_settings_context(SettingsUiContext {
        config_mode: ConfigMode::DefaultOwned,
        persistence: PersistenceMode::Persistent,
    });

    assert_eq!(app.settings_popup.commit_label(), "Save");
}

#[test]
fn ephemeral_settings_context_uses_apply_commit_label() {
    let app = app_with_settings_context(SettingsUiContext {
        config_mode: ConfigMode::Temporary,
        persistence: PersistenceMode::Ephemeral,
    });

    assert_eq!(app.settings_popup.commit_label(), "Apply");
}

#[test]
fn settings_popup_rejects_invalid_remote_rule_url() {
    let mut settings = crate::settings::AppSettings::default();
    settings.proxy = Some(crate::settings::ProxySettings {
        presets: vec![crate::settings::ProxyPresetSettings {
            name: "dev".to_string(),
            map_remote: crate::settings::ProxyMapRemoteSettings {
                enable: true,
                rules: vec![crate::settings::ProxyMapRemoteRule {
                    from: "not a url".to_string(),
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
fn settings_popup_adds_reorders_and_deletes_remote_rules() {
    let mut popup = SettingsPopup::new();
    popup.open(settings_with_proxy_presets("dev"));

    popup.add_remote_rule_after(None);
    popup.add_remote_rule_after(Some(0));
    assert_eq!(
        popup.draft().proxy.as_ref().unwrap().presets[0]
            .map_remote
            .rules
            .len(),
        2
    );

    popup.draft_mut_for_tests().proxy.as_mut().unwrap().presets[0]
        .map_remote
        .rules[0]
        .from = "https://first.example.com".to_string();
    popup.draft_mut_for_tests().proxy.as_mut().unwrap().presets[0]
        .map_remote
        .rules[1]
        .from = "https://second.example.com".to_string();

    popup.move_remote_rule_down(0);
    assert_eq!(
        popup.draft().proxy.as_ref().unwrap().presets[0]
            .map_remote
            .rules[0]
            .from,
        "https://second.example.com"
    );

    popup.delete_remote_rule(0);
    assert_eq!(
        popup.draft().proxy.as_ref().unwrap().presets[0]
            .map_remote
            .rules
            .len(),
        1
    );
}

#[test]
fn m_opens_settings_popup() {
    let mut app = App::new(ui_settings(true));

    app.handle_key_event(key(KeyCode::Char('m')));

    assert!(app.settings_popup.visible);
    assert!(app.is_popup_focused(PopupFocus::Settings));
    assert_eq!(app.settings_popup.focus, SettingsPaneFocus::Topics);
}

#[test]
fn settings_popup_topics_focus_ignores_content_shortcuts() {
    let mut app = App::new(ui_settings(true));
    app.handle_key_event(key(KeyCode::Char('m')));
    app.settings_popup
        .select_topic_for_tests(SettingsTopic::Recording);

    app.handle_key_event(key(KeyCode::Char(' ')));

    assert!(app.settings_popup.draft().recording.start_record_on_launch);
    assert!(!app.settings_popup.is_dirty());
}

#[test]
fn clean_settings_popup_esc_closes() {
    let mut app = App::new(ui_settings(true));
    app.handle_key_event(key(KeyCode::Char('m')));

    app.handle_key_event(key(KeyCode::Esc));

    assert!(!app.settings_popup.visible);
    assert!(!app.is_popup_focused(PopupFocus::Settings));
}

#[test]
fn dirty_settings_popup_esc_opens_unsaved_confirm() {
    let mut app = App::new(ui_settings(true));
    app.handle_key_event(key(KeyCode::Char('m')));
    app.settings_popup
        .draft_mut_for_tests()
        .recording
        .start_record_on_launch = false;

    app.handle_key_event(key(KeyCode::Esc));

    assert!(app.settings_popup.visible);
    assert!(app.settings_popup.is_confirming_unsaved());
    assert!(app.is_popup_focused(PopupFocus::Settings));
}

#[test]
fn unsaved_confirm_esc_discards_and_closes_settings() {
    let mut app = App::new(ui_settings(true));
    app.handle_key_event(key(KeyCode::Char('m')));
    app.settings_popup
        .draft_mut_for_tests()
        .recording
        .start_record_on_launch = false;
    app.handle_key_event(key(KeyCode::Esc));

    app.handle_key_event(key(KeyCode::Esc));

    assert!(!app.settings_popup.visible);
    assert!(app.take_settings_save_request().is_none());
}

#[test]
fn unsaved_confirm_enter_queues_save_request() {
    let mut app = App::new(ui_settings(true));
    app.handle_key_event(key(KeyCode::Char('m')));
    app.settings_popup
        .draft_mut_for_tests()
        .recording
        .start_record_on_launch = false;
    app.handle_key_event(key(KeyCode::Esc));

    app.handle_key_event(key(KeyCode::Enter));

    assert!(app.take_settings_save_request().is_some());
}

#[test]
fn successful_settings_save_closes_popup_focus() {
    let mut app = App::new(ui_settings(true));
    app.focus_panel(PanelFocus::Detail);
    app.handle_key_event(key(KeyCode::Char('m')));

    app.handle_key_event(key(KeyCode::Char('s')));
    let saved = app
        .take_settings_save_request()
        .expect("settings save should be queued");
    app.finish_settings_save(saved, crate::runtime::settings::SettingsRevision::INITIAL);

    assert!(!app.settings_popup.visible);
    assert!(!app.is_popup_focused(PopupFocus::Settings));
    assert!(app.is_panel_focused(PanelFocus::Detail));

    app.handle_key_event(key(KeyCode::Char('r')));

    assert!(!app.is_recording());
}

#[test]
fn tui_save_completion_keeps_the_authoritative_transaction_revision() {
    let mut app = App::new(ui_settings(true));
    app.open_settings_popup();
    app.settings_popup
        .draft_mut_for_tests()
        .recording
        .start_record_on_launch = false;
    let saved = app.settings_popup.draft().clone();
    let revision = crate::runtime::settings::SettingsRevision::new(2);

    app.apply_settings_transaction_commit_from_origin(
        std::sync::Arc::new(saved.clone()),
        revision,
        crate::runtime::settings::SettingsTransactionOrigin::Tui,
    )
    .expect("authoritative transaction commit");
    app.finish_settings_save(saved, revision);

    assert_eq!(app.settings_revision(), revision);
}

#[test]
fn failed_settings_save_keeps_settings_popup_focused() {
    let mut app = App::new(ui_settings(true));
    app.handle_key_event(key(KeyCode::Char('m')));

    app.fail_settings_save("disk full".to_string());

    assert!(app.settings_popup.visible);
    assert!(app.is_popup_focused(PopupFocus::Settings));
    assert_eq!(app.settings_popup.error(), Some("disk full"));
}

#[test]
fn settings_popup_edits_server_port_field() {
    let mut app = App::new(ui_settings(true));
    app.handle_key_event(key(KeyCode::Char('m')));

    focus_settings_content(&mut app);
    app.handle_key_event(key(KeyCode::Enter));
    app.handle_key_event(key(KeyCode::Backspace));
    app.handle_key_event(key(KeyCode::Backspace));
    app.handle_key_event(key(KeyCode::Backspace));
    app.handle_key_event(key(KeyCode::Backspace));
    for ch in ['9', '0', '1', '4'] {
        app.handle_key_event(key(KeyCode::Char(ch)));
    }
    app.handle_key_event(key(KeyCode::Enter));

    assert_eq!(9014, app.settings_popup.draft().server.port);
    assert!(app.settings_popup.is_dirty());
}

#[test]
fn settings_popup_space_toggles_recording_launch_setting() {
    let mut app = App::new(ui_settings(true));
    app.handle_key_event(key(KeyCode::Char('m')));
    app.settings_popup
        .select_topic_for_tests(SettingsTopic::Recording);
    focus_settings_content(&mut app);

    app.handle_key_event(key(KeyCode::Char(' ')));

    assert!(!app.settings_popup.draft().recording.start_record_on_launch);
}

#[test]
fn settings_revision_starts_at_one_after_settings_load() {
    let app = App::new(ui_settings(true));

    assert_eq!(
        app.settings_revision(),
        crate::runtime::settings::SettingsRevision::INITIAL
    );
}

#[test]
fn external_commit_refreshes_an_open_clean_popup_without_closing_it() {
    let mut app = App::new(ui_settings(true));
    app.open_settings_popup();
    let mut committed = AppSettings::default();
    committed.server.port = 9193;

    app.apply_settings_transaction_commit(
        std::sync::Arc::new(committed),
        crate::runtime::settings::SettingsRevision::new(2),
    )
    .expect("clean popup refresh");

    assert!(app.settings_popup.visible);
    assert!(!app.settings_popup.is_dirty());
    assert_eq!(app.settings_popup.draft().server.port, 9193);
    assert_eq!(
        app.settings_revision(),
        crate::runtime::settings::SettingsRevision::new(2)
    );
}

#[test]
fn external_commit_never_overwrites_a_dirty_popup_draft() {
    let mut app = App::new(ui_settings(true));
    app.open_settings_popup();
    app.settings_popup.draft_mut_for_tests().server.port = 9194;
    let mut committed = AppSettings::default();
    committed.server.port = 9195;

    let error = app
        .apply_settings_transaction_commit(
            std::sync::Arc::new(committed),
            crate::runtime::settings::SettingsRevision::new(2),
        )
        .expect_err("dirty popup must have rejected transaction admission");

    assert_eq!(
        error.code(),
        crate::control_rpc::protocol::ControlErrorCode::TuiDraftConflict
    );
    assert_eq!(app.settings_popup.draft().server.port, 9194);
    assert_eq!(
        app.settings_revision(),
        crate::runtime::settings::SettingsRevision::INITIAL
    );
}

#[test]
fn pending_transaction_keeps_popup_readable_but_blocks_edits_and_saves() {
    let mut app = App::new(ui_settings(true));
    app.open_settings_popup();
    app.set_settings_transaction_pending(true);
    let before = app.settings_popup.draft().clone();

    focus_settings_content(&mut app);
    app.handle_key_event(key(KeyCode::Enter));
    app.handle_key_event(key(KeyCode::Char('9')));
    app.handle_key_event(key(KeyCode::Enter));
    app.handle_key_event(key(KeyCode::Char('s')));
    app.handle_key_event(key(KeyCode::Esc));

    assert!(
        app.settings_popup.visible,
        "pending save must reach a terminal result before the popup can close"
    );
    assert_eq!(app.settings_popup.draft(), &before);
    assert!(app.take_settings_save_request().is_none());
    assert!(app.settings_popup.error().is_some());
}
