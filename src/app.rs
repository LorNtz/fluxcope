use crate::proxy_handler::CapturedData;
use crossterm::event::{KeyCode, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use ratatui::widgets::ListState;

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

    pub fn selected_request(&self) -> Option<&CapturedData> {
        self.request_list
            .state
            .selected()
            .and_then(|index| self.requests.get(index))
    }

    pub fn handle_key_press(&mut self, key_code: KeyCode) -> bool {
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
            KeyCode::Char('@') => {
                self.log_panel.toggle();
                false
            }
            _ => false,
        }
    }

    pub fn handle_mouse(&mut self, mouse: MouseEvent) {
        let col = mouse.column;
        let row = mouse.row;

        match mouse.kind {
            MouseEventKind::ScrollDown => {
                if self.log_panel.visible && self.log_panel.rect.contains((col, row).into()) {
                    self.log_panel.scroll.scroll_down();
                } else if self.detail_panel.rect.contains((col, row).into()) {
                    self.detail_panel.scroll.scroll_down();
                } else if self.request_list.rect.contains((col, row).into()) {
                    self.next();
                }
            }
            MouseEventKind::ScrollUp => {
                if self.log_panel.visible && self.log_panel.rect.contains((col, row).into()) {
                    self.log_panel.scroll.scroll_up();
                } else if self.detail_panel.rect.contains((col, row).into()) {
                    self.detail_panel.scroll.scroll_up();
                } else if self.request_list.rect.contains((col, row).into()) {
                    self.previous();
                }
            }
            _ => {}
        }
    }

    pub fn handle_app_event(&mut self, event: AppEvent) {
        match event {
            AppEvent::NetworkRequest(req) => self.add_request(req),
            AppEvent::LogMessage(msg) => self.log_panel.add_log(msg),
        }
    }
}
