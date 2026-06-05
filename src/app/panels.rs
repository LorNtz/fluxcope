use crate::settings::RequestListSettings;
use ratatui::layout::Position;
use tui_tree_widget::TreeState;

use super::request_tree::{RequestTreeEntry, origin_identifier};

#[derive(Clone, Copy, Debug, PartialEq)]
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
}

impl DetailPanel {
    pub fn new() -> Self {
        Self {
            active_tab: MainDisplayTab::RequestHeader,
            scroll: ScrollState::new(),
            selected_header_row: None,
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
