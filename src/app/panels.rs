use crate::{
    logging::{LogRecord, LogRetentionPolicy},
    settings::RequestListSettings,
};
use ratatui::layout::Position;
use std::time::Instant;
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
    body_text: BodyTextState,
    body_text_revision: u64,
}

impl DetailPanel {
    pub fn new() -> Self {
        Self {
            active_tab: MainDisplayTab::RequestHeader,
            scroll: ScrollState::new(),
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
            self.active_tab = tab;
            self.reset_content_position();
        }
    }

    pub fn reset_content_position(&mut self) {
        self.scroll.reset();
        self.selected_header_row = None;
        self.body_viewer.reset();
        self.body_text = BodyTextState::Idle;
        self.body_text_revision = self.body_text_revision.wrapping_add(1);
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

pub struct LogPanel {
    logs: std::collections::VecDeque<LogRecord>,
    retained_bytes: usize,
    retention: LogRetentionPolicy,
    rendered_text: String,
    rendered_text_dirty: bool,
    revision: u64,
    pub scroll: ScrollState,
    pub visible: bool,
}

impl LogPanel {
    pub fn with_retention(retention: LogRetentionPolicy) -> Self {
        Self {
            logs: std::collections::VecDeque::new(),
            retained_bytes: 0,
            retention,
            rendered_text: String::new(),
            rendered_text_dirty: false,
            revision: 0,
            scroll: ScrollState::new(),
            visible: false,
        }
    }

    pub fn add_log(&mut self, record: LogRecord) {
        self.retained_bytes = self.retained_bytes.saturating_add(record.len());
        self.logs.push_back(record);
        let mut evicted_any = false;
        while self.logs.len() > self.retention.max_records
            || self.retained_bytes > self.retention.max_bytes
        {
            let Some(evicted) = self.logs.pop_front() else {
                break;
            };
            self.retained_bytes = self.retained_bytes.saturating_sub(evicted.len());
            evicted_any = true;
        }
        self.revision = self.revision.wrapping_add(1);
        if evicted_any {
            self.rendered_text_dirty = true;
        } else if !self.rendered_text_dirty {
            if !self.rendered_text.is_empty() {
                self.rendered_text.push('\n');
            }
            if let Some(record) = self.logs.back() {
                self.rendered_text.push_str(record.as_str());
            }
        }
    }

    pub fn render_text(&mut self) -> &str {
        if self.rendered_text_dirty {
            self.rebuild_rendered_text();
            self.rendered_text_dirty = false;
        }
        &self.rendered_text
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    fn rebuild_rendered_text(&mut self) {
        let newline_bytes = self.logs.len().saturating_sub(1);
        let mut rendered = String::with_capacity(self.retained_bytes + newline_bytes);
        for (index, record) in self.logs.iter().enumerate() {
            if index > 0 {
                rendered.push('\n');
            }
            rendered.push_str(record.as_str());
        }
        self.rendered_text = rendered;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_retention_evicts_by_count_and_bytes() {
        let mut panel = LogPanel::with_retention(LogRetentionPolicy {
            max_records: 2,
            max_bytes: 5,
        });

        panel.add_log(LogRecord::system("abc".to_string()));
        panel.add_log(LogRecord::system("de".to_string()));
        panel.add_log(LogRecord::system("fghi".to_string()));

        assert_eq!(panel.render_text(), "fghi");
        assert_eq!(panel.retained_bytes, 4);
    }
}
