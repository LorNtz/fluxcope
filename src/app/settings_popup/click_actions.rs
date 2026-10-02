use super::{
    DialogActionKind, EditMode, ProxyRuleTable, ProxyWidget, RuleEditField, SettingsClickTarget,
    SettingsPaneFocus, SettingsPopup, SettingsPopupAction,
};

impl SettingsPopup {
    pub(crate) fn handle_click(
        &mut self,
        target: SettingsClickTarget,
        double_click: bool,
        transaction_pending: bool,
    ) -> SettingsPopupAction {
        if transaction_pending {
            if let SettingsClickTarget::Topic(topic) = target
                && matches!(
                    self.mode,
                    EditMode::Browse | EditMode::RuleTable { .. } | EditMode::PrefilterTable { .. }
                )
            {
                self.select_topic(topic);
                self.focus = SettingsPaneFocus::Topics;
                self.bump_presentation_revision();
            } else {
                self.mark_transaction_pending();
            }
            return SettingsPopupAction::None;
        }
        let action = self.apply_click(target, double_click);
        self.bump_presentation_revision();
        action
    }

    fn apply_click(
        &mut self,
        target: SettingsClickTarget,
        double_click: bool,
    ) -> SettingsPopupAction {
        if let Some(action) = self.click_overlay(target) {
            return action;
        }
        if self.position_active_inline_cursor(target) {
            return SettingsPopupAction::None;
        }
        if !self.apply_current_field() || !self.apply_current_prefilter_pattern() {
            return SettingsPopupAction::None;
        }
        match target {
            SettingsClickTarget::Topic(topic) => {
                self.select_topic(topic);
                self.focus = SettingsPaneFocus::Topics;
            }
            SettingsClickTarget::Table { row } if row < self.row_count() => {
                if self.selected_row != row
                    || !matches!(
                        self.mode,
                        EditMode::RuleTable { .. } | EditMode::PrefilterTable { .. }
                    )
                {
                    self.focus_content_row(row);
                    self.start_selected_edit();
                }
                self.focus = SettingsPaneFocus::Content;
            }
            SettingsClickTarget::Field { row, cursor } if row < self.row_count() => {
                self.focus_content_row(row);
                self.start_selected_edit();
                self.position_active_inline_cursor(SettingsClickTarget::Field { row, cursor });
            }
            SettingsClickTarget::Checkbox { row } if row < self.row_count() => {
                self.focus_content_row(row);
                self.toggle_selected_checkbox();
            }
            SettingsClickTarget::Select(target) => self.start_select(target),
            SettingsClickTarget::PrefilterRow { index, toggle } => {
                if index < self.prefilter_pattern_count() {
                    self.select_prefilter_pattern(index);
                    if toggle {
                        self.toggle_prefilter_pattern(index);
                    } else if double_click {
                        self.start_prefilter_pattern_editor(index);
                    }
                }
            }
            SettingsClickTarget::PrefilterInput { index, cursor } => {
                if index < self.prefilter_pattern_count() {
                    self.select_prefilter_pattern(index);
                    self.start_prefilter_pattern_editor(index);
                    self.position_active_inline_cursor(SettingsClickTarget::PrefilterInput {
                        index,
                        cursor,
                    });
                }
            }
            SettingsClickTarget::RuleRow {
                table,
                index,
                toggle,
                field,
            } => {
                self.click_rule_row(table, index, toggle, field, double_click);
            }
            SettingsClickTarget::Outside => {
                // Table focus is part of the base popup, not a dismissible overlay.
                self.mode = EditMode::Browse;
                return self.handle_escape();
            }
            _ => {}
        }
        SettingsPopupAction::None
    }

    fn click_overlay(&mut self, target: SettingsClickTarget) -> Option<SettingsPopupAction> {
        match self.mode {
            EditMode::UnsavedConfirm { .. } => Some(match target {
                SettingsClickTarget::DialogAction(action) => self.activate_dialog_action(action),
                SettingsClickTarget::Outside => {
                    self.activate_dialog_action(DialogActionKind::KeepEditing)
                }
                _ => SettingsPopupAction::None,
            }),
            EditMode::Select { .. } => {
                match target {
                    SettingsClickTarget::Outside => {
                        self.close_active_select();
                    }
                    SettingsClickTarget::SelectOption { index } => {
                        self.commit_active_select_filtered_index(index);
                    }
                    SettingsClickTarget::SelectFilter {
                        cursor: Some(cursor),
                    } => {
                        if let EditMode::Select { state, .. } = &mut self.mode {
                            state.set_filter_cursor(cursor);
                        }
                    }
                    _ => {}
                }
                Some(SettingsPopupAction::None)
            }
            EditMode::RuleEditor { .. } => {
                match target {
                    SettingsClickTarget::Outside => {
                        self.handle_escape();
                    }
                    SettingsClickTarget::RuleField { field, cursor } => {
                        self.position_rule_cursor(field, cursor)
                    }
                    _ => {}
                }
                Some(SettingsPopupAction::None)
            }
            _ => None,
        }
    }

    fn position_active_inline_cursor(&mut self, target: SettingsClickTarget) -> bool {
        let (input, clicked_cursor) = match (&mut self.mode, target) {
            (
                EditMode::Field { input, .. },
                SettingsClickTarget::Field {
                    row,
                    cursor: clicked,
                },
            ) if row == self.selected_row => (input, clicked),
            (
                EditMode::PrefilterEditor { index, input },
                SettingsClickTarget::PrefilterInput {
                    index: clicked_index,
                    cursor: clicked,
                },
            ) if *index == clicked_index => (input, clicked),
            _ => return false,
        };
        if let Some(clicked) = clicked_cursor {
            input.set_cursor(clicked);
        }
        true
    }

    fn focus_content_row(&mut self, row: usize) {
        self.focus = SettingsPaneFocus::Content;
        self.selected_row = row;
        self.mode = EditMode::Browse;
        self.request_selected_visible();
    }

    fn click_rule_row(
        &mut self,
        table: ProxyRuleTable,
        index: usize,
        toggle: bool,
        field: RuleEditField,
        double_click: bool,
    ) {
        if index >= self.rule_count(table) {
            return;
        }
        let widget = match table {
            ProxyRuleTable::Remote => ProxyWidget::RemoteRules,
            ProxyRuleTable::Local => ProxyWidget::LocalRules,
        };
        let Some(row) = self.proxy_widget_row(widget) else {
            return;
        };
        self.focus_content_row(row);
        self.select_rule(table, index);
        if toggle {
            self.toggle_rule(table, index);
        } else if double_click {
            self.start_rule_editor(table, index);
            self.position_rule_cursor(field, None);
        }
    }

    fn position_rule_cursor(&mut self, field: RuleEditField, clicked: Option<usize>) {
        if let EditMode::RuleEditor {
            from,
            to,
            active_field,
            ..
        } = &mut self.mode
        {
            *active_field = field;
            let input = match field {
                RuleEditField::From => from,
                RuleEditField::To => to,
            };
            if let Some(clicked) = clicked {
                input.set_cursor(clicked);
            }
        }
    }
}
