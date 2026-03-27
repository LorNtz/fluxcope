use crate::proxy_handler::CapturedData;
use ratatui::layout::Rect;
use ratatui::widgets::ListState;

#[derive(Debug, PartialEq)]
pub enum ActiveTab {
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

pub struct App {
    pub requests: Vec<CapturedData>,
    pub logs: Vec<String>,
    pub state: ListState,
    pub log_scroll: u16,
    pub max_log_scroll: u16,
    pub active_tab: ActiveTab,
    pub recording: bool,
    pub vertical_scroll: u16,
    pub max_vertical_scroll: u16,
    pub log_panel_visible: bool,
    // Panel rects for mouse hit-testing (updated each frame)
    pub main_display_rect: Rect,
    pub log_panel_rect: Rect,
    pub request_list_rect: Rect,
}

impl App {
    pub fn new() -> Self {
        Self {
            requests: vec![],
            logs: vec![],
            state: ListState::default(),
            log_scroll: 0,
            max_log_scroll: 0,
            active_tab: ActiveTab::RequestHeader,
            recording: true,
            vertical_scroll: 0,
            max_vertical_scroll: 0,
            log_panel_visible: true,
            main_display_rect: Rect::default(),
            log_panel_rect: Rect::default(),
            request_list_rect: Rect::default(),
        }
    }

    pub fn add_request(&mut self, req: CapturedData) {
        if !self.recording {
            return;
        }

        let was_empty = self.requests.is_empty();
        self.requests.push(req.clone());
        log::info!("request received: {}", req.uri);

        // Auto-select the first request when captured
        if was_empty {
            self.state.select(Some(0));
        }
    }

    pub fn add_log(&mut self, msg: String) {
        self.logs.push(msg);
        // Auto-scroll to bottom (we'll handle this in rendering)
    }

    pub fn next(&mut self) {
        if self.requests.is_empty() {
            return;
        }
        let i = match self.state.selected() {
            Some(i) => {
                if i >= self.requests.len() - 1 {
                    0
                } else {
                    i + 1
                }
            }
            None => 0,
        };
        self.state.select(Some(i));
        self.reset_scroll();
    }

    pub fn previous(&mut self) {
        if self.requests.is_empty() {
            return;
        }
        let i = match self.state.selected() {
            Some(i) => {
                if i == 0 {
                    self.requests.len() - 1
                } else {
                    i - 1
                }
            }
            None => 0,
        };
        self.state.select(Some(i));
        self.reset_scroll();
    }

    pub fn next_tab(&mut self) {
        self.active_tab = match self.active_tab {
            ActiveTab::RequestHeader => ActiveTab::RequestBody,
            ActiveTab::RequestBody => ActiveTab::ResponseHeader,
            ActiveTab::ResponseHeader => ActiveTab::ResponseBody,
            ActiveTab::ResponseBody => ActiveTab::RequestHeader,
        };
        self.reset_scroll();
    }

    pub fn scroll_down(&mut self) {
        self.vertical_scroll = self.vertical_scroll.saturating_add(1).min(self.max_vertical_scroll);
    }

    pub fn scroll_up(&mut self) {
        self.vertical_scroll = self.vertical_scroll.saturating_sub(1);
    }

    pub fn scroll_log_down(&mut self) {
        self.log_scroll = self.log_scroll.saturating_add(1).min(self.max_log_scroll);
    }

    pub fn scroll_log_up(&mut self) {
        self.log_scroll = self.log_scroll.saturating_sub(1);
    }

    fn reset_scroll(&mut self) {
        self.vertical_scroll = 0;
    }

    pub fn toggle_log_panel(&mut self) {
        self.log_panel_visible = !self.log_panel_visible;
    }
}
