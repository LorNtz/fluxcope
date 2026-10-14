use crate::app::{App, PanelFocus, PopupFocus, RequestTreeNodeSnapshot};
#[cfg(test)]
use crate::app::{
    BODY_LOADING_TEXT, BodyViewerKey, MainDisplayTab, ProxyRow, ProxyRuleTable, SettingsPaneFocus,
    SettingsPopup, SettingsTopic,
};
use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
#[cfg(test)]
use ratatui::layout::Position;
#[cfg(test)]
use ratatui::symbols;
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout, Margin, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};
#[cfg(test)]
use std::borrow::Cow;

mod certificate_popup;
use certificate_popup::render_certificate_popup;
mod chrome;
use chrome::panel_block;
mod detail;
use detail::DetailView;
#[cfg(test)]
use detail::{body_editor_text_area, detail_content_area};
mod log_view;
use log_view::LogView;
mod request_list;
use request_list::RequestListView;
#[cfg(test)]
use request_list::build_request_tree_items;
mod select_widget;
mod settings;
use settings::SettingsPopupView;
#[cfg(test)]
use settings::{
    PEM_FILENAME_INPUT_WIDTH_COLS, PORT_INPUT_WIDTH_COLS, SETTING_TEXT_FIELD_HEIGHT,
    SettingsContentItem, SettingsTextInputControl, TEXT_INPUT_CHROME_WIDTH, action_dialog_area,
    proxy_rule_table_widget, rule_editor_area, settings_content_height,
    settings_content_items_with_error, settings_field_layout, settings_popup_area,
    settings_popup_layout, settings_select_control, settings_select_layout,
    settings_table_max_height,
};
mod terminal_text;
#[cfg(test)]
use terminal_text::text_width;

trait View {
    fn area(&self) -> Rect;
    fn set_area(&mut self, area: Rect);
    fn render(&mut self, frame: &mut Frame, app: &mut App);

    fn layout(&mut self, area: Rect, _app: &App) {
        self.set_area(area);
    }

    fn contains_mouse(&self, mouse: MouseEvent) -> bool {
        self.area().contains((mouse.column, mouse.row).into())
    }
}

trait MouseHandler: View {
    fn handle_mouse(&mut self, _mouse: MouseEvent, _app: &mut App) -> bool {
        false
    }
}

pub struct RootView {
    area: Rect,
    status: StatusView,
    request_list: RequestListView,
    detail: DetailView,
    log: LogView,
    settings_popup: SettingsPopupView,
}

impl RootView {
    pub fn new() -> Self {
        Self {
            area: Rect::default(),
            status: StatusView::new(),
            request_list: RequestListView::new(),
            detail: DetailView::new(),
            log: LogView::new(),
            settings_popup: SettingsPopupView::new(),
        }
    }

    pub fn render(&mut self, frame: &mut Frame, app: &mut App) {
        View::layout(self, frame.area(), app);
        View::render(self, frame, app);
    }

    pub fn handle_mouse(&mut self, mouse: MouseEvent, app: &mut App) {
        let _ = MouseHandler::handle_mouse(self, mouse, app);
    }
}

impl View for RootView {
    fn area(&self) -> Rect {
        self.area
    }

    fn set_area(&mut self, area: Rect) {
        self.area = area;
    }

    fn render(&mut self, frame: &mut Frame, app: &mut App) {
        self.status.render(frame, app);
        if app.log_panel.visible {
            self.log.render(frame, app);
        } else {
            self.request_list.render(frame, app);
            self.detail.render(frame, app);
        }

        if app.certificate_popup.visible {
            render_certificate_popup(frame, app);
        }

        if app.settings_popup.visible {
            self.settings_popup.render(frame, app);
        } else {
            self.settings_popup.clear();
        }
    }

    fn layout(&mut self, area: Rect, app: &App) {
        self.set_area(area);

        let root_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(3), Constraint::Min(0)])
            .split(area);

        self.status.layout(root_chunks[0], app);
        if app.log_panel.visible {
            self.request_list.layout(Rect::default(), app);
            self.detail.layout(Rect::default(), app);
            self.log.layout(root_chunks[1], app);
        } else {
            let chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(30), Constraint::Percentage(70)])
                .split(root_chunks[1]);

            self.request_list.layout(chunks[0], app);
            self.detail.layout(chunks[1], app);
            self.log.layout(Rect::default(), app);
        }
    }
}

impl MouseHandler for RootView {
    fn handle_mouse(&mut self, mouse: MouseEvent, app: &mut App) -> bool {
        if app.settings_popup.visible {
            return self.settings_popup.handle_mouse(mouse, app, self.area());
        }

        if app.certificate_popup.visible {
            return true;
        }

        if self.status.handle_mouse(mouse, app) {
            return true;
        }

        if app.log_panel.visible {
            self.log.handle_mouse(mouse, app)
        } else if app.is_request_search_editing() {
            self.request_list.handle_mouse(mouse, app)
        } else {
            self.detail.handle_mouse(mouse, app) || self.request_list.handle_mouse(mouse, app)
        }
    }
}

const SETTINGS_BUTTON: &str = "[Settings]";

struct StatusView {
    area: Rect,
}

impl StatusView {
    fn new() -> Self {
        Self {
            area: Rect::default(),
        }
    }

    fn settings_button_area(&self) -> Rect {
        let inner = self.area.inner(Margin {
            horizontal: 1,
            vertical: 1,
        });
        let width = SETTINGS_BUTTON.len() as u16;
        if inner.width < width || inner.is_empty() {
            return Rect::default();
        }
        Rect::new(inner.right() - width, inner.y, width, 1)
    }
}

impl View for StatusView {
    fn area(&self) -> Rect {
        self.area
    }

    fn set_area(&mut self, area: Rect) {
        self.area = area;
    }

    fn render(&mut self, frame: &mut Frame, app: &mut App) {
        let (icon, label, style) = if app.is_recording() {
            ("●", "Recording ON", Style::default().fg(Color::LightRed))
        } else {
            ("○", "Recording OFF", Style::default().fg(Color::DarkGray))
        };
        let status = Line::from(vec![
            Span::styled(icon, style),
            Span::raw(format!(" {label}")),
            Span::styled(
                format!("  •  {} captures", app.capture_count()),
                Style::default().fg(Color::DarkGray),
            ),
        ]);

        let block = panel_block("Status", false);
        let mut text_area = block.inner(self.area);
        let button_area = self.settings_button_area();
        if !button_area.is_empty() {
            text_area.width = button_area.x.saturating_sub(text_area.x).saturating_sub(1);
        }
        frame.render_widget(block, self.area);
        frame.render_widget(Paragraph::new(status).alignment(Alignment::Left), text_area);
        frame.render_widget(
            Span::styled(
                SETTINGS_BUTTON,
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ),
            button_area,
        );
    }
}

impl MouseHandler for StatusView {
    fn handle_mouse(&mut self, mouse: MouseEvent, app: &mut App) -> bool {
        if mouse.kind == MouseEventKind::Down(MouseButton::Left)
            && self
                .settings_button_area()
                .contains((mouse.column, mouse.row).into())
        {
            app.open_settings_popup();
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests;
