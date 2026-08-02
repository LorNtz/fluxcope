use super::*;
use crate::{
    capture::{
        BodySide, CapturePolicy, CapturePublisher, CaptureSequence, CapturedExchange,
        DecodeDisplayMode, DecodeKey, DecodePolicy, DecodeResult, RequestCaptureInput,
        start_decode_service,
    },
    recording::RecordingState,
    settings::{RecordingPrefilterPatternSettings, RequestListSettings, UiSettings},
    ui::RootView,
};
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use edtui::{EditorMode, Index2, clipboard::ClipboardTrait};
use http::Method;
use hyper::HeaderMap;
use ratatui::{Terminal, backend::TestBackend};
use std::{cell::RefCell, rc::Rc};
use tokio_util::sync::CancellationToken;

fn ui_settings(auto_expand: bool) -> UiSettings {
    UiSettings {
        request_list: RequestListSettings { auto_expand },
    }
}

fn captured(uri: &str) -> CapturedExchange {
    captured_with_sequence(0, uri)
}

fn captured_with_sequence(sequence: u64, uri: &str) -> CapturedExchange {
    CapturedExchange {
        sequence: CaptureSequence::new(sequence),
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

fn tree_path(identifiers: &[&str]) -> Vec<String> {
    identifiers
        .iter()
        .map(|identifier| (*identifier).to_string())
        .collect()
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::empty())
}

fn focus_settings_content(app: &mut App) {
    app.handle_key_event(key(KeyCode::Right));
    assert_eq!(app.settings_popup.focus, SettingsPaneFocus::Content);
}

fn ctrl_key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::CONTROL)
}

fn key_with_kind(code: KeyCode, kind: KeyEventKind) -> KeyEvent {
    KeyEvent::new_with_kind(code, KeyModifiers::empty(), kind)
}

fn render_app(app: &mut App) {
    let _ = app.prepare_current_body_display(std::time::Instant::now());
    let backend = TestBackend::new(100, 12);
    let mut terminal = Terminal::new(backend).expect("test backend should initialize");
    let mut ui = RootView::new();

    terminal
        .draw(|frame| ui.render(frame, app))
        .expect("request tree should render in tests");
}

struct TestClipboard {
    text: Rc<RefCell<String>>,
}

impl ClipboardTrait for TestClipboard {
    fn set_text(&mut self, text: String) {
        *self.text.borrow_mut() = text;
    }

    fn get_text(&mut self) -> String {
        self.text.borrow().clone()
    }
}

fn body_viewer_app_with_body(body: &str) -> App {
    let mut app = App::new(ui_settings(true));
    let mut req = captured("https://some.host.com/api");
    req.req_body = Some(body.to_string());
    app.add_request(req);
    app.focus_panel(PanelFocus::Detail);
    app.detail_panel.select_tab(MainDisplayTab::RequestBody);
    app.enter_current_body_viewer();
    app
}

fn attach_test_clipboard(app: &mut App) -> Rc<RefCell<String>> {
    let text = Rc::new(RefCell::new(String::new()));
    app.detail_panel
        .body_viewer
        .editor_mut()
        .expect("body viewer editor should be loaded")
        .set_clipboard(TestClipboard {
            text: Rc::clone(&text),
        });
    text
}

fn press_chars(app: &mut App, chars: &[char]) {
    for ch in chars {
        app.handle_key_event(key(KeyCode::Char(*ch)));
    }
}

fn settings_with_proxy_presets(active: &str) -> crate::settings::AppSettings {
    crate::settings::AppSettings {
        proxy: Some(crate::settings::ProxySettings {
            enable: true,
            active_preset: Some(active.to_string()),
            presets: vec![
                crate::settings::ProxyPresetSettings {
                    name: "dev".to_string(),
                    ..crate::settings::ProxyPresetSettings::default()
                },
                crate::settings::ProxyPresetSettings {
                    name: "qa".to_string(),
                    ..crate::settings::ProxyPresetSettings::default()
                },
                crate::settings::ProxyPresetSettings {
                    name: "prod".to_string(),
                    ..crate::settings::ProxyPresetSettings::default()
                },
            ],
        }),
        ..crate::settings::AppSettings::default()
    }
}

fn app_with_proxy_presets(active: &str) -> App {
    App::with_settings(
        settings_with_proxy_presets(active),
        RecordingState::default(),
    )
}

mod body_display;
mod body_viewer;
mod focus_input;
mod request_tree;
mod settings_general;
mod settings_proxy;
mod settings_recording;
