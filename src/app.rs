mod detail;
mod focus;
mod input;
mod panels;
mod request_list_search;
mod request_tree;
mod requests;
mod settings_draft;
mod settings_popup;
mod single_line_input;

#[cfg(test)]
mod tests;

#[cfg(unix)]
use crate::control::{AppControlSummary, CaptureStoreRuntimeStatus, MappingRuntimeStatus};
use crate::runtime::settings::{SettingsRevision, SettingsTransactionOrigin};
pub(crate) use crate::settings::SettingsUiContext;
#[cfg(test)]
use crate::settings::UiSettings;
use crate::{
    capture::{CaptureRetentionPolicy, CaptureStore, DecodeClient},
    logging::{LogRecord, LogRetentionPolicy},
    recording::RecordingState,
    settings::AppSettings,
};
use focus::FocusState;
use request_list_search::RequestListSearch;
pub(crate) use request_tree::RequestTreeModel;
use std::sync::Arc;

pub(crate) use detail::{
    BODY_LOADING_TEXT, BODY_TEXT_TAB_WIDTH, BodyDisplayPreparation, BodyRenderText,
};
pub use detail::{BodyViewerKey, DetailPanel, MainDisplayTab};
pub use focus::{PanelFocus, PopupFocus};
pub use panels::{CertificatePopup, LogPanel, RequestListPanel};
pub(crate) use request_list_search::SearchTitleStatus;
pub(crate) use request_tree::RequestTreeNodeSnapshot;
#[cfg(test)]
pub(crate) use settings_popup::ProxyRow;
pub use settings_popup::{
    ActionDialog, FieldEditKind, SettingsPaneFocus, SettingsPopup, SettingsPopupAction,
    SettingsTopic,
};
pub(crate) use settings_popup::{
    PROXY_PRESET_SELECT_MAX_VISIBLE_ITEMS, PrefilterPatternEditState, ProxyRuleTable, ProxyWidget,
    RULE_EDITOR_KEY_HINTS, RecordingWidget, RuleEditField, RuleEditorState, SelectTarget,
    SettingsKeyHint, SettingsScrollRequest, SettingsSelectId, validate_settings,
};

pub(crate) fn benchmark_request_tree(captures: Vec<crate::capture::CapturedExchange>) -> usize {
    fn count(nodes: &[RequestTreeNodeSnapshot]) -> usize {
        nodes
            .iter()
            .map(|node| 1_usize.saturating_add(count(&node.children)))
            .sum()
    }

    let summaries = captures
        .into_iter()
        .map(crate::capture::CaptureRecord::from_completed)
        .map(|capture| capture.summary());
    count(&RequestTreeModel::from_requests(summaries).snapshot())
}

pub(crate) fn benchmark_retained_log_join(record_count: usize, record_bytes: usize) -> usize {
    let mut panel = LogPanel::with_retention(LogRetentionPolicy {
        max_records: record_count,
        max_bytes: record_count.saturating_mul(record_bytes),
    });
    let message = "x".repeat(record_bytes);
    for _ in 0..record_count {
        panel.add_log(LogRecord::system(message.clone()));
    }
    panel.render_text().len()
}

pub struct App {
    captures: CaptureStore,
    pub recording: RecordingState,
    focus: FocusState,
    pub request_list: RequestListPanel,
    pub detail_panel: DetailPanel,
    pub log_panel: LogPanel,
    pub certificate_popup: CertificatePopup,
    pub settings_popup: SettingsPopup,
    settings: Arc<AppSettings>,
    pending_settings_save: Option<Arc<AppSettings>>,
    settings_revision: SettingsRevision,
    #[cfg(unix)]
    mapping_status: MappingRuntimeStatus,
    settings_transaction_pending: bool,
    decode_client: Option<DecodeClient>,
    request_tree: Arc<RequestTreeModel>,
    request_tree_revision: u64,
    request_search: RequestListSearch,
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

    #[cfg(test)]
    pub fn with_settings_and_log_retention(
        settings: AppSettings,
        recording: RecordingState,
        log_retention: LogRetentionPolicy,
    ) -> Self {
        Self::with_policies(
            Arc::new(settings),
            recording,
            log_retention,
            CaptureRetentionPolicy::default(),
            SettingsUiContext::default(),
        )
    }

    pub(crate) fn with_runtime_policies(
        settings: impl Into<Arc<AppSettings>>,
        recording: RecordingState,
        log_retention: LogRetentionPolicy,
        capture_retention: CaptureRetentionPolicy,
        settings_context: SettingsUiContext,
    ) -> Self {
        Self::with_policies(
            settings.into(),
            recording,
            log_retention,
            capture_retention,
            settings_context,
        )
    }

    fn with_policies(
        settings: Arc<AppSettings>,
        recording: RecordingState,
        log_retention: LogRetentionPolicy,
        capture_retention: CaptureRetentionPolicy,
        settings_context: SettingsUiContext,
    ) -> Self {
        #[cfg(unix)]
        let mapping_status = mapping_runtime_status(&settings);
        let ui_settings = settings.ui.clone();
        Self {
            captures: CaptureStore::new(capture_retention),
            recording,
            focus: FocusState::new(),
            request_list: RequestListPanel::new(ui_settings.request_list),
            detail_panel: DetailPanel::new(),
            log_panel: LogPanel::with_retention(log_retention),
            certificate_popup: CertificatePopup::new(),
            settings_popup: SettingsPopup::with_context(settings_context),
            settings,
            pending_settings_save: None,
            settings_revision: SettingsRevision::INITIAL,
            settings_transaction_pending: false,
            decode_client: None,
            #[cfg(unix)]
            mapping_status,
            request_tree: Arc::new(RequestTreeModel::default()),
            request_tree_revision: 0,
            request_search: RequestListSearch::default(),
        }
    }

    pub fn is_recording(&self) -> bool {
        self.recording.is_enabled()
    }
    #[cfg(unix)]
    pub(crate) fn set_recording_enabled(&mut self, enabled: bool) -> bool {
        self.recording.set_enabled(enabled)
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

    pub fn take_settings_save_request(&mut self) -> Option<Arc<AppSettings>> {
        self.pending_settings_save.take()
    }

    pub fn finish_settings_save(
        &mut self,
        saved: impl Into<Arc<AppSettings>>,
        revision: SettingsRevision,
    ) {
        let saved = saved.into();
        self.settings_popup.mark_saved();
        self.close_popup_focus();
        self.request_list.auto_expand = saved.ui.request_list.auto_expand;
        #[cfg(unix)]
        {
            self.mapping_status = mapping_runtime_status(&saved);
        }
        self.settings = saved;
        self.settings_revision = revision;
        self.settings_transaction_pending = false;
    }

    #[cfg(unix)]
    pub(crate) fn control_summary(&self) -> AppControlSummary {
        let retention = self.captures.retention_policy();
        AppControlSummary {
            recording_enabled: self.is_recording(),
            retained_capture_count: self.capture_count(),
            settings_revision: self.settings_revision.get(),
            mapping: self.mapping_status.clone(),
            capture_store: CaptureStoreRuntimeStatus {
                revision: self.captures.revision(),
                retained_bytes: self.captures.retained_bytes(),
                maximum_retained_bytes: retention.max_bytes,
                maximum_retained_records: retention.max_records,
            },
        }
    }

    #[cfg(test)]
    pub(crate) fn settings_revision(&self) -> SettingsRevision {
        self.settings_revision
    }

    pub(crate) fn set_settings_transaction_pending(&mut self, pending: bool) {
        self.settings_transaction_pending = pending;
    }

    pub(crate) fn settings_transaction_pending(&self) -> bool {
        self.settings_transaction_pending
    }

    #[cfg(test)]
    pub(crate) fn apply_settings_transaction_commit(
        &mut self,
        settings: Arc<AppSettings>,
        revision: SettingsRevision,
    ) -> Result<(), crate::control_rpc::protocol::ControlError> {
        self.apply_settings_transaction_commit_from_origin(
            settings,
            revision,
            SettingsTransactionOrigin::Mcp,
        )
    }

    pub(crate) fn apply_settings_transaction_commit_from_origin(
        &mut self,
        settings: Arc<AppSettings>,
        revision: SettingsRevision,
        origin: SettingsTransactionOrigin,
    ) -> Result<(), crate::control_rpc::protocol::ControlError> {
        if origin == SettingsTransactionOrigin::Mcp && self.settings_popup.is_dirty() {
            return Err(crate::control_rpc::protocol::ControlError::new(
                crate::control_rpc::protocol::ControlErrorCode::TuiDraftConflict,
                "mapping settings conflict with an unsaved TUI draft",
                false,
                serde_json::json!({"current_revision": self.settings_revision}),
            ));
        }
        #[cfg(unix)]
        {
            self.mapping_status = mapping_runtime_status(&settings);
        }
        self.settings = Arc::clone(&settings);
        self.request_list.auto_expand = settings.ui.request_list.auto_expand;
        self.settings_revision = revision;
        self.settings_transaction_pending = false;
        if self.settings_popup.visible && !self.settings_popup.is_dirty() {
            self.settings_popup.open(settings);
        }
        Ok(())
    }

    pub fn fail_settings_save(&mut self, message: String) {
        log::error!("Failed to save settings: {message}");
        self.settings_popup.mark_save_failed(message);
        self.settings_transaction_pending = false;
        self.focus.open_popup(PopupFocus::Settings);
    }

    pub fn append_log(&mut self, record: LogRecord) {
        self.log_panel.add_log(record);
    }

    pub(crate) fn set_decode_client(&mut self, client: DecodeClient) {
        self.decode_client = Some(client);
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

#[cfg(unix)]
fn mapping_runtime_status(settings: &AppSettings) -> MappingRuntimeStatus {
    settings
        .proxy
        .as_ref()
        .map_or_else(MappingRuntimeStatus::default, |proxy| {
            let active = proxy
                .active_preset
                .as_deref()
                .and_then(|name| proxy.presets.iter().find(|preset| preset.name == name));
            MappingRuntimeStatus {
                configured: true,
                enabled: proxy.enable,
                active_preset: proxy.active_preset.clone(),
                map_remote_enabled: active.map(|preset| preset.map_remote.enable),
                map_local_enabled: active.map(|preset| preset.map_local.enable),
            }
        })
}
