mod event;
mod focus;
mod input;
mod panels;
mod request_tree;
mod requests;

#[cfg(test)]
mod tests;

use crate::{proxy_handler::CapturedData, recording::RecordingState, settings::UiSettings};
use focus::FocusState;

pub use event::AppEvent;
pub use focus::{PanelFocus, PopupFocus};
pub use panels::{CertificatePopup, DetailPanel, LogPanel, MainDisplayTab, RequestListPanel};
pub use request_tree::RequestTreeEntry;

pub struct App {
    pub requests: Vec<CapturedData>,
    pub recording: RecordingState,
    focus: FocusState,
    pub request_list: RequestListPanel,
    pub detail_panel: DetailPanel,
    pub log_panel: LogPanel,
    pub certificate_popup: CertificatePopup,
}

impl App {
    #[cfg(test)]
    pub fn new(ui_settings: UiSettings) -> Self {
        Self::with_recording(ui_settings, RecordingState::default())
    }

    pub fn with_recording(ui_settings: UiSettings, recording: RecordingState) -> Self {
        Self {
            requests: vec![],
            recording,
            focus: FocusState::new(),
            request_list: RequestListPanel::new(ui_settings.request_list),
            detail_panel: DetailPanel::new(),
            log_panel: LogPanel::new(),
            certificate_popup: CertificatePopup::new(),
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

    pub fn focus_panel(&mut self, panel: PanelFocus) {
        let panel = match (panel, self.log_panel.visible) {
            (PanelFocus::Log, false) => PanelFocus::Detail,
            (PanelFocus::RequestList | PanelFocus::Detail, true) => PanelFocus::Log,
            _ => panel,
        };
        self.focus.focus_panel(panel);
    }
}
