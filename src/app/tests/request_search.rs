use std::sync::atomic::AtomicBool;

use super::*;
use crate::request_search::{RequestSearchDispatch, SearchJobOutcome, run_search};

fn begin_query(app: &mut App, query: &str) {
    app.handle_key_event(key(KeyCode::Char('/')));
    press_chars(app, &query.chars().collect::<Vec<_>>());
}

fn finish_query(app: &mut App) {
    app.complete_pending_request_search();
}

#[test]
fn idle_dispatch_poll_does_not_rebuild_a_dirty_request_tree() {
    let mut app = App::new(ui_settings(false));
    app.add_request(captured("https://a.com/api/item"));
    assert_eq!(app.request_tree_revision, 0);

    assert!(app.take_request_search_dispatch().is_none());
    assert_eq!(app.request_tree_revision, 0);
}

fn commit_query(app: &mut App, query: &str) {
    begin_query(app, query);
    finish_query(app);
    app.handle_key_event(key(KeyCode::Enter));
}

#[test]
fn editing_escape_restores_selection_openings_and_scroll_offset() {
    let mut app = App::new(ui_settings(false));
    for sequence in 0..30 {
        app.add_request(captured_with_sequence(
            sequence,
            &format!("https://a.com/api/item{sequence}"),
        ));
    }
    let baseline = tree_path(&["origin:https://a.com"]);
    app.request_list.state.open(baseline.clone());
    app.request_list.state.select(baseline.clone());
    render_app(&mut app);
    for _ in 0..5 {
        app.request_list.scroll_down();
    }
    render_app(&mut app);
    let baseline_offset = app.request_list.state.get_offset();

    begin_query(&mut app, "item20");
    finish_query(&mut app);
    assert_eq!(
        app.request_list.state.selected(),
        tree_path(&["origin:https://a.com", "segment:api", "request:20"])
    );

    app.handle_key_event(key(KeyCode::Esc));
    render_app(&mut app);

    assert_eq!(app.request_list.state.selected(), baseline);
    assert_eq!(app.request_list.state.get_offset(), baseline_offset);
    assert_eq!(
        app.request_list.state.opened(),
        &[baseline.clone()]
            .into_iter()
            .collect::<std::collections::HashSet<_>>()
    );
    assert!(app.request_search_query().is_none());
}

#[test]
fn empty_enter_cancels_and_nonempty_enter_commits_zero_results() {
    let mut app = App::new(ui_settings(false));
    app.add_request(captured("https://a.com/api/item"));
    let baseline = app.request_list.state.selected().to_vec();

    app.handle_key_event(key(KeyCode::Char('/')));
    app.handle_key_event(key(KeyCode::Enter));
    assert!(!app.is_request_search_editing());
    assert!(app.request_search_query().is_none());
    assert_eq!(app.request_list.state.selected(), baseline);

    begin_query(&mut app, "absent");
    finish_query(&mut app);
    assert_eq!(
        app.request_search_title_status(),
        SearchTitleStatus::NoMatches
    );
    app.handle_key_event(key(KeyCode::Enter));

    assert!(!app.is_request_search_editing());
    assert_eq!(app.request_search_query(), Some("absent"));
    assert_eq!(
        app.request_search_title_status(),
        SearchTitleStatus::NoMatches
    );
}

#[test]
fn pending_query_can_commit_and_selects_when_result_arrives() {
    let mut app = App::new(ui_settings(false));
    app.add_request(captured("https://a.com/api/needle"));

    begin_query(&mut app, "needle");
    app.handle_key_event(key(KeyCode::Enter));
    assert_eq!(
        app.request_search_title_status(),
        SearchTitleStatus::Pending
    );
    finish_query(&mut app);

    assert_eq!(
        app.request_list.state.selected(),
        tree_path(&["origin:https://a.com", "segment:api", "request:0"])
    );
    assert_eq!(
        app.request_search_title_status(),
        SearchTitleStatus::Matches {
            selected: Some(1),
            total: 1,
        }
    );
}

#[test]
fn replacement_query_escape_restores_prior_committed_search() {
    let mut app = App::new(ui_settings(false));
    app.add_request(captured_with_sequence(0, "https://a.com/api/alpha"));
    app.add_request(captured_with_sequence(1, "https://a.com/api/beta"));
    commit_query(&mut app, "alpha");
    let committed_selection = app.request_list.state.selected().to_vec();

    begin_query(&mut app, "beta");
    finish_query(&mut app);
    assert_eq!(
        app.request_list.state.selected().last().unwrap(),
        "request:1"
    );
    app.handle_key_event(key(KeyCode::Esc));

    assert_eq!(app.request_search_query(), Some("alpha"));
    assert_eq!(app.request_list.state.selected(), committed_selection);
    assert!(app.request_search_results().is_some());
}

#[test]
fn committed_navigation_is_position_based_wraps_and_handles_no_selection() {
    let mut app = App::new(ui_settings(false));
    for sequence in 0..3 {
        app.add_request(captured_with_sequence(
            sequence,
            &format!("https://a.com/api/item{sequence}"),
        ));
    }
    commit_query(&mut app, "item");
    assert_eq!(
        app.request_list.state.selected().last().unwrap(),
        "request:0"
    );

    app.handle_key_event(key(KeyCode::Char('n')));
    assert_eq!(
        app.request_list.state.selected().last().unwrap(),
        "request:1"
    );
    app.handle_key_event(key(KeyCode::Char('p')));
    assert_eq!(
        app.request_list.state.selected().last().unwrap(),
        "request:0"
    );
    app.handle_key_event(key(KeyCode::Char('p')));
    assert_eq!(
        app.request_list.state.selected().last().unwrap(),
        "request:2"
    );

    app.request_list
        .state
        .select(tree_path(&["origin:https://a.com", "segment:api"]));
    app.handle_key_event(key(KeyCode::Char('n')));
    assert_eq!(
        app.request_list.state.selected().last().unwrap(),
        "request:0"
    );

    app.request_list.state.select(Vec::new());
    app.handle_key_event(key(KeyCode::Char('p')));
    assert_eq!(
        app.request_list.state.selected().last().unwrap(),
        "request:2"
    );
}

#[test]
fn query_edits_and_tree_revisions_reject_stale_results() {
    let mut app = App::new(ui_settings(false));
    app.add_request(captured("https://a.com/api/alpha-beta"));
    begin_query(&mut app, "alpha");
    let Some(RequestSearchDispatch::Run(first_request)) = app.take_request_search_dispatch() else {
        panic!("first query should dispatch");
    };

    app.handle_key_event(key(KeyCode::Char('-')));
    let Some(RequestSearchDispatch::Run(second_request)) = app.take_request_search_dispatch()
    else {
        panic!("edited query should dispatch");
    };
    let stale = run_search(&first_request, &AtomicBool::new(false));
    assert!(!app.apply_request_search_outcome(&stale));
    assert_eq!(
        app.request_search_title_status(),
        SearchTitleStatus::Pending
    );

    let current = run_search(&second_request, &AtomicBool::new(false));
    assert!(app.apply_request_search_outcome(&current));

    app.add_request(captured_with_sequence(1, "https://a.com/api/alpha-new"));
    let Some(RequestSearchDispatch::Run(refresh_request)) = app.take_request_search_dispatch()
    else {
        panic!("tree change should refresh");
    };
    assert!(app.request_search_refreshing());
    assert!(!app.apply_request_search_outcome(&current));
    let refreshed = run_search(&refresh_request, &AtomicBool::new(false));
    assert!(app.apply_request_search_outcome(&refreshed));
}

#[test]
fn incoming_capture_refresh_never_steals_selection_and_clear_preserves_query() {
    let mut app = App::new(ui_settings(false));
    app.add_request(captured("https://a.com/api/first"));
    commit_query(&mut app, "future");
    let baseline = app.request_list.state.selected().to_vec();

    app.add_request(captured_with_sequence(1, "https://a.com/api/future"));
    finish_query(&mut app);
    assert_eq!(app.request_list.state.selected(), baseline);
    assert_eq!(
        app.request_search_title_status(),
        SearchTitleStatus::Matches {
            selected: None,
            total: 1,
        }
    );

    app.clear_requests();
    finish_query(&mut app);
    assert_eq!(app.request_search_query(), Some("future"));
    assert_eq!(
        app.request_search_title_status(),
        SearchTitleStatus::NoMatches
    );
}

#[test]
fn committed_search_survives_focus_and_ordinary_tree_navigation() {
    let mut app = App::new(ui_settings(true));
    app.add_request(captured_with_sequence(0, "https://a.com/api/item0"));
    app.add_request(captured_with_sequence(1, "https://a.com/api/item1"));
    commit_query(&mut app, "item");

    app.handle_key_event(key(KeyCode::Tab));
    assert!(app.is_panel_focused(PanelFocus::Detail));
    assert_eq!(app.request_search_query(), Some("item"));
    app.handle_key_event(key(KeyCode::Tab));
    app.handle_key_event(key(KeyCode::Char('j')));

    assert_eq!(app.request_search_query(), Some("item"));
    assert!(app.request_search_results().is_some());
}

#[test]
fn editing_captures_shortcuts_and_paste_limit_feedback_clears_on_success() {
    let mut app = App::new(ui_settings(false));
    let recording_before = app.is_recording();
    app.handle_key_event(key(KeyCode::Char('/')));
    assert!(!app.handle_key_event(key(KeyCode::Char('q'))));
    app.handle_key_event(key(KeyCode::Char('r')));
    app.handle_key_event(key(KeyCode::Tab));
    app.handle_key_event(ctrl_key(KeyCode::Char('c')));

    assert_eq!(app.request_search_query(), Some("qr"));
    assert_eq!(app.is_recording(), recording_before);
    assert!(app.is_panel_focused(PanelFocus::RequestList));

    app.handle_key_event(key(KeyCode::Home));
    assert!(app.handle_request_search_paste(&"x".repeat(511)));
    assert_eq!(
        app.request_search_title_status(),
        SearchTitleStatus::QueryLimitReached
    );
    assert_eq!(app.request_search_query().unwrap().len(), 2);

    assert!(app.handle_request_search_paste("\nA\t"));
    assert_ne!(
        app.request_search_title_status(),
        SearchTitleStatus::QueryLimitReached
    );
    assert_eq!(app.request_search_query(), Some("Aqr"));
}

#[test]
fn editing_auto_expand_openings_are_removed_on_cancel() {
    let mut app = App::new(ui_settings(true));
    app.add_request(captured("https://a.com/base/one"));
    let baseline_opened = app.request_list.state.opened().clone();
    begin_query(&mut app, "missing");

    app.add_request(captured_with_sequence(1, "https://new.example/path/two"));
    assert!(
        app.request_list
            .state
            .opened()
            .contains(&tree_path(&["origin:https://new.example"]))
    );
    app.handle_key_event(key(KeyCode::Esc));

    assert_eq!(app.request_list.state.opened(), &baseline_opened);
}

#[test]
fn query_change_removes_only_the_previous_match_reveal_openings() {
    let mut app = App::new(ui_settings(false));
    app.add_request(captured_with_sequence(0, "https://a.com/one/alpha"));
    app.add_request(captured_with_sequence(1, "https://a.com/two/beta"));
    let origin = tree_path(&["origin:https://a.com"]);
    app.request_list.state.open(origin.clone());

    begin_query(&mut app, "alpha");
    finish_query(&mut app);
    let first_branch = tree_path(&["origin:https://a.com", "segment:one"]);
    assert!(app.request_list.state.opened().contains(&first_branch));

    for _ in 0.."alpha".len() {
        app.handle_key_event(key(KeyCode::Backspace));
    }
    press_chars(&mut app, &['b', 'e', 't', 'a']);
    finish_query(&mut app);
    let second_branch = tree_path(&["origin:https://a.com", "segment:two"]);

    assert!(!app.request_list.state.opened().contains(&first_branch));
    assert!(app.request_list.state.opened().contains(&second_branch));
    assert!(app.request_list.state.opened().contains(&origin));
}

#[test]
fn committed_navigation_accumulates_match_ancestor_openings() {
    let mut app = App::new(ui_settings(false));
    app.add_request(captured_with_sequence(0, "https://a.com/one/item"));
    app.add_request(captured_with_sequence(1, "https://a.com/two/item"));
    commit_query(&mut app, "item");
    let first_branch = tree_path(&["origin:https://a.com", "segment:one"]);
    let second_branch = tree_path(&["origin:https://a.com", "segment:two"]);
    assert!(app.request_list.state.opened().contains(&first_branch));
    assert!(!app.request_list.state.opened().contains(&second_branch));

    app.handle_key_event(key(KeyCode::Char('n')));

    assert!(app.request_list.state.opened().contains(&first_branch));
    assert!(app.request_list.state.opened().contains(&second_branch));
}

#[test]
fn committed_navigation_skips_deleted_stale_paths_while_refreshing() {
    let mut app = App::new(ui_settings(true));
    app.add_request(captured_with_sequence(0, "https://a.com/api/item0"));
    app.add_request(captured_with_sequence(1, "https://a.com/api/item1"));
    commit_query(&mut app, "item");
    assert_eq!(app.delete_selected_requests(), 1);

    assert!(app.navigate_request_search(true));
    assert_eq!(
        app.request_list.state.selected().last().unwrap(),
        "request:1"
    );
    assert_eq!(app.request_search_query(), Some("item"));
}

#[test]
fn failed_search_is_nonfatal_and_retries_after_tree_change() {
    let mut app = App::new(ui_settings(false));
    app.add_request(captured("https://a.com/api/item"));
    begin_query(&mut app, "item");
    let Some(RequestSearchDispatch::Run(request)) = app.take_request_search_dispatch() else {
        panic!("query should dispatch");
    };
    let failure = SearchJobOutcome::Failed {
        key: request.key,
        message: "injected failure".into(),
    };
    assert!(app.apply_request_search_outcome(&failure));
    assert_eq!(app.request_search_title_status(), SearchTitleStatus::Failed);

    app.add_request(captured_with_sequence(1, "https://a.com/api/item2"));
    assert!(matches!(
        app.take_request_search_dispatch(),
        Some(RequestSearchDispatch::Run(_))
    ));
    assert_eq!(
        app.request_search_title_status(),
        SearchTitleStatus::Pending
    );
}
