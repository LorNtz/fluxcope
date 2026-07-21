mod body_viewer;
mod focus;
mod input;
mod panels;
mod request_tree;
mod requests;
mod settings_popup;

#[cfg(test)]
mod tests;

#[cfg(test)]
use crate::settings::UiSettings;
use crate::{
    capture::CapturedExchange,
    logging::{LogRecord, LogRetentionPolicy},
    recording::RecordingState,
    settings::AppSettings,
};
use focus::FocusState;

pub(crate) use body_viewer::BODY_TEXT_TAB_WIDTH;
pub use body_viewer::BodyViewerKey;
#[cfg(test)]
pub(crate) use body_viewer::{format_request_body, format_response_body};
pub use focus::{PanelFocus, PopupFocus};
pub use panels::{CertificatePopup, DetailPanel, LogPanel, MainDisplayTab, RequestListPanel};
pub use request_tree::RequestTreeEntry;
#[cfg(test)]
pub(crate) use settings_popup::ProxyRow;
pub use settings_popup::{
    ActionDialog, FieldEditKind, SettingsPaneFocus, SettingsPopup, SettingsPopupAction,
    SettingsTopic,
};
pub(crate) use settings_popup::{
    PROXY_PRESET_SELECT_MAX_VISIBLE_ITEMS, ProxyRuleTable, ProxyWidget, RULE_EDITOR_KEY_HINTS,
    RuleEditField, RuleEditorState, SelectTarget, SettingsKeyHint, SettingsScrollRequest,
    SettingsSelectId,
};

pub struct App {
    pub requests: Vec<CapturedExchange>,
    pub recording: RecordingState,
    focus: FocusState,
    pub request_list: RequestListPanel,
    pub detail_panel: DetailPanel,
    pub log_panel: LogPanel,
    pub certificate_popup: CertificatePopup,
    pub settings_popup: SettingsPopup,
    settings: AppSettings,
    pending_settings_save: Option<AppSettings>,
}

impl App {
    #[cfg(test)]
    pub fn new(ui_settings: UiSettings) -> Self {
        Self::with_recording(ui_settings, RecordingState::default())
    }

    #[cfg(test)]
    pub fn with_recording(ui_settings: UiSettings, recording: RecordingState) -> Self {
        let settings = AppSettings {
            ui: ui_settings,
            ..AppSettings::default()
        };
        Self::with_settings(settings, recording)
    }

    #[cfg(test)]
    pub fn with_settings(settings: AppSettings, recording: RecordingState) -> Self {
        Self::with_settings_and_log_retention(settings, recording, LogRetentionPolicy::default())
    }

    pub fn with_settings_and_log_retention(
        settings: AppSettings,
        recording: RecordingState,
        log_retention: LogRetentionPolicy,
    ) -> Self {
        let ui_settings = settings.ui.clone();
        Self {
            requests: vec![],
            recording,
            focus: FocusState::new(),
            request_list: RequestListPanel::new(ui_settings.request_list),
            detail_panel: DetailPanel::new(),
            log_panel: LogPanel::with_retention(log_retention),
            certificate_popup: CertificatePopup::new(),
            settings_popup: SettingsPopup::new(),
            settings,
            pending_settings_save: None,
        }
    }

    pub fn is_recording(&self) -> bool {
        self.recording.is_enabled()
    }

    pub fn toggle_recording(&mut self) {
        let enabled = self.recording.toggle();
        if enabled {
            log::info!("recording enabled");
        } else {
            log::info!("recording disabled");
        }
    }

    pub fn is_panel_focused(&self, panel: PanelFocus) -> bool {
        self.focus.popup().is_none() && self.focus.panel() == panel
    }

    pub fn is_popup_focused(&self, popup: PopupFocus) -> bool {
        self.focus.popup() == Some(popup)
    }

    pub fn take_settings_save_request(&mut self) -> Option<AppSettings> {
        self.pending_settings_save.take()
    }

    pub fn finish_settings_save(&mut self, saved: AppSettings) {
        self.settings_popup.mark_saved();
        self.close_popup_focus();
        self.request_list.auto_expand = saved.ui.request_list.auto_expand;
        self.settings = saved;
    }

    pub fn fail_settings_save(&mut self, message: String) {
        log::error!("Failed to save settings: {message}");
        self.settings_popup.mark_save_failed(message);
        self.focus.open_popup(PopupFocus::Settings);
    }

    pub fn append_log(&mut self, record: LogRecord) {
        self.log_panel.add_log(record);
    }

    pub fn set_certificate_download_url(&mut self, download_url: String) {
        self.certificate_popup.set_download_url(download_url);
    }

    pub fn focus_panel(&mut self, panel: PanelFocus) {
        let panel = match (panel, self.log_panel.visible) {
            (PanelFocus::Log, false) => PanelFocus::Detail,
            (PanelFocus::RequestList | PanelFocus::Detail, true) => PanelFocus::Log,
            _ => panel,
        };
        self.focus.focus_panel(panel);
    }

    pub(in crate::app) fn close_popup_focus(&mut self) {
        self.focus.close_popup();
        self.ensure_focusable_panel();
    }

    pub(in crate::app) fn ensure_focusable_panel(&mut self) {
        if self.log_panel.visible {
            self.focus_panel(PanelFocus::Log);
        } else if self.focus.panel() == PanelFocus::Log {
            self.focus_panel(PanelFocus::Detail);
        }
    }
}
