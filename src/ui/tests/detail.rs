use super::*;

#[test]
fn mapped_response_body_expands_tabs_only_for_paragraph_rendering() {
    let raw_body = "{\n\t\"route\": true\n}";
    let mut req = captured(0, "https://a.com/api");
    req.local_path = Some("/tmp/api.json".to_string());
    req.res_body = Some(raw_body.to_string());
    let mut app = App::new(ui_settings(true));
    app.add_request(req);
    app.focus_panel(PanelFocus::Detail);
    app.detail_panel.select_tab(MainDisplayTab::ResponseBody);

    let (ui, buffer) = render_to_buffer(&mut app);

    assert!(
        buffer
            .content()
            .iter()
            .all(|cell| !cell.symbol().contains('\t'))
    );
    let key = BodyViewerKey::new(CaptureSequence::new(0), MainDisplayTab::ResponseBody);
    assert_eq!(app.cached_body_text(key), Some(raw_body));
    assert_eq!(
        app.cached_body_render_text(key),
        Some("{\n    \"route\": true\n}")
    );
    assert!(find_buffer_text(&buffer, ui.right_panel.detail.area(), "\"route\": true").is_some());

    assert!(app.enter_current_body_viewer());
    assert_eq!(
        app.detail_panel.body_viewer.editor_mut().unwrap().lines,
        edtui::Lines::from(raw_body)
    );
}

#[tokio::test]
async fn pending_body_renders_loading_without_making_it_editor_content() {
    let shutdown = CancellationToken::new();
    let service = start_decode_service(DecodePolicy::default(), shutdown.clone());
    let mut req = captured(0, "https://a.com/api");
    req.req_body = Some("body".to_string());
    let mut app = App::new(ui_settings(true));
    app.add_request(req);
    app.focus_panel(PanelFocus::Detail);
    app.detail_panel.select_tab(MainDisplayTab::RequestBody);
    app.detail_panel.scroll.offset = 4;
    app.set_decode_client(service.client.clone());

    let (ui, buffer) = render_to_buffer(&mut app);
    let key = BodyViewerKey::new(CaptureSequence::new(0), MainDisplayTab::RequestBody);

    assert!(find_buffer_text(&buffer, ui.right_panel.detail.area(), BODY_LOADING_TEXT).is_some());
    assert!(app.cached_body_text(key).is_none());
    assert!(!app.enter_current_body_viewer());
    assert_eq!(app.detail_panel.scroll.offset, 4);

    render_to_buffer_with_size(&mut app, 12, 5);
    assert_eq!(app.detail_panel.scroll.max_offset, 0);
    assert_eq!(app.detail_panel.scroll.offset, 4);
    app.detail_panel.scroll_down();
    app.detail_panel.scroll_up();
    assert_eq!(app.detail_panel.scroll.offset, 4);

    let completed_body = (0..20)
        .map(|index| format!("line {index}"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(app.apply_decode_result(DecodeResult {
        key: DecodeKey {
            sequence: CaptureSequence::new(0),
            side: BodySide::Request,
            revision: 0,
            mode: DecodeDisplayMode::Request,
        },
        text: completed_body,
        limited: false,
        error: None,
    }));
    render_to_buffer(&mut app);
    assert!(app.detail_panel.scroll.max_offset >= 4);
    assert_eq!(app.detail_panel.scroll.offset, 4);

    shutdown.cancel();
    service
        .task
        .await
        .expect("decode service should join")
        .expect("decode service should stop");
}

#[test]
fn detail_tabs_render_on_panel_border_without_tabs_box() {
    let mut app = App::new(ui_settings(true));
    let (ui, detail_top_row) = render_detail_top_row(&mut app);
    let detail_area = ui.right_panel.detail.area();

    assert_eq!(detail_area.y, ui.right_panel.area().y);
    assert!(
        detail_top_row.contains("Request Header"),
        "{detail_top_row:?}"
    );
    assert!(
        detail_top_row.contains("Response Body"),
        "{detail_top_row:?}"
    );
}

#[test]
fn focused_detail_panel_keeps_unselected_tabs_default_color() {
    let mut app = App::new(ui_settings(true));
    app.focus_panel(PanelFocus::Detail);
    let (ui, buffer) = render_to_buffer(&mut app);
    let detail_area = ui.right_panel.detail.area();
    let selected_tab = tab_click_position(detail_area, MainDisplayTab::RequestHeader);
    let unselected_tab = tab_click_position(detail_area, MainDisplayTab::RequestBody);

    assert_eq!(buffer[selected_tab].fg, Color::Green);
    assert!(buffer[selected_tab].modifier.contains(Modifier::BOLD));
    assert_eq!(buffer[unselected_tab].fg, Color::Reset);
    assert!(!buffer[unselected_tab].modifier.contains(Modifier::BOLD));
}

#[test]
fn mouse_request_switch_preserves_detail_viewport_position() {
    let mut first = captured(0, "https://a.com/api/first");
    let mut second = captured(1, "https://a.com/api/second");
    first.req_headers = (0..10)
        .map(|index| (format!("x-row-{index}"), format!("first-{index}")))
        .collect();
    second.req_headers = (0..10)
        .map(|index| (format!("x-row-{index}"), format!("second-{index}")))
        .collect();
    let mut app = App::new(ui_settings(true));
    app.add_request(first);
    app.add_request(second);

    let (mut ui, buffer) = render_to_buffer(&mut app);
    let second_request = find_buffer_text(&buffer, ui.request_list.area(), "second")
        .expect("second request should be visible");
    app.detail_panel.scroll.offset = 4;

    ui.handle_mouse(
        mouse(
            MouseEventKind::Down(MouseButton::Left),
            second_request.x,
            second_request.y,
        ),
        &mut app,
    );

    assert_eq!(
        app.selected_request_sequence(),
        Some(CaptureSequence::new(1))
    );
    assert_eq!(app.detail_panel.scroll.offset, 4);

    let (ui, buffer) = render_to_buffer(&mut app);
    let content_area = detail_content_area(ui.right_panel.detail.area());
    let matching_row = find_buffer_text(&buffer, content_area, "second-2")
        .expect("matching header row should be rendered");
    assert_eq!(matching_row.y, content_area.y);
}

#[test]
fn request_switch_clamps_preserved_detail_offset_to_shorter_content() {
    let mut first = captured(0, "https://a.com/api/first");
    first.req_headers = (0..10)
        .map(|index| (format!("x-row-{index}"), format!("first-{index}")))
        .collect();
    let second = captured(1, "https://a.com/api/second");
    let mut app = App::new(ui_settings(true));
    app.add_request(first);
    app.add_request(second);
    render_to_buffer(&mut app);
    app.detail_panel.scroll.offset = 6;

    app.next();

    assert_eq!(app.detail_panel.scroll.offset, 6);
    render_to_buffer(&mut app);
    assert_eq!(app.detail_panel.scroll.max_offset, 0);
    assert_eq!(app.detail_panel.scroll.offset, 0);
}

#[test]
fn mouse_click_selects_detail_tab_on_panel_border() {
    let mut app = App::new(ui_settings(true));
    app.detail_panel.scroll.offset = 3;
    let mut ui = laid_out_ui(&app);
    let position = tab_click_position(ui.right_panel.detail.area(), MainDisplayTab::ResponseHeader);

    ui.handle_mouse(
        mouse(
            MouseEventKind::Down(MouseButton::Left),
            position.x,
            position.y,
        ),
        &mut app,
    );

    assert_eq!(app.detail_panel.active_tab, MainDisplayTab::ResponseHeader);
    assert_eq!(app.detail_panel.scroll.offset, 0);
    assert!(app.is_panel_focused(PanelFocus::Detail));

    app.detail_panel.scroll.offset = 2;
    let request_header =
        tab_click_position(ui.right_panel.detail.area(), MainDisplayTab::RequestHeader);
    ui.handle_mouse(
        mouse(
            MouseEventKind::Down(MouseButton::Left),
            request_header.x,
            request_header.y,
        ),
        &mut app,
    );
    assert_eq!(app.detail_panel.scroll.offset, 3);

    ui.handle_mouse(
        mouse(
            MouseEventKind::Down(MouseButton::Left),
            position.x,
            position.y,
        ),
        &mut app,
    );
    assert_eq!(app.detail_panel.scroll.offset, 2);
}

#[test]
fn mouse_click_selects_single_header_table_row_without_breaking_tabs() {
    let mut app = App::new(ui_settings(true));
    let mut req = captured(0, "https://a.com/api");
    req.req_headers = vec![
        ("accept".to_string(), "application/json".to_string()),
        ("x-token".to_string(), "secret".to_string()),
    ];
    app.add_request(req);

    let mut ui = laid_out_ui(&app);
    let content_area = detail_content_area(ui.right_panel.detail.area());
    let header_row = Position::new(content_area.x, content_area.y.saturating_add(2));

    ui.handle_mouse(
        mouse(
            MouseEventKind::Down(MouseButton::Left),
            header_row.x,
            header_row.y,
        ),
        &mut app,
    );

    assert_eq!(app.detail_panel.selected_header_row, Some(2));

    let (mut ui, buffer) = render_to_buffer(&mut app);
    let content_area = detail_content_area(ui.right_panel.detail.area());
    assert_eq!(
        buffer[(content_area.x, content_area.y.saturating_add(2))].bg,
        Color::White
    );

    let position = tab_click_position(ui.right_panel.detail.area(), MainDisplayTab::RequestBody);
    ui.handle_mouse(
        mouse(
            MouseEventKind::Down(MouseButton::Left),
            position.x,
            position.y,
        ),
        &mut app,
    );

    assert_eq!(app.detail_panel.active_tab, MainDisplayTab::RequestBody);
    assert_eq!(app.detail_panel.selected_header_row, None);
}

#[test]
fn entered_body_tab_renders_editor_content() {
    let mut app = App::new(ui_settings(true));
    let mut req = captured(0, "https://a.com/api");
    req.req_body = Some("alpha beta".to_string());
    app.add_request(req);
    app.focus_panel(PanelFocus::Detail);
    app.detail_panel.select_tab(MainDisplayTab::RequestBody);
    app.enter_current_body_viewer();

    let (ui, buffer) = render_to_buffer(&mut app);
    let detail_area = ui.right_panel.detail.area();
    let text_area = body_editor_text_area(detail_area);

    assert!(find_buffer_text(&buffer, text_area, "alpha beta").is_some());
    assert!(find_buffer_text(&buffer, detail_area, "Normal").is_some());
    assert!(app.detail_panel.body_viewer.editor_mut().is_some());
}

#[test]
fn body_editor_render_keeps_plain_body_tab_scroll_offset() {
    let mut app = App::new(ui_settings(true));
    let mut req = captured(0, "https://a.com/api");
    req.req_body = Some("alpha\nbeta\ngamma".to_string());
    app.add_request(req);
    app.focus_panel(PanelFocus::Detail);
    app.detail_panel.select_tab(MainDisplayTab::RequestBody);
    render_to_buffer(&mut app);
    app.detail_panel.scroll.offset = 4;
    assert!(app.enter_current_body_viewer());

    let (mut ui, _) = render_to_buffer(&mut app);
    let response_header =
        tab_click_position(ui.right_panel.detail.area(), MainDisplayTab::ResponseHeader);
    ui.handle_mouse(
        mouse(
            MouseEventKind::Down(MouseButton::Left),
            response_header.x,
            response_header.y,
        ),
        &mut app,
    );
    let request_body =
        tab_click_position(ui.right_panel.detail.area(), MainDisplayTab::RequestBody);
    ui.handle_mouse(
        mouse(
            MouseEventKind::Down(MouseButton::Left),
            request_body.x,
            request_body.y,
        ),
        &mut app,
    );

    assert_eq!(app.detail_panel.scroll.offset, 4);
}

#[test]
fn body_viewer_jump_overlay_labels_visible_match_and_jumps() {
    let mut app = App::new(ui_settings(true));
    let mut req = captured(0, "https://a.com/api");
    req.req_body = Some("alpha beta".to_string());
    app.add_request(req);
    app.focus_panel(PanelFocus::Detail);
    app.detail_panel.select_tab(MainDisplayTab::RequestBody);
    app.enter_current_body_viewer();

    app.handle_key_event(key(KeyCode::Char('s')));
    app.handle_key_event(key(KeyCode::Char('b')));
    app.handle_key_event(key(KeyCode::Char('e')));
    let (ui, buffer) = render_to_buffer(&mut app);
    let text_area = body_editor_text_area(ui.right_panel.detail.area());
    let label_position =
        find_buffer_text(&buffer, text_area, "aeta").expect("jump label should render");

    assert_eq!(buffer[label_position].bg, Color::Green);

    app.handle_key_event(key(KeyCode::Char('a')));
    let editor = app.detail_panel.body_viewer.editor_mut().unwrap();

    assert_eq!(editor.cursor.row, 0);
    assert_eq!(editor.cursor.col, 6);
}

#[test]
fn body_viewer_jump_shows_safe_labels_after_first_query_char() {
    let mut app = App::new(ui_settings(true));
    let mut req = captured(0, "https://a.com/api");
    req.req_body = Some("be be be be be be".to_string());
    app.add_request(req);
    app.focus_panel(PanelFocus::Detail);
    app.detail_panel.select_tab(MainDisplayTab::RequestBody);
    app.enter_current_body_viewer();

    app.handle_key_event(key(KeyCode::Char('s')));
    app.handle_key_event(key(KeyCode::Char('b')));
    let (ui, buffer) = render_to_buffer(&mut app);
    let text_area = body_editor_text_area(ui.right_panel.detail.area());
    let label_position =
        find_buffer_text(&buffer, text_area, "ae").expect("jump label should render");

    assert_eq!(buffer[label_position].bg, Color::Green);
}

#[test]
fn body_viewer_jump_skips_possible_refinement_chars_as_labels() {
    let mut app = App::new(ui_settings(true));
    let mut req = captured(0, "https://a.com/api");
    req.req_body = Some("be be be be be be be be be be be be".to_string());
    app.add_request(req);
    app.focus_panel(PanelFocus::Detail);
    app.detail_panel.select_tab(MainDisplayTab::RequestBody);
    app.enter_current_body_viewer();

    app.handle_key_event(key(KeyCode::Char('s')));
    app.handle_key_event(key(KeyCode::Char('b')));
    let (ui, buffer) = render_to_buffer(&mut app);
    let text_area = body_editor_text_area(ui.right_panel.detail.area());

    assert!(find_buffer_text(&buffer, text_area, "ee").is_none());
}

#[test]
fn body_viewer_jump_refines_query_when_possible_continuation_is_pressed() {
    let mut app = App::new(ui_settings(true));
    let mut req = captured(0, "https://a.com/api");
    req.req_body = Some("xx be yy".to_string());
    app.add_request(req);
    app.focus_panel(PanelFocus::Detail);
    app.detail_panel.select_tab(MainDisplayTab::RequestBody);
    app.enter_current_body_viewer();

    app.handle_key_event(key(KeyCode::Char('s')));
    app.handle_key_event(key(KeyCode::Char('b')));
    render_to_buffer(&mut app);

    app.handle_key_event(key(KeyCode::Char('e')));
    let editor = app.detail_panel.body_viewer.editor_mut().unwrap();
    assert_eq!(editor.cursor.col, 0);

    let (ui, buffer) = render_to_buffer(&mut app);
    let text_area = body_editor_text_area(ui.right_panel.detail.area());
    let label_position =
        find_buffer_text(&buffer, text_area, "ae").expect("refined jump label should render");

    assert_eq!(buffer[label_position].bg, Color::Green);

    app.handle_key_event(key(KeyCode::Char('a')));
    let editor = app.detail_panel.body_viewer.editor_mut().unwrap();

    assert_eq!(editor.cursor.row, 0);
    assert_eq!(editor.cursor.col, 3);
}

#[test]
fn body_viewer_jump_label_h_takes_priority_over_editor_motion() {
    let mut app = App::new(ui_settings(true));
    let mut req = captured(0, "https://a.com/api");
    req.req_body = Some("be be be be be be".to_string());
    app.add_request(req);
    app.focus_panel(PanelFocus::Detail);
    app.detail_panel.select_tab(MainDisplayTab::RequestBody);
    app.enter_current_body_viewer();

    app.handle_key_event(key(KeyCode::Char('s')));
    app.handle_key_event(key(KeyCode::Char('b')));
    let (ui, buffer) = render_to_buffer(&mut app);
    let text_area = body_editor_text_area(ui.right_panel.detail.area());
    let label_position =
        find_buffer_text(&buffer, text_area, "he").expect("h jump label should render");

    assert_eq!(buffer[label_position].bg, Color::Green);

    app.handle_key_event(key(KeyCode::Char('h')));
    let editor = app.detail_panel.body_viewer.editor_mut().unwrap();

    assert_eq!(editor.cursor.row, 0);
    assert_eq!(editor.cursor.col, 15);
}

#[test]
fn body_viewer_jump_query_can_refine_before_overlay_render() {
    let mut app = App::new(ui_settings(true));
    let mut req = captured(0, "https://a.com/api");
    req.req_body = Some("xx be yy".to_string());
    app.add_request(req);
    app.focus_panel(PanelFocus::Detail);
    app.detail_panel.select_tab(MainDisplayTab::RequestBody);
    app.enter_current_body_viewer();

    app.handle_key_event(key(KeyCode::Char('s')));
    app.handle_key_event(key(KeyCode::Char('b')));
    app.handle_key_event(key(KeyCode::Char('e')));
    let (ui, buffer) = render_to_buffer(&mut app);
    let text_area = body_editor_text_area(ui.right_panel.detail.area());
    let label_position =
        find_buffer_text(&buffer, text_area, "ae").expect("refined jump label should render");

    assert_eq!(buffer[label_position].bg, Color::Green);

    app.handle_key_event(key(KeyCode::Char('a')));
    let editor = app.detail_panel.body_viewer.editor_mut().unwrap();

    assert_eq!(editor.cursor.row, 0);
    assert_eq!(editor.cursor.col, 3);
}
