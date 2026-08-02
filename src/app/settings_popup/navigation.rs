use super::field_editor::edit_text_value;
use super::{
    EditMode, ProxyRuleTable, ProxyWidget, RecordingWidget, RuleEditField,
    SETTINGS_BROWSE_KEY_HINTS, SETTINGS_CHECKBOX_BROWSE_KEY_HINTS,
    SETTINGS_EMPTY_RULE_TABLE_KEY_HINTS, SETTINGS_FIELD_BROWSE_KEY_HINTS,
    SETTINGS_FIELD_EDIT_KEY_HINTS, SETTINGS_RULE_TABLE_BROWSE_KEY_HINTS,
    SETTINGS_RULE_TABLE_KEY_HINTS, SETTINGS_SELECT_BROWSE_KEY_HINTS, SETTINGS_SELECT_KEY_HINTS,
    SETTINGS_TOPIC_KEY_HINTS, SettingsKeyHint, SettingsPaneFocus, SettingsPopup,
    SettingsPopupAction, SettingsScrollRequest, SettingsTopic,
};
use crossterm::event::{KeyCode, KeyEvent};
use tui_scrollview::ScrollViewState;

impl SettingsPopup {
    pub(crate) fn key_hints(&self) -> &'static [SettingsKeyHint] {
        match self.mode {
            EditMode::UnsavedConfirm => &[],
            EditMode::Field { .. } => SETTINGS_FIELD_EDIT_KEY_HINTS,
            EditMode::PrefilterEditor { .. } => SETTINGS_FIELD_EDIT_KEY_HINTS,
            EditMode::PrefilterTable { .. } => self.prefilter_table_key_hints(),
            EditMode::Select { .. } => SETTINGS_SELECT_KEY_HINTS,
            EditMode::RuleTable { table, .. } | EditMode::RuleEditor { table, .. } => {
                self.rule_table_key_hints(table)
            }
            EditMode::Browse => self.browse_key_hints(),
        }
    }

    fn browse_key_hints(&self) -> &'static [SettingsKeyHint] {
        if self.focus == SettingsPaneFocus::Topics {
            return SETTINGS_TOPIC_KEY_HINTS;
        }

        match self.topic {
            SettingsTopic::Server | SettingsTopic::Certificate => SETTINGS_FIELD_BROWSE_KEY_HINTS,
            SettingsTopic::Recording => match self.selected_recording_widget() {
                Some(
                    RecordingWidget::StartRecordingOnLaunch | RecordingWidget::PrefilterEnabled,
                ) => SETTINGS_CHECKBOX_BROWSE_KEY_HINTS,
                Some(RecordingWidget::IncludeUrlPatterns) => SETTINGS_RULE_TABLE_BROWSE_KEY_HINTS,
                None => SETTINGS_BROWSE_KEY_HINTS,
            },
            SettingsTopic::Interface => SETTINGS_CHECKBOX_BROWSE_KEY_HINTS,
            SettingsTopic::Proxy => match self.selected_proxy_widget() {
                Some(ProxyWidget::Preset) => SETTINGS_SELECT_BROWSE_KEY_HINTS,
                Some(ProxyWidget::PresetName) => SETTINGS_FIELD_BROWSE_KEY_HINTS,
                Some(
                    ProxyWidget::MappingEnabled
                    | ProxyWidget::MapRemoteEnabled
                    | ProxyWidget::MapLocalEnabled,
                ) => SETTINGS_CHECKBOX_BROWSE_KEY_HINTS,
                Some(ProxyWidget::RemoteRules | ProxyWidget::LocalRules) => {
                    SETTINGS_RULE_TABLE_BROWSE_KEY_HINTS
                }
                None => SETTINGS_BROWSE_KEY_HINTS,
            },
        }
    }

    fn rule_table_key_hints(&self, table: ProxyRuleTable) -> &'static [SettingsKeyHint] {
        if self.rule_count(table) == 0 {
            SETTINGS_EMPTY_RULE_TABLE_KEY_HINTS
        } else {
            SETTINGS_RULE_TABLE_KEY_HINTS
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> SettingsPopupAction {
        match self.mode {
            EditMode::UnsavedConfirm => self.handle_unsaved_confirm_key(key),
            EditMode::Field { .. } => self.handle_field_key(key),
            EditMode::PrefilterTable { .. } => self.handle_prefilter_table_key(key),
            EditMode::PrefilterEditor { .. } => self.handle_prefilter_editor_key(key),
            EditMode::Select { .. } => self.handle_select_key(key),
            EditMode::RuleTable { .. } => self.handle_rule_table_key(key),
            EditMode::RuleEditor { .. } => self.handle_rule_editor_key(key),
            EditMode::Browse => self.handle_browse_key(key),
        }
    }

    fn handle_browse_key(&mut self, key: KeyEvent) -> SettingsPopupAction {
        match key.code {
            KeyCode::Esc => self.handle_escape(),
            KeyCode::Char('s') => self.save_action(),
            KeyCode::Enter => {
                match self.focus {
                    SettingsPaneFocus::Topics => self.focus = SettingsPaneFocus::Content,
                    SettingsPaneFocus::Content => {
                        self.start_selected_edit();
                    }
                }
                SettingsPopupAction::None
            }
            KeyCode::Char(' ') if self.focus == SettingsPaneFocus::Content => {
                self.toggle_selected_checkbox();
                SettingsPopupAction::None
            }
            KeyCode::Char('a') if self.focus == SettingsPaneFocus::Content => {
                if !self.add_prefilter_pattern_from_browse() {
                    self.add_rule_from_key();
                }
                SettingsPopupAction::None
            }
            KeyCode::Char('d') if self.focus == SettingsPaneFocus::Content => {
                self.delete_rule_from_key();
                SettingsPopupAction::None
            }
            KeyCode::Char('J') if self.focus == SettingsPaneFocus::Content => {
                self.move_rule_down_from_key();
                SettingsPopupAction::None
            }
            KeyCode::Char('K') if self.focus == SettingsPaneFocus::Content => {
                self.move_rule_up_from_key();
                SettingsPopupAction::None
            }
            KeyCode::Char('h') | KeyCode::Left => {
                self.focus = SettingsPaneFocus::Topics;
                SettingsPopupAction::None
            }
            KeyCode::Char('l') | KeyCode::Right => {
                self.focus = SettingsPaneFocus::Content;
                self.request_selected_visible();
                SettingsPopupAction::None
            }
            KeyCode::Char('j') | KeyCode::Down => {
                self.select_next();
                SettingsPopupAction::None
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.select_previous();
                SettingsPopupAction::None
            }
            KeyCode::PageDown => {
                self.scroll.scroll_page_down();
                SettingsPopupAction::None
            }
            KeyCode::PageUp => {
                self.scroll.scroll_page_up();
                SettingsPopupAction::None
            }
            _ => SettingsPopupAction::None,
        }
    }

    fn handle_unsaved_confirm_key(&mut self, key: KeyEvent) -> SettingsPopupAction {
        match key.code {
            KeyCode::Enter => self.save_action(),
            KeyCode::Esc => SettingsPopupAction::Close,
            _ => SettingsPopupAction::None,
        }
    }

    fn handle_rule_table_key(&mut self, key: KeyEvent) -> SettingsPopupAction {
        let (table, selected_rule) = match &self.mode {
            EditMode::RuleTable {
                table,
                selected_rule,
            } => (*table, *selected_rule),
            _ => return SettingsPopupAction::None,
        };

        match key.code {
            KeyCode::Esc => {
                self.mode = EditMode::Browse;
            }
            KeyCode::Enter | KeyCode::Char('e') => {
                self.start_rule_editor(table, selected_rule);
            }
            KeyCode::Char(' ') => {
                self.toggle_rule(table, selected_rule);
            }
            KeyCode::Char('a') => {
                let insert_after = (self.rule_count(table) > 0).then_some(selected_rule);
                let inserted = self.add_rule_after(table, insert_after);
                self.select_rule(table, inserted);
            }
            KeyCode::Char('d') => {
                self.delete_rule(table, selected_rule);
            }
            KeyCode::Char('J') => {
                self.move_rule_down(table, selected_rule);
            }
            KeyCode::Char('K') => {
                self.move_rule_up(table, selected_rule);
            }
            KeyCode::Char('j') | KeyCode::Down => {
                self.select_rule(
                    table,
                    selected_rule
                        .saturating_add(1)
                        .min(self.rule_count(table).saturating_sub(1)),
                );
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.select_rule(table, selected_rule.saturating_sub(1));
            }
            KeyCode::PageDown => {
                self.select_rule(
                    table,
                    selected_rule
                        .saturating_add(self.rule_table_page_size(table))
                        .min(self.rule_count(table).saturating_sub(1)),
                );
            }
            KeyCode::PageUp => {
                self.select_rule(
                    table,
                    selected_rule.saturating_sub(self.rule_table_page_size(table)),
                );
            }
            _ => {}
        }

        SettingsPopupAction::None
    }

    fn handle_rule_editor_key(&mut self, key: KeyEvent) -> SettingsPopupAction {
        match key.code {
            KeyCode::Esc => {
                if let EditMode::RuleEditor { table, index, .. } = &self.mode {
                    let table = *table;
                    let index = *index;
                    self.mode = EditMode::RuleTable {
                        table,
                        selected_rule: self.clamp_rule_index(table, index),
                    };
                }
            }
            KeyCode::Enter => {
                let EditMode::RuleEditor {
                    table,
                    index,
                    from,
                    to,
                    ..
                } = &self.mode
                else {
                    return SettingsPopupAction::None;
                };
                let table = *table;
                let index = *index;
                let from = from.clone();
                let to = to.clone();
                self.apply_rule_editor_value(table, index, from, to);
                self.mode = EditMode::RuleTable {
                    table,
                    selected_rule: self.clamp_rule_index(table, index),
                };
            }
            KeyCode::Tab | KeyCode::BackTab | KeyCode::Down | KeyCode::Up => {
                self.switch_rule_editor_field();
            }
            KeyCode::Backspace | KeyCode::Left | KeyCode::Right | KeyCode::Char(_) => {
                if let EditMode::RuleEditor {
                    from,
                    to,
                    active_field,
                    from_cursor,
                    to_cursor,
                    ..
                } = &mut self.mode
                {
                    match active_field {
                        RuleEditField::From => edit_text_value(key, from, from_cursor),
                        RuleEditField::To => edit_text_value(key, to, to_cursor),
                    }
                }
            }
            _ => {}
        }

        SettingsPopupAction::None
    }

    fn handle_escape(&mut self) -> SettingsPopupAction {
        match &self.mode {
            EditMode::Browse if self.is_dirty() => {
                self.mode = EditMode::UnsavedConfirm;
                SettingsPopupAction::None
            }
            EditMode::Browse => SettingsPopupAction::Close,
            EditMode::RuleEditor { table, index, .. } => {
                let table = *table;
                let index = *index;
                self.mode = EditMode::RuleTable {
                    table,
                    selected_rule: self.clamp_rule_index(table, index),
                };
                SettingsPopupAction::None
            }
            _ => {
                self.mode = EditMode::Browse;
                SettingsPopupAction::None
            }
        }
    }

    fn save_action(&mut self) -> SettingsPopupAction {
        match self.validate() {
            Ok(()) => SettingsPopupAction::Save(self.draft.snapshot()),
            Err(error) => {
                self.mode = EditMode::Browse;
                self.draft.set_error(error);
                SettingsPopupAction::None
            }
        }
    }

    fn select_next(&mut self) {
        if self.focus == SettingsPaneFocus::Topics {
            let topics = SettingsTopic::all();
            let index = topics
                .iter()
                .position(|topic| *topic == self.topic)
                .unwrap_or(0);
            self.select_topic(topics[(index + 1) % topics.len()]);
        } else {
            self.clamp_selected_row();
            self.selected_row = self
                .selected_row
                .saturating_add(1)
                .min(self.row_count().saturating_sub(1));
            self.request_selected_visible();
        }
    }

    fn select_previous(&mut self) {
        if self.focus == SettingsPaneFocus::Topics {
            let topics = SettingsTopic::all();
            let index = topics
                .iter()
                .position(|topic| *topic == self.topic)
                .unwrap_or(0);
            self.select_topic(topics[(index + topics.len() - 1) % topics.len()]);
        } else {
            self.clamp_selected_row();
            self.selected_row = self.selected_row.saturating_sub(1);
            self.request_selected_visible();
        }
    }

    pub(super) fn select_topic(&mut self, topic: SettingsTopic) {
        self.topic = topic;
        self.selected_row = 0;
        self.scroll = ScrollViewState::default();
        self.scroll_request = None;
        self.reset_table_scrolls();
        self.mode = EditMode::Browse;
        self.draft.clear_error();
        self.field_hint = None;
        self.clamp_selected_row();
    }

    pub(super) fn clamp_selected_row(&mut self) {
        self.selected_row = self.selected_row.min(self.row_count().saturating_sub(1));
    }

    pub(super) fn request_selected_visible(&mut self) {
        if self.focus == SettingsPaneFocus::Content {
            self.scroll_request = Some(SettingsScrollRequest::EnsureSelectedVisible);
        }
    }

    pub(super) fn select_rule(&mut self, table: ProxyRuleTable, selected_rule: usize) {
        self.mode = EditMode::RuleTable {
            table,
            selected_rule: self.clamp_rule_index(table, selected_rule),
        };
        self.ensure_rule_visible(table);
        self.request_selected_visible();
    }

    fn toggle_selected_checkbox(&mut self) {
        self.clamp_selected_row();
        match (self.topic, self.selected_row) {
            (SettingsTopic::Recording, _) => self.toggle_selected_recording_checkbox(),
            (SettingsTopic::Interface, 0) => {
                self.draft.ui.request_list.auto_expand = !self.draft.ui.request_list.auto_expand;
            }
            (SettingsTopic::Proxy, _) => self.toggle_selected_proxy_checkbox(),
            _ => {}
        }
    }
}
