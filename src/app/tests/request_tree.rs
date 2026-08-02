use super::*;

#[test]
fn requests_are_ordered_by_capture_sequence() {
    let mut app = App::new(ui_settings(true));

    app.add_request(captured_with_sequence(1, "https://a.com/b"));
    app.add_request(captured_with_sequence(0, "https://a.com/a"));

    let uris = app
        .captures()
        .map(|req| req.request.original_uri.clone())
        .collect::<Vec<_>>();

    assert_eq!(uris, ["https://a.com/a", "https://a.com/b"]);

    app.request_list.state.select(vec![
        "origin:https://a.com".to_string(),
        "request:1".to_string(),
    ]);

    assert_eq!(
        app.selected_request()
            .map(|req| req.request.original_uri.clone()),
        Some("https://a.com/b".to_string())
    );
}

#[test]
fn request_list_branches_stay_folded_by_default() {
    let mut app = App::new(ui_settings(false));

    app.add_request(captured("https://some.host.com/api/v1/getUserInfo?a=1"));

    assert!(app.request_list.state.opened().is_empty());
    assert_eq!(app.request_list.state.selected().len(), 1);
    assert_eq!(
        app.request_list.state.selected()[0],
        "origin:https://some.host.com"
    );
    assert!(app.selected_request().is_none());
}

#[test]
fn request_list_auto_expand_opens_new_request_branches() {
    let mut app = App::new(ui_settings(true));

    app.add_request(captured("https://some.host.com/api/v1/getUserInfo?a=1"));

    assert!(
        app.request_list
            .state
            .opened()
            .contains(&vec!["origin:https://some.host.com".to_string()])
    );
    assert!(app.request_list.state.opened().contains(&vec![
        "origin:https://some.host.com".to_string(),
        "segment:api".to_string()
    ]));
    assert!(app.request_list.state.opened().contains(&vec![
        "origin:https://some.host.com".to_string(),
        "segment:api".to_string(),
        "segment:v1".to_string()
    ]));
    assert_eq!(
        app.selected_request()
            .map(|req| req.request.original_uri.clone()),
        Some("https://some.host.com/api/v1/getUserInfo?a=1".to_string())
    );
}

#[test]
fn request_list_preserves_user_opened_branches_when_auto_expand_is_disabled() {
    let mut app = App::new(ui_settings(false));
    let origin = "origin:https://some.host.com".to_string();
    let api = "segment:api".to_string();
    let v1 = "segment:v1".to_string();

    app.add_request(captured("https://some.host.com/api/first"));
    app.request_list.state.open(vec![origin.clone()]);
    app.request_list
        .state
        .open(vec![origin.clone(), api.clone()]);
    app.add_request(captured("https://some.host.com/api/v1/getUserInfo?a=1"));

    assert!(
        app.request_list
            .state
            .opened()
            .contains(&vec![origin.clone(), api.clone()])
    );
    assert!(
        !app.request_list
            .state
            .opened()
            .contains(&vec![origin, api, v1])
    );
}

#[test]
fn request_list_l_and_enter_toggle_selected_subtree() {
    let mut app = App::new(ui_settings(false));
    let origin = tree_path(&["origin:https://some.host.com"]);

    app.add_request(captured("https://some.host.com/api/v1/getUserInfo"));

    app.handle_key_event(key(KeyCode::Char('l')));
    assert!(app.request_list.state.opened().contains(&origin));

    app.handle_key_event(key(KeyCode::Char('l')));
    assert!(!app.request_list.state.opened().contains(&origin));

    app.handle_key_event(key(KeyCode::Char('l')));
    assert!(app.request_list.state.opened().contains(&origin));

    app.handle_key_event(key(KeyCode::Enter));
    assert!(!app.request_list.state.opened().contains(&origin));
    assert!(app.is_panel_focused(PanelFocus::RequestList));

    app.handle_key_event(key(KeyCode::Enter));
    assert!(app.request_list.state.opened().contains(&origin));
    assert!(app.is_panel_focused(PanelFocus::RequestList));
}

#[test]
fn request_list_enter_release_does_not_toggle_subtree_twice() {
    let mut app = App::new(ui_settings(false));
    let origin = tree_path(&["origin:https://some.host.com"]);

    app.add_request(captured("https://some.host.com/api/v1/getUserInfo"));

    app.handle_key_event(key_with_kind(KeyCode::Enter, KeyEventKind::Press));
    app.handle_key_event(key_with_kind(KeyCode::Enter, KeyEventKind::Release));

    assert!(app.request_list.state.opened().contains(&origin));
}

#[test]
fn request_list_enter_on_selected_request_focuses_detail_panel() {
    let mut app = App::new(ui_settings(true));

    app.add_request(captured("https://some.host.com/api/v1/getUserInfo"));
    assert!(app.selected_request().is_some());

    app.handle_key_event(key(KeyCode::Enter));

    assert!(app.is_panel_focused(PanelFocus::Detail));
    assert_eq!(
        app.selected_request()
            .map(|req| req.request.original_uri.clone()),
        Some("https://some.host.com/api/v1/getUserInfo".to_string())
    );
}

#[test]
fn request_list_e_expands_selected_subtree_only() {
    let mut app = App::new(ui_settings(false));
    let origin = "origin:https://some.host.com".to_string();
    let api = "segment:api".to_string();

    app.add_request(captured_with_sequence(0, "https://some.host.com/api/v1/a"));
    app.add_request(captured_with_sequence(1, "https://some.host.com/api/v2/b"));
    app.add_request(captured_with_sequence(2, "https://other.host.com/api/v3/c"));
    app.request_list.state.open(vec![origin.clone()]);
    app.request_list
        .state
        .select(vec![origin.clone(), api.clone()]);

    app.handle_key_event(key(KeyCode::Char('e')));

    assert!(
        app.request_list
            .state
            .opened()
            .contains(&vec![origin.clone(), api.clone()])
    );
    assert!(app.request_list.state.opened().contains(&vec![
        origin.clone(),
        api.clone(),
        "segment:v1".to_string()
    ]));
    assert!(app.request_list.state.opened().contains(&vec![
        origin.clone(),
        api,
        "segment:v2".to_string()
    ]));
    assert!(
        !app.request_list
            .state
            .opened()
            .contains(&vec!["origin:https://other.host.com".to_string()])
    );
}

#[test]
fn request_list_e_ignores_selected_request_leaf() {
    let mut app = App::new(ui_settings(false));
    let leaf = tree_path(&["origin:https://some.host.com", "segment:api", "request:0"]);

    app.add_request(captured_with_sequence(0, "https://some.host.com/api/a"));
    app.request_list.state.select(leaf);

    app.handle_key_event(key(KeyCode::Char('e')));

    assert!(app.request_list.state.opened().is_empty());
}

#[test]
fn request_list_uppercase_e_expands_all_subtrees() {
    let mut app = App::new(ui_settings(false));

    app.add_request(captured_with_sequence(0, "https://a.com/api/v1/a"));
    app.add_request(captured_with_sequence(1, "https://b.com/api/v2/b"));

    app.handle_key_event(key(KeyCode::Char('E')));

    assert!(
        app.request_list
            .state
            .opened()
            .contains(&tree_path(&["origin:https://a.com"]))
    );
    assert!(
        app.request_list
            .state
            .opened()
            .contains(&tree_path(&["origin:https://a.com", "segment:api"]))
    );
    assert!(
        app.request_list
            .state
            .opened()
            .contains(&tree_path(&["origin:https://b.com"]))
    );
    assert!(
        app.request_list
            .state
            .opened()
            .contains(&tree_path(&["origin:https://b.com", "segment:api"]))
    );
}

#[test]
fn request_list_uppercase_w_collapses_all_and_keeps_selection_visible() {
    let mut app = App::new(ui_settings(true));

    app.add_request(captured_with_sequence(0, "https://a.com/api/v1/a"));
    app.request_list.state.select(tree_path(&[
        "origin:https://a.com",
        "segment:api",
        "request:0",
    ]));

    app.handle_key_event(key(KeyCode::Char('W')));

    assert!(app.request_list.state.opened().is_empty());
    assert_eq!(
        app.request_list.state.selected(),
        tree_path(&["origin:https://a.com"])
    );
}

#[test]
fn request_list_w_collapses_selected_subtree_children_only() {
    let mut app = App::new(ui_settings(false));
    let selected = tree_path(&["origin:https://a.com", "segment:api"]);
    let selected_child = tree_path(&["origin:https://a.com", "segment:api", "segment:v1"]);
    let sibling = tree_path(&["origin:https://a.com", "segment:other"]);
    let other_root = tree_path(&["origin:https://b.com"]);

    app.add_request(captured_with_sequence(0, "https://a.com/api/v1/a"));
    app.add_request(captured_with_sequence(1, "https://a.com/other/v2/b"));
    app.add_request(captured_with_sequence(2, "https://b.com/api/v3/c"));
    app.handle_key_event(key(KeyCode::Char('E')));
    app.request_list.state.select(selected.clone());

    app.handle_key_event(key(KeyCode::Char('w')));

    assert!(app.request_list.state.opened().contains(&selected));
    assert!(!app.request_list.state.opened().contains(&selected_child));
    assert!(app.request_list.state.opened().contains(&sibling));
    assert!(app.request_list.state.opened().contains(&other_root));
}

#[test]
fn request_list_w_ignores_selected_request_leaf() {
    let mut app = App::new(ui_settings(false));
    let origin = tree_path(&["origin:https://some.host.com"]);
    let api = tree_path(&["origin:https://some.host.com", "segment:api"]);
    let leaf = tree_path(&["origin:https://some.host.com", "segment:api", "request:0"]);

    app.add_request(captured_with_sequence(0, "https://some.host.com/api/a"));
    app.request_list.state.open(origin.clone());
    app.request_list.state.open(api.clone());
    app.request_list.state.select(leaf);

    app.handle_key_event(key(KeyCode::Char('w')));

    assert!(app.request_list.state.opened().contains(&origin));
    assert!(app.request_list.state.opened().contains(&api));
}

#[test]
fn delete_selected_requests_removes_leaf_request() {
    let mut app = App::new(ui_settings(true));

    app.add_request(captured_with_sequence(0, "https://some.host.com/api/a"));
    app.add_request(captured_with_sequence(1, "https://some.host.com/api/b"));
    app.request_list.state.select(vec![
        "origin:https://some.host.com".to_string(),
        "segment:api".to_string(),
        "request:0".to_string(),
    ]);

    assert_eq!(app.delete_selected_requests(), 1);

    assert_eq!(app.capture_count(), 1);
    assert_eq!(
        app.capture_at(0)
            .map(|capture| capture.request.original_uri.clone()),
        Some("https://some.host.com/api/b".to_string())
    );
    assert_eq!(
        app.request_list.state.selected(),
        [
            "origin:https://some.host.com".to_string(),
            "segment:api".to_string(),
            "request:1".to_string()
        ]
    );
    assert_eq!(
        app.selected_request()
            .map(|req| req.request.original_uri.clone()),
        Some("https://some.host.com/api/b".to_string())
    );
}

#[test]
fn delete_selected_requests_removes_subtree_requests() {
    let mut app = App::new(ui_settings(true));
    let origin = "origin:https://some.host.com".to_string();
    let api = "segment:api".to_string();
    let v1 = "segment:v1".to_string();

    app.add_request(captured_with_sequence(0, "https://some.host.com/api/v1/a"));
    app.add_request(captured_with_sequence(1, "https://some.host.com/api/v1/b"));
    app.add_request(captured_with_sequence(2, "https://some.host.com/api/v2/c"));
    app.request_list
        .state
        .select(vec![origin.clone(), api.clone(), v1.clone()]);

    assert_eq!(app.delete_selected_requests(), 2);

    assert_eq!(app.capture_count(), 1);
    assert_eq!(
        app.capture_at(0)
            .map(|capture| capture.request.original_uri.clone()),
        Some("https://some.host.com/api/v2/c".to_string())
    );
    assert_eq!(
        app.request_list.state.selected(),
        [origin.clone(), api.clone(), "segment:v2".to_string()]
    );
    assert!(
        app.request_list
            .state
            .opened()
            .contains(&vec![origin.clone(), api.clone()])
    );
    assert!(
        !app.request_list
            .state
            .opened()
            .contains(&vec![origin, api, v1])
    );
}

#[test]
fn delete_selected_requests_selects_previous_leaf_sibling() {
    let mut app = App::new(ui_settings(true));

    app.add_request(captured_with_sequence(0, "https://some.host.com/api/a"));
    app.add_request(captured_with_sequence(1, "https://some.host.com/api/b"));
    app.add_request(captured_with_sequence(2, "https://some.host.com/api/c"));
    app.request_list.state.select(tree_path(&[
        "origin:https://some.host.com",
        "segment:api",
        "request:1",
    ]));

    assert_eq!(app.delete_selected_requests(), 1);

    assert_eq!(
        app.request_list.state.selected(),
        tree_path(&["origin:https://some.host.com", "segment:api", "request:0"])
    );
    assert_eq!(
        app.selected_request()
            .map(|req| req.request.original_uri.clone()),
        Some("https://some.host.com/api/a".to_string())
    );
}

#[test]
fn delete_selected_requests_selects_next_root_when_first_branch_disappears() {
    let mut app = App::new(ui_settings(true));

    app.add_request(captured_with_sequence(0, "https://a.com/some/path/api1"));
    app.add_request(captured_with_sequence(1, "https://a.com/some/path/api2"));
    app.add_request(captured_with_sequence(2, "https://b.com/api4"));
    app.request_list
        .state
        .select(tree_path(&["origin:https://a.com", "segment:some"]));

    assert_eq!(app.delete_selected_requests(), 2);

    assert_eq!(
        app.request_list.state.selected(),
        tree_path(&["origin:https://b.com"])
    );
    assert_eq!(app.capture_count(), 1);
    assert_eq!(
        app.capture_at(0)
            .map(|capture| capture.request.original_uri.clone()),
        Some("https://b.com/api4".to_string())
    );
}

#[test]
fn delete_selected_requests_selects_deepest_visible_node_in_previous_root() {
    let mut app = App::new(ui_settings(true));

    app.add_request(captured_with_sequence(0, "https://a.com/some/path/api1"));
    app.add_request(captured_with_sequence(1, "https://a.com/some/path/api2"));
    app.add_request(captured_with_sequence(2, "https://a.com/some/path/api3"));
    app.add_request(captured_with_sequence(3, "https://b.com/api4"));
    app.request_list
        .state
        .select(tree_path(&["origin:https://b.com", "request:3"]));

    assert_eq!(app.delete_selected_requests(), 1);

    assert_eq!(
        app.request_list.state.selected(),
        tree_path(&[
            "origin:https://a.com",
            "segment:some",
            "segment:path",
            "request:2"
        ])
    );
    assert_eq!(
        app.selected_request()
            .map(|req| req.request.original_uri.clone()),
        Some("https://a.com/some/path/api3".to_string())
    );
}

#[test]
fn delete_selected_requests_selects_folded_previous_branch_node() {
    let mut app = App::new(ui_settings(true));
    let some_path = tree_path(&["origin:https://a.com", "segment:some"]);

    app.add_request(captured_with_sequence(0, "https://a.com/some/path/api1"));
    app.add_request(captured_with_sequence(1, "https://a.com/some/path/api2"));
    app.add_request(captured_with_sequence(2, "https://a.com/some/path/api3"));
    app.add_request(captured_with_sequence(3, "https://b.com/api4"));
    app.request_list.state.close(&some_path);
    app.request_list
        .state
        .select(tree_path(&["origin:https://b.com", "request:3"]));

    assert_eq!(app.delete_selected_requests(), 1);

    assert_eq!(app.request_list.state.selected(), some_path);
    assert!(app.selected_request().is_none());
}

#[test]
fn delete_selected_requests_preserves_scroll_when_new_selection_is_visible() {
    let mut app = App::new(ui_settings(true));

    for sequence in 0..30 {
        app.add_request(captured_with_sequence(
            sequence,
            &format!("https://some.host.com/api/item{sequence}"),
        ));
    }

    render_app(&mut app);
    for _ in 0..8 {
        app.request_list.scroll_down();
    }
    render_app(&mut app);
    let offset_before_delete = app.request_list.state.get_offset();
    app.request_list.state.select(tree_path(&[
        "origin:https://some.host.com",
        "segment:api",
        "request:10",
    ]));

    assert_eq!(app.delete_selected_requests(), 1);
    render_app(&mut app);

    assert_eq!(app.request_list.state.get_offset(), offset_before_delete);
    assert_eq!(
        app.request_list.state.selected(),
        tree_path(&["origin:https://some.host.com", "segment:api", "request:9"])
    );
}

#[test]
fn clear_requests_drops_records_and_resets_tree_state() {
    let mut app = App::new(ui_settings(true));

    app.add_request(captured_with_sequence(0, "https://some.host.com/api/a"));
    app.add_request(captured_with_sequence(1, "https://some.host.com/api/b"));
    app.detail_panel.scroll.offset = 1;
    app.detail_panel.select_tab(MainDisplayTab::RequestBody);
    app.detail_panel.scroll.offset = 2;
    assert!(app.capture_retained_bytes() > 0);

    app.clear_requests();

    assert_eq!(app.capture_count(), 0);
    assert_eq!(app.capture_retained_bytes(), 0);
    assert!(app.request_list.state.selected().is_empty());
    assert!(app.request_list.state.opened().is_empty());
    assert_eq!(app.detail_panel.scroll.offset, 0);
    app.detail_panel.select_tab(MainDisplayTab::RequestHeader);
    assert_eq!(app.detail_panel.scroll.offset, 0);
}
