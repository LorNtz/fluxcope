const FOCUSABLE_PANELS_WITH_LOG: [PanelFocus; 1] = [PanelFocus::Log];
const FOCUSABLE_PANELS_WITHOUT_LOG: [PanelFocus; 2] = [PanelFocus::RequestList, PanelFocus::Detail];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanelFocus {
    RequestList,
    Detail,
    Log,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PopupFocus {
    Certificate,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) struct FocusState {
    panel: PanelFocus,
    popup: Option<PopupFocus>,
}

impl FocusState {
    pub(in crate::app) fn new() -> Self {
        Self {
            panel: PanelFocus::RequestList,
            popup: None,
        }
    }

    pub(in crate::app) fn panel(self) -> PanelFocus {
        self.panel
    }

    pub(in crate::app) fn popup(self) -> Option<PopupFocus> {
        self.popup
    }

    pub(in crate::app) fn focus_panel(&mut self, panel: PanelFocus) {
        self.panel = panel;
    }

    pub(in crate::app) fn open_popup(&mut self, popup: PopupFocus) {
        self.popup = Some(popup);
    }

    pub(in crate::app) fn close_popup(&mut self) {
        self.popup = None;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) enum FocusDirection {
    Left,
    Down,
    Up,
    Right,
}

pub(in crate::app) fn focusable_panels(log_visible: bool) -> &'static [PanelFocus] {
    if log_visible {
        &FOCUSABLE_PANELS_WITH_LOG
    } else {
        &FOCUSABLE_PANELS_WITHOUT_LOG
    }
}

pub(in crate::app) fn focus_neighbor(
    current: PanelFocus,
    direction: FocusDirection,
    log_visible: bool,
) -> Option<PanelFocus> {
    if log_visible {
        return None;
    }

    match (current, direction) {
        (PanelFocus::RequestList, FocusDirection::Right) => Some(PanelFocus::Detail),
        (PanelFocus::Detail, FocusDirection::Left) => Some(PanelFocus::RequestList),
        _ => None,
    }
}
