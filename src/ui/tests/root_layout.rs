use super::*;

#[test]
fn mouse_movement_and_scroll_do_not_move_panel_focus() {
    let mut app = App::new(ui_settings(true));
    let ui = laid_out_ui(&app);

    ui.handle_mouse(
        mouse_inside(MouseEventKind::Moved, ui.right_panel.detail.area()),
        &mut app,
    );
    assert!(app.is_panel_focused(PanelFocus::RequestList));

    app.log_panel.visible = true;
    app.focus_panel(PanelFocus::Log);
    let ui = laid_out_ui(&app);
    ui.handle_mouse(
        mouse_inside(MouseEventKind::ScrollDown, ui.log.area()),
        &mut app,
    );
    assert!(app.is_panel_focused(PanelFocus::Log));
}

#[test]
fn mouse_click_focuses_clicked_panel() {
    let mut app = App::new(ui_settings(true));
    let ui = laid_out_ui(&app);

    ui.handle_mouse(mouse_down_inside(ui.right_panel.detail.area()), &mut app);
    assert!(app.is_panel_focused(PanelFocus::Detail));

    ui.handle_mouse(mouse_down_inside(ui.request_list.area()), &mut app);
    assert!(app.is_panel_focused(PanelFocus::RequestList));

    app.log_panel.visible = true;
    let ui = laid_out_ui(&app);
    ui.handle_mouse(mouse_down_inside(ui.log.area()), &mut app);
    assert!(app.is_panel_focused(PanelFocus::Log));
}

#[test]
fn status_panel_does_not_take_focus() {
    let mut app = App::new(ui_settings(true));
    let ui = laid_out_ui(&app);

    ui.handle_mouse(mouse_down_inside(ui.status.area()), &mut app);

    assert!(app.is_panel_focused(PanelFocus::RequestList));
}

#[test]
fn hidden_log_panel_leaves_detail_on_right_panel() {
    let app = App::new(ui_settings(true));
    let ui = laid_out_ui(&app);

    assert_eq!(ui.right_panel.detail.area(), ui.right_panel.area());
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
    assert_eq!(ui.right_panel.area(), Rect::default());
    assert_eq!(ui.right_panel.detail.area(), Rect::default());
}
