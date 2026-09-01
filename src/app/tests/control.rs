use super::{captured, captured_with_sequence, ui_settings};
use crate::{
    app::App,
    recording::RecordingState,
    settings::{
        AppSettings, ProxyMapLocalSettings, ProxyMapRemoteSettings, ProxyPresetSettings,
        ProxySettings,
    },
};

#[test]
fn control_summary_reads_authoritative_recording_capture_and_settings_state() {
    let mut app = App::with_recording(ui_settings(false), RecordingState::new(true));
    app.add_request(captured_with_sequence(1, "https://status.example/one"));
    app.add_request(captured_with_sequence(2, "https://status.example/two"));

    let summary = app.control_summary();

    assert!(summary.recording_enabled);
    assert_eq!(summary.retained_capture_count, 2);
    assert_eq!(
        summary.settings_revision,
        crate::runtime::settings::SettingsRevision::INITIAL.get()
    );
    assert_eq!(summary.capture_store.revision, 2);
    assert!(summary.capture_store.retained_bytes > 0);
    assert!(!summary.mapping.configured);
}

#[test]
fn control_summary_reflects_live_state_instead_of_a_startup_copy() {
    let mut app = App::with_recording(ui_settings(false), RecordingState::new(false));
    let initial = app.control_summary();
    assert!(!initial.recording_enabled);
    assert_eq!(initial.retained_capture_count, 0);

    app.toggle_recording();
    app.add_request(captured("https://status.example/live"));
    let current = app.control_summary();

    assert!(current.recording_enabled);
    assert_eq!(current.retained_capture_count, 1);
}

#[test]
fn control_summary_reports_current_mapping_gates() {
    let app = App::with_settings(
        AppSettings {
            proxy: Some(ProxySettings {
                enable: false,
                active_preset: Some("dev".to_owned()),
                presets: vec![ProxyPresetSettings {
                    name: "dev".to_owned(),
                    map_remote: ProxyMapRemoteSettings {
                        enable: true,
                        rules: Vec::new(),
                    },
                    map_local: ProxyMapLocalSettings {
                        enable: false,
                        rules: Vec::new(),
                    },
                }],
            }),
            ..AppSettings::default()
        },
        RecordingState::new(true),
    );

    let mapping = app.control_summary().mapping;
    assert!(mapping.configured);
    assert!(!mapping.enabled);
    assert_eq!(mapping.active_preset.as_deref(), Some("dev"));
    assert_eq!(mapping.map_remote_enabled, Some(true));
    assert_eq!(mapping.map_local_enabled, Some(false));
}
