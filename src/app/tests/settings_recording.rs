use super::*;

#[test]
fn settings_popup_toggles_recording_prefilter_without_removing_patterns() {
    let mut settings = crate::settings::AppSettings::default();
    settings
        .recording
        .prefilter
        .include_url_patterns
        .push(RecordingPrefilterPatternSettings::new(
            "https://api.example.com/*",
        ));
    let mut app = App::with_settings(settings, RecordingState::default());
    app.handle_key_event(key(KeyCode::Char('m')));
    app.settings_popup
        .select_topic_for_tests(SettingsTopic::Recording);
    focus_settings_content(&mut app);
    app.handle_key_event(key(KeyCode::Down));

    app.handle_key_event(key(KeyCode::Char(' ')));

    assert!(!app.settings_popup.draft().recording.prefilter.enable);
    assert_eq!(
        app.settings_popup
            .draft()
            .recording
            .prefilter
            .include_url_patterns,
        vec![RecordingPrefilterPatternSettings::new(
            "https://api.example.com/*"
        )]
    );
}

#[test]
fn settings_popup_space_toggles_selected_prefilter_pattern() {
    let mut settings = crate::settings::AppSettings::default();
    settings.recording.prefilter.include_url_patterns =
        vec![RecordingPrefilterPatternSettings::new(
            "https://api.example.com/*",
        )];
    let mut app = App::with_settings(settings, RecordingState::default());
    app.handle_key_event(key(KeyCode::Char('m')));
    app.settings_popup
        .select_topic_for_tests(SettingsTopic::Recording);
    focus_settings_content(&mut app);
    app.handle_key_event(key(KeyCode::Down));
    app.handle_key_event(key(KeyCode::Down));
    app.handle_key_event(key(KeyCode::Enter));

    app.handle_key_event(key(KeyCode::Char(' ')));

    let pattern = &app
        .settings_popup
        .draft()
        .recording
        .prefilter
        .include_url_patterns[0];
    assert_eq!(pattern.pattern, "https://api.example.com/*");
    assert!(!pattern.enable);
}

#[test]
fn settings_popup_edits_and_saves_invalid_prefilter_pattern_without_validation_error() {
    let mut app = App::new(ui_settings(true));
    app.handle_key_event(key(KeyCode::Char('m')));
    app.settings_popup
        .select_topic_for_tests(SettingsTopic::Recording);
    focus_settings_content(&mut app);
    app.handle_key_event(key(KeyCode::Down));
    app.handle_key_event(key(KeyCode::Down));
    app.handle_key_event(key(KeyCode::Enter));
    app.handle_key_event(key(KeyCode::Char('a')));
    app.handle_key_event(key(KeyCode::Enter));
    for _ in 0.."https://example.com/*".chars().count() {
        app.handle_key_event(key(KeyCode::Backspace));
    }
    app.handle_key_event(key(KeyCode::Char('[')));
    app.handle_key_event(key(KeyCode::Enter));
    app.handle_key_event(key(KeyCode::Esc));

    app.handle_key_event(key(KeyCode::Char('s')));

    let saved = app
        .take_settings_save_request()
        .expect("invalid glob should still be saved");
    assert_eq!(
        saved.recording.prefilter.include_url_patterns,
        vec![RecordingPrefilterPatternSettings::new("[")]
    );
    assert!(app.settings_popup.error().is_none());
}

#[test]
fn settings_popup_prefilter_deletion_clamps_selection_and_handles_empty_table() {
    let mut settings = crate::settings::AppSettings::default();
    settings.recording.prefilter.include_url_patterns = vec![
        RecordingPrefilterPatternSettings::new("first"),
        RecordingPrefilterPatternSettings::new("second"),
    ];
    let mut app = App::with_settings(settings, RecordingState::default());
    app.handle_key_event(key(KeyCode::Char('m')));
    app.settings_popup
        .select_topic_for_tests(SettingsTopic::Recording);
    focus_settings_content(&mut app);
    app.handle_key_event(key(KeyCode::Down));
    app.handle_key_event(key(KeyCode::Down));
    app.handle_key_event(key(KeyCode::Enter));
    app.handle_key_event(key(KeyCode::Char('j')));

    app.handle_key_event(key(KeyCode::Char('d')));

    assert_eq!(
        app.settings_popup
            .draft()
            .recording
            .prefilter
            .include_url_patterns,
        vec![RecordingPrefilterPatternSettings::new("first")]
    );
    assert_eq!(app.settings_popup.active_prefilter_pattern(), Some(0));

    app.handle_key_event(key(KeyCode::Char('d')));

    assert!(
        app.settings_popup
            .draft()
            .recording
            .prefilter
            .include_url_patterns
            .is_empty()
    );
    assert_eq!(app.settings_popup.active_prefilter_pattern(), None);
}

#[test]
fn settings_popup_prefilter_reorders_patterns_and_tracks_selection() {
    let mut settings = crate::settings::AppSettings::default();
    settings.recording.prefilter.include_url_patterns = vec![
        RecordingPrefilterPatternSettings::new("first"),
        RecordingPrefilterPatternSettings {
            pattern: "second".to_string(),
            enable: false,
        },
        RecordingPrefilterPatternSettings::new("third"),
    ];
    let mut app = App::with_settings(settings, RecordingState::default());
    app.handle_key_event(key(KeyCode::Char('m')));
    app.settings_popup
        .select_topic_for_tests(SettingsTopic::Recording);
    focus_settings_content(&mut app);
    app.handle_key_event(key(KeyCode::Down));
    app.handle_key_event(key(KeyCode::Down));
    app.handle_key_event(key(KeyCode::Enter));
    app.handle_key_event(key(KeyCode::Char('j')));

    app.handle_key_event(key(KeyCode::Char('J')));

    assert_eq!(
        app.settings_popup
            .draft()
            .recording
            .prefilter
            .include_url_patterns,
        vec![
            RecordingPrefilterPatternSettings::new("first"),
            RecordingPrefilterPatternSettings::new("third"),
            RecordingPrefilterPatternSettings {
                pattern: "second".to_string(),
                enable: false,
            },
        ]
    );
    assert_eq!(app.settings_popup.active_prefilter_pattern(), Some(2));

    app.handle_key_event(key(KeyCode::Char('K')));

    assert_eq!(
        app.settings_popup
            .draft()
            .recording
            .prefilter
            .include_url_patterns,
        vec![
            RecordingPrefilterPatternSettings::new("first"),
            RecordingPrefilterPatternSettings {
                pattern: "second".to_string(),
                enable: false,
            },
            RecordingPrefilterPatternSettings::new("third"),
        ]
    );
    assert_eq!(app.settings_popup.active_prefilter_pattern(), Some(1));
}

#[test]
fn settings_popup_prefilter_editor_escape_discards_changes() {
    let mut settings = crate::settings::AppSettings::default();
    settings.recording.prefilter.include_url_patterns = vec![RecordingPrefilterPatternSettings {
        pattern: "original".to_string(),
        enable: false,
    }];
    let mut app = App::with_settings(settings, RecordingState::default());
    app.handle_key_event(key(KeyCode::Char('m')));
    app.settings_popup
        .select_topic_for_tests(SettingsTopic::Recording);
    focus_settings_content(&mut app);
    app.handle_key_event(key(KeyCode::Down));
    app.handle_key_event(key(KeyCode::Down));
    app.handle_key_event(key(KeyCode::Enter));
    app.handle_key_event(key(KeyCode::Enter));

    app.handle_key_event(key(KeyCode::Char('x')));
    app.handle_key_event(key(KeyCode::Esc));

    assert_eq!(
        app.settings_popup
            .draft()
            .recording
            .prefilter
            .include_url_patterns,
        vec![RecordingPrefilterPatternSettings {
            pattern: "original".to_string(),
            enable: false,
        }]
    );
    assert_eq!(app.settings_popup.active_prefilter_pattern(), Some(0));
}
