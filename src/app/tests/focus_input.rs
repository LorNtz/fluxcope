use super::*;

#[test]
fn focus_starts_on_request_list() {
    let app = App::new(ui_settings(true));

    assert!(app.is_panel_focused(PanelFocus::RequestList));
    assert!(!app.is_popup_focused(PopupFocus::Certificate));
}

#[test]
fn recording_state_can_start_disabled() {
    let app = App::with_recording(ui_settings(true), RecordingState::new(false));

    assert!(!app.is_recording());
}

#[test]
fn log_panel_starts_hidden() {
    let app = App::new(ui_settings(true));

    assert!(!app.log_panel.visible);
}

#[test]
fn r_toggles_recording_as_global_key() {
    let mut app = App::new(ui_settings(true));

    app.focus_panel(PanelFocus::Detail);
    app.handle_key_event(key(KeyCode::Char('r')));
    assert!(!app.is_recording());
    assert!(app.is_panel_focused(PanelFocus::Detail));

    app.handle_key_event(key(KeyCode::Char('r')));
    assert!(app.is_recording());
}

#[test]
fn key_release_events_do_not_run_app_commands() {
    let mut app = App::new(ui_settings(true));

    assert!(!app.handle_key_event(key_with_kind(KeyCode::Char('q'), KeyEventKind::Release)));
    assert!(app.is_recording());

    app.handle_key_event(key_with_kind(KeyCode::Char('r'), KeyEventKind::Release));

    assert!(app.is_recording());
}

#[test]
fn tab_cycles_focus_through_visible_panels() {
    let mut app = App::new(ui_settings(true));

    assert!(!app.handle_key_event(key(KeyCode::Tab)));
    assert!(app.is_panel_focused(PanelFocus::Detail));

    assert!(!app.handle_key_event(key(KeyCode::Tab)));
    assert!(app.is_panel_focused(PanelFocus::RequestList));

    app.handle_key_event(key(KeyCode::Char('@')));
    assert!(app.log_panel.visible);
    assert!(app.is_panel_focused(PanelFocus::Log));

    assert!(!app.handle_key_event(key(KeyCode::Tab)));
    assert!(app.is_panel_focused(PanelFocus::Log));

    app.handle_key_event(key(KeyCode::Char('@')));
    assert!(!app.log_panel.visible);
    assert!(app.is_panel_focused(PanelFocus::Detail));
    assert!(!app.handle_key_event(key(KeyCode::Tab)));
    assert!(app.is_panel_focused(PanelFocus::RequestList));
}

#[test]
fn directional_focus_uses_semantic_panel_graph() {
    let mut app = App::new(ui_settings(true));

    app.handle_key_event(ctrl_key(KeyCode::Char('l')));
    assert!(app.is_panel_focused(PanelFocus::Detail));

    app.handle_key_event(ctrl_key(KeyCode::Char('j')));
    assert!(app.is_panel_focused(PanelFocus::Detail));

    app.handle_key_event(key(KeyCode::Char('@')));
    assert!(app.is_panel_focused(PanelFocus::Log));

    app.handle_key_event(ctrl_key(KeyCode::Char('h')));
    assert!(app.is_panel_focused(PanelFocus::Log));
}

#[test]
fn hiding_focused_log_panel_moves_focus_to_detail() {
    let mut app = App::new(ui_settings(true));

    app.handle_key_event(key(KeyCode::Char('@')));
    assert!(app.log_panel.visible);
    assert!(app.is_panel_focused(PanelFocus::Log));

    app.handle_key_event(key(KeyCode::Char('@')));

    assert!(!app.log_panel.visible);
    assert!(app.is_panel_focused(PanelFocus::Detail));
}

#[test]
fn certificate_popup_takes_modal_focus_until_escape() {
    let mut app = App::new(ui_settings(true));

    app.handle_key_event(key(KeyCode::Char('@')));
    app.handle_key_event(key(KeyCode::Char('c')));

    assert!(app.certificate_popup.visible);
    assert!(app.is_popup_focused(PopupFocus::Certificate));
    assert!(!app.is_panel_focused(PanelFocus::Log));

    app.handle_key_event(ctrl_key(KeyCode::Char('h')));
    assert_eq!(app.focus.panel(), PanelFocus::Log);
    assert!(app.is_popup_focused(PopupFocus::Certificate));

    app.handle_key_event(key(KeyCode::Esc));
    assert!(!app.certificate_popup.visible);
    assert!(!app.is_popup_focused(PopupFocus::Certificate));
    assert!(app.is_panel_focused(PanelFocus::Log));
}

#[test]
fn detail_focus_uses_h_l_to_switch_tabs() {
    let mut app = App::new(ui_settings(true));

    app.focus_panel(PanelFocus::Detail);
    app.handle_key_event(key(KeyCode::Char('h')));
    assert_eq!(app.detail_panel.active_tab, MainDisplayTab::ResponseBody);

    app.handle_key_event(key(KeyCode::Char('l')));
    assert_eq!(app.detail_panel.active_tab, MainDisplayTab::RequestHeader);

    app.handle_key_event(key(KeyCode::Char('l')));
    assert_eq!(app.detail_panel.active_tab, MainDisplayTab::RequestBody);
}

#[test]
fn detail_tabs_restore_their_own_scroll_offsets() {
    let mut app = App::new(ui_settings(true));
    app.focus_panel(PanelFocus::Detail);
    app.detail_panel.scroll.offset = 3;

    app.handle_key_event(key(KeyCode::Char('l')));
    assert_eq!(app.detail_panel.active_tab, MainDisplayTab::RequestBody);
    assert_eq!(app.detail_panel.scroll.offset, 0);
    app.detail_panel.scroll.offset = 5;

    app.handle_key_event(key(KeyCode::Char('l')));
    assert_eq!(app.detail_panel.active_tab, MainDisplayTab::ResponseHeader);
    assert_eq!(app.detail_panel.scroll.offset, 0);

    app.handle_key_event(key(KeyCode::Char('h')));
    assert_eq!(app.detail_panel.active_tab, MainDisplayTab::RequestBody);
    assert_eq!(app.detail_panel.scroll.offset, 5);

    app.handle_key_event(key(KeyCode::Char('h')));
    assert_eq!(app.detail_panel.active_tab, MainDisplayTab::RequestHeader);
    assert_eq!(app.detail_panel.scroll.offset, 3);
}
