use crate::{
    logging::{LogRecord, LogRetentionPolicy},
    settings::RequestListSettings,
};
use ratatui::layout::Position;
use tui_tree_widget::TreeState;

use super::request_tree::{RequestTreeEntry, origin_identifier};

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
    max_scroll_offset: usize,
}

impl RequestListPanel {
    pub fn new(setting: RequestListSettings) -> Self {
        Self {
            state: TreeState::default(),
            auto_expand: setting.auto_expand,
            max_scroll_offset: 0,
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

    pub(crate) fn update_scroll_bounds(&mut self, visible_rows: usize, viewport_rows: usize) {
        self.max_scroll_offset = if viewport_rows == 0 {
            0
        } else {
            visible_rows.saturating_sub(viewport_rows)
        };
        let overflow = self
            .state
            .get_offset()
            .saturating_sub(self.max_scroll_offset);
        self.state.scroll_up(overflow);
    }

    pub fn scroll_down(&mut self) -> bool {
        if self.state.get_offset() >= self.max_scroll_offset {
            return false;
        }
        self.state.scroll_down(1)
    }

    pub fn scroll_up(&mut self) -> bool {
        self.state.scroll_up(1)
    }

    pub(in crate::app) fn restore_scroll_offset(&mut self, offset: usize) {
        let current = self.state.get_offset();
        if current < offset {
            self.state.scroll_down(offset - current);
        } else {
            self.state.scroll_up(current - offset);
        }
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
