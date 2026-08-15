use super::*;
use crate::request_search::{RequestSearchDispatch, SearchJobOutcome};

fn type_search_query(app: &mut App, query: &str) {
    app.handle_key_event(key(KeyCode::Char('/')));
    for character in query.chars() {
        app.handle_key_event(key(KeyCode::Char(character)));
    }
}

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

#[test]
fn request_search_renders_floating_overlay_with_static_title() {
    let mut app = App::new(ui_settings(false));
    app.add_request(captured(0, "https://a.com/api/item"));
    app.handle_key_event(key(KeyCode::Char('/')));

    let (ui, buffer) = render_to_buffer(&mut app);
    let request_area = ui.request_list.area();
    let rendered = (request_area.y..request_area.bottom())
        .map(|row| buffer_row(&buffer, row, request_area.x, request_area.width))
        .collect::<Vec<_>>()
        .join("\n");

    assert!(rendered.contains("Search"), "{rendered}");
    assert!(rendered.contains("/ "), "{rendered}");
    assert!(rendered.contains("Requests • Searching"), "{rendered}");
}

#[test]
fn request_search_panel_title_covers_pending_ready_unselected_failed_and_limit_states() {
    let mut app = App::new(ui_settings(false));
    app.add_request(captured(0, "https://a.com/api/item"));
    type_search_query(&mut app, "item");

    let (_, pending) = render_to_buffer_with_size(&mut app, 180, 12);
    assert!(buffer_text(&pending).contains("Requests • Searching [item] • …"));

    app.complete_pending_request_search();
    app.handle_key_event(key(KeyCode::Enter));
    let (_, ready) = render_to_buffer_with_size(&mut app, 180, 12);
    assert!(buffer_text(&ready).contains("Requests • Searching [item] • 1/1"));

    app.request_list
        .state
        .select(vec!["origin:https://a.com".to_string()]);
    let (_, unselected) = render_to_buffer_with_size(&mut app, 180, 12);
    assert!(buffer_text(&unselected).contains("Requests • Searching [item] • –/1"));

    app.handle_key_event(key(KeyCode::Char('/')));
    app.handle_request_search_paste(&"x".repeat(513));
    let (_, limited) = render_to_buffer_with_size(&mut app, 180, 12);
    assert!(buffer_text(&limited).contains("Query limit reached"));
    app.handle_key_event(key(KeyCode::Esc));

    app.handle_key_event(key(KeyCode::Char('/')));
    app.handle_key_event(key(KeyCode::Char('x')));
    let Some(RequestSearchDispatch::Run(request)) = app.take_request_search_dispatch() else {
        panic!("query should dispatch");
    };
    app.apply_request_search_outcome(&SearchJobOutcome::Failed {
        key: request.key,
        message: "injected UI failure".into(),
    });
    let (_, failed) = render_to_buffer_with_size(&mut app, 180, 12);
    assert!(buffer_text(&failed).contains("Search failed"));
}

#[test]
fn request_search_title_truncates_query_before_result_status_at_narrow_width() {
    let mut app = App::new(ui_settings(false));
    app.add_request(captured(0, "https://a.com/api/item"));
    type_search_query(&mut app, "a-very-long-query-that-is-not-present");
    app.complete_pending_request_search();

    let (_, buffer) = render_to_buffer_with_size(&mut app, 140, 12);
    let text = buffer_text(&buffer);
    assert!(
        text.contains("Requests • Searching [a…] • No matches"),
        "{text}"
    );
    assert!(!text.contains("a-very-long-query"), "{text}");
}

#[test]
fn request_search_highlights_all_matched_cells_over_selected_background() {
    let mut app = App::new(ui_settings(false));
    app.add_request(captured(0, "https://a.com/api/anaXana"));
    type_search_query(&mut app, "ana");
    app.complete_pending_request_search();
    app.handle_key_event(key(KeyCode::Enter));

    let (ui, buffer) = render_to_buffer_with_size(&mut app, 140, 12);
    let viewport = ui.request_list.area().inner(Margin {
        vertical: 1,
        horizontal: 1,
    });
    let label =
        find_buffer_text(&buffer, viewport, "anaXana").expect("matching leaf should be visible");

    for column in [0, 1, 2, 4, 5, 6] {
        assert_eq!(buffer[(label.x + column, label.y)].bg, Color::Yellow);
    }
    assert_eq!(buffer[(label.x + 3, label.y)].bg, Color::White);
}

#[test]
fn request_search_does_not_repeat_last_row_highlights_into_blank_space() {
    let mut app = App::new(ui_settings(false));
    app.add_request(captured(0, "https://a.com/api/matched"));
    type_search_query(&mut app, "matched");
    app.complete_pending_request_search();
    app.handle_key_event(key(KeyCode::Enter));

    let (ui, buffer) = render_to_buffer_with_size(&mut app, 140, 20);
    let viewport = ui.request_list.area().inner(Margin {
        vertical: 1,
        horizontal: 1,
    });
    let label =
        find_buffer_text(&buffer, viewport, "matched").expect("matching leaf should be visible");

    assert_eq!(buffer[(label.x, label.y)].bg, Color::Yellow);
    for row in label.y.saturating_add(1)..viewport.bottom() {
        assert_ne!(buffer[(label.x, row)].bg, Color::Yellow);
    }
}

#[test]
fn request_search_uses_borderless_fallback_in_tiny_request_panel() {
    let mut app = App::new(ui_settings(false));
    app.handle_key_event(key(KeyCode::Char('/')));

    let (ui, buffer) = render_to_buffer_with_size(&mut app, 40, 6);
    let request_area = ui.request_list.area();
    let rendered = (request_area.y..request_area.bottom())
        .map(|row| buffer_row(&buffer, row, request_area.x, request_area.width))
        .collect::<Vec<_>>()
        .join("\n");

    assert!(rendered.contains("/ "), "{rendered}");
    assert!(!rendered.contains("Search"), "{rendered}");
}

#[test]
fn request_search_mouse_click_positions_cursor_and_outside_click_is_modal() {
    let mut app = App::new(ui_settings(false));
    type_search_query(&mut app, "abc");
    let (mut ui, buffer) = render_to_buffer_with_size(&mut app, 140, 12);
    let request_area = ui.request_list.area();
    let input =
        find_buffer_text(&buffer, request_area, "/ abc").expect("search input should render");

    ui.handle_mouse(
        mouse(
            MouseEventKind::Down(MouseButton::Left),
            input.x + 3,
            input.y,
        ),
        &mut app,
    );
    app.handle_key_event(key(KeyCode::Char('X')));
    assert_eq!(app.request_search_query(), Some("aXbc"));

    let detail_area = ui.detail.area();
    ui.handle_mouse(mouse_down_inside(detail_area), &mut app);
    assert!(app.is_panel_focused(PanelFocus::RequestList));
    assert!(app.is_request_search_editing());
}

#[test]
fn request_search_refresh_title_keeps_last_results_and_adds_marker() {
    let mut app = App::new(ui_settings(false));
    app.add_request(captured(0, "https://a.com/api/item"));
    type_search_query(&mut app, "item");
    app.complete_pending_request_search();
    app.handle_key_event(key(KeyCode::Enter));
    let selected = app.request_list.state.selected().to_vec();

    app.add_request(captured(1, "https://a.com/api/other"));
    let (_, buffer) = render_to_buffer_with_size(&mut app, 180, 12);
    let text = buffer_text(&buffer);

    assert!(
        text.contains("Requests • Searching [item] • 1/1 • ↻"),
        "{text}"
    );
    assert_eq!(app.request_list.state.selected(), selected);
    assert!(app.request_search_results().is_some());
}

fn buffer_text(buffer: &Buffer) -> String {
    (buffer.area.y..buffer.area.bottom())
        .map(|row| buffer_row(buffer, row, buffer.area.x, buffer.area.width))
        .collect::<Vec<_>>()
        .join("\n")
}
