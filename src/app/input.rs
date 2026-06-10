use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use super::{
    App, PanelFocus, PopupFocus,
    focus::{FocusDirection, focus_neighbor, focusable_panels},
};

impl App {
    pub fn handle_key_event(&mut self, key: KeyEvent) -> bool {
        if key.kind == KeyEventKind::Release {
            return false;
        }

        if let Some(popup) = self.focus.popup() {
            self.handle_popup_key(popup, key);
            return false;
        }

        if self.is_panel_focused(PanelFocus::Detail) && self.detail_panel.body_viewer.is_active() {
            self.detail_panel.body_viewer.handle_key(key);
            return false;
        }

        if is_plain_key(key) && key.code == KeyCode::Char('q') {
            return true;
        }

        if self.handle_focus_key(key) || self.handle_global_panel_key(key) {
            return false;
        }

        match self.focus.panel() {
            PanelFocus::RequestList => self.handle_request_list_key(key),
            PanelFocus::Detail => self.handle_detail_key(key),
            PanelFocus::Log => self.handle_log_key(key),
        }

        false
    }

    fn handle_focus_key(&mut self, key: KeyEvent) -> bool {
        if !is_plain_key(key) && key.modifiers != KeyModifiers::CONTROL {
            return false;
        }

        match key.code {
            KeyCode::Tab if is_plain_key(key) => {
                self.focus_next_panel();
                true
            }
            KeyCode::BackTab if is_plain_key(key) => {
                self.focus_previous_panel();
                true
            }
            KeyCode::Char('h') | KeyCode::Char('H') if key.modifiers == KeyModifiers::CONTROL => {
                self.focus_in_direction(FocusDirection::Left);
                true
            }
            KeyCode::Char('j') | KeyCode::Char('J') if key.modifiers == KeyModifiers::CONTROL => {
                self.focus_in_direction(FocusDirection::Down);
                true
            }
            KeyCode::Char('k') | KeyCode::Char('K') if key.modifiers == KeyModifiers::CONTROL => {
                self.focus_in_direction(FocusDirection::Up);
                true
            }
            KeyCode::Char('l') | KeyCode::Char('L') if key.modifiers == KeyModifiers::CONTROL => {
                self.focus_in_direction(FocusDirection::Right);
                true
            }
            _ => false,
        }
    }

    fn handle_global_panel_key(&mut self, key: KeyEvent) -> bool {
        if !is_plain_key(key) {
            return false;
        }

        match key.code {
            KeyCode::Char('r') => {
                self.toggle_recording();
                true
            }
            KeyCode::Char('@') => {
                self.toggle_log_panel();
                true
            }
            KeyCode::Char('c') | KeyCode::Char('C') => {
                self.open_certificate_popup();
                true
            }
            _ => false,
        }
    }

    fn handle_popup_key(&mut self, popup: PopupFocus, key: KeyEvent) {
        match popup {
            PopupFocus::Certificate => {
                if is_plain_key(key) && key.code == KeyCode::Esc {
                    self.close_certificate_popup();
                }
            }
        }
    }

    fn handle_request_list_key(&mut self, key: KeyEvent) {
        if !is_plain_key(key) {
            return;
        }

        match key.code {
            KeyCode::Char('j') | KeyCode::Down => self.next(),
            KeyCode::Char('k') | KeyCode::Up => self.previous(),
            KeyCode::Char('h') | KeyCode::Left => {
                let changed = self.request_list.state.key_left();
                self.apply_request_list_change(changed);
            }
            KeyCode::Char('l') => {
                self.toggle_selected_request_subtree();
            }
            KeyCode::Right => {
                self.open_selected_request_subtree();
            }
            KeyCode::Enter if self.selected_request_leaf() => {
                self.focus_panel(PanelFocus::Detail);
            }
            KeyCode::Enter | KeyCode::Char(' ') => {
                self.toggle_selected_request_subtree();
            }
            KeyCode::PageDown => {
                self.request_list.scroll_down();
            }
            KeyCode::PageUp => {
                self.request_list.scroll_up();
            }
            KeyCode::Char('d') => {
                self.delete_selected_requests();
            }
            KeyCode::Char('D') => {
                self.clear_requests();
            }
            KeyCode::Char('e') => {
                self.expand_selected_request_subtree();
            }
            KeyCode::Char('E') => {
                self.expand_all_request_subtrees();
            }
            KeyCode::Char('w') => {
                self.collapse_selected_request_subtree_children();
            }
            KeyCode::Char('W') => {
                self.collapse_all_request_subtrees();
            }
            _ => {}
        }
    }

    fn handle_detail_key(&mut self, key: KeyEvent) {
        if !is_plain_key(key) {
            return;
        }

        match key.code {
            KeyCode::Enter
                if self.detail_panel.active_tab.is_body() && self.selected_request().is_some() =>
            {
                self.enter_current_body_viewer();
            }
            KeyCode::Char('j') | KeyCode::Char('J') | KeyCode::Down | KeyCode::PageDown => {
                self.detail_panel.scroll.scroll_down();
            }
            KeyCode::Char('k') | KeyCode::Char('K') | KeyCode::Up | KeyCode::PageUp => {
                self.detail_panel.scroll.scroll_up();
            }
            KeyCode::Char('h') | KeyCode::Left => {
                self.detail_panel.previous_tab();
            }
            KeyCode::Char('l') | KeyCode::Right => {
                self.detail_panel.next_tab();
            }
            _ => {}
        }
    }

    fn handle_log_key(&mut self, key: KeyEvent) {
        if !is_plain_key(key) {
            return;
        }

        match key.code {
            KeyCode::Char('j') | KeyCode::Char('J') | KeyCode::Down | KeyCode::PageDown => {
                self.log_panel.scroll.scroll_down();
            }
            KeyCode::Char('k') | KeyCode::Char('K') | KeyCode::Up | KeyCode::PageUp => {
                self.log_panel.scroll.scroll_up();
            }
            _ => {}
        }
    }

    fn focus_next_panel(&mut self) {
        let panels = focusable_panels(self.log_panel.visible);
        let current_index = panels
            .iter()
            .position(|panel| *panel == self.focus.panel())
            .unwrap_or(0);
        self.focus_panel(panels[(current_index + 1) % panels.len()]);
    }

    fn focus_previous_panel(&mut self) {
        let panels = focusable_panels(self.log_panel.visible);
        let current_index = panels
            .iter()
            .position(|panel| *panel == self.focus.panel())
            .unwrap_or(0);
        self.focus_panel(panels[(current_index + panels.len() - 1) % panels.len()]);
    }

    fn focus_in_direction(&mut self, direction: FocusDirection) {
        if let Some(panel) = focus_neighbor(self.focus.panel(), direction, self.log_panel.visible) {
            self.focus_panel(panel);
        }
    }

    fn toggle_log_panel(&mut self) {
        self.log_panel.toggle();
        if self.log_panel.visible {
            self.focus_panel(PanelFocus::Log);
        } else {
            self.ensure_focusable_panel();
        }
    }

    fn ensure_focusable_panel(&mut self) {
        if self.log_panel.visible {
            self.focus_panel(PanelFocus::Log);
        } else if self.focus.panel() == PanelFocus::Log {
            self.focus_panel(PanelFocus::Detail);
        }
    }

    fn open_certificate_popup(&mut self) {
        self.certificate_popup.open();
        self.focus.open_popup(PopupFocus::Certificate);
    }

    fn close_certificate_popup(&mut self) {
        self.certificate_popup.close();
        self.focus.close_popup();
        self.ensure_focusable_panel();
    }
}

fn is_plain_key(key: KeyEvent) -> bool {
    key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT
}
