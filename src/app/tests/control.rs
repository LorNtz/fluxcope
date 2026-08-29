use super::{captured, captured_with_sequence, ui_settings};
use crate::{app::App, recording::RecordingState};

#[test]
fn control_summary_reads_authoritative_recording_capture_and_settings_state() {
    let mut app = App::with_recording(ui_settings(false), RecordingState::new(true));
    app.add_request(captured_with_sequence(1, "https://status.example/one"));
    app.add_request(captured_with_sequence(2, "https://status.example/two"));

    let summary = app.control_summary();

    assert!(summary.recording_enabled);
    assert_eq!(summary.retained_capture_count, 2);
    assert_eq!(summary.settings_revision, 0);
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
