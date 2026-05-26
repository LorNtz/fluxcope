use crate::{proxy_handler::CapturedData, settings::{UiSettings, RequestListSettings}};
use crossterm::event::KeyCode;
use ratatui::layout::Position;
use tui_tree_widget::TreeState;
use url::Url;

const ORIGIN_IDENTIFIER_PREFIX: &str = "origin:";
const SEGMENT_IDENTIFIER_PREFIX: &str = "segment:";
const REQUEST_IDENTIFIER_PREFIX: &str = "request:";

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MainDisplayTab {
    RequestHeader,
    RequestBody,
    ResponseHeader,
    ResponseBody,
}

impl MainDisplayTab {
    pub fn title(self) -> &'static str {
        match self {
            MainDisplayTab::RequestHeader => "Request Header",
            MainDisplayTab::RequestBody => "Request Body",
            MainDisplayTab::ResponseHeader => "Response Header",
            MainDisplayTab::ResponseBody => "Response Body",
        }
    }

    pub fn index(self) -> usize {
        match self {
            MainDisplayTab::RequestHeader => 0,
            MainDisplayTab::RequestBody => 1,
            MainDisplayTab::ResponseHeader => 2,
            MainDisplayTab::ResponseBody => 3,
        }
    }
}

#[derive(Debug)]
pub enum AppEvent {
    NetworkRequest(CapturedData),
    LogMessage(String),
    CertificateDownloadReady(String),
}

pub struct ScrollState {
    pub offset: u16,
    pub max_offset: u16,
}

impl ScrollState {
    pub fn new() -> Self {
        Self {
            offset: 0,
            max_offset: 0,
        }
    }

    pub fn scroll_down(&mut self) {
        self.offset = self.offset.saturating_add(1).min(self.max_offset);
    }

    pub fn scroll_up(&mut self) {
        self.offset = self.offset.saturating_sub(1);
    }

    pub fn reset(&mut self) {
        self.offset = 0;
    }
}

pub struct RequestListPanel {
    pub state: TreeState<String>,
    auto_expand: bool,
}

impl RequestListPanel {
    pub fn new(setting: RequestListSettings) -> Self {
        Self {
            state: TreeState::default(),
            auto_expand: setting.auto_expand,
        }
    }

    fn open_entry(&mut self, entry: &RequestTreeEntry) {
        for branch in entry.branch_paths() {
            self.state.open(branch);
        }
    }

    fn select_entry(&mut self, entry: &RequestTreeEntry) {
        self.state.select(entry.request_path());
    }

    fn select_origin(&mut self, entry: &RequestTreeEntry) {
        self.state.select(vec![origin_identifier(&entry.origin)]);
    }

    pub fn click_at(&mut self, position: Position) -> bool {
        self.state.click_at(position)
    }

    pub fn scroll_down(&mut self) -> bool {
        self.state.scroll_down(1)
    }

    pub fn scroll_up(&mut self) -> bool {
        self.state.scroll_up(1)
    }
}

pub struct DetailPanel {
    pub active_tab: MainDisplayTab,
    pub scroll: ScrollState,
}

impl DetailPanel {
    pub fn new() -> Self {
        Self {
            active_tab: MainDisplayTab::RequestHeader,
            scroll: ScrollState::new(),
        }
    }

    pub fn next_tab(&mut self) {
        self.active_tab = match self.active_tab {
            MainDisplayTab::RequestHeader => MainDisplayTab::RequestBody,
            MainDisplayTab::RequestBody => MainDisplayTab::ResponseHeader,
            MainDisplayTab::ResponseHeader => MainDisplayTab::ResponseBody,
            MainDisplayTab::ResponseBody => MainDisplayTab::RequestHeader,
        };
        self.scroll.reset();
    }
}

pub struct LogPanel {
    pub logs: Vec<String>,
    pub scroll: ScrollState,
    pub visible: bool,
}

impl LogPanel {
    pub fn new() -> Self {
        Self {
            logs: vec![],
            scroll: ScrollState::new(),
            visible: true,
        }
    }

    pub fn add_log(&mut self, msg: String) {
        self.logs.push(msg);
    }

    pub fn toggle(&mut self) {
        self.visible = !self.visible;
    }
}

pub struct CertificatePopup {
    pub visible: bool,
    pub download_url: Option<String>,
}

impl CertificatePopup {
    pub fn new() -> Self {
        Self {
            visible: false,
            download_url: None,
        }
    }

    pub fn open(&mut self) {
        self.visible = true;
    }

    pub fn close(&mut self) {
        self.visible = false;
    }

    pub fn set_download_url(&mut self, download_url: String) {
        self.download_url = Some(download_url);
    }
}

pub struct App {
    pub requests: Vec<CapturedData>,
    pub recording: bool,
    pub request_list: RequestListPanel,
    pub detail_panel: DetailPanel,
    pub log_panel: LogPanel,
    pub certificate_popup: CertificatePopup,
}

impl App {
    pub fn new(ui_settings: UiSettings) -> Self {
        Self {
            requests: vec![],
            recording: true,
            request_list: RequestListPanel::new(ui_settings.request_list),
            detail_panel: DetailPanel::new(),
            log_panel: LogPanel::new(),
            certificate_popup: CertificatePopup::new(),
        }
    }

    pub fn add_request(&mut self, req: CapturedData) {
        if !self.recording {
            return;
        }

        let was_empty = self.requests.is_empty();
        let request_index = self.requests.len();
        let tree_entry = request_tree_entry(&req, request_index);
        self.requests.push(req.clone());
        log::info!("request received: {}", req.uri);

        if self.request_list.auto_expand {
            self.request_list.open_entry(&tree_entry);
        }
        if was_empty {
            if self.request_list.auto_expand {
                self.request_list.select_entry(&tree_entry);
            } else {
                self.request_list.select_origin(&tree_entry);
            }
        }
    }

    pub fn next(&mut self) {
        let changed = self.request_list.state.key_down();
        self.apply_request_list_change(changed);
    }

    pub fn previous(&mut self) {
        let changed = self.request_list.state.key_up();
        self.apply_request_list_change(changed);
    }

    pub fn selected_request(&self) -> Option<&CapturedData> {
        selected_request_index(self.request_list.state.selected())
            .and_then(|index| self.requests.get(index))
    }

    pub fn handle_key_press(&mut self, key_code: KeyCode) -> bool {
        if self.certificate_popup.visible && key_code == KeyCode::Esc {
            self.certificate_popup.close();
            return false;
        }

        match key_code {
            KeyCode::Char('q') => true,
            KeyCode::Tab => {
                self.detail_panel.next_tab();
                false
            }
            KeyCode::Char('J') => {
                self.detail_panel.scroll.scroll_down();
                false
            }
            KeyCode::Char('K') => {
                self.detail_panel.scroll.scroll_up();
                false
            }
            KeyCode::Char('j') | KeyCode::Down => {
                self.next();
                false
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.previous();
                false
            }
            KeyCode::Char('h') | KeyCode::Left => {
                let changed = self.request_list.state.key_left();
                self.apply_request_list_change(changed);
                false
            }
            KeyCode::Char('l') | KeyCode::Right => {
                let changed = self.request_list.state.key_right();
                self.apply_request_list_change(changed);
                false
            }
            KeyCode::Enter | KeyCode::Char(' ') => {
                let changed = self.request_list.state.toggle_selected();
                self.apply_request_list_change(changed);
                false
            }
            KeyCode::PageDown => {
                self.request_list.scroll_down();
                false
            }
            KeyCode::PageUp => {
                self.request_list.scroll_up();
                false
            }
            KeyCode::Char('@') => {
                self.log_panel.toggle();
                false
            }
            KeyCode::Char('c') | KeyCode::Char('C') => {
                self.certificate_popup.open();
                false
            }
            _ => false,
        }
    }

    pub fn handle_app_event(&mut self, event: AppEvent) {
        match event {
            AppEvent::NetworkRequest(req) => self.add_request(req),
            AppEvent::LogMessage(msg) => self.log_panel.add_log(msg),
            AppEvent::CertificateDownloadReady(download_url) => {
                self.certificate_popup.set_download_url(download_url)
            }
        }
    }

    pub fn apply_request_list_change(&mut self, changed: bool) {
        if changed {
            self.detail_panel.scroll.reset();
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RequestTreeEntry {
    pub origin: String,
    pub segments: Vec<String>,
    request_index: usize,
}

impl RequestTreeEntry {
    pub fn request_identifier(&self) -> String {
        request_identifier(self.request_index)
    }

    pub fn request_path(&self) -> Vec<String> {
        let mut path = self.parent_path();
        path.push(self.request_identifier());
        path
    }

    fn branch_paths(&self) -> Vec<Vec<String>> {
        let mut branches = Vec::new();
        let mut path = vec![origin_identifier(&self.origin)];
        branches.push(path.clone());

        for segment in parent_segments(&self.segments) {
            path.push(segment_identifier(segment));
            branches.push(path.clone());
        }

        branches
    }

    fn parent_path(&self) -> Vec<String> {
        let mut path = vec![origin_identifier(&self.origin)];
        for segment in parent_segments(&self.segments) {
            path.push(segment_identifier(segment));
        }
        path
    }
}

pub fn request_tree_entry(req: &CapturedData, request_index: usize) -> RequestTreeEntry {
    let (origin, segments) = Url::parse(&req.uri)
        .ok()
        .filter(Url::has_host)
        .map(|url| {
            (
                absolute_url_origin(&url),
                path_segments_with_query(url.path(), url.query()),
            )
        })
        .unwrap_or_else(|| fallback_origin_and_segments(req));

    RequestTreeEntry {
        origin,
        segments,
        request_index,
    }
}

fn selected_request_index(path: &[String]) -> Option<usize> {
    path.last()?
        .strip_prefix(REQUEST_IDENTIFIER_PREFIX)?
        .parse()
        .ok()
}

fn origin_identifier(origin: &str) -> String {
    format!("{ORIGIN_IDENTIFIER_PREFIX}{origin}")
}

fn segment_identifier(segment: &str) -> String {
    format!("{SEGMENT_IDENTIFIER_PREFIX}{segment}")
}

fn request_identifier(index: usize) -> String {
    format!("{REQUEST_IDENTIFIER_PREFIX}{index}")
}

fn parent_segments(segments: &[String]) -> &[String] {
    segments
        .split_last()
        .map_or(&[], |(_, parent_segments)| parent_segments)
}

fn absolute_url_origin(url: &Url) -> String {
    let host = url.host_str().unwrap_or("(unknown host)");
    match url.port() {
        Some(port) => format!("{}://{}:{}", url.scheme(), host, port),
        None => format!("{}://{}", url.scheme(), host),
    }
}

fn fallback_origin_and_segments(req: &CapturedData) -> (String, Vec<String>) {
    let origin = request_header(req, "host")
        .map(|host| format!("https://{host}"))
        .unwrap_or_else(|| "(unknown host)".to_string());
    let (path, query) = req
        .uri
        .split_once('?')
        .map_or((req.uri.as_str(), None), |(path, query)| {
            (path, Some(query))
        });

    (origin, path_segments_with_query(path, query))
}

fn request_header<'a>(req: &'a CapturedData, name: &str) -> Option<&'a str> {
    req.req_headers
        .iter()
        .find(|(key, value)| key.eq_ignore_ascii_case(name) && !value.is_empty())
        .map(|(_, value)| value.as_str())
}

fn path_segments_with_query(path: &str, query: Option<&str>) -> Vec<String> {
    let mut segments: Vec<String> = path
        .trim_start_matches('/')
        .split('/')
        .filter(|segment| !segment.is_empty())
        .map(str::to_string)
        .collect();

    match (segments.last_mut(), query) {
        (Some(last), Some(query)) => {
            last.push('?');
            last.push_str(query);
        }
        (None, Some(query)) => segments.push(format!("?{query}")),
        (None, None) => segments.push("/".to_string()),
        (Some(_), None) => {}
    }

    segments
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::RequestListSettings;
    use http::Method;

    fn ui_settings(auto_expand: bool) -> UiSettings {
        UiSettings {
            request_list: RequestListSettings { auto_expand },
        }
    }

    fn captured(uri: &str) -> CapturedData {
        CapturedData {
            id: uuid::Uuid::nil(),
            method: Method::GET,
            uri: uri.to_string(),
            status: None,
            req_headers: vec![("host".to_string(), "fallback.example.com".to_string())],
            res_headers: vec![],
            req_body: None,
            res_body: None,
        }
    }

    #[test]
    fn request_tree_entry_splits_absolute_url() {
        let entry = request_tree_entry(
            &captured("https://some.host.com/api/v1/getUserInfo?a=1&b=2"),
            7,
        );

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
    fn request_tree_entry_uses_host_header_for_origin_form_uri() {
        let entry = request_tree_entry(&captured("/common/getSomeOtherInfo"), 3);

        assert_eq!(entry.origin, "https://fallback.example.com");
        assert_eq!(entry.segments, ["common", "getSomeOtherInfo"]);
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
}
