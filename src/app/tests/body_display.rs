use super::*;

#[test]
fn rendering_body_tab_caches_formatted_body_text_by_key() {
    let mut app = App::new(ui_settings(true));
    let mut req = captured("https://some.host.com/api");
    req.req_body = Some(r#"{"request":true}"#.to_string());
    req.res_body = Some(r#"{"response":true}"#.to_string());
    app.add_request(req);
    app.focus_panel(PanelFocus::Detail);
    app.detail_panel.select_tab(MainDisplayTab::RequestBody);

    render_app(&mut app);

    let request_key = BodyViewerKey::new(CaptureSequence::new(0), MainDisplayTab::RequestBody);
    assert_eq!(
        app.cached_body_text(request_key),
        Some("{\n  \"request\": true\n}")
    );

    app.detail_panel.select_tab(MainDisplayTab::ResponseBody);
    assert!(app.cached_body_text(request_key).is_none());

    render_app(&mut app);

    let response_key = BodyViewerKey::new(CaptureSequence::new(0), MainDisplayTab::ResponseBody);
    assert_eq!(
        app.cached_body_text(response_key),
        Some("{\n  \"response\": true\n}")
    );
}

#[tokio::test]
async fn selected_streaming_body_progress_refreshes_without_per_chunk_render_events() {
    let (tx, mut rx) = tokio::sync::mpsc::channel(1);
    let publisher = CapturePublisher::new(tx, CapturePolicy::default());
    let headers = HeaderMap::new();
    let handle = publisher
        .try_start(RequestCaptureInput {
            method: Method::POST,
            original_uri: "https://some.host.com/api",
            effective_uri: "https://some.host.com/api",
            local_path: None,
            headers: &headers,
        })
        .expect("streaming capture should be admitted");
    let record = rx.recv().await.expect("capture should be published");
    let mut app = App::new(ui_settings(true));
    app.add_capture(record);
    app.detail_panel.select_tab(MainDisplayTab::RequestBody);
    let initial_key = app.current_body_viewer_key().expect("initial body key");
    assert_eq!(
        app.prepare_current_body_display(std::time::Instant::now()),
        BodyDisplayPreparation::ReadyToRender
    );
    assert_eq!(
        app.cached_body_text(initial_key),
        Some("(Body streaming… 0 bytes observed)")
    );

    handle.append(BodySide::Request, b"abc");

    assert!(app.refresh_selected_live_body());
    let updated_key = app.current_body_viewer_key().expect("updated body key");
    assert_ne!(updated_key, initial_key);
    assert!(app.cached_body_text(initial_key).is_none());
    assert_eq!(
        app.cached_body_text(updated_key),
        Some("(Body streaming… 3 bytes observed)")
    );
    assert!(!app.refresh_selected_live_body());
}

#[tokio::test]
async fn completed_body_waits_for_decode_without_caching_loading_as_content() {
    let shutdown = CancellationToken::new();
    let service = start_decode_service(DecodePolicy::default(), shutdown.clone());
    let mut app = App::new(ui_settings(true));
    let mut req = captured("https://some.host.com/api");
    req.req_body = Some("decoded body".to_string());
    app.add_request(req);
    app.detail_panel.select_tab(MainDisplayTab::RequestBody);
    app.set_decode_client(service.client.clone());
    let key = app.current_body_viewer_key().expect("body key");
    let started_at = std::time::Instant::now();

    assert_eq!(
        app.prepare_current_body_display(started_at),
        BodyDisplayPreparation::Pending { started_at }
    );
    assert!(matches!(
        app.body_render_text(key),
        Some(BodyRenderText::Loading)
    ));
    assert!(app.cached_body_text(key).is_none());
    assert!(!app.enter_current_body_viewer());
    assert!(!app.detail_panel.body_viewer.is_active());
    app.log_panel.visible = true;
    assert_eq!(
        app.prepare_current_body_display(started_at),
        BodyDisplayPreparation::ReadyToRender
    );
    app.log_panel.visible = false;
    assert_eq!(
        app.prepare_current_body_display(started_at),
        BodyDisplayPreparation::ReadyToRender
    );
    app.settings_popup.visible = true;
    assert_eq!(
        app.prepare_current_body_display(started_at),
        BodyDisplayPreparation::ReadyToRender
    );
    app.settings_popup.visible = false;

    assert!(app.apply_decode_result(DecodeResult {
        key: DecodeKey {
            sequence: CaptureSequence::new(0),
            side: BodySide::Request,
            revision: 0,
            mode: DecodeDisplayMode::Request,
        },
        text: "decoded body".to_string(),
        limited: false,
        error: None,
    }));
    assert_eq!(
        app.prepare_current_body_display(started_at + std::time::Duration::from_millis(1)),
        BodyDisplayPreparation::ReadyToRender
    );
    assert_eq!(app.cached_body_text(key), Some("decoded body"));
    assert!(app.enter_current_body_viewer());

    shutdown.cancel();
    service
        .task
        .await
        .expect("decode service should join")
        .expect("decode service should stop");
}

#[tokio::test]
async fn completed_zero_byte_body_bypasses_decoder_and_is_immediately_ready() {
    let shutdown = CancellationToken::new();
    let service = start_decode_service(
        DecodePolicy {
            max_queued_input_bytes: 0,
            ..DecodePolicy::default()
        },
        shutdown.clone(),
    );
    let mut app = App::new(ui_settings(true));
    app.add_request(captured("https://some.host.com/api"));
    app.detail_panel.select_tab(MainDisplayTab::RequestBody);
    app.set_decode_client(service.client.clone());
    let key = app.current_body_viewer_key().expect("body key");

    assert_eq!(
        app.prepare_current_body_display(std::time::Instant::now()),
        BodyDisplayPreparation::ReadyToRender
    );
    assert_eq!(app.cached_body_text(key), Some("(No body)"));
    assert_eq!(service.metrics.snapshot().rejected, 0);

    shutdown.cancel();
    service
        .task
        .await
        .expect("decode service should join")
        .expect("decode service should stop");
}

#[tokio::test]
async fn decoder_queue_rejection_is_unavailable_content_not_an_editor_body() {
    let shutdown = CancellationToken::new();
    let service = start_decode_service(
        DecodePolicy {
            max_queued_input_bytes: 0,
            ..DecodePolicy::default()
        },
        shutdown.clone(),
    );
    let mut app = App::new(ui_settings(true));
    let mut req = captured("https://some.host.com/api");
    req.req_body = Some("body".to_string());
    app.add_request(req);
    app.detail_panel.select_tab(MainDisplayTab::RequestBody);
    app.set_decode_client(service.client.clone());
    let key = app.current_body_viewer_key().expect("body key");

    assert_eq!(
        app.prepare_current_body_display(std::time::Instant::now()),
        BodyDisplayPreparation::ReadyToRender
    );
    assert!(matches!(
        app.body_render_text(key),
        Some(BodyRenderText::Text(
            "(Body display unavailable: decoder queue is full)"
        ))
    ));
    assert!(app.cached_body_text(key).is_none());
    assert!(!app.enter_current_body_viewer());
    assert_eq!(service.metrics.snapshot().rejected, 1);

    shutdown.cancel();
    service
        .task
        .await
        .expect("decode service should join")
        .expect("decode service should stop");
}

#[tokio::test]
async fn decode_result_for_previous_selection_cannot_replace_current_body() {
    let shutdown = CancellationToken::new();
    let service = start_decode_service(DecodePolicy::default(), shutdown.clone());
    let mut first = captured_with_sequence(0, "https://some.host.com/api/first");
    first.req_body = Some("first".to_string());
    let mut second = captured_with_sequence(1, "https://some.host.com/api/second");
    second.req_body = Some("second".to_string());
    let second_path = tree_path(&["origin:https://some.host.com", "segment:api", "request:1"]);
    let mut app = App::new(ui_settings(true));
    app.add_request(first);
    app.add_request(second);
    app.detail_panel.select_tab(MainDisplayTab::RequestBody);
    app.set_decode_client(service.client.clone());
    assert!(matches!(
        app.prepare_current_body_display(std::time::Instant::now()),
        BodyDisplayPreparation::Pending { .. }
    ));

    app.request_list.state.select(second_path);
    app.apply_request_list_change(true);

    assert!(!app.apply_decode_result(DecodeResult {
        key: DecodeKey {
            sequence: CaptureSequence::new(0),
            side: BodySide::Request,
            revision: 0,
            mode: DecodeDisplayMode::Request,
        },
        text: "stale first".to_string(),
        limited: false,
        error: None,
    }));
    let current_key = app.current_body_viewer_key().expect("current body key");
    assert!(app.cached_body_text(current_key).is_none());
    assert!(matches!(
        app.prepare_current_body_display(std::time::Instant::now()),
        BodyDisplayPreparation::Pending { .. }
    ));

    shutdown.cancel();
    service
        .task
        .await
        .expect("decode service should join")
        .expect("decode service should stop");
}

#[test]
fn entering_body_viewer_consumes_cached_body_text() {
    let mut app = App::new(ui_settings(true));
    let mut req = captured("https://some.host.com/api");
    req.req_body = Some(r#"{"request":true}"#.to_string());
    app.add_request(req);
    app.focus_panel(PanelFocus::Detail);
    app.detail_panel.select_tab(MainDisplayTab::RequestBody);
    render_app(&mut app);

    let request_key = BodyViewerKey::new(CaptureSequence::new(0), MainDisplayTab::RequestBody);
    assert!(app.cached_body_text(request_key).is_some());

    app.handle_key_event(key(KeyCode::Enter));

    assert!(app.detail_panel.body_viewer.is_active());
    assert!(app.cached_body_text(request_key).is_none());
}

#[test]
fn detail_enter_does_not_activate_body_viewer_without_selected_request() {
    let mut app = App::new(ui_settings(true));
    app.focus_panel(PanelFocus::Detail);
    app.detail_panel.select_tab(MainDisplayTab::RequestBody);

    app.handle_key_event(key(KeyCode::Enter));

    assert!(!app.detail_panel.body_viewer.is_active());
}

#[test]
fn panel_keys_apply_only_to_focused_panel() {
    let mut app = App::new(ui_settings(true));

    app.add_request(captured_with_sequence(0, "https://some.host.com/api/a"));
    app.add_request(captured_with_sequence(1, "https://some.host.com/api/b"));
    let selected_before = app.request_list.state.selected().to_vec();
    app.detail_panel.scroll.max_offset = 5;
    app.focus_panel(PanelFocus::Detail);

    app.handle_key_event(key(KeyCode::Char('j')));

    assert_eq!(app.request_list.state.selected(), selected_before);
    assert_eq!(app.detail_panel.scroll.offset, 1);

    app.focus_panel(PanelFocus::RequestList);
    app.handle_key_event(key(KeyCode::Char('j')));

    assert_ne!(app.request_list.state.selected(), selected_before);
}

#[test]
fn request_switch_preserves_detail_scroll_and_clears_request_scoped_state() {
    let mut app = App::new(ui_settings(true));

    let mut first = captured_with_sequence(0, "https://some.host.com/api/a");
    first.req_body = Some("first".to_string());
    let mut second = captured_with_sequence(1, "https://some.host.com/api/b");
    second.req_body = Some("second".to_string());
    app.add_request(first);
    app.add_request(second);
    app.detail_panel.select_tab(MainDisplayTab::RequestBody);
    render_app(&mut app);
    let first_key = app.current_body_viewer_key().expect("first body key");
    assert!(app.cached_body_text(first_key).is_some());
    app.detail_panel.scroll.offset = 3;
    app.detail_panel.selected_header_row = Some(2);

    app.next();

    assert_eq!(
        app.selected_request_sequence(),
        Some(CaptureSequence::new(1))
    );
    assert_eq!(app.detail_panel.scroll.offset, 3);
    assert_eq!(app.detail_panel.selected_header_row, None);
    assert!(app.cached_body_text(first_key).is_none());

    render_app(&mut app);
    assert!(app.enter_current_body_viewer());
    assert!(app.detail_panel.body_viewer.is_active());
    app.detail_panel.scroll.offset = 2;

    app.previous();

    assert_eq!(
        app.selected_request_sequence(),
        Some(CaptureSequence::new(0))
    );
    assert_eq!(app.detail_panel.scroll.offset, 2);
    assert!(!app.detail_panel.body_viewer.is_active());
}
