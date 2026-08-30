pub(crate) use crate::settings::mapping_ops::ProxyRuleTable;
use crate::{
    select::SelectState,
    settings::{
        AppSettings, ConfigMode, SettingsUiContext, mapping_ops::find_mapping_preset_index,
    },
};
use tui_scrollview::ScrollViewState;

use super::settings_draft::SettingsDraft;

mod recording;
pub(crate) use recording::{PrefilterPatternEditState, RecordingWidget};
mod field_editor;
mod navigation;
mod proxy;
mod validation;
use validation::validate_settings;

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
        let Some(active) = proxy.active_preset.as_deref() else {
            return &Self::PRESET_ONLY;
        };
        if find_mapping_preset_index(proxy, active).is_some() {
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
    context: SettingsUiContext,
    mode: EditMode,
    field_hint: Option<FieldEditHint>,
    scroll_request: Option<SettingsScrollRequest>,
    prefilter_table: SettingsTableState,
    remote_rule_table: SettingsTableState,
    local_rule_table: SettingsTableState,
    presentation_revision: u64,
}

impl SettingsPopup {
    #[cfg(test)]
    pub fn new() -> Self {
        Self::with_context(SettingsUiContext::default())
    }

    pub(crate) fn with_context(context: SettingsUiContext) -> Self {
        let settings = AppSettings::default();
        Self {
            visible: false,
            focus: SettingsPaneFocus::Content,
            topic: SettingsTopic::Server,
            selected_row: 0,
            scroll: ScrollViewState::default(),
            context,
            draft: SettingsDraft::new(settings),
            mode: EditMode::Browse,
            field_hint: None,
            scroll_request: None,
            prefilter_table: SettingsTableState::default(),
            remote_rule_table: SettingsTableState::default(),
            local_rule_table: SettingsTableState::default(),
            presentation_revision: 0,
        }
    }

    pub fn open(&mut self, settings: AppSettings) {
        self.bump_presentation_revision();
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
        self.bump_presentation_revision();
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
                    label: self.commit_label(),
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

    pub(crate) fn commit_label(&self) -> &'static str {
        match self.context.persistence {
            crate::settings::PersistenceMode::Persistent => "Save",
            crate::settings::PersistenceMode::Ephemeral => "Apply",
        }
    }

    pub(crate) fn mode_label(&self) -> &'static str {
        match self.context.config_mode {
            ConfigMode::DefaultOwned => "Default config (persistent)",
            ConfigMode::ReadOnlyFile => "Read-only file (ephemeral)",
            ConfigMode::Temporary => "Temporary (ephemeral)",
        }
    }

    pub fn mark_saved(&mut self) {
        self.close();
    }

    pub fn mark_save_failed(&mut self, message: String) {
        self.bump_presentation_revision();
        self.mode = EditMode::Browse;
        self.draft.set_error(message);
        self.visible = true;
    }

    pub fn validate(&self) -> Result<(), String> {
        validate_settings(&self.draft)
    }

    pub(crate) fn take_scroll_request(&mut self) -> Option<SettingsScrollRequest> {
        self.scroll_request.take()
    }

    pub(crate) fn presentation_revision(&self) -> u64 {
        self.presentation_revision
    }

    pub(super) fn bump_presentation_revision(&mut self) {
        self.presentation_revision = self.presentation_revision.wrapping_add(1);
    }

    pub(super) fn reset_table_scrolls(&mut self) {
        self.prefilter_table.reset();
        self.remote_rule_table.reset();
        self.local_rule_table.reset();
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
        self.bump_presentation_revision();
        &mut self.draft
    }

    #[cfg(test)]
    pub(crate) fn select_topic_for_tests(&mut self, topic: SettingsTopic) {
        self.select_topic(topic);
        self.bump_presentation_revision();
    }
}
