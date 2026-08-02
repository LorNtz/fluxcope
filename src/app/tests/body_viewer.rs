use super::*;

#[test]
fn detail_body_enter_activates_viewer_and_esc_exits() {
    let mut app = App::new(ui_settings(true));
    let mut req = captured("https://some.host.com/api");
    req.req_body = Some("alpha".to_string());
    app.add_request(req);
    app.focus_panel(PanelFocus::Detail);
    app.detail_panel.select_tab(MainDisplayTab::RequestBody);

    app.handle_key_event(key(KeyCode::Enter));

    assert!(app.detail_panel.body_viewer.is_active());
    render_app(&mut app);
    assert!(app.detail_panel.body_viewer.editor_mut().is_some());

    app.handle_key_event(key(KeyCode::Esc));

    assert!(!app.detail_panel.body_viewer.is_active());
    assert!(app.detail_panel.body_viewer.editor_mut().is_none());
}

#[test]
fn detail_body_viewer_shadows_app_keys_and_stays_read_only() {
    let mut app = App::new(ui_settings(true));
    let mut req = captured("https://some.host.com/api");
    req.req_body = Some("alpha".to_string());
    app.add_request(req);
    app.focus_panel(PanelFocus::Detail);
    app.detail_panel.select_tab(MainDisplayTab::RequestBody);
    app.handle_key_event(key(KeyCode::Enter));
    render_app(&mut app);

    let lines_before = app
        .detail_panel
        .body_viewer
        .editor_mut()
        .unwrap()
        .lines
        .clone();

    assert!(!app.handle_key_event(key(KeyCode::Char('q'))));
    app.handle_key_event(key(KeyCode::Char('r')));
    app.handle_key_event(key(KeyCode::Char('l')));
    app.handle_key_event(key(KeyCode::Char('x')));

    assert!(app.is_recording());
    assert_eq!(app.detail_panel.active_tab, MainDisplayTab::RequestBody);
    assert_eq!(
        app.detail_panel.body_viewer.editor_mut().unwrap().lines,
        lines_before
    );
}

#[test]
fn detail_body_viewer_search_can_move_cursor_without_editing() {
    let mut app = App::new(ui_settings(true));
    let mut req = captured("https://some.host.com/api");
    req.req_body = Some("alpha beta".to_string());
    app.add_request(req);
    app.focus_panel(PanelFocus::Detail);
    app.detail_panel.select_tab(MainDisplayTab::RequestBody);
    app.handle_key_event(key(KeyCode::Enter));
    render_app(&mut app);

    app.handle_key_event(key(KeyCode::Char('/')));
    app.handle_key_event(key(KeyCode::Char('b')));
    app.handle_key_event(key(KeyCode::Enter));

    let editor = app.detail_panel.body_viewer.editor_mut().unwrap();
    assert_eq!(editor.cursor.row, 0);
    assert_eq!(editor.cursor.col, 6);
    assert_eq!(editor.lines, edtui::Lines::from("alpha beta"));
}

#[test]
fn detail_body_viewer_yanks_current_line_in_normal_mode() {
    let mut app = body_viewer_app_with_body("alpha\nbeta");
    let clipboard = attach_test_clipboard(&mut app);

    press_chars(&mut app, &['y', 'y']);

    assert_eq!(&*clipboard.borrow(), "\nalpha");
    let editor = app.detail_panel.body_viewer.editor_mut().unwrap();
    assert_eq!(editor.cursor, Index2::new(0, 0));
    assert_eq!(editor.mode, EditorMode::Normal);
    assert!(editor.selection.is_none());
    assert_eq!(editor.lines, edtui::Lines::from("alpha\nbeta"));
}

#[test]
fn detail_body_viewer_yank_word_uses_operator_motion_without_moving_cursor() {
    let mut app = body_viewer_app_with_body("alpha beta");
    let clipboard = attach_test_clipboard(&mut app);

    press_chars(&mut app, &['y', 'w']);

    assert_eq!(&*clipboard.borrow(), "alpha ");
    let editor = app.detail_panel.body_viewer.editor_mut().unwrap();
    assert_eq!(editor.cursor, Index2::new(0, 0));
    assert_eq!(editor.mode, EditorMode::Normal);
    assert!(editor.selection.is_none());
}

#[test]
fn detail_body_viewer_yank_to_end_of_line_restores_cursor() {
    let mut app = body_viewer_app_with_body("alpha beta");
    let clipboard = attach_test_clipboard(&mut app);
    app.detail_panel.body_viewer.editor_mut().unwrap().cursor = Index2::new(0, 6);

    press_chars(&mut app, &['y', '$']);

    assert_eq!(&*clipboard.borrow(), "beta");
    let editor = app.detail_panel.body_viewer.editor_mut().unwrap();
    assert_eq!(editor.cursor, Index2::new(0, 6));
    assert_eq!(editor.mode, EditorMode::Normal);
    assert!(editor.selection.is_none());
}

#[test]
fn detail_body_viewer_yank_backward_word_excludes_cursor() {
    let mut app = body_viewer_app_with_body("alpha beta");
    let clipboard = attach_test_clipboard(&mut app);
    app.detail_panel.body_viewer.editor_mut().unwrap().cursor = Index2::new(0, 6);

    press_chars(&mut app, &['y', 'b']);

    assert_eq!(&*clipboard.borrow(), "alpha ");
    let editor = app.detail_panel.body_viewer.editor_mut().unwrap();
    assert_eq!(editor.cursor, Index2::new(0, 6));
    assert_eq!(editor.mode, EditorMode::Normal);
    assert!(editor.selection.is_none());
}

#[test]
fn detail_body_viewer_yanks_single_forward_char() {
    let mut app = body_viewer_app_with_body("alpha");
    let clipboard = attach_test_clipboard(&mut app);
    app.detail_panel.body_viewer.editor_mut().unwrap().cursor = Index2::new(0, 1);

    press_chars(&mut app, &['y', 'l']);

    assert_eq!(&*clipboard.borrow(), "l");
    let editor = app.detail_panel.body_viewer.editor_mut().unwrap();
    assert_eq!(editor.cursor, Index2::new(0, 1));
    assert_eq!(editor.mode, EditorMode::Normal);
    assert!(editor.selection.is_none());
}

#[test]
fn detail_body_viewer_yanks_single_backward_char() {
    let mut app = body_viewer_app_with_body("alpha");
    let clipboard = attach_test_clipboard(&mut app);
    app.detail_panel.body_viewer.editor_mut().unwrap().cursor = Index2::new(0, 2);

    press_chars(&mut app, &['y', 'h']);

    assert_eq!(&*clipboard.borrow(), "l");
    let editor = app.detail_panel.body_viewer.editor_mut().unwrap();
    assert_eq!(editor.cursor, Index2::new(0, 2));
    assert_eq!(editor.mode, EditorMode::Normal);
    assert!(editor.selection.is_none());
}

#[test]
fn detail_body_viewer_yanks_to_start_of_line() {
    let mut app = body_viewer_app_with_body("alpha");
    let clipboard = attach_test_clipboard(&mut app);
    app.detail_panel.body_viewer.editor_mut().unwrap().cursor = Index2::new(0, 3);

    press_chars(&mut app, &['y', '0']);

    assert_eq!(&*clipboard.borrow(), "alp");
    let editor = app.detail_panel.body_viewer.editor_mut().unwrap();
    assert_eq!(editor.cursor, Index2::new(0, 3));
    assert_eq!(editor.mode, EditorMode::Normal);
    assert!(editor.selection.is_none());
}

#[test]
fn detail_body_viewer_yanks_linewise_motion() {
    let mut app = body_viewer_app_with_body("alpha\nbeta\ngamma");
    let clipboard = attach_test_clipboard(&mut app);

    press_chars(&mut app, &['y', 'j']);

    assert_eq!(&*clipboard.borrow(), "\nalpha\nbeta");
    let editor = app.detail_panel.body_viewer.editor_mut().unwrap();
    assert_eq!(editor.cursor, Index2::new(0, 0));
    assert_eq!(editor.mode, EditorMode::Normal);
    assert!(editor.selection.is_none());
}

#[test]
fn detail_body_viewer_yanks_to_last_line_with_g_motion() {
    let mut app = body_viewer_app_with_body("alpha\nbeta\ngamma");
    let clipboard = attach_test_clipboard(&mut app);

    press_chars(&mut app, &['y', 'G']);

    assert_eq!(&*clipboard.borrow(), "\nalpha\nbeta\ngamma");
    let editor = app.detail_panel.body_viewer.editor_mut().unwrap();
    assert_eq!(editor.cursor, Index2::new(0, 0));
    assert_eq!(editor.mode, EditorMode::Normal);
    assert!(editor.selection.is_none());
}

#[test]
fn detail_body_viewer_yanks_to_first_line_with_gg_motion() {
    let mut app = body_viewer_app_with_body("alpha\nbeta\ngamma");
    let clipboard = attach_test_clipboard(&mut app);
    app.detail_panel.body_viewer.editor_mut().unwrap().cursor = Index2::new(2, 0);

    press_chars(&mut app, &['y', 'g', 'g']);

    assert_eq!(&*clipboard.borrow(), "\nalpha\nbeta\ngamma");
    let editor = app.detail_panel.body_viewer.editor_mut().unwrap();
    assert_eq!(editor.cursor, Index2::new(2, 0));
    assert_eq!(editor.mode, EditorMode::Normal);
    assert!(editor.selection.is_none());
}

#[test]
fn detail_body_viewer_yanks_to_end_of_word() {
    let mut app = body_viewer_app_with_body("alpha beta");
    let clipboard = attach_test_clipboard(&mut app);
    app.detail_panel.body_viewer.editor_mut().unwrap().cursor = Index2::new(0, 6);

    press_chars(&mut app, &['y', 'e']);

    assert_eq!(&*clipboard.borrow(), "beta");
    let editor = app.detail_panel.body_viewer.editor_mut().unwrap();
    assert_eq!(editor.cursor, Index2::new(0, 6));
    assert_eq!(editor.mode, EditorMode::Normal);
    assert!(editor.selection.is_none());
}

#[test]
fn detail_body_viewer_yanks_inner_word_text_object() {
    let mut app = body_viewer_app_with_body("alpha beta");
    let clipboard = attach_test_clipboard(&mut app);
    app.detail_panel.body_viewer.editor_mut().unwrap().cursor = Index2::new(0, 2);

    press_chars(&mut app, &['y', 'i', 'w']);

    assert_eq!(&*clipboard.borrow(), "alpha");
    let editor = app.detail_panel.body_viewer.editor_mut().unwrap();
    assert_eq!(editor.cursor, Index2::new(0, 2));
    assert_eq!(editor.mode, EditorMode::Normal);
    assert!(editor.selection.is_none());
}

#[test]
fn detail_body_viewer_yanks_inner_delimiter_text_object() {
    let mut app = body_viewer_app_with_body("name \"value\" done");
    let clipboard = attach_test_clipboard(&mut app);
    app.detail_panel.body_viewer.editor_mut().unwrap().cursor = Index2::new(0, 7);

    press_chars(&mut app, &['y', 'i', '"']);

    assert_eq!(&*clipboard.borrow(), "value");
    let editor = app.detail_panel.body_viewer.editor_mut().unwrap();
    assert_eq!(editor.cursor, Index2::new(0, 7));
    assert_eq!(editor.mode, EditorMode::Normal);
    assert!(editor.selection.is_none());
}

#[test]
fn detail_body_viewer_missing_text_object_does_not_copy_stale_selection() {
    let mut app = body_viewer_app_with_body("name value done");
    let clipboard = attach_test_clipboard(&mut app);
    app.detail_panel.body_viewer.editor_mut().unwrap().cursor = Index2::new(0, 5);
    press_chars(&mut app, &['v', 'l']);

    {
        let editor = app.detail_panel.body_viewer.editor_mut().unwrap();
        editor.cursor = Index2::new(0, 7);
        editor.mode = EditorMode::Normal;
        assert!(editor.selection.is_some());
    }
    press_chars(&mut app, &['y', 'i', '"']);

    assert_eq!(&*clipboard.borrow(), "");
    let editor = app.detail_panel.body_viewer.editor_mut().unwrap();
    assert_eq!(editor.cursor, Index2::new(0, 7));
    assert_eq!(editor.mode, EditorMode::Normal);
    assert!(editor.selection.is_some());
}

#[test]
fn detail_body_viewer_visual_y_still_copies_selection() {
    let mut app = body_viewer_app_with_body("alpha beta");
    let clipboard = attach_test_clipboard(&mut app);

    press_chars(&mut app, &['v', 'l', 'y']);

    assert_eq!(&*clipboard.borrow(), "al");
    let editor = app.detail_panel.body_viewer.editor_mut().unwrap();
    assert_eq!(editor.cursor, Index2::new(0, 1));
    assert_eq!(editor.mode, EditorMode::Normal);
    assert!(editor.selection.is_none());
}

#[test]
fn detail_body_viewer_escape_cancels_pending_yank_without_exiting() {
    let mut app = body_viewer_app_with_body("alpha");
    let clipboard = attach_test_clipboard(&mut app);

    app.handle_key_event(key(KeyCode::Char('y')));
    app.handle_key_event(key(KeyCode::Esc));
    press_chars(&mut app, &['y', 'y']);

    assert!(app.detail_panel.body_viewer.is_active());
    assert_eq!(&*clipboard.borrow(), "\nalpha");
    let editor = app.detail_panel.body_viewer.editor_mut().unwrap();
    assert_eq!(editor.cursor, Index2::new(0, 0));
    assert_eq!(editor.mode, EditorMode::Normal);
    assert!(editor.selection.is_none());
}

#[test]
fn detail_body_viewer_pending_yank_consumes_jump_start_key() {
    let mut app = body_viewer_app_with_body("alpha");
    let clipboard = attach_test_clipboard(&mut app);

    press_chars(&mut app, &['y', 's', 'y', 'y']);

    assert_eq!(&*clipboard.borrow(), "\nalpha");
    let editor = app.detail_panel.body_viewer.editor_mut().unwrap();
    assert_eq!(editor.cursor, Index2::new(0, 0));
    assert_eq!(editor.mode, EditorMode::Normal);
    assert!(editor.selection.is_none());
}

#[test]
fn detail_body_viewer_unsupported_yank_suffix_cancels_pending_operator() {
    let mut app = body_viewer_app_with_body("alpha");
    let clipboard = attach_test_clipboard(&mut app);

    press_chars(&mut app, &['y', 'z', 'l']);

    assert_eq!(&*clipboard.borrow(), "");
    let editor = app.detail_panel.body_viewer.editor_mut().unwrap();
    assert_eq!(editor.cursor, Index2::new(0, 1));
    assert_eq!(editor.mode, EditorMode::Normal);
    assert!(editor.selection.is_none());
}

#[test]
fn detail_body_viewer_resets_on_tab_change() {
    let mut app = App::new(ui_settings(true));
    let mut req = captured("https://some.host.com/api");
    req.req_body = Some("alpha".to_string());
    app.add_request(req);
    app.focus_panel(PanelFocus::Detail);
    app.detail_panel.select_tab(MainDisplayTab::RequestBody);
    app.handle_key_event(key(KeyCode::Enter));

    app.detail_panel.next_tab();

    assert!(!app.detail_panel.body_viewer.is_active());
    assert_eq!(app.detail_panel.active_tab, MainDisplayTab::ResponseHeader);
}
