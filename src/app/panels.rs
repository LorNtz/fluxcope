use crate::settings::RequestListSettings;
use ratatui::layout::Position;
use tui_tree_widget::TreeState;

use super::body_viewer::{BODY_TEXT_TAB, BodyViewer, BodyViewerKey};
use super::request_tree::{RequestTreeEntry, origin_identifier};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MainDisplayTab {
    RequestHeader,
    RequestBody,
    ResponseHeader,
    ResponseBody,
}

impl MainDisplayTab {
    const ALL: [Self; 4] = [
        Self::RequestHeader,
        Self::RequestBody,
        Self::ResponseHeader,
        Self::ResponseBody,
    ];

    pub fn all() -> &'static [Self] {
        &Self::ALL
    }

    pub fn title(self) -> &'static str {
        match self {
            MainDisplayTab::RequestHeader => "Request Header",
            MainDisplayTab::RequestBody => "Request Body",
            MainDisplayTab::ResponseHeader => "Response Header",
            MainDisplayTab::ResponseBody => "Response Body",
        }
    }

    pub fn index(self) -> usize {
        Self::ALL
            .iter()
            .position(|tab| *tab == self)
            .unwrap_or_default()
    }

    pub fn is_body(self) -> bool {
        matches!(self, Self::RequestBody | Self::ResponseBody)
    }
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
    pub(in crate::app) auto_expand: bool,
}

impl RequestListPanel {
    pub fn new(setting: RequestListSettings) -> Self {
        Self {
            state: TreeState::default(),
            auto_expand: setting.auto_expand,
        }
    }

    pub(in crate::app) fn open_entry(&mut self, entry: &RequestTreeEntry) {
        for branch in entry.branch_paths() {
            self.state.open(branch);
        }
    }

    pub(in crate::app) fn select_entry(&mut self, entry: &RequestTreeEntry) {
        self.state.select(entry.request_path());
    }

    pub(in crate::app) fn select_origin(&mut self, entry: &RequestTreeEntry) {
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
    pub selected_header_row: Option<usize>,
    pub body_viewer: BodyViewer,
    cached_body_text: Option<CachedBodyText>,
}

impl DetailPanel {
    pub fn new() -> Self {
        Self {
            active_tab: MainDisplayTab::RequestHeader,
            scroll: ScrollState::new(),
            selected_header_row: None,
            body_viewer: BodyViewer::new(),
            cached_body_text: None,
        }
    }

    pub fn next_tab(&mut self) {
        self.select_tab(match self.active_tab {
            MainDisplayTab::RequestHeader => MainDisplayTab::RequestBody,
            MainDisplayTab::RequestBody => MainDisplayTab::ResponseHeader,
            MainDisplayTab::ResponseHeader => MainDisplayTab::ResponseBody,
            MainDisplayTab::ResponseBody => MainDisplayTab::RequestHeader,
        });
    }

    pub fn previous_tab(&mut self) {
        self.select_tab(match self.active_tab {
            MainDisplayTab::RequestHeader => MainDisplayTab::ResponseBody,
            MainDisplayTab::RequestBody => MainDisplayTab::RequestHeader,
            MainDisplayTab::ResponseHeader => MainDisplayTab::RequestBody,
            MainDisplayTab::ResponseBody => MainDisplayTab::ResponseHeader,
        });
    }

    pub fn select_tab(&mut self, tab: MainDisplayTab) {
        if self.active_tab != tab {
            self.active_tab = tab;
            self.reset_content_position();
        }
    }

    pub fn reset_content_position(&mut self) {
        self.scroll.reset();
        self.selected_header_row = None;
        self.body_viewer.reset();
        self.cached_body_text = None;
    }

    pub(in crate::app) fn cached_body_text(&self, key: BodyViewerKey) -> Option<&str> {
        self.cached_body_text
            .as_ref()
            .filter(|cached| cached.key == key)
            .map(|cached| cached.source_text.as_str())
    }

    pub(in crate::app) fn cached_body_render_text(&self, key: BodyViewerKey) -> Option<&str> {
        self.cached_body_text
            .as_ref()
            .filter(|cached| cached.key == key)
            .map(CachedBodyText::render_text)
    }

    pub(in crate::app) fn cache_body_text(&mut self, key: BodyViewerKey, text: String) {
        self.cached_body_text = Some(CachedBodyText::new(key, text));
    }

    pub(in crate::app) fn take_cached_body_text(&mut self, key: BodyViewerKey) -> Option<String> {
        if self
            .cached_body_text
            .as_ref()
            .is_some_and(|cached| cached.key == key)
        {
            self.cached_body_text
                .take()
                .map(|cached| cached.source_text)
        } else {
            None
        }
    }
}

struct CachedBodyText {
    key: BodyViewerKey,
    source_text: String,
    render_text: Option<String>,
}

impl CachedBodyText {
    fn new(key: BodyViewerKey, source_text: String) -> Self {
        // Literal tabs printed by Crossterm advance beyond Ratatui's tracked cell position.
        let render_text = source_text
            .contains('\t')
            .then(|| source_text.replace('\t', BODY_TEXT_TAB));
        Self {
            key,
            source_text,
            render_text,
        }
    }

    fn render_text(&self) -> &str {
        self.render_text.as_deref().unwrap_or(&self.source_text)
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
            visible: false,
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
