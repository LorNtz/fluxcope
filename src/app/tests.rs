use super::*;
use crate::{
    proxy_handler::CapturedData,
    settings::{RequestListSettings, UiSettings},
    ui::RootView,
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use http::Method;
use ratatui::{Terminal, backend::TestBackend};

fn ui_settings(auto_expand: bool) -> UiSettings {
    UiSettings {
        request_list: RequestListSettings { auto_expand },
    }
}

fn captured(uri: &str) -> CapturedData {
    captured_with_sequence(0, uri)
}

fn captured_with_sequence(sequence: u64, uri: &str) -> CapturedData {
    CapturedData {
        id: uuid::Uuid::nil(),
        sequence,
        method: Method::GET,
        uri: uri.to_string(),
        mapped_uri: None,
        local_path: None,
        status: None,
        req_headers: vec![("host".to_string(), "fallback.example.com".to_string())],
        res_headers: vec![],
        req_body: None,
        res_body: None,
    }
}

fn mapped_captured_with_sequence(sequence: u64, uri: &str, mapped_uri: &str) -> CapturedData {
    let mut captured = captured_with_sequence(sequence, uri);
    captured.mapped_uri = Some(mapped_uri.to_string());
    captured
}

fn tree_path(identifiers: &[&str]) -> Vec<String> {
    identifiers
        .iter()
        .map(|identifier| (*identifier).to_string())
        .collect()
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::empty())
}

fn ctrl_key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::CONTROL)
}

fn render_app(app: &mut App) {
    let backend = TestBackend::new(100, 12);
    let mut terminal = Terminal::new(backend).expect("test backend should initialize");
    let mut ui = RootView::new();

    terminal
        .draw(|frame| ui.render(frame, app))
        .expect("request tree should render in tests");
}

#[test]
fn focus_starts_on_request_list() {
    let app = App::new(ui_settings(true));

    assert!(app.is_panel_focused(PanelFocus::RequestList));
    assert!(!app.is_popup_focused(PopupFocus::Certificate));
}

#[test]
fn tab_cycles_focus_through_visible_panels() {
    let mut app = App::new(ui_settings(true));

    assert!(!app.handle_key_event(key(KeyCode::Tab)));
    assert!(app.is_panel_focused(PanelFocus::Detail));

    assert!(!app.handle_key_event(key(KeyCode::Tab)));
    assert!(app.is_panel_focused(PanelFocus::Log));

    assert!(!app.handle_key_event(key(KeyCode::Tab)));
    assert!(app.is_panel_focused(PanelFocus::RequestList));

    app.log_panel.visible = false;
    assert!(!app.handle_key_event(key(KeyCode::Tab)));
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
    assert!(app.is_panel_focused(PanelFocus::Log));

    app.handle_key_event(ctrl_key(KeyCode::Char('k')));
    assert!(app.is_panel_focused(PanelFocus::Detail));

    app.handle_key_event(ctrl_key(KeyCode::Char('h')));
    assert!(app.is_panel_focused(PanelFocus::RequestList));
}

#[test]
fn hiding_focused_log_panel_moves_focus_to_detail() {
    let mut app = App::new(ui_settings(true));

    app.focus_panel(PanelFocus::Log);
    app.handle_key_event(key(KeyCode::Char('@')));

    assert!(!app.log_panel.visible);
    assert!(app.is_panel_focused(PanelFocus::Detail));
}

#[test]
fn certificate_popup_takes_modal_focus_until_escape() {
    let mut app = App::new(ui_settings(true));

    app.focus_panel(PanelFocus::Log);
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
fn request_tree_entry_from_captured_request_splits_absolute_url() {
    let entry = RequestTreeEntry::from(&captured_with_sequence(
        7,
        "https://some.host.com/api/v1/getUserInfo?a=1&b=2",
    ));

    assert_eq!(entry.origin, "https://some.host.com");
    assert_eq!(entry.segments, ["api", "v1", "getUserInfo?a=1&b=2"]);
    assert_eq!(
        entry.request_path(),
        [
            "origin:https://some.host.com",
            "segment:api",
            "segment:v1",
            "request:7"
        ]
    );
}

#[test]
fn request_tree_entry_uses_mapped_uri_for_display_path() {
    let entry = RequestTreeEntry::from(&mapped_captured_with_sequence(
        7,
        "https://a.com/original/path?a=1",
        "http://b.test.com/mapped/path?a=1",
    ));

    assert_eq!(entry.origin, "http://b.test.com");
    assert_eq!(entry.segments, ["mapped", "path?a=1"]);
    assert_eq!(
        entry.request_path(),
        ["origin:http://b.test.com", "segment:mapped", "request:7"]
    );
}

#[test]
fn request_tree_entry_from_captured_request_uses_host_header_for_origin_form_uri() {
    let entry = RequestTreeEntry::from(&captured("/common/getSomeOtherInfo"));

    assert_eq!(entry.origin, "https://fallback.example.com");
    assert_eq!(entry.segments, ["common", "getSomeOtherInfo"]);
}

#[test]
fn requests_are_ordered_by_capture_sequence() {
    let mut app = App::new(ui_settings(true));

    app.add_request(captured_with_sequence(1, "https://a.com/b"));
    app.add_request(captured_with_sequence(0, "https://a.com/a"));

    let uris = app
        .requests
        .iter()
        .map(|req| req.uri.as_str())
        .collect::<Vec<_>>();

    assert_eq!(uris, ["https://a.com/a", "https://a.com/b"]);

    app.request_list.state.select(vec![
        "origin:https://a.com".to_string(),
        "request:1".to_string(),
    ]);

    assert_eq!(
        app.selected_request().map(|req| req.uri.as_str()),
        Some("https://a.com/b")
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
        app.selected_request().map(|req| req.uri.as_str()),
        Some("https://some.host.com/api/v1/getUserInfo?a=1")
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

    assert_eq!(app.requests.len(), 1);
    assert_eq!(app.requests[0].uri, "https://some.host.com/api/b");
    assert_eq!(
        app.request_list.state.selected(),
        [
            "origin:https://some.host.com".to_string(),
            "segment:api".to_string(),
            "request:1".to_string()
        ]
    );
    assert_eq!(
        app.selected_request().map(|req| req.uri.as_str()),
        Some("https://some.host.com/api/b")
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

    assert_eq!(app.requests.len(), 1);
    assert_eq!(app.requests[0].uri, "https://some.host.com/api/v2/c");
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
        app.selected_request().map(|req| req.uri.as_str()),
        Some("https://some.host.com/api/a")
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
    assert_eq!(app.requests.len(), 1);
    assert_eq!(app.requests[0].uri, "https://b.com/api4");
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
        app.selected_request().map(|req| req.uri.as_str()),
        Some("https://a.com/some/path/api3")
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
    assert!(app.requests.capacity() > 0);

    app.clear_requests();

    assert!(app.requests.is_empty());
    assert_eq!(app.requests.capacity(), 0);
    assert!(app.request_list.state.selected().is_empty());
    assert!(app.request_list.state.opened().is_empty());
    assert_eq!(app.detail_panel.scroll.offset, 0);
}
