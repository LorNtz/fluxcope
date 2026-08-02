use super::panels::ScrollState;
use std::time::Instant;

mod body_viewer;
pub use body_viewer::BodyViewerKey;
pub(crate) use body_viewer::{BODY_LOADING_TEXT, BODY_TEXT_TAB_WIDTH};
use body_viewer::{BODY_TEXT_TAB, BodyViewer};

mod body_content;
mod keymap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BodyDisplayPreparation {
    ReadyToRender,
    Pending { started_at: Instant },
}

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
    const COUNT: usize = Self::ALL.len();

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
        match self {
            Self::RequestHeader => 0,
            Self::RequestBody => 1,
            Self::ResponseHeader => 2,
            Self::ResponseBody => 3,
        }
    }

    pub fn is_body(self) -> bool {
        matches!(self, Self::RequestBody | Self::ResponseBody)
    }
}

pub struct DetailPanel {
    pub active_tab: MainDisplayTab,
    pub scroll: ScrollState,
    // The active tab uses `scroll`; its saved slot is refreshed when the tab changes.
    tab_scroll_offsets: [u16; MainDisplayTab::COUNT],
    scroll_bounds_valid: bool,
    pub selected_header_row: Option<usize>,
    pub body_viewer: BodyViewer,
    body_text: BodyTextState,
    body_text_revision: u64,
}

impl DetailPanel {
    pub fn new() -> Self {
        Self {
            active_tab: MainDisplayTab::RequestHeader,
            scroll: ScrollState::new(),
            tab_scroll_offsets: [0; MainDisplayTab::COUNT],
            scroll_bounds_valid: true,
            selected_header_row: None,
            body_viewer: BodyViewer::new(),
            body_text: BodyTextState::Idle,
            body_text_revision: 0,
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
            self.tab_scroll_offsets[self.active_tab.index()] = self.scroll.offset;
            self.active_tab = tab;
            self.scroll.offset = self.tab_scroll_offsets[tab.index()];
            self.scroll.max_offset = 0;
            self.reset_request_content();
        }
    }

    pub fn reset_content_position(&mut self) {
        self.scroll.reset();
        self.tab_scroll_offsets.fill(0);
        self.reset_request_content();
    }

    pub(in crate::app) fn reset_request_content(&mut self) {
        self.scroll_bounds_valid = false;
        self.selected_header_row = None;
        self.body_viewer.reset();
        self.body_text = BodyTextState::Idle;
        self.body_text_revision = self.body_text_revision.wrapping_add(1);
    }

    pub fn scroll_down(&mut self) {
        if self.scroll_bounds_valid {
            self.scroll.scroll_down();
        }
    }

    pub fn scroll_up(&mut self) {
        if self.scroll_bounds_valid {
            self.scroll.scroll_up();
        }
    }

    pub(crate) fn update_scroll_bounds(&mut self, max_offset: u16) -> u16 {
        self.scroll.max_offset = max_offset;
        self.scroll.offset = self.scroll.offset.min(max_offset);
        self.scroll_bounds_valid = true;
        self.scroll.offset
    }

    pub(crate) fn suspend_scroll_bounds(&mut self) {
        self.scroll.max_offset = 0;
        self.scroll_bounds_valid = false;
    }

    pub(in crate::app) fn cached_body_text(&self, key: BodyViewerKey) -> Option<&str> {
        match &self.body_text {
            BodyTextState::Ready(cached) if cached.key == key => Some(&cached.source_text),
            BodyTextState::Idle
            | BodyTextState::Pending { .. }
            | BodyTextState::Ready(_)
            | BodyTextState::Unavailable { .. } => None,
        }
    }

    #[cfg(test)]
    pub(in crate::app) fn cached_body_render_text(&self, key: BodyViewerKey) -> Option<&str> {
        match &self.body_text {
            BodyTextState::Ready(cached) if cached.key == key => Some(cached.render_text()),
            BodyTextState::Idle
            | BodyTextState::Pending { .. }
            | BodyTextState::Ready(_)
            | BodyTextState::Unavailable { .. } => None,
        }
    }

    pub(in crate::app) fn body_render_text(
        &self,
        key: BodyViewerKey,
    ) -> Option<BodyRenderText<'_>> {
        match &self.body_text {
            BodyTextState::Pending {
                key: pending_key, ..
            } if *pending_key == key => Some(BodyRenderText::Loading),
            BodyTextState::Ready(cached) if cached.key == key => {
                Some(BodyRenderText::Text(cached.render_text()))
            }
            BodyTextState::Unavailable {
                key: unavailable_key,
                message,
            } if *unavailable_key == key => Some(BodyRenderText::Text(message)),
            BodyTextState::Idle
            | BodyTextState::Pending { .. }
            | BodyTextState::Ready(_)
            | BodyTextState::Unavailable { .. } => None,
        }
    }

    pub(in crate::app) fn has_body_text_state(&self, key: BodyViewerKey) -> bool {
        self.body_text.key() == Some(key)
    }

    pub(in crate::app) fn pending_body_started_at(&self, key: BodyViewerKey) -> Option<Instant> {
        match self.body_text {
            BodyTextState::Pending {
                key: pending_key,
                defer_started_at: Some(started_at),
            } if pending_key == key => Some(started_at),
            BodyTextState::Idle
            | BodyTextState::Pending { .. }
            | BodyTextState::Ready(_)
            | BodyTextState::Unavailable { .. } => None,
        }
    }

    pub(in crate::app) fn set_body_text_pending(
        &mut self,
        key: BodyViewerKey,
        started_at: Instant,
    ) {
        self.body_text = BodyTextState::Pending {
            key,
            defer_started_at: Some(started_at),
        };
        self.body_text_revision = self.body_text_revision.wrapping_add(1);
    }

    pub(in crate::app) fn body_text_is_pending(&self, key: BodyViewerKey) -> bool {
        matches!(self.body_text, BodyTextState::Pending { key: pending_key, .. } if pending_key == key)
    }

    pub(in crate::app) fn disable_pending_body_deferral(&mut self) {
        if let BodyTextState::Pending {
            defer_started_at, ..
        } = &mut self.body_text
        {
            *defer_started_at = None;
        }
    }

    pub(in crate::app) fn cache_body_text(&mut self, key: BodyViewerKey, text: String) {
        self.body_text = BodyTextState::Ready(CachedBodyText::new(key, text));
        self.body_text_revision = self.body_text_revision.wrapping_add(1);
    }

    pub(in crate::app) fn set_body_text_unavailable(
        &mut self,
        key: BodyViewerKey,
        message: &'static str,
    ) {
        self.body_text = BodyTextState::Unavailable { key, message };
        self.body_text_revision = self.body_text_revision.wrapping_add(1);
    }

    pub(in crate::app) fn take_cached_body_text(&mut self, key: BodyViewerKey) -> Option<String> {
        if !matches!(&self.body_text, BodyTextState::Ready(cached) if cached.key == key) {
            return None;
        }

        let BodyTextState::Ready(cached) =
            std::mem::replace(&mut self.body_text, BodyTextState::Idle)
        else {
            unreachable!("body text state was checked before replacement");
        };
        self.body_text_revision = self.body_text_revision.wrapping_add(1);
        Some(cached.source_text)
    }

    pub(in crate::app) fn body_text_revision(&self) -> u64 {
        self.body_text_revision
    }
}

pub(crate) enum BodyRenderText<'a> {
    Loading,
    Text(&'a str),
}

enum BodyTextState {
    Idle,
    Pending {
        key: BodyViewerKey,
        defer_started_at: Option<Instant>,
    },
    Ready(CachedBodyText),
    Unavailable {
        key: BodyViewerKey,
        message: &'static str,
    },
}

impl BodyTextState {
    fn key(&self) -> Option<BodyViewerKey> {
        match self {
            Self::Idle => None,
            Self::Pending { key, .. }
            | Self::Unavailable { key, .. }
            | Self::Ready(CachedBodyText { key, .. }) => Some(*key),
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
