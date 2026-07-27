use crate::settings::{
    AppSettings, ProxyMapLocalRule, ProxyMapRemoteRule, ProxyPresetSettings, ProxySettings,
};
use crate::{
    mapping::validate_proxy_settings,
    select::{SelectCommit, SelectItem, SelectItemRole, SelectOutcome, SelectState},
};
use crossterm::event::{KeyCode, KeyEvent};
use tui_scrollview::ScrollViewState;

use super::settings_draft::SettingsDraft;

mod recording;
pub(crate) use recording::{PrefilterPatternEditState, RecordingWidget};

pub(crate) const PROXY_PRESET_SELECT_MAX_VISIBLE_ITEMS: usize = 6;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsTopic {
    Server,
    Certificate,
    Recording,
    Interface,
    Proxy,
}

impl SettingsTopic {
    const ALL: [Self; 5] = [
        Self::Server,
        Self::Certificate,
        Self::Recording,
        Self::Interface,
        Self::Proxy,
    ];

    pub fn all() -> &'static [Self] {
        &Self::ALL
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::Server => "Server",
            Self::Certificate => "Certificate",
            Self::Recording => "Recording",
            Self::Interface => "Interface",
            Self::Proxy => "Proxy",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsPaneFocus {
    Topics,
    Content,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SettingsScrollRequest {
    EnsureSelectedVisible,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SettingsTableState {
    scroll_offset: usize,
    last_visible_rows: usize,
}

impl Default for SettingsTableState {
    fn default() -> Self {
        Self {
            scroll_offset: 0,
            last_visible_rows: 1,
        }
    }
}

impl SettingsTableState {
    fn scroll_offset(self) -> usize {
        self.scroll_offset
    }

    fn last_visible_rows(self) -> usize {
        self.last_visible_rows.max(1)
    }

    fn reset(&mut self) {
        self.scroll_offset = 0;
        self.last_visible_rows = 1;
    }

    fn set_last_visible_rows(&mut self, row_count: usize, visible_rows: usize) {
        self.last_visible_rows = visible_rows.max(1);
        self.clamp(row_count);
    }

    fn clamp(&mut self, row_count: usize) {
        self.scroll_offset = self.scroll_offset.min(self.max_scroll_offset(row_count));
    }

    fn ensure_visible(&mut self, row_count: usize, selected_row: usize) {
        if row_count == 0 {
            self.scroll_offset = 0;
            return;
        }

        let visible_rows = self.last_visible_rows();
        if selected_row < self.scroll_offset {
            self.scroll_offset = selected_row;
        } else if selected_row >= self.scroll_offset.saturating_add(visible_rows) {
            self.scroll_offset = selected_row.saturating_add(1).saturating_sub(visible_rows);
        }
        self.clamp(row_count);
    }

    fn scroll_down(&mut self, row_count: usize) -> bool {
        let next = self
            .scroll_offset
            .saturating_add(1)
            .min(self.max_scroll_offset(row_count));
        let changed = next != self.scroll_offset;
        self.scroll_offset = next;
        changed
    }

    fn scroll_up(&mut self) -> bool {
        let next = self.scroll_offset.saturating_sub(1);
        let changed = next != self.scroll_offset;
        self.scroll_offset = next;
        changed
    }

    fn max_scroll_offset(self, row_count: usize) -> usize {
        row_count.saturating_sub(self.last_visible_rows())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldEditKind {
    ServerPort,
    CertificateStoreDir,
    CertificatePemFilename,
    ProxyPresetName,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct FieldEditState<'a> {
    pub value: &'a str,
    pub cursor: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ProxyRuleTable {
    Remote,
    Local,
}

impl ProxyRuleTable {
    pub(crate) fn title(self) -> &'static str {
        match self {
            Self::Remote => "Map Remote Rules",
            Self::Local => "Map Local Rules",
        }
    }

    fn editor_title(self) -> &'static str {
        match self {
            Self::Remote => "Edit Map Remote Rule",
            Self::Local => "Edit Map Local Rule",
        }
    }
}

enum EditableRulesMut<'a> {
    Remote(&'a mut Vec<ProxyMapRemoteRule>),
    Local(&'a mut Vec<ProxyMapLocalRule>),
}

impl EditableRulesMut<'_> {
    fn insert_default(&mut self, index: usize) {
        match self {
            Self::Remote(rules) => rules.insert(
                index.min(rules.len()),
                ProxyMapRemoteRule {
                    from: "https://example.com".to_string(),
                    to: "http://localhost:3000".to_string(),
                    enable: true,
                },
            ),
            Self::Local(rules) => rules.insert(
                index.min(rules.len()),
                ProxyMapLocalRule {
                    from: "https://example.com".to_string(),
                    to: "~/mock-response.json".to_string(),
                    enable: true,
                },
            ),
        }
    }

    fn remove(&mut self, index: usize) {
        match self {
            Self::Remote(rules) if index < rules.len() => {
                rules.remove(index);
            }
            Self::Local(rules) if index < rules.len() => {
                rules.remove(index);
            }
            Self::Remote(_) | Self::Local(_) => {}
        }
    }

    fn swap(&mut self, first: usize, second: usize) {
        match self {
            Self::Remote(rules) if first < rules.len() && second < rules.len() => {
                rules.swap(first, second);
            }
            Self::Local(rules) if first < rules.len() && second < rules.len() => {
                rules.swap(first, second);
            }
            Self::Remote(_) | Self::Local(_) => {}
        }
    }

    fn toggle(&mut self, index: usize) {
        match self {
            Self::Remote(rules) => {
                if let Some(rule) = rules.get_mut(index) {
                    rule.enable = !rule.enable;
                }
            }
            Self::Local(rules) => {
                if let Some(rule) = rules.get_mut(index) {
                    rule.enable = !rule.enable;
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ProxyWidget {
    Preset,
    PresetName,
    MappingEnabled,
    MapRemoteEnabled,
    RemoteRules,
    MapLocalEnabled,
    LocalRules,
}

impl ProxyWidget {
    const ACTIVE_PRESET: [Self; 7] = [
        Self::Preset,
        Self::PresetName,
        Self::MappingEnabled,
        Self::MapRemoteEnabled,
        Self::RemoteRules,
        Self::MapLocalEnabled,
        Self::LocalRules,
    ];
    const PRESET_ONLY: [Self; 1] = [Self::Preset];
    const EMPTY: [Self; 0] = [];

    fn visible_for(settings: &AppSettings) -> &'static [Self] {
        let Some(proxy) = settings.proxy.as_ref() else {
            return &Self::EMPTY;
        };
        if proxy.presets.is_empty() {
            return &Self::EMPTY;
        }
        let active = proxy.active_preset.as_deref();
        if active.is_some_and(|active| proxy.presets.iter().any(|preset| preset.name == active)) {
            &Self::ACTIVE_PRESET
        } else {
            &Self::PRESET_ONLY
        }
    }

    fn row_in(self, widgets: &[Self]) -> Option<usize> {
        widgets.iter().position(|widget| *widget == self)
    }

    fn from_row(widgets: &[Self], row: usize) -> Option<Self> {
        widgets.get(row).copied()
    }

    fn table(self) -> Option<ProxyRuleTable> {
        match self {
            Self::Preset
            | Self::PresetName
            | Self::MappingEnabled
            | Self::MapRemoteEnabled
            | Self::MapLocalEnabled => None,
            Self::RemoteRules => Some(ProxyRuleTable::Remote),
            Self::LocalRules => Some(ProxyRuleTable::Local),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SelectTarget {
    ProxyPreset,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SettingsSelectId {
    ProxyPreset(ProxyPresetChoice),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ProxyPresetChoice {
    Existing(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RuleEditField {
    From,
    To,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RuleEditorState<'a> {
    pub table: ProxyRuleTable,
    pub title: &'static str,
    pub from: &'a str,
    pub to: &'a str,
    pub active_field: RuleEditField,
    pub from_cursor: usize,
    pub to_cursor: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DialogActionKind {
    Save,
    Discard,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DialogAction {
    pub label: &'static str,
    pub key_hint: &'static str,
    pub kind: DialogActionKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActionDialog {
    pub title: &'static str,
    pub message_lines: Vec<&'static str>,
    pub actions: Vec<DialogAction>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum SettingsPopupAction {
    None,
    Save(AppSettings),
    Close,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SettingsKeyHint {
    pub(crate) label: &'static str,
    pub(crate) key: &'static str,
}

const SETTINGS_TOPIC_KEY_HINTS: &[SettingsKeyHint] = &[
    SettingsKeyHint {
        label: "Save",
        key: "s",
    },
    SettingsKeyHint {
        label: "Close",
        key: "Esc",
    },
    SettingsKeyHint {
        label: "Pane",
        key: "h/l",
    },
    SettingsKeyHint {
        label: "Move",
        key: "j/k",
    },
    SettingsKeyHint {
        label: "Open",
        key: "Enter",
    },
];

const SETTINGS_BROWSE_KEY_HINTS: &[SettingsKeyHint] = &[
    SettingsKeyHint {
        label: "Save",
        key: "s",
    },
    SettingsKeyHint {
        label: "Close",
        key: "Esc",
    },
    SettingsKeyHint {
        label: "Pane",
        key: "h/l",
    },
];

const SETTINGS_FIELD_BROWSE_KEY_HINTS: &[SettingsKeyHint] = &[
    SettingsKeyHint {
        label: "Save",
        key: "s",
    },
    SettingsKeyHint {
        label: "Close",
        key: "Esc",
    },
    SettingsKeyHint {
        label: "Pane",
        key: "h/l",
    },
    SettingsKeyHint {
        label: "Move",
        key: "j/k",
    },
    SettingsKeyHint {
        label: "Edit",
        key: "Enter",
    },
];

const SETTINGS_SELECT_BROWSE_KEY_HINTS: &[SettingsKeyHint] = &[
    SettingsKeyHint {
        label: "Save",
        key: "s",
    },
    SettingsKeyHint {
        label: "Close",
        key: "Esc",
    },
    SettingsKeyHint {
        label: "Pane",
        key: "h/l",
    },
    SettingsKeyHint {
        label: "Move",
        key: "j/k",
    },
    SettingsKeyHint {
        label: "Choose",
        key: "Enter",
    },
];

const SETTINGS_CHECKBOX_BROWSE_KEY_HINTS: &[SettingsKeyHint] = &[
    SettingsKeyHint {
        label: "Save",
        key: "s",
    },
    SettingsKeyHint {
        label: "Close",
        key: "Esc",
    },
    SettingsKeyHint {
        label: "Pane",
        key: "h/l",
    },
    SettingsKeyHint {
        label: "Move",
        key: "j/k",
    },
    SettingsKeyHint {
        label: "Toggle",
        key: "Space",
    },
];

const SETTINGS_RULE_TABLE_BROWSE_KEY_HINTS: &[SettingsKeyHint] = &[
    SettingsKeyHint {
        label: "Save",
        key: "s",
    },
    SettingsKeyHint {
        label: "Close",
        key: "Esc",
    },
    SettingsKeyHint {
        label: "Move",
        key: "j/k",
    },
    SettingsKeyHint {
        label: "Table",
        key: "Enter",
    },
    SettingsKeyHint {
        label: "Add",
        key: "a",
    },
];

const SETTINGS_FIELD_EDIT_KEY_HINTS: &[SettingsKeyHint] = &[
    SettingsKeyHint {
        label: "Apply",
        key: "Enter",
    },
    SettingsKeyHint {
        label: "Cancel",
        key: "Esc",
    },
    SettingsKeyHint {
        label: "Cursor",
        key: "Left/Right",
    },
    SettingsKeyHint {
        label: "Type",
        key: "text",
    },
];

const SETTINGS_SELECT_KEY_HINTS: &[SettingsKeyHint] = &[
    SettingsKeyHint {
        label: "Choose",
        key: "Enter",
    },
    SettingsKeyHint {
        label: "Cancel",
        key: "Esc",
    },
    SettingsKeyHint {
        label: "Move",
        key: "Up/Down",
    },
    SettingsKeyHint {
        label: "Page",
        key: "PgUp/PgDn",
    },
    SettingsKeyHint {
        label: "Filter",
        key: "type",
    },
];

const SETTINGS_RULE_TABLE_KEY_HINTS: &[SettingsKeyHint] = &[
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

const SETTINGS_EMPTY_RULE_TABLE_KEY_HINTS: &[SettingsKeyHint] = &[
    SettingsKeyHint {
        label: "Back",
        key: "Esc",
    },
    SettingsKeyHint {
        label: "Add",
        key: "a",
    },
];

pub(crate) const RULE_EDITOR_KEY_HINTS: &[SettingsKeyHint] = &[
    SettingsKeyHint {
        label: "Apply",
        key: "Enter",
    },
    SettingsKeyHint {
        label: "Cancel",
        key: "Esc",
    },
    SettingsKeyHint {
        label: "Switch",
        key: "Tab",
    },
    SettingsKeyHint {
        label: "Cursor",
        key: "Left/Right",
    },
    SettingsKeyHint {
        label: "Type",
        key: "text",
    },
];

#[derive(Clone, Debug, PartialEq, Eq)]
enum EditMode {
    Browse,
    Select {
        target: SelectTarget,
        state: SelectState,
    },
    RuleTable {
        table: ProxyRuleTable,
        selected_rule: usize,
    },
    RuleEditor {
        table: ProxyRuleTable,
        index: usize,
        from: String,
        to: String,
        active_field: RuleEditField,
        from_cursor: usize,
        to_cursor: usize,
    },
    PrefilterTable {
        selected_pattern: usize,
    },
    PrefilterEditor {
        index: usize,
        value: String,
        cursor: usize,
    },
    Field {
        kind: FieldEditKind,
        value: String,
        cursor: usize,
    },
    UnsavedConfirm,
}

enum SelectCommitEffect {
    Close,
    KeepOpen,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FieldApplyOutcome {
    CloseEditor,
    KeepEditing,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct FieldEditHint {
    kind: FieldEditKind,
    message: &'static str,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg(test)]
pub(crate) enum ProxyRow {
    Preset,
    PresetName,
    MappingEnabled,
    MapRemoteEnabled,
    RemoteHeader,
    RemoteRule(usize),
    MapLocalEnabled,
    LocalHeader,
    LocalRule(usize),
}

pub struct SettingsPopup {
    pub visible: bool,
    pub focus: SettingsPaneFocus,
    pub topic: SettingsTopic,
    pub selected_row: usize,
    pub scroll: ScrollViewState,
    draft: SettingsDraft,
    mode: EditMode,
    field_hint: Option<FieldEditHint>,
    scroll_request: Option<SettingsScrollRequest>,
    prefilter_table: SettingsTableState,
    remote_rule_table: SettingsTableState,
    local_rule_table: SettingsTableState,
}

impl SettingsPopup {
    pub fn new() -> Self {
        let settings = AppSettings::default();
        Self {
            visible: false,
            focus: SettingsPaneFocus::Content,
            topic: SettingsTopic::Server,
            selected_row: 0,
            scroll: ScrollViewState::default(),
            draft: SettingsDraft::new(settings),
            mode: EditMode::Browse,
            field_hint: None,
            scroll_request: None,
            prefilter_table: SettingsTableState::default(),
            remote_rule_table: SettingsTableState::default(),
            local_rule_table: SettingsTableState::default(),
        }
    }

    pub fn open(&mut self, settings: AppSettings) {
        self.visible = true;
        self.focus = SettingsPaneFocus::Topics;
        self.topic = SettingsTopic::Server;
        self.selected_row = 0;
        self.scroll = ScrollViewState::default();
        self.draft.replace(settings);
        self.mode = EditMode::Browse;
        self.field_hint = None;
        self.scroll_request = None;
        self.reset_table_scrolls();
    }

    pub fn close(&mut self) {
        self.visible = false;
        self.draft.reset();
        self.mode = EditMode::Browse;
        self.field_hint = None;
        self.scroll_request = None;
        self.reset_table_scrolls();
    }

    pub fn draft(&self) -> &AppSettings {
        &self.draft
    }

    pub fn error(&self) -> Option<&str> {
        self.draft.error()
    }

    pub(crate) fn field_hint(&self, kind: FieldEditKind) -> Option<&str> {
        self.field_hint
            .as_ref()
            .filter(|hint| hint.kind == kind)
            .map(|hint| hint.message)
    }

    pub fn is_dirty(&self) -> bool {
        self.draft.is_dirty()
    }

    pub fn is_confirming_unsaved(&self) -> bool {
        matches!(self.mode, EditMode::UnsavedConfirm)
    }

    pub fn unsaved_dialog(&self) -> Option<ActionDialog> {
        self.is_confirming_unsaved().then(|| ActionDialog {
            title: "Unsaved Settings",
            message_lines: vec!["You have unsaved setting changes."],
            actions: vec![
                DialogAction {
                    label: "Save",
                    key_hint: "Enter",
                    kind: DialogActionKind::Save,
                },
                DialogAction {
                    label: "Discard",
                    key_hint: "Esc",
                    kind: DialogActionKind::Discard,
                },
            ],
        })
    }

    pub fn mark_saved(&mut self) {
        self.close();
    }

    pub fn mark_save_failed(&mut self, message: String) {
        self.mode = EditMode::Browse;
        self.draft.set_error(message);
        self.visible = true;
    }

    pub fn validate(&self) -> Result<(), String> {
        validate_settings(&self.draft)
    }

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

    fn select_topic(&mut self, topic: SettingsTopic) {
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

    fn clamp_selected_row(&mut self) {
        self.selected_row = self.selected_row.min(self.row_count().saturating_sub(1));
    }

    fn request_selected_visible(&mut self) {
        if self.focus == SettingsPaneFocus::Content {
            self.scroll_request = Some(SettingsScrollRequest::EnsureSelectedVisible);
        }
    }

    fn select_rule(&mut self, table: ProxyRuleTable, selected_rule: usize) {
        self.mode = EditMode::RuleTable {
            table,
            selected_rule: self.clamp_rule_index(table, selected_rule),
        };
        self.ensure_rule_visible(table);
        self.request_selected_visible();
    }

    fn start_field_edit(&mut self, kind: FieldEditKind, value: String) {
        self.clear_field_hint(kind);
        self.mode = EditMode::Field {
            kind,
            cursor: value.chars().count(),
            value,
        };
    }

    fn handle_field_key(&mut self, key: KeyEvent) -> SettingsPopupAction {
        let mut apply_value = None;
        let mut clear_hint = None;
        let mut close = false;

        match &mut self.mode {
            EditMode::Field {
                kind,
                value,
                cursor,
                ..
            } => match key.code {
                KeyCode::Esc => {
                    clear_hint = Some(*kind);
                    close = true;
                }
                KeyCode::Enter => {
                    apply_value = Some((*kind, value.clone()));
                }
                KeyCode::Backspace | KeyCode::Char(_) => {
                    edit_text_value(key, value, cursor);
                    clear_hint = Some(*kind);
                }
                KeyCode::Left | KeyCode::Right => {
                    edit_text_value(key, value, cursor);
                }
                _ => {}
            },
            _ => return SettingsPopupAction::None,
        }

        if let Some(kind) = clear_hint {
            self.clear_field_hint(kind);
        }
        if let Some((kind, value)) = apply_value {
            match self.apply_field_value(kind, value) {
                FieldApplyOutcome::CloseEditor => close = true,
                FieldApplyOutcome::KeepEditing => {}
            }
        }
        if close {
            self.mode = EditMode::Browse;
        }

        SettingsPopupAction::None
    }

    fn start_selected_edit(&mut self) {
        self.clamp_selected_row();
        match (self.topic, self.selected_row) {
            (SettingsTopic::Server, 0) => {
                self.start_field_edit(
                    FieldEditKind::ServerPort,
                    self.draft.server.port.to_string(),
                );
            }
            (SettingsTopic::Certificate, 0) => {
                self.start_field_edit(
                    FieldEditKind::CertificateStoreDir,
                    self.draft.certificate.store_dir.clone(),
                );
            }
            (SettingsTopic::Certificate, 1) => {
                self.start_field_edit(
                    FieldEditKind::CertificatePemFilename,
                    self.draft.certificate.pem_filename.clone(),
                );
            }
            (SettingsTopic::Recording, _) => self.start_recording_selected_edit(),
            (SettingsTopic::Proxy, _) => match self.selected_proxy_widget() {
                Some(ProxyWidget::Preset) => self.start_proxy_preset_select(),
                Some(ProxyWidget::PresetName) => {
                    if let Some(name) = active_preset(&self.draft).map(|preset| preset.name.clone())
                    {
                        self.start_field_edit(FieldEditKind::ProxyPresetName, name);
                    }
                }
                Some(ProxyWidget::RemoteRules | ProxyWidget::LocalRules) => {
                    if let Some(table) = self.selected_proxy_table() {
                        self.mode = EditMode::RuleTable {
                            table,
                            selected_rule: 0,
                        };
                        self.ensure_rule_visible(table);
                        self.request_selected_visible();
                    }
                }
                Some(
                    ProxyWidget::MappingEnabled
                    | ProxyWidget::MapRemoteEnabled
                    | ProxyWidget::MapLocalEnabled,
                )
                | None => {}
            },
            _ => {}
        }
    }

    fn handle_select_key(&mut self, key: KeyEvent) -> SettingsPopupAction {
        let EditMode::Select { target, mut state } =
            std::mem::replace(&mut self.mode, EditMode::Browse)
        else {
            return SettingsPopupAction::None;
        };

        let items = self.select_items(target);
        let outcome = state.handle_key(key, &items, PROXY_PRESET_SELECT_MAX_VISIBLE_ITEMS);
        drop(items);

        match outcome {
            SelectOutcome::Committed(commit) => {
                let effect = self.apply_select_commit(target, commit);
                self.apply_select_commit_effect(effect, target, state);
            }
            SelectOutcome::Closed => {
                self.mode = EditMode::Browse;
            }
            SelectOutcome::None | SelectOutcome::Opened => {
                self.mode = EditMode::Select { target, state };
            }
        }

        SettingsPopupAction::None
    }

    pub(crate) fn start_proxy_preset_select(&mut self) {
        self.start_select(SelectTarget::ProxyPreset);
    }

    pub(crate) fn start_select(&mut self, target: SelectTarget) {
        self.focus_select_target(target);
        let items = self.select_items(target);
        if items.is_empty() {
            self.mode = EditMode::Browse;
            return;
        }
        let selected = self.active_select_id(target);
        let mut state = SelectState::new();
        state.open_with_selected(
            &items,
            selected.as_ref(),
            PROXY_PRESET_SELECT_MAX_VISIBLE_ITEMS,
        );
        drop(items);
        self.mode = EditMode::Select { target, state };
    }

    fn focus_select_target(&mut self, target: SelectTarget) {
        match target {
            SelectTarget::ProxyPreset => {
                self.focus = SettingsPaneFocus::Content;
                self.topic = SettingsTopic::Proxy;
                self.selected_row = self
                    .proxy_widget_row(ProxyWidget::Preset)
                    .unwrap_or_default();
                self.request_selected_visible();
            }
        }
    }

    pub(crate) fn take_scroll_request(&mut self) -> Option<SettingsScrollRequest> {
        self.scroll_request.take()
    }

    pub(crate) fn active_select_target(&self) -> Option<SelectTarget> {
        match &self.mode {
            EditMode::Select { target, .. } => Some(*target),
            _ => None,
        }
    }

    pub(crate) fn select_state(&self, target: SelectTarget) -> Option<&SelectState> {
        match &self.mode {
            EditMode::Select {
                target: active,
                state,
            } if *active == target => Some(state),
            _ => None,
        }
    }

    pub(crate) fn select_is_selected(&self, target: SelectTarget) -> bool {
        match target {
            SelectTarget::ProxyPreset => self.proxy_preset_is_selected(),
        }
    }

    pub(crate) fn select_selected_label(&self, target: SelectTarget) -> &str {
        match target {
            SelectTarget::ProxyPreset => self.proxy_preset_label(),
        }
    }

    pub(crate) fn close_active_select(&mut self) -> bool {
        if matches!(self.mode, EditMode::Select { .. }) {
            self.mode = EditMode::Browse;
            return true;
        }
        false
    }

    pub(crate) fn commit_active_select_filtered_index(&mut self, filtered_index: usize) -> bool {
        let EditMode::Select { target, mut state } =
            std::mem::replace(&mut self.mode, EditMode::Browse)
        else {
            return false;
        };

        let items = self.select_items(target);
        let commit = state.commit_filtered_index(&items, filtered_index);
        drop(items);

        if let Some(commit) = commit {
            let effect = self.apply_select_commit(target, commit);
            self.apply_select_commit_effect(effect, target, state);
            true
        } else {
            self.mode = EditMode::Select { target, state };
            false
        }
    }

    pub(crate) fn scroll_active_select_down(&mut self) -> bool {
        self.scroll_active_select(true)
    }

    pub(crate) fn scroll_active_select_up(&mut self) -> bool {
        self.scroll_active_select(false)
    }

    fn scroll_active_select(&mut self, down: bool) -> bool {
        let EditMode::Select { target, mut state } =
            std::mem::replace(&mut self.mode, EditMode::Browse)
        else {
            return false;
        };

        let items = self.select_items(target);
        if down {
            state.scroll_down(&items, PROXY_PRESET_SELECT_MAX_VISIBLE_ITEMS);
        } else {
            state.scroll_up(&items, PROXY_PRESET_SELECT_MAX_VISIBLE_ITEMS);
        }
        drop(items);
        self.mode = EditMode::Select { target, state };
        true
    }

    fn apply_field_value(&mut self, kind: FieldEditKind, value: String) -> FieldApplyOutcome {
        match kind {
            FieldEditKind::ServerPort => match value.parse::<u16>() {
                Ok(port) if port > 0 => {
                    self.draft.server.port = port;
                    self.draft.clear_error();
                    self.clear_field_hint(kind);
                }
                _ => {
                    self.draft
                        .set_error("server.port must be between 1 and 65535");
                }
            },
            FieldEditKind::CertificateStoreDir => {
                self.draft.certificate.store_dir = value;
                self.draft.clear_error();
                self.clear_field_hint(kind);
            }
            FieldEditKind::CertificatePemFilename => {
                self.draft.certificate.pem_filename = value;
                self.draft.clear_error();
                self.clear_field_hint(kind);
            }
            FieldEditKind::ProxyPresetName => {
                return self.apply_proxy_preset_name(value);
            }
        }
        FieldApplyOutcome::CloseEditor
    }

    pub(crate) fn active_field_edit(&self, kind: FieldEditKind) -> Option<FieldEditState<'_>> {
        match &self.mode {
            EditMode::Field {
                kind: active_field,
                value,
                cursor,
                ..
            } if *active_field == kind => Some(FieldEditState {
                value,
                cursor: *cursor,
            }),
            _ => None,
        }
    }

    pub(crate) fn proxy_preset_label(&self) -> &str {
        active_preset(&self.draft).map_or("(none)", |preset| preset.name.as_str())
    }

    pub(crate) fn proxy_preset_is_selected(&self) -> bool {
        self.focus == SettingsPaneFocus::Content
            && self.topic == SettingsTopic::Proxy
            && self.selected_proxy_widget() == Some(ProxyWidget::Preset)
    }

    fn proxy_preset_select_items(&self) -> Vec<SelectItem<'_, SettingsSelectId>> {
        self.draft
            .proxy
            .as_ref()
            .map(|proxy| {
                proxy
                    .presets
                    .iter()
                    .enumerate()
                    .map(|(index, preset)| {
                        SelectItem::value(
                            SettingsSelectId::ProxyPreset(ProxyPresetChoice::Existing(index)),
                            preset.name.as_str(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    pub(crate) fn select_items(
        &self,
        target: SelectTarget,
    ) -> Vec<SelectItem<'_, SettingsSelectId>> {
        match target {
            SelectTarget::ProxyPreset => self.proxy_preset_select_items(),
        }
    }

    fn active_select_id(&self, target: SelectTarget) -> Option<SettingsSelectId> {
        match target {
            SelectTarget::ProxyPreset => self.active_proxy_preset_select_id(),
        }
    }

    fn active_proxy_preset_select_id(&self) -> Option<SettingsSelectId> {
        let proxy = self.draft.proxy.as_ref()?;
        let active = proxy.active_preset.as_deref()?;
        proxy
            .presets
            .iter()
            .position(|preset| preset.name == active)
            .map(|index| SettingsSelectId::ProxyPreset(ProxyPresetChoice::Existing(index)))
    }

    fn apply_select_commit(
        &mut self,
        target: SelectTarget,
        commit: SelectCommit<SettingsSelectId>,
    ) -> SelectCommitEffect {
        match (target, commit.id, commit.role) {
            (
                SelectTarget::ProxyPreset,
                SettingsSelectId::ProxyPreset(ProxyPresetChoice::Existing(index)),
                SelectItemRole::Value,
            ) => {
                self.apply_proxy_preset_selection(index);
                SelectCommitEffect::Close
            }
            (
                SelectTarget::ProxyPreset,
                SettingsSelectId::ProxyPreset(_),
                SelectItemRole::Action,
            ) => {
                self.draft.clear_error();
                SelectCommitEffect::KeepOpen
            }
        }
    }

    fn apply_select_commit_effect(
        &mut self,
        effect: SelectCommitEffect,
        target: SelectTarget,
        state: SelectState,
    ) {
        match effect {
            SelectCommitEffect::Close => {
                self.mode = EditMode::Browse;
            }
            SelectCommitEffect::KeepOpen => {
                self.mode = EditMode::Select { target, state };
            }
        }
    }

    fn apply_proxy_preset_selection(&mut self, index: usize) {
        if let Some(proxy) = &mut self.draft.proxy
            && let Some(name) = proxy.presets.get(index).map(|preset| preset.name.clone())
        {
            proxy.active_preset = Some(name);
            self.draft.clear_error();
            self.field_hint = None;
            self.reset_table_scrolls();
            self.clamp_selected_row();
        }
    }

    pub(crate) fn proxy_table_is_selected(&self, table: ProxyRuleTable) -> bool {
        self.focus == SettingsPaneFocus::Content
            && self.topic == SettingsTopic::Proxy
            && self.selected_proxy_table() == Some(table)
    }

    pub(crate) fn visible_proxy_widgets(&self) -> &'static [ProxyWidget] {
        ProxyWidget::visible_for(&self.draft)
    }

    pub(crate) fn proxy_widget_row(&self, widget: ProxyWidget) -> Option<usize> {
        widget.row_in(self.visible_proxy_widgets())
    }

    pub(crate) fn active_proxy_table_rule(&self, table: ProxyRuleTable) -> Option<usize> {
        match &self.mode {
            EditMode::RuleTable {
                table: active,
                selected_rule,
            }
            | EditMode::RuleEditor {
                table: active,
                index: selected_rule,
                ..
            } if *active == table => Some(*selected_rule),
            _ => None,
        }
    }

    pub(crate) fn rule_table_scroll_offset(&self, table: ProxyRuleTable) -> usize {
        self.rule_table_state(table).scroll_offset()
    }

    pub(crate) fn rule_table_row_count(&self, table: ProxyRuleTable) -> usize {
        self.rule_count(table)
    }

    pub(crate) fn sync_rule_table_view(&mut self, table: ProxyRuleTable, visible_rows: usize) {
        let row_count = self.rule_count(table);
        let state = self.rule_table_state_mut(table);
        // PageUp/PageDown use the last rendered viewport size because table height is UI-derived.
        state.set_last_visible_rows(row_count, visible_rows);
    }

    pub(crate) fn scroll_rule_table_down(&mut self, table: ProxyRuleTable) -> bool {
        let row_count = self.rule_count(table);
        self.rule_table_state_mut(table).scroll_down(row_count)
    }

    pub(crate) fn scroll_rule_table_up(&mut self, table: ProxyRuleTable) -> bool {
        self.rule_table_state_mut(table).scroll_up()
    }

    pub(crate) fn rule_editor(&self) -> Option<RuleEditorState<'_>> {
        match &self.mode {
            EditMode::RuleEditor {
                table,
                from,
                to,
                active_field,
                from_cursor,
                to_cursor,
                ..
            } => Some(RuleEditorState {
                table: *table,
                title: table.editor_title(),
                from,
                to,
                active_field: *active_field,
                from_cursor: *from_cursor,
                to_cursor: *to_cursor,
            }),
            _ => None,
        }
    }

    fn start_rule_editor(&mut self, table: ProxyRuleTable, index: usize) {
        let Some((from, to)) = self.rule_values(table, index) else {
            return;
        };
        let from_cursor = from.chars().count();
        let to_cursor = to.chars().count();
        self.mode = EditMode::RuleEditor {
            table,
            index,
            from,
            to,
            active_field: RuleEditField::From,
            from_cursor,
            to_cursor,
        };
    }

    fn switch_rule_editor_field(&mut self) {
        if let EditMode::RuleEditor { active_field, .. } = &mut self.mode {
            *active_field = match active_field {
                RuleEditField::From => RuleEditField::To,
                RuleEditField::To => RuleEditField::From,
            };
        }
    }

    fn rule_values(&self, table: ProxyRuleTable, index: usize) -> Option<(String, String)> {
        let preset = active_preset(&self.draft)?;
        match table {
            ProxyRuleTable::Remote => preset
                .map_remote
                .rules
                .get(index)
                .map(|rule| (rule.from.clone(), rule.to.clone())),
            ProxyRuleTable::Local => preset
                .map_local
                .rules
                .get(index)
                .map(|rule| (rule.from.clone(), rule.to.clone())),
        }
    }

    fn apply_rule_editor_value(
        &mut self,
        table: ProxyRuleTable,
        index: usize,
        from: String,
        to: String,
    ) {
        match table {
            ProxyRuleTable::Remote => {
                if let Some(rule) = self
                    .active_preset_mut()
                    .and_then(|preset| preset.map_remote.rules.get_mut(index))
                {
                    rule.from = from;
                    rule.to = to;
                    self.draft.clear_error();
                }
            }
            ProxyRuleTable::Local => {
                if let Some(rule) = self
                    .active_preset_mut()
                    .and_then(|preset| preset.map_local.rules.get_mut(index))
                {
                    rule.from = from;
                    rule.to = to;
                    self.draft.clear_error();
                }
            }
        }
    }

    fn toggle_selected_checkbox(&mut self) {
        self.clamp_selected_row();
        match (self.topic, self.selected_row) {
            (SettingsTopic::Recording, _) => self.toggle_selected_recording_checkbox(),
            (SettingsTopic::Interface, 0) => {
                self.draft.ui.request_list.auto_expand = !self.draft.ui.request_list.auto_expand;
            }
            (SettingsTopic::Proxy, _) => match self.selected_proxy_widget() {
                Some(ProxyWidget::MappingEnabled) => {
                    if let Some(proxy) = self.draft.proxy.as_mut() {
                        proxy.enable = !proxy.enable;
                    }
                }
                Some(ProxyWidget::MapRemoteEnabled) => {
                    if let Some(preset) = self.active_preset_mut() {
                        preset.map_remote.enable = !preset.map_remote.enable;
                    }
                }
                Some(ProxyWidget::MapLocalEnabled) => {
                    if let Some(preset) = self.active_preset_mut() {
                        preset.map_local.enable = !preset.map_local.enable;
                    }
                }
                _ => {
                    if let Some(index) = self.selected_remote_rule_index() {
                        self.toggle_rule(ProxyRuleTable::Remote, index);
                    } else if let Some(index) = self.selected_local_rule_index() {
                        self.toggle_rule(ProxyRuleTable::Local, index);
                    }
                }
            },
            _ => {}
        }
    }

    fn add_rule_from_key(&mut self) {
        if self.topic != SettingsTopic::Proxy {
            return;
        }
        self.clamp_selected_row();

        match self.selected_proxy_table() {
            Some(ProxyRuleTable::Remote) => {
                let insert_after = self.selected_remote_rule_index();
                let inserted = self.add_rule_after(ProxyRuleTable::Remote, insert_after);
                if matches!(self.mode, EditMode::RuleTable { .. }) {
                    self.mode = EditMode::RuleTable {
                        table: ProxyRuleTable::Remote,
                        selected_rule: inserted,
                    };
                }
            }
            Some(ProxyRuleTable::Local) => {
                let insert_after = self.selected_local_rule_index();
                let inserted = self.add_rule_after(ProxyRuleTable::Local, insert_after);
                if matches!(self.mode, EditMode::RuleTable { .. }) {
                    self.mode = EditMode::RuleTable {
                        table: ProxyRuleTable::Local,
                        selected_rule: inserted,
                    };
                }
            }
            None => {}
        }
    }

    fn delete_rule_from_key(&mut self) {
        if self.topic != SettingsTopic::Proxy {
            return;
        }
        self.clamp_selected_row();

        if let Some(index) = self.selected_remote_rule_index() {
            self.delete_rule(ProxyRuleTable::Remote, index);
        } else if let Some(index) = self.selected_local_rule_index() {
            self.delete_rule(ProxyRuleTable::Local, index);
        }
    }

    fn add_rule_after(&mut self, table: ProxyRuleTable, index: Option<usize>) -> usize {
        let current_count = self.rule_count(table);
        let inserted = index
            .map_or(current_count, |index| index.saturating_add(1))
            .min(current_count);
        if let Some(mut rules) = self.editable_rules_mut(table) {
            rules.insert_default(inserted);
        }
        self.clamp_rule_table_scroll(table);
        inserted
    }

    fn delete_rule(&mut self, table: ProxyRuleTable, index: usize) {
        if let Some(mut rules) = self.editable_rules_mut(table) {
            rules.remove(index);
        }
        self.clamp_rule_table_scroll(table);
        self.select_rule(table, index);
    }

    fn move_rule_up(&mut self, table: ProxyRuleTable, index: usize) {
        if index > 0
            && let Some(mut rules) = self.editable_rules_mut(table)
        {
            rules.swap(index - 1, index);
        }
        self.select_rule(table, index.saturating_sub(1));
    }

    fn move_rule_down(&mut self, table: ProxyRuleTable, index: usize) {
        if let Some(mut rules) = self.editable_rules_mut(table) {
            rules.swap(index, index.saturating_add(1));
        }
        self.select_rule(table, index.saturating_add(1));
    }

    fn toggle_rule(&mut self, table: ProxyRuleTable, index: usize) {
        if let Some(mut rules) = self.editable_rules_mut(table) {
            rules.toggle(index);
        }
    }

    fn move_rule_up_from_key(&mut self) {
        if self.topic != SettingsTopic::Proxy {
            return;
        }
        self.clamp_selected_row();

        if let Some(index) = self.selected_remote_rule_index() {
            self.move_rule_up(ProxyRuleTable::Remote, index);
        } else if let Some(index) = self.selected_local_rule_index() {
            self.move_rule_up(ProxyRuleTable::Local, index);
        }
    }

    fn move_rule_down_from_key(&mut self) {
        if self.topic != SettingsTopic::Proxy {
            return;
        }
        self.clamp_selected_row();

        if let Some(index) = self.selected_remote_rule_index() {
            self.move_rule_down(ProxyRuleTable::Remote, index);
        } else if let Some(index) = self.selected_local_rule_index() {
            self.move_rule_down(ProxyRuleTable::Local, index);
        }
    }

    fn selected_remote_rule_index(&self) -> Option<usize> {
        match &self.mode {
            EditMode::RuleTable {
                table: ProxyRuleTable::Remote,
                selected_rule,
            }
            | EditMode::RuleEditor {
                table: ProxyRuleTable::Remote,
                index: selected_rule,
                ..
            } => Some(*selected_rule),
            _ => None,
        }
    }

    fn selected_local_rule_index(&self) -> Option<usize> {
        match &self.mode {
            EditMode::RuleTable {
                table: ProxyRuleTable::Local,
                selected_rule,
            }
            | EditMode::RuleEditor {
                table: ProxyRuleTable::Local,
                index: selected_rule,
                ..
            } => Some(*selected_rule),
            _ => None,
        }
    }

    fn selected_proxy_table(&self) -> Option<ProxyRuleTable> {
        if self.topic != SettingsTopic::Proxy {
            return None;
        }

        match &self.mode {
            EditMode::RuleTable { table, .. } | EditMode::RuleEditor { table, .. } => Some(*table),
            _ => self.selected_proxy_widget().and_then(ProxyWidget::table),
        }
    }

    fn rule_count(&self, table: ProxyRuleTable) -> usize {
        active_preset(&self.draft).map_or(0, |preset| match table {
            ProxyRuleTable::Remote => preset.map_remote.rules.len(),
            ProxyRuleTable::Local => preset.map_local.rules.len(),
        })
    }

    fn clamp_rule_index(&self, table: ProxyRuleTable, index: usize) -> usize {
        index.min(self.rule_count(table).saturating_sub(1))
    }

    fn rule_table_page_size(&self, table: ProxyRuleTable) -> usize {
        self.rule_table_state(table).last_visible_rows()
    }

    fn ensure_rule_visible(&mut self, table: ProxyRuleTable) {
        let Some(selected) = self.active_proxy_table_rule(table) else {
            return;
        };
        let row_count = self.rule_count(table);
        self.rule_table_state_mut(table)
            .ensure_visible(row_count, selected);
    }

    fn clamp_rule_table_scroll(&mut self, table: ProxyRuleTable) {
        let row_count = self.rule_count(table);
        self.rule_table_state_mut(table).clamp(row_count);
    }

    fn reset_table_scrolls(&mut self) {
        self.prefilter_table.reset();
        self.remote_rule_table.reset();
        self.local_rule_table.reset();
    }

    fn rule_table_state(&self, table: ProxyRuleTable) -> &SettingsTableState {
        match table {
            ProxyRuleTable::Remote => &self.remote_rule_table,
            ProxyRuleTable::Local => &self.local_rule_table,
        }
    }

    fn rule_table_state_mut(&mut self, table: ProxyRuleTable) -> &mut SettingsTableState {
        match table {
            ProxyRuleTable::Remote => &mut self.remote_rule_table,
            ProxyRuleTable::Local => &mut self.local_rule_table,
        }
    }

    fn active_preset_index(&self) -> Option<usize> {
        let proxy = self.draft.proxy.as_ref()?;
        let active = proxy.active_preset.as_deref()?;
        proxy
            .presets
            .iter()
            .position(|preset| preset.name == active)
    }

    fn active_preset_mut(&mut self) -> Option<&mut ProxyPresetSettings> {
        let index = self.active_preset_index()?;
        self.draft.proxy.as_mut()?.presets.get_mut(index)
    }

    fn editable_rules_mut(&mut self, table: ProxyRuleTable) -> Option<EditableRulesMut<'_>> {
        let preset = self.active_preset_mut()?;
        Some(match table {
            ProxyRuleTable::Remote => EditableRulesMut::Remote(&mut preset.map_remote.rules),
            ProxyRuleTable::Local => EditableRulesMut::Local(&mut preset.map_local.rules),
        })
    }

    fn selected_proxy_widget(&self) -> Option<ProxyWidget> {
        ProxyWidget::from_row(self.visible_proxy_widgets(), self.selected_row)
    }

    fn apply_proxy_preset_name(&mut self, value: String) -> FieldApplyOutcome {
        let Some(index) = self.active_preset_index() else {
            self.set_field_hint(
                FieldEditKind::ProxyPresetName,
                "select a proxy preset first",
            );
            return FieldApplyOutcome::KeepEditing;
        };
        let trimmed = value.trim();
        if trimmed.is_empty() {
            self.set_field_hint(
                FieldEditKind::ProxyPresetName,
                "preset name cannot be empty",
            );
            return FieldApplyOutcome::KeepEditing;
        }
        let duplicate = self.draft.proxy.as_ref().is_some_and(|proxy| {
            proxy
                .presets
                .iter()
                .enumerate()
                .any(|(preset_index, preset)| preset_index != index && preset.name == trimmed)
        });
        if duplicate {
            self.set_field_hint(FieldEditKind::ProxyPresetName, "preset name already exists");
            return FieldApplyOutcome::KeepEditing;
        }

        if let Some(proxy) = self.draft.proxy.as_mut()
            && let Some(preset) = proxy.presets.get_mut(index)
        {
            let name = trimmed.to_string();
            let old_name = std::mem::replace(&mut preset.name, name);
            if proxy.active_preset.as_deref() == Some(old_name.as_str()) {
                proxy.active_preset = Some(preset.name.clone());
            }
            self.draft.clear_error();
            self.clear_field_hint(FieldEditKind::ProxyPresetName);
            self.clamp_selected_row();
            return FieldApplyOutcome::CloseEditor;
        }

        self.set_field_hint(
            FieldEditKind::ProxyPresetName,
            "select a proxy preset first",
        );
        FieldApplyOutcome::KeepEditing
    }

    fn set_field_hint(&mut self, kind: FieldEditKind, message: &'static str) {
        self.field_hint = Some(FieldEditHint { kind, message });
    }

    fn clear_field_hint(&mut self, kind: FieldEditKind) {
        if self
            .field_hint
            .as_ref()
            .is_some_and(|hint| hint.kind == kind)
        {
            self.field_hint = None;
        }
    }

    #[cfg(test)]
    pub fn add_remote_rule_after(&mut self, index: Option<usize>) {
        self.add_rule_after(ProxyRuleTable::Remote, index);
    }

    #[cfg(test)]
    pub fn delete_remote_rule(&mut self, index: usize) {
        self.delete_rule(ProxyRuleTable::Remote, index);
    }

    #[cfg(test)]
    pub fn move_remote_rule_down(&mut self, index: usize) {
        self.move_rule_down(ProxyRuleTable::Remote, index);
    }

    pub fn row_count(&self) -> usize {
        match self.topic {
            SettingsTopic::Server => 1,
            SettingsTopic::Certificate => 2,
            SettingsTopic::Recording => RecordingWidget::all().len(),
            SettingsTopic::Interface => 1,
            SettingsTopic::Proxy => self.visible_proxy_widgets().len(),
        }
    }

    #[cfg(test)]
    pub(crate) fn draft_mut_for_tests(&mut self) -> &mut AppSettings {
        &mut self.draft
    }

    #[cfg(test)]
    pub(crate) fn select_topic_for_tests(&mut self, topic: SettingsTopic) {
        self.select_topic(topic);
    }

    #[cfg(test)]
    pub(crate) fn select_proxy_row_for_tests(&mut self, row: ProxyRow) {
        match row {
            ProxyRow::Preset => {
                self.selected_row = self.proxy_widget_row_for_tests(ProxyWidget::Preset);
                self.mode = EditMode::Browse;
            }
            ProxyRow::PresetName => {
                self.selected_row = self.proxy_widget_row_for_tests(ProxyWidget::PresetName);
                self.mode = EditMode::Browse;
            }
            ProxyRow::MappingEnabled => {
                self.selected_row = self.proxy_widget_row_for_tests(ProxyWidget::MappingEnabled);
                self.mode = EditMode::Browse;
            }
            ProxyRow::MapRemoteEnabled => {
                self.selected_row = self.proxy_widget_row_for_tests(ProxyWidget::MapRemoteEnabled);
                self.mode = EditMode::Browse;
            }
            ProxyRow::RemoteHeader => {
                self.selected_row = self.proxy_widget_row_for_tests(ProxyWidget::RemoteRules);
                self.mode = EditMode::Browse;
            }
            ProxyRow::RemoteRule(index) => {
                self.selected_row = self.proxy_widget_row_for_tests(ProxyWidget::RemoteRules);
                self.mode = EditMode::RuleTable {
                    table: ProxyRuleTable::Remote,
                    selected_rule: self.clamp_rule_index(ProxyRuleTable::Remote, index),
                };
                self.ensure_rule_visible(ProxyRuleTable::Remote);
            }
            ProxyRow::MapLocalEnabled => {
                self.selected_row = self.proxy_widget_row_for_tests(ProxyWidget::MapLocalEnabled);
                self.mode = EditMode::Browse;
            }
            ProxyRow::LocalHeader => {
                self.selected_row = self.proxy_widget_row_for_tests(ProxyWidget::LocalRules);
                self.mode = EditMode::Browse;
            }
            ProxyRow::LocalRule(index) => {
                self.selected_row = self.proxy_widget_row_for_tests(ProxyWidget::LocalRules);
                self.mode = EditMode::RuleTable {
                    table: ProxyRuleTable::Local,
                    selected_rule: self.clamp_rule_index(ProxyRuleTable::Local, index),
                };
                self.ensure_rule_visible(ProxyRuleTable::Local);
            }
        }
    }

    #[cfg(test)]
    fn proxy_widget_row_for_tests(&self, widget: ProxyWidget) -> usize {
        self.proxy_widget_row(widget)
            .expect("proxy widget should be visible for test")
    }

    #[cfg(test)]
    pub(crate) fn rule_editor_for_tests(&self) -> Option<RuleEditorState<'_>> {
        self.rule_editor()
    }

    #[cfg(test)]
    pub(crate) fn rule_table_scroll_offset_for_tests(&self, table: ProxyRuleTable) -> usize {
        self.rule_table_scroll_offset(table)
    }
}

fn byte_index_for_char(value: &str, char_index: usize) -> usize {
    value
        .char_indices()
        .nth(char_index)
        .map_or(value.len(), |(index, _)| index)
}

fn edit_text_value(key: KeyEvent, value: &mut String, cursor: &mut usize) {
    match key.code {
        KeyCode::Backspace => {
            if *cursor > 0 {
                let remove_start = byte_index_for_char(value, *cursor - 1);
                let remove_end = byte_index_for_char(value, *cursor);
                value.replace_range(remove_start..remove_end, "");
                *cursor -= 1;
            }
        }
        KeyCode::Left => {
            *cursor = cursor.saturating_sub(1);
        }
        KeyCode::Right => {
            *cursor = cursor.saturating_add(1).min(value.chars().count());
        }
        KeyCode::Char(ch) => {
            let index = byte_index_for_char(value, *cursor);
            value.insert(index, ch);
            *cursor += 1;
        }
        _ => {}
    }
}

fn active_preset(settings: &AppSettings) -> Option<&ProxyPresetSettings> {
    let proxy = settings.proxy.as_ref()?;
    let active = proxy.active_preset.as_deref()?;
    proxy.presets.iter().find(|preset| preset.name == active)
}

fn validate_settings(settings: &AppSettings) -> Result<(), String> {
    if settings.server.port == 0 {
        return Err("server.port must be between 1 and 65535".to_string());
    }

    if let Some(proxy) = &settings.proxy {
        validate_proxy(proxy)?;
    }

    Ok(())
}

fn validate_proxy(proxy: &ProxySettings) -> Result<(), String> {
    validate_proxy_settings(proxy)
        .into_iter()
        .next()
        .map_or(Ok(()), |diagnostic| Err(diagnostic.message))
}
