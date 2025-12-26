use ratatui::widgets::ListState;
use crate::proxy_handler::CapturedData;

pub enum ActiveTab {
    Request,
    Response,
    Body,
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
    pub log_state: ListState,
    pub active_tab: ActiveTab,
    pub recording: bool,
}

impl App {
    pub fn new() -> Self {
        Self {
            requests: vec![],
            logs: vec![],
            state: ListState::default(),
            log_state: ListState::default(),
            active_tab: ActiveTab::Request,
            recording: true,
        }
    }

    pub fn add_request(&mut self, req: CapturedData) {
        if !self.recording { return; }
        
        self.requests.push(req.clone());
        log::info!("request received: {}", req.uri);
    }

    pub fn add_log(&mut self, msg: String) {
        self.logs.push(msg);
        // Auto-scroll to bottom
        self.log_state.select(Some(self.logs.len() - 1));
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
    }

    pub fn next_tab(&mut self) {
        self.active_tab = match self.active_tab {
            ActiveTab::Request => ActiveTab::Response,
            ActiveTab::Response => ActiveTab::Body,
            ActiveTab::Body => ActiveTab::Request,
        };
    }
}