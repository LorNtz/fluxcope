use super::{
    EditMode, FieldApplyOutcome, FieldEditHint, FieldEditKind, FieldEditState, ProxyWidget,
    SettingsPopup, SettingsPopupAction, SettingsTopic,
};
use crate::text_input::TextInputState;
use crossterm::event::{KeyCode, KeyEvent};

impl SettingsPopup {
    fn start_field_edit(&mut self, kind: FieldEditKind, value: String) {
        self.clear_field_hint(kind);
        self.mode = EditMode::Field {
            kind,
            input: TextInputState::new(value),
        };
    }

    pub(super) fn handle_field_key(&mut self, key: KeyEvent) -> SettingsPopupAction {
        if key.code == KeyCode::Enter {
            self.apply_current_field();
            return SettingsPopupAction::None;
        }
        let EditMode::Field { kind, input } = &mut self.mode else {
            return SettingsPopupAction::None;
        };
        let kind = *kind;
        match key.code {
            KeyCode::Esc => {
                self.clear_field_hint(kind);
                self.mode = EditMode::Browse;
            }
            KeyCode::Backspace | KeyCode::Char(_) => {
                input.handle_key(key);
                self.clear_field_hint(kind);
            }
            KeyCode::Left | KeyCode::Right => {
                input.handle_key(key);
            }
            _ => {}
        }
        SettingsPopupAction::None
    }

    pub(super) fn apply_current_field(&mut self) -> bool {
        let mode = std::mem::replace(&mut self.mode, EditMode::Browse);
        let EditMode::Field { kind, mut input } = mode else {
            self.mode = mode;
            return true;
        };
        if self.apply_field_value(kind, &mut input) == FieldApplyOutcome::KeepEditing {
            self.mode = EditMode::Field { kind, input };
            return false;
        }
        true
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

    fn apply_field_value(
        &mut self,
        kind: FieldEditKind,
        input: &mut TextInputState,
    ) -> FieldApplyOutcome {
        match kind {
            FieldEditKind::ServerPort => match input.text().parse::<u16>() {
                Ok(port) if port > 0 => {
                    self.draft.server.port = port;
                    self.draft.clear_error();
                    self.clear_field_hint(kind);
                }
                _ => {
                    self.draft
                        .set_error("server.port must be between 1 and 65535");
                    return FieldApplyOutcome::KeepEditing;
                }
            },
            FieldEditKind::CertificateStoreDir => {
                self.draft.certificate.store_dir = std::mem::take(input).into_text();
                self.draft.clear_error();
                self.clear_field_hint(kind);
            }
            FieldEditKind::CertificatePemFilename => {
                self.draft.certificate.pem_filename = std::mem::take(input).into_text();
                self.draft.clear_error();
                self.clear_field_hint(kind);
            }
            FieldEditKind::ProxyPresetName => {
                return self.apply_proxy_preset_name(input.text().to_owned());
            }
        }
        FieldApplyOutcome::CloseEditor
    }

    pub(crate) fn active_field_edit(&self, kind: FieldEditKind) -> Option<FieldEditState<'_>> {
        match &self.mode {
            EditMode::Field {
                kind: active_field,
                input,
            } if *active_field == kind => Some(FieldEditState {
                value: input.text(),
                cursor: input.cursor(),
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
