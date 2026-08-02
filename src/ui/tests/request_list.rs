use super::*;

#[test]
fn request_tree_orders_branch_nodes_before_leaf_requests() {
    let mut app = App::new(ui_settings(true));
    app.add_request(captured(0, "https://a.com/some/api2"));
    app.add_request(captured(1, "https://a.com/some/path/api1"));

    let items = build_request_tree_items(&mut app);
    let origin = &items[0];
    let some = &origin.children()[0];
    let child_identifiers = some
        .children()
        .iter()
        .map(|child| child.identifier().as_str())
        .collect::<Vec<_>>();

    assert_eq!(child_identifiers, ["segment:path", "request:0"]);
}

#[test]
fn request_tree_preserves_branch_incoming_order_before_leaves() {
    let mut app = App::new(ui_settings(true));
    app.add_request(captured(0, "https://a.com/some/api0"));
    app.add_request(captured(1, "https://a.com/some/b/api1"));
    app.add_request(captured(2, "https://a.com/some/a/api2"));

    let items = build_request_tree_items(&mut app);
    let some = &items[0].children()[0];
    let child_identifiers = some
        .children()
        .iter()
        .map(|child| child.identifier().as_str())
        .collect::<Vec<_>>();

    assert_eq!(child_identifiers, ["segment:b", "segment:a", "request:0"]);
}

#[test]
fn request_tree_displays_subtree_nodes_with_trailing_slashes() {
    let mut app = App::new(ui_settings(true));
    app.add_request(captured(0, "https://a.com/path/to/api"));
    let (ui, buffer) = render_to_buffer(&mut app);
    let request_area = ui.request_list.area();
    let rendered_rows = (request_area.y..request_area.bottom())
        .map(|row| buffer_row(&buffer, row, request_area.x, request_area.width))
        .collect::<Vec<_>>()
        .join("\n");

    assert!(rendered_rows.contains("https://a.com/"), "{rendered_rows}");
    assert!(rendered_rows.contains("path/"), "{rendered_rows}");
    assert!(rendered_rows.contains("to/"), "{rendered_rows}");
    assert!(rendered_rows.contains("api"), "{rendered_rows}");
    assert!(!rendered_rows.contains("api/"), "{rendered_rows}");
}

#[test]
fn request_tree_displays_subtree_leaf_counts_with_muted_style() {
    let mut app = App::new(ui_settings(true));
    app.add_request(captured(0, "https://a.com/path/one"));
    app.add_request(captured(1, "https://a.com/path/two"));
    app.add_request(captured(2, "https://a.com/other"));
    let (ui, buffer) = render_to_buffer(&mut app);
    let request_area = ui.request_list.area();

    assert!(find_buffer_text(&buffer, request_area, "https://a.com/ 3").is_some());
    let path_count = find_buffer_text(&buffer, request_area, "path/ 2")
        .expect("path subtree count should render");
    let count_column = path_count.x + text_width("path/ ");

    assert_eq!(buffer[(count_column, path_count.y)].fg, Color::DarkGray);
    assert!(find_buffer_text(&buffer, request_area, "other 1").is_none());
}

#[test]
fn request_tree_mouse_scroll_stops_at_last_full_viewport() {
    let mut app = App::new(ui_settings(true));
    for sequence in 0..12 {
        app.add_request(captured(sequence, &format!("https://a.com/item{sequence}")));
    }
    let (mut ui, _buffer) = render_to_buffer_with_size(&mut app, 100, 12);

    for _ in 0..20 {
        ui.handle_mouse(
            mouse_inside(MouseEventKind::ScrollDown, ui.request_list.area()),
            &mut app,
        );
    }

    assert_eq!(app.request_list.state.get_offset(), 6);
    let (ui, buffer) = render_to_buffer_with_size(&mut app, 100, 12);
    let viewport = ui.request_list.area().inner(Margin {
        vertical: 1,
        horizontal: 1,
    });
    let first_visible = find_buffer_text(&buffer, viewport, "item5")
        .expect("the final viewport should start at item5");
    let last_visible = find_buffer_text(&buffer, viewport, "item11")
        .expect("the final viewport should end at item11");

    assert_eq!(first_visible.y, viewport.y);
    assert_eq!(last_visible.y, viewport.bottom() - 1);
}

#[test]
fn request_tree_mouse_scroll_keeps_short_content_at_top() {
    let mut app = App::new(ui_settings(true));
    app.add_request(captured(0, "https://a.com/item0"));
    app.add_request(captured(1, "https://a.com/item1"));
    let (mut ui, _buffer) = render_to_buffer_with_size(&mut app, 100, 12);

    for _ in 0..5 {
        ui.handle_mouse(
            mouse_inside(MouseEventKind::ScrollDown, ui.request_list.area()),
            &mut app,
        );
    }

    assert_eq!(app.request_list.state.get_offset(), 0);
}

#[test]
fn request_tree_page_down_uses_viewport_scroll_limit() {
    let mut app = App::new(ui_settings(true));
    for sequence in 0..12 {
        app.add_request(captured(sequence, &format!("https://a.com/item{sequence}")));
    }
    let (_ui, _buffer) = render_to_buffer_with_size(&mut app, 100, 12);

    for _ in 0..20 {
        app.handle_key_event(key(KeyCode::PageDown));
    }

    assert_eq!(app.request_list.state.get_offset(), 6);
}

#[test]
fn request_tree_taller_viewport_reclamps_scroll_offset() {
    let mut app = App::new(ui_settings(true));
    for sequence in 0..12 {
        app.add_request(captured(sequence, &format!("https://a.com/item{sequence}")));
    }
    let (mut ui, _buffer) = render_to_buffer_with_size(&mut app, 100, 12);
    for _ in 0..20 {
        ui.handle_mouse(
            mouse_inside(MouseEventKind::ScrollDown, ui.request_list.area()),
            &mut app,
        );
    }
    assert!(app.request_list.state.get_offset() > 0);

    let (_ui, _buffer) = render_to_buffer_with_size(&mut app, 100, 20);

    assert_eq!(app.request_list.state.get_offset(), 0);
}

#[test]
fn request_tree_fold_reclamps_scroll_offset_to_visible_rows() {
    let mut app = App::new(ui_settings(true));
    for sequence in 0..12 {
        app.add_request(captured(sequence, &format!("https://a.com/item{sequence}")));
    }
    app.add_request(captured(12, "https://b.com/item12"));
    let (mut ui, _buffer) = render_to_buffer_with_size(&mut app, 100, 12);
    for _ in 0..20 {
        ui.handle_mouse(
            mouse_inside(MouseEventKind::ScrollDown, ui.request_list.area()),
            &mut app,
        );
    }
    assert!(app.request_list.state.get_offset() > 0);

    app.request_list
        .state
        .close(&["origin:https://a.com".to_string()]);
    let (_ui, _buffer) = render_to_buffer_with_size(&mut app, 100, 12);

    assert_eq!(app.request_list.state.get_offset(), 0);
}
