use super::{
    EditMode, FieldApplyOutcome, FieldEditHint, FieldEditKind, FieldEditState, ProxyWidget,
    SettingsPopup, SettingsPopupAction, SettingsTopic,
};
use crossterm::event::{KeyCode, KeyEvent};

impl SettingsPopup {
    fn start_field_edit(&mut self, kind: FieldEditKind, value: String) {
        self.clear_field_hint(kind);
        self.mode = EditMode::Field {
            kind,
            cursor: value.chars().count(),
            value,
        };
    }

    pub(super) fn handle_field_key(&mut self, key: KeyEvent) -> SettingsPopupAction {
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

    pub(super) fn start_selected_edit(&mut self) {
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
                    if let Some(name) = self.active_proxy_preset().map(|preset| preset.name.clone())
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

    pub(super) fn set_field_hint(&mut self, kind: FieldEditKind, message: &'static str) {
        self.field_hint = Some(FieldEditHint { kind, message });
    }

    pub(super) fn clear_field_hint(&mut self, kind: FieldEditKind) {
        if self
            .field_hint
            .as_ref()
            .is_some_and(|hint| hint.kind == kind)
        {
            self.field_hint = None;
        }
    }
}

fn byte_index_for_char(value: &str, char_index: usize) -> usize {
    value
        .char_indices()
        .nth(char_index)
        .map_or(value.len(), |(index, _)| index)
}

pub(super) fn edit_text_value(key: KeyEvent, value: &mut String, cursor: &mut usize) {
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
