use crossterm::event::{KeyCode, KeyEvent};

use crate::settings::RecordingPrefilterPatternSettings;

use super::{
    EditMode, SettingsKeyHint, SettingsPaneFocus, SettingsPopup, SettingsTopic, edit_text_value,
};

const PREFILTER_TABLE_KEY_HINTS: &[SettingsKeyHint] = &[
    SettingsKeyHint {
        label: "Back",
        key: "Esc",
    },
    SettingsKeyHint {
        label: "Edit",
        key: "Enter/e",
    },
    SettingsKeyHint {
        label: "Toggle",
        key: "Space",
    },
    SettingsKeyHint {
        label: "Add/del",
        key: "a/d",
    },
    SettingsKeyHint {
        label: "Move",
        key: "j/k/Pg/J/K",
    },
];

const EMPTY_PREFILTER_TABLE_KEY_HINTS: &[SettingsKeyHint] = &[
    SettingsKeyHint {
        label: "Back",
        key: "Esc",
    },
    SettingsKeyHint {
        label: "Add",
        key: "a",
    },
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RecordingWidget {
    StartRecordingOnLaunch,
    PrefilterEnabled,
    IncludeUrlPatterns,
}

impl RecordingWidget {
    const ALL: [Self; 3] = [
        Self::StartRecordingOnLaunch,
        Self::PrefilterEnabled,
        Self::IncludeUrlPatterns,
    ];

    pub(crate) fn all() -> &'static [Self] {
        &Self::ALL
    }

    pub(crate) fn row(self) -> usize {
        Self::ALL
            .iter()
            .position(|widget| *widget == self)
            .expect("recording widgets are declared in RecordingWidget::ALL")
    }

    fn from_row(row: usize) -> Option<Self> {
        Self::ALL.get(row).copied()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PrefilterPatternEditState<'a> {
    pub index: usize,
    pub value: &'a str,
    pub cursor: usize,
}

impl SettingsPopup {
    pub(super) fn selected_recording_widget(&self) -> Option<RecordingWidget> {
        RecordingWidget::from_row(self.selected_row)
    }

    pub(super) fn prefilter_table_key_hints(&self) -> &'static [SettingsKeyHint] {
        if self.prefilter_pattern_count() == 0 {
            EMPTY_PREFILTER_TABLE_KEY_HINTS
        } else {
            PREFILTER_TABLE_KEY_HINTS
        }
    }

    pub(super) fn start_recording_selected_edit(&mut self) {
        if self.selected_recording_widget() == Some(RecordingWidget::IncludeUrlPatterns) {
            self.mode = EditMode::PrefilterTable {
                selected_pattern: 0,
            };
            self.ensure_prefilter_pattern_visible();
            self.request_selected_visible();
        }
    }

    pub(super) fn toggle_selected_recording_checkbox(&mut self) {
        match self.selected_recording_widget() {
            Some(RecordingWidget::StartRecordingOnLaunch) => {
                self.draft.recording.start_record_on_launch =
                    !self.draft.recording.start_record_on_launch;
            }
            Some(RecordingWidget::PrefilterEnabled) => {
                self.draft.recording.prefilter.enable = !self.draft.recording.prefilter.enable;
            }
            Some(RecordingWidget::IncludeUrlPatterns) | None => {}
        }
    }

    pub(super) fn add_prefilter_pattern_from_browse(&mut self) -> bool {
        if self.topic != SettingsTopic::Recording
            || self.focus != SettingsPaneFocus::Content
            || self.selected_recording_widget() != Some(RecordingWidget::IncludeUrlPatterns)
        {
            return false;
        }

        let inserted = self.add_prefilter_pattern_after(None);
        self.select_prefilter_pattern(inserted);
        true
    }

    pub(super) fn handle_prefilter_table_key(
        &mut self,
        key: KeyEvent,
    ) -> super::SettingsPopupAction {
        let EditMode::PrefilterTable { selected_pattern } = self.mode else {
            return super::SettingsPopupAction::None;
        };

        match key.code {
            KeyCode::Esc => self.mode = EditMode::Browse,
            KeyCode::Enter | KeyCode::Char('e') => {
                self.start_prefilter_pattern_editor(selected_pattern);
            }
            KeyCode::Char(' ') => self.toggle_prefilter_pattern(selected_pattern),
            KeyCode::Char('a') => {
                let insert_after = (self.prefilter_pattern_count() > 0).then_some(selected_pattern);
                let inserted = self.add_prefilter_pattern_after(insert_after);
                self.select_prefilter_pattern(inserted);
            }
            KeyCode::Char('d') => self.delete_prefilter_pattern(selected_pattern),
            KeyCode::Char('J') => self.move_prefilter_pattern_down(selected_pattern),
            KeyCode::Char('K') => self.move_prefilter_pattern_up(selected_pattern),
            KeyCode::Char('j') | KeyCode::Down => self.select_prefilter_pattern(
                selected_pattern
                    .saturating_add(1)
                    .min(self.prefilter_pattern_count().saturating_sub(1)),
            ),
            KeyCode::Char('k') | KeyCode::Up => {
                self.select_prefilter_pattern(selected_pattern.saturating_sub(1));
            }
            KeyCode::PageDown => self.select_prefilter_pattern(
                selected_pattern
                    .saturating_add(self.prefilter_table.last_visible_rows())
                    .min(self.prefilter_pattern_count().saturating_sub(1)),
            ),
            KeyCode::PageUp => self.select_prefilter_pattern(
                selected_pattern.saturating_sub(self.prefilter_table.last_visible_rows()),
            ),
            _ => {}
        }

        super::SettingsPopupAction::None
    }

    pub(super) fn handle_prefilter_editor_key(
        &mut self,
        key: KeyEvent,
    ) -> super::SettingsPopupAction {
        let mut apply = None;
        let mut close = None;

        if let EditMode::PrefilterEditor {
            index,
            value,
            cursor,
        } = &mut self.mode
        {
            match key.code {
                KeyCode::Esc => close = Some(*index),
                KeyCode::Enter => apply = Some((*index, value.clone())),
                KeyCode::Backspace | KeyCode::Left | KeyCode::Right | KeyCode::Char(_) => {
                    edit_text_value(key, value, cursor);
                }
                _ => {}
            }
        }

        if let Some((index, value)) = apply {
            if let Some(pattern) = self
                .draft
                .recording
                .prefilter
                .include_url_patterns
                .get_mut(index)
            {
                pattern.pattern = value;
                self.draft.clear_error();
            }
            close = Some(index);
        }
        if let Some(index) = close {
            self.select_prefilter_pattern(index);
        }

        super::SettingsPopupAction::None
    }

    fn start_prefilter_pattern_editor(&mut self, index: usize) {
        let Some(value) = self
            .draft
            .recording
            .prefilter
            .include_url_patterns
            .get(index)
            .map(|pattern| pattern.pattern.clone())
        else {
            return;
        };
        self.mode = EditMode::PrefilterEditor {
            index,
            cursor: value.chars().count(),
            value,
        };
    }

    fn add_prefilter_pattern_after(&mut self, index: Option<usize>) -> usize {
        let patterns = &mut self.draft.recording.prefilter.include_url_patterns;
        let inserted = index
            .map_or(patterns.len(), |index| index.saturating_add(1))
            .min(patterns.len());
        patterns.insert(
            inserted,
            RecordingPrefilterPatternSettings::new("https://example.com/*"),
        );
        self.clamp_prefilter_table_scroll();
        inserted
    }

    fn delete_prefilter_pattern(&mut self, index: usize) {
        let patterns = &mut self.draft.recording.prefilter.include_url_patterns;
        if index < patterns.len() {
            patterns.remove(index);
        }
        self.clamp_prefilter_table_scroll();
        self.select_prefilter_pattern(index);
    }

    fn move_prefilter_pattern_up(&mut self, index: usize) {
        if index > 0 {
            self.draft
                .recording
                .prefilter
                .include_url_patterns
                .swap(index - 1, index);
        }
        self.select_prefilter_pattern(index.saturating_sub(1));
    }

    fn move_prefilter_pattern_down(&mut self, index: usize) {
        let patterns = &mut self.draft.recording.prefilter.include_url_patterns;
        if index.saturating_add(1) < patterns.len() {
            patterns.swap(index, index + 1);
        }
        self.select_prefilter_pattern(index.saturating_add(1));
    }

    pub(crate) fn toggle_prefilter_pattern(&mut self, index: usize) {
        if let Some(pattern) = self
            .draft
            .recording
            .prefilter
            .include_url_patterns
            .get_mut(index)
        {
            pattern.enable = !pattern.enable;
        }
    }

    pub(crate) fn select_prefilter_pattern(&mut self, selected_pattern: usize) {
        self.focus = SettingsPaneFocus::Content;
        self.topic = SettingsTopic::Recording;
        self.selected_row = RecordingWidget::IncludeUrlPatterns.row();
        self.mode = EditMode::PrefilterTable {
            selected_pattern: self.clamp_prefilter_pattern_index(selected_pattern),
        };
        self.ensure_prefilter_pattern_visible();
        self.request_selected_visible();
    }

    pub(crate) fn prefilter_table_is_selected(&self) -> bool {
        self.focus == SettingsPaneFocus::Content
            && self.topic == SettingsTopic::Recording
            && self.selected_recording_widget() == Some(RecordingWidget::IncludeUrlPatterns)
    }

    pub(crate) fn active_prefilter_pattern(&self) -> Option<usize> {
        let active = match self.mode {
            EditMode::PrefilterTable { selected_pattern } => Some(selected_pattern),
            EditMode::PrefilterEditor { index, .. } => Some(index),
            _ => None,
        };
        active.filter(|index| *index < self.prefilter_pattern_count())
    }

    pub(crate) fn prefilter_pattern_edit(&self) -> Option<PrefilterPatternEditState<'_>> {
        match &self.mode {
            EditMode::PrefilterEditor {
                index,
                value,
                cursor,
            } => Some(PrefilterPatternEditState {
                index: *index,
                value,
                cursor: *cursor,
            }),
            _ => None,
        }
    }

    pub(crate) fn prefilter_table_scroll_offset(&self) -> usize {
        self.prefilter_table.scroll_offset()
    }

    pub(crate) fn prefilter_pattern_count(&self) -> usize {
        self.draft.recording.prefilter.include_url_patterns.len()
    }

    pub(crate) fn sync_prefilter_table_view(&mut self, visible_rows: usize) {
        let row_count = self.prefilter_pattern_count();
        self.prefilter_table
            .set_last_visible_rows(row_count, visible_rows);
    }

    pub(crate) fn scroll_prefilter_table_down(&mut self) -> bool {
        self.prefilter_table
            .scroll_down(self.prefilter_pattern_count())
    }

    pub(crate) fn scroll_prefilter_table_up(&mut self) -> bool {
        self.prefilter_table.scroll_up()
    }

    fn clamp_prefilter_pattern_index(&self, index: usize) -> usize {
        index.min(self.prefilter_pattern_count().saturating_sub(1))
    }

    fn ensure_prefilter_pattern_visible(&mut self) {
        let Some(selected) = self.active_prefilter_pattern() else {
            return;
        };
        self.prefilter_table
            .ensure_visible(self.prefilter_pattern_count(), selected);
    }

    fn clamp_prefilter_table_scroll(&mut self) {
        self.prefilter_table.clamp(self.prefilter_pattern_count());
    }
}
