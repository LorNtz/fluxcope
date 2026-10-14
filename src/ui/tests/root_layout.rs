use super::*;

#[test]
fn mouse_movement_and_scroll_do_not_move_panel_focus() {
    let mut app = App::new(ui_settings(true));
    let mut ui = laid_out_ui(&app);

    ui.handle_mouse(
        mouse_inside(MouseEventKind::Moved, ui.detail.area()),
        &mut app,
    );
    assert!(app.is_panel_focused(PanelFocus::RequestList));

    app.log_panel.visible = true;
    app.focus_panel(PanelFocus::Log);
    let mut ui = laid_out_ui(&app);
    ui.handle_mouse(
        mouse_inside(MouseEventKind::ScrollDown, ui.log.area()),
        &mut app,
    );
    assert!(app.is_panel_focused(PanelFocus::Log));
}

#[test]
fn mouse_click_focuses_clicked_panel() {
    let mut app = App::new(ui_settings(true));
    let mut ui = laid_out_ui(&app);

    ui.handle_mouse(mouse_down_inside(ui.detail.area()), &mut app);
    assert!(app.is_panel_focused(PanelFocus::Detail));

    ui.handle_mouse(mouse_down_inside(ui.request_list.area()), &mut app);
    assert!(app.is_panel_focused(PanelFocus::RequestList));

    app.log_panel.visible = true;
    let mut ui = laid_out_ui(&app);
    ui.handle_mouse(mouse_down_inside(ui.log.area()), &mut app);
    assert!(app.is_panel_focused(PanelFocus::Log));
}

#[test]
fn status_panel_does_not_take_focus() {
    let mut app = App::new(ui_settings(true));
    let mut ui = laid_out_ui(&app);

    ui.handle_mouse(mouse_down_inside(ui.status.area()), &mut app);

    assert!(app.is_panel_focused(PanelFocus::RequestList));
}

#[test]
fn hidden_log_panel_places_detail_on_workspace_right() {
    let app = App::new(ui_settings(true));
    let ui = laid_out_ui(&app);

    assert_eq!(ui.detail.area(), Rect::new(30, 3, 70, 9));
    assert_eq!(ui.log.area(), Rect::default());
}

#[test]
fn visible_log_panel_uses_workspace_below_status() {
    let mut app = App::new(ui_settings(true));
    app.log_panel.visible = true;
    let ui = laid_out_ui(&app);

    assert_eq!(ui.status.area(), Rect::new(0, 0, 100, 3));
    assert_eq!(ui.log.area(), Rect::new(0, 3, 100, 9));
    assert_eq!(ui.request_list.area(), Rect::default());
    assert_eq!(ui.detail.area(), Rect::default());
}

#[test]
fn status_settings_button_hit_region_matches_visible_cells() {
    for width in [100, 24, 12, 11] {
        let mut app = App::new(ui_settings(true));
        let (mut ui, buffer) = render_to_buffer_with_size(&mut app, width, 28);
        let button = find_buffer_text(&buffer, ui.status.area(), "[Settings]");
        assert_eq!(button, (width >= 12).then(|| Position::new(width - 11, 1)));
        for y in 0..3 {
            for x in 0..width {
                ui.handle_mouse(
                    mouse(MouseEventKind::Down(MouseButton::Left), x, y),
                    &mut app,
                );
                let inside = width >= 12 && y == 1 && (width - 11..width - 1).contains(&x);
                assert_eq!(
                    app.settings_popup.visible, inside,
                    "at {x},{y}, width {width}"
                );
                if inside {
                    assert!(app.is_popup_focused(PopupFocus::Settings));
                    assert!(app.take_settings_save_request().is_none());
                    app.close_settings_popup();
                }
                assert!(app.is_panel_focused(PanelFocus::RequestList));
            }
        }
    }
}

#[test]
fn status_settings_button_ignores_nonleft_events() {
    let mut app = App::new(ui_settings(true));
    let (mut ui, buffer) = render_to_buffer_with_size(&mut app, 100, 28);
    let button = find_buffer_text(&buffer, ui.status.area(), "[Settings]").unwrap();
    for kind in [
        MouseEventKind::Down(MouseButton::Right),
        MouseEventKind::Down(MouseButton::Middle),
        MouseEventKind::Up(MouseButton::Left),
        MouseEventKind::Drag(MouseButton::Left),
        MouseEventKind::Moved,
        MouseEventKind::ScrollDown,
        MouseEventKind::ScrollUp,
    ] {
        ui.handle_mouse(mouse(kind, button.x, button.y), &mut app);
        assert!(!app.settings_popup.visible);
        assert!(app.is_panel_focused(PanelFocus::RequestList));
    }
}

#[test]
fn status_settings_button_respects_popup_priority() {
    let mut app = App::new(ui_settings(true));
    app.open_settings_popup();
    app.settings_popup.draft_mut_for_tests().server.port = 9014;
    let (mut ui, buffer) = render_to_buffer_with_size(&mut app, 100, 28);
    let button = find_buffer_text(&buffer, ui.status.area(), "[Settings]").unwrap();
    ui.handle_mouse(
        mouse(MouseEventKind::Down(MouseButton::Left), button.x, button.y),
        &mut app,
    );
    assert!(app.settings_popup.is_confirming_unsaved());
    assert_eq!(app.settings_popup.draft().server.port, 9014);
    assert!(app.take_settings_save_request().is_none());

    app.close_settings_popup();
    app.handle_key_event(key(KeyCode::Char('c')));
    let (mut ui, buffer) = render_to_buffer_with_size(&mut app, 100, 28);
    let button = find_buffer_text(&buffer, ui.status.area(), "[Settings]").unwrap();
    ui.handle_mouse(
        mouse(MouseEventKind::Down(MouseButton::Left), button.x, button.y),
        &mut app,
    );
    assert!(app.certificate_popup.visible);
    assert!(!app.settings_popup.visible);
    assert!(app.is_popup_focused(PopupFocus::Certificate));
}

#[test]
fn status_settings_button_opens_over_search_and_log_without_losing_state() {
    for log_visible in [false, true] {
        let mut app = App::new(ui_settings(true));
        if log_visible {
            app.log_panel.visible = true;
            app.focus_panel(PanelFocus::Log);
            for index in 0..40 {
                app.append_log(crate::logging::LogRecord::system(format!(
                    "log entry {index}"
                )));
            }
            app.log_panel.scroll.offset = 3;
        } else {
            app.begin_request_search();
            app.handle_paste("keep search");
            app.handle_key_event(key(KeyCode::Left));
        }
        let (mut ui, buffer) = render_to_buffer_with_size(&mut app, 100, 28);
        let button = find_buffer_text(&buffer, ui.status.area(), "[Settings]").unwrap();
        ui.handle_mouse(
            mouse(MouseEventKind::Down(MouseButton::Left), button.x, button.y),
            &mut app,
        );
        assert!(app.is_popup_focused(PopupFocus::Settings));
        app.close_settings_popup();
        if log_visible {
            assert!(app.log_panel.visible);
            assert!(app.is_panel_focused(PanelFocus::Log));
            assert_eq!(app.log_panel.scroll.offset, 3);
        } else {
            assert!(app.is_request_search_editing());
            assert!(app.is_panel_focused(PanelFocus::RequestList));
            assert_eq!(app.request_search_query(), Some("keep search"));
            assert_eq!(app.request_search_input().unwrap().cursor(), 10);
        }
    }
}

#[test]
fn status_settings_button_follows_resize_without_old_hit_region() {
    let mut app = App::new(ui_settings(true));
    let (mut ui, _) = render_to_buffer_with_size(&mut app, 100, 28);
    for width in [36, 11, 100] {
        let mut terminal = Terminal::new(TestBackend::new(width, 28)).unwrap();
        terminal.draw(|frame| ui.render(frame, &mut app)).unwrap();
        ui.handle_mouse(
            mouse(MouseEventKind::Down(MouseButton::Left), 89, 1),
            &mut app,
        );
        assert_eq!(app.settings_popup.visible, width == 100);
        if app.settings_popup.visible {
            app.close_settings_popup();
        }
        if width == 36 {
            ui.handle_mouse(
                mouse(MouseEventKind::Down(MouseButton::Left), 25, 1),
                &mut app,
            );
            assert!(app.settings_popup.visible);
            app.close_settings_popup();
        }
    }
}

#[test]
fn status_settings_button_preserves_active_body_editor() {
    let mut app = App::new(ui_settings(true));
    let mut request = captured(0, "https://a.com/api");
    request.req_body = Some("alpha beta".to_string());
    app.add_request(request);
    app.focus_panel(PanelFocus::Detail);
    app.detail_panel.select_tab(MainDisplayTab::RequestBody);
    assert!(app.enter_current_body_viewer());
    app.handle_key_event(key(KeyCode::Char('w')));
    let (mut ui, buffer) = render_to_buffer_with_size(&mut app, 100, 28);
    let button = find_buffer_text(&buffer, ui.status.area(), "[Settings]").unwrap();
    ui.handle_mouse(
        mouse(MouseEventKind::Down(MouseButton::Left), button.x, button.y),
        &mut app,
    );
    assert!(app.is_popup_focused(PopupFocus::Settings));
    let mut terminal = Terminal::new(TestBackend::new(100, 28)).unwrap();
    terminal.draw(|frame| ui.render(frame, &mut app)).unwrap();
    let area = settings_popup_area(terminal.backend().buffer().area);
    let close = find_buffer_text(
        terminal.backend().buffer(),
        Rect::new(area.x, area.y, area.width, 1),
        "[x]",
    )
    .unwrap();
    ui.handle_mouse(
        mouse(MouseEventKind::Down(MouseButton::Left), close.x, close.y),
        &mut app,
    );
    assert!(!app.settings_popup.visible);
    assert!(app.is_panel_focused(PanelFocus::Detail));
    assert!(app.detail_panel.body_viewer.is_active());
    let editor = app.detail_panel.body_viewer.editor_mut().unwrap();
    assert_eq!(editor.cursor, edtui::Index2::new(0, 6));
    assert_eq!(editor.lines, edtui::Lines::from("alpha beta"));
}
