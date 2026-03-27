use crate::proxy_handler::CapturedData;
use ratatui::layout::Rect;
use ratatui::widgets::ListState;

#[derive(Debug, PartialEq)]
pub enum MainDisplayTab {
    RequestHeader,
    RequestBody,
    ResponseHeader,
    ResponseBody,
}

#[derive(Debug)]
pub enum AppEvent {
    NetworkRequest(CapturedData),
    LogMessage(String),
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
    pub state: ListState,
    pub rect: Rect,
}

impl RequestListPanel {
    pub fn new() -> Self {
        Self {
            state: ListState::default(),
            rect: Rect::default(),
        }
    }
}

pub struct DetailPanel {
    pub active_tab: MainDisplayTab,
    pub scroll: ScrollState,
    pub rect: Rect,
}

impl DetailPanel {
    pub fn new() -> Self {
        Self {
            active_tab: MainDisplayTab::RequestHeader,
            scroll: ScrollState::new(),
            rect: Rect::default(),
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
    pub rect: Rect,
}

impl LogPanel {
    pub fn new() -> Self {
        Self {
            logs: vec![],
            scroll: ScrollState::new(),
            visible: true,
            rect: Rect::default(),
        }
    }

    pub fn add_log(&mut self, msg: String) {
        self.logs.push(msg);
    }

    pub fn toggle(&mut self) {
        self.visible = !self.visible;
    }
}

pub struct App {
    pub requests: Vec<CapturedData>,
    pub recording: bool,
    pub request_list: RequestListPanel,
    pub detail_panel: DetailPanel,
    pub log_panel: LogPanel,
}

impl App {
    pub fn new() -> Self {
        Self {
            requests: vec![],
            recording: true,
            request_list: RequestListPanel::new(),
            detail_panel: DetailPanel::new(),
            log_panel: LogPanel::new(),
        }
    }

    pub fn add_request(&mut self, req: CapturedData) {
        if !self.recording {
            return;
        }

        let was_empty = self.requests.is_empty();
        self.requests.push(req.clone());
        log::info!("request received: {}", req.uri);

        if was_empty {
            self.request_list.state.select(Some(0));
        }
    }

    pub fn next(&mut self) {
        if self.requests.is_empty() {
            return;
        }
        let i = match self.request_list.state.selected() {
            Some(i) => {
                if i >= self.requests.len() - 1 {
                    0
                } else {
                    i + 1
                }
            }
            None => 0,
        };
        self.request_list.state.select(Some(i));
        self.detail_panel.scroll.reset();
    }

    pub fn previous(&mut self) {
        if self.requests.is_empty() {
            return;
        }
        let i = match self.request_list.state.selected() {
            Some(i) => {
                if i == 0 {
                    self.requests.len() - 1
                } else {
                    i - 1
                }
            }
            None => 0,
        };
        self.request_list.state.select(Some(i));
        self.detail_panel.scroll.reset();
    }
}
