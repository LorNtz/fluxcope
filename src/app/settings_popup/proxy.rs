#[cfg(test)]
use super::ProxyRow;
use super::{
    EditMode, FieldApplyOutcome, FieldEditKind, PROXY_PRESET_SELECT_MAX_VISIBLE_ITEMS,
    ProxyPresetChoice, ProxyRuleTable, ProxyWidget, RuleEditField, RuleEditorState,
    SelectCommitEffect, SelectTarget, SettingsPaneFocus, SettingsPopup, SettingsPopupAction,
    SettingsSelectId, SettingsTableState, SettingsTopic,
};
use crate::{
    select::{SelectCommit, SelectItem, SelectItemRole, SelectOutcome, SelectState},
    settings::{
        AppSettings, ProxyMapLocalRule, ProxyMapRemoteRule, ProxyPresetSettings,
        mapping_ops::{
            MappingMutation, MappingMutationErrorKind, apply_mapping_mutation,
            find_mapping_preset_index,
        },
    },
};
use crossterm::event::KeyEvent;

impl SettingsPopup {
    pub(super) fn handle_select_key(&mut self, key: KeyEvent) -> SettingsPopupAction {
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
        self.bump_presentation_revision();
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
            self.bump_presentation_revision();
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
            self.bump_presentation_revision();
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
        self.bump_presentation_revision();
        true
    }

    pub(crate) fn proxy_preset_label(&self) -> &str {
        active_preset(&self.draft).map_or("(none)", |preset| preset.name.as_str())
    }

    pub(crate) fn active_proxy_preset(&self) -> Option<&ProxyPresetSettings> {
        active_preset(&self.draft)
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
        active_preset_index(&self.draft)
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
        let name = self
            .draft
            .proxy
            .as_ref()
            .and_then(|proxy| proxy.presets.get(index))
            .map(|preset| preset.name.clone());
        if let Some(name) = name
            && apply_mapping_mutation(
                &mut self.draft,
                MappingMutation::SetActivePreset { name: Some(name) },
            )
            .is_ok()
        {
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

    pub(super) fn start_rule_editor(&mut self, table: ProxyRuleTable, index: usize) {
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

    pub(super) fn switch_rule_editor_field(&mut self) {
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

    pub(super) fn apply_rule_editor_value(
        &mut self,
        table: ProxyRuleTable,
        index: usize,
        from: String,
        to: String,
    ) {
        let Some(preset) = self.active_preset_name().map(str::to_string) else {
            self.draft.set_error("select a proxy preset first");
            return;
        };
        let mutation = match table {
            ProxyRuleTable::Remote => MappingMutation::UpdateRemoteRule {
                preset,
                index,
                from,
                to,
            },
            ProxyRuleTable::Local => MappingMutation::UpdateLocalRule {
                preset,
                index,
                from,
                to,
            },
        };
        match apply_mapping_mutation(&mut self.draft, mutation) {
            Ok(_) => self.draft.clear_error(),
            Err(error) => self.draft.set_error(error.message),
        }
    }

    pub(super) fn toggle_selected_proxy_checkbox(&mut self) {
        match self.selected_proxy_widget() {
            Some(ProxyWidget::MappingEnabled) => {
                if let Some(enabled) = self.draft.proxy.as_ref().map(|proxy| !proxy.enable) {
                    let _ = apply_mapping_mutation(
                        &mut self.draft,
                        MappingMutation::SetGlobalEnabled { enabled },
                    );
                }
            }
            Some(ProxyWidget::MapRemoteEnabled) => {
                if let Some((preset, enabled)) = self
                    .active_proxy_preset()
                    .map(|preset| (preset.name.clone(), !preset.map_remote.enable))
                {
                    let _ = apply_mapping_mutation(
                        &mut self.draft,
                        MappingMutation::SetTableEnabled {
                            preset,
                            table: ProxyRuleTable::Remote,
                            enabled,
                        },
                    );
                }
            }
            Some(ProxyWidget::MapLocalEnabled) => {
                if let Some((preset, enabled)) = self
                    .active_proxy_preset()
                    .map(|preset| (preset.name.clone(), !preset.map_local.enable))
                {
                    let _ = apply_mapping_mutation(
                        &mut self.draft,
                        MappingMutation::SetTableEnabled {
                            preset,
                            table: ProxyRuleTable::Local,
                            enabled,
                        },
                    );
                }
            }
            _ => {
                if let Some(index) = self.selected_remote_rule_index() {
                    self.toggle_rule(ProxyRuleTable::Remote, index);
                } else if let Some(index) = self.selected_local_rule_index() {
                    self.toggle_rule(ProxyRuleTable::Local, index);
                }
            }
        }
    }

    pub(super) fn add_rule_from_key(&mut self) {
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

    pub(super) fn delete_rule_from_key(&mut self) {
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

    pub(super) fn add_rule_after(&mut self, table: ProxyRuleTable, index: Option<usize>) -> usize {
        let current_count = self.rule_count(table);
        let inserted = index
            .map_or(current_count, |index| index.saturating_add(1))
            .min(current_count);
        if let Some(preset) = self.active_preset_name().map(str::to_string) {
            let mutation = match table {
                ProxyRuleTable::Remote => MappingMutation::InsertRemoteRule {
                    preset,
                    index: inserted,
                    rule: ProxyMapRemoteRule {
                        from: "https://example.com".to_string(),
                        to: "http://localhost:3000".to_string(),
                        enable: true,
                    },
                },
                ProxyRuleTable::Local => MappingMutation::InsertLocalRule {
                    preset,
                    index: inserted,
                    rule: ProxyMapLocalRule {
                        from: "https://example.com".to_string(),
                        to: "~/mock-response.json".to_string(),
                        enable: true,
                    },
                },
            };
            self.apply_proxy_mutation(mutation);
        }
        self.clamp_rule_table_scroll(table);
        inserted
    }

    pub(super) fn delete_rule(&mut self, table: ProxyRuleTable, index: usize) {
        if index < self.rule_count(table)
            && let Some(preset) = self.active_preset_name().map(str::to_string)
        {
            self.apply_proxy_mutation(MappingMutation::DeleteRule {
                preset,
                table,
                index,
            });
        }
        self.clamp_rule_table_scroll(table);
        self.select_rule(table, index);
    }

    pub(super) fn move_rule_up(&mut self, table: ProxyRuleTable, index: usize) {
        if index > 0
            && index < self.rule_count(table)
            && let Some(preset) = self.active_preset_name().map(str::to_string)
        {
            self.apply_proxy_mutation(MappingMutation::MoveRule {
                preset,
                table,
                from: index,
                to: index - 1,
            });
        }
        self.select_rule(table, index.saturating_sub(1));
    }

    pub(super) fn move_rule_down(&mut self, table: ProxyRuleTable, index: usize) {
        let destination = index.saturating_add(1);
        if destination < self.rule_count(table)
            && let Some(preset) = self.active_preset_name().map(str::to_string)
        {
            self.apply_proxy_mutation(MappingMutation::MoveRule {
                preset,
                table,
                from: index,
                to: destination,
            });
        }
        self.select_rule(table, destination);
    }

    pub(super) fn toggle_rule(&mut self, table: ProxyRuleTable, index: usize) {
        let enabled = self.active_proxy_preset().and_then(|preset| match table {
            ProxyRuleTable::Remote => preset.map_remote.rules.get(index).map(|rule| !rule.enable),
            ProxyRuleTable::Local => preset.map_local.rules.get(index).map(|rule| !rule.enable),
        });
        if let Some(enabled) = enabled
            && let Some(preset) = self.active_preset_name().map(str::to_string)
        {
            self.apply_proxy_mutation(MappingMutation::SetRuleEnabled {
                preset,
                table,
                index,
                enabled,
            });
        }
    }

    pub(super) fn move_rule_up_from_key(&mut self) {
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

    pub(super) fn move_rule_down_from_key(&mut self) {
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

    pub(super) fn selected_proxy_table(&self) -> Option<ProxyRuleTable> {
        if self.topic != SettingsTopic::Proxy {
            return None;
        }

        match &self.mode {
            EditMode::RuleTable { table, .. } | EditMode::RuleEditor { table, .. } => Some(*table),
            _ => self.selected_proxy_widget().and_then(ProxyWidget::table),
        }
    }

    pub(super) fn rule_count(&self, table: ProxyRuleTable) -> usize {
        active_preset(&self.draft).map_or(0, |preset| match table {
            ProxyRuleTable::Remote => preset.map_remote.rules.len(),
            ProxyRuleTable::Local => preset.map_local.rules.len(),
        })
    }

    pub(super) fn clamp_rule_index(&self, table: ProxyRuleTable, index: usize) -> usize {
        index.min(self.rule_count(table).saturating_sub(1))
    }

    pub(super) fn rule_table_page_size(&self, table: ProxyRuleTable) -> usize {
        self.rule_table_state(table).last_visible_rows()
    }

    pub(super) fn ensure_rule_visible(&mut self, table: ProxyRuleTable) {
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

    fn active_preset_name(&self) -> Option<&str> {
        active_preset(&self.draft).map(|preset| preset.name.as_str())
    }

    fn apply_proxy_mutation(&mut self, mutation: MappingMutation) -> bool {
        match apply_mapping_mutation(&mut self.draft, mutation) {
            Ok(_) => true,
            Err(error) => {
                self.draft.set_error(error.message);
                false
            }
        }
    }

    pub(super) fn selected_proxy_widget(&self) -> Option<ProxyWidget> {
        ProxyWidget::from_row(self.visible_proxy_widgets(), self.selected_row)
    }

    pub(super) fn apply_proxy_preset_name(&mut self, value: String) -> FieldApplyOutcome {
        let Some(name) = self.active_preset_name().map(str::to_string) else {
            self.set_field_hint(
                FieldEditKind::ProxyPresetName,
                "select a proxy preset first",
            );
            return FieldApplyOutcome::KeepEditing;
        };
        match apply_mapping_mutation(
            &mut self.draft,
            MappingMutation::RenamePreset {
                name,
                new_name: value,
            },
        ) {
            Ok(_) => {
                self.draft.clear_error();
                self.clear_field_hint(FieldEditKind::ProxyPresetName);
                self.clamp_selected_row();
                FieldApplyOutcome::CloseEditor
            }
            Err(error) => {
                let message = match error.kind {
                    MappingMutationErrorKind::DuplicatePresetName => "preset name already exists",
                    MappingMutationErrorKind::InvalidField => "preset name cannot be empty",
                    MappingMutationErrorKind::ProxyNotFound
                    | MappingMutationErrorKind::PresetNotFound
                    | MappingMutationErrorKind::RuleNotFound => "select a proxy preset first",
                };
                self.set_field_hint(FieldEditKind::ProxyPresetName, message);
                FieldApplyOutcome::KeepEditing
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn add_remote_rule_after(&mut self, index: Option<usize>) {
        self.add_rule_after(ProxyRuleTable::Remote, index);
        self.bump_presentation_revision();
    }

    #[cfg(test)]
    pub(crate) fn delete_remote_rule(&mut self, index: usize) {
        self.delete_rule(ProxyRuleTable::Remote, index);
        self.bump_presentation_revision();
    }

    #[cfg(test)]
    pub(crate) fn move_remote_rule_down(&mut self, index: usize) {
        self.move_rule_down(ProxyRuleTable::Remote, index);
        self.bump_presentation_revision();
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

fn active_preset(settings: &AppSettings) -> Option<&ProxyPresetSettings> {
    let proxy = settings.proxy.as_ref()?;
    active_preset_index(settings).and_then(|index| proxy.presets.get(index))
}

fn active_preset_index(settings: &AppSettings) -> Option<usize> {
    let proxy = settings.proxy.as_ref()?;
    find_mapping_preset_index(proxy, proxy.active_preset.as_deref()?)
}
