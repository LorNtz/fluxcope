use std::cell::RefCell;

use crossterm::event::{MouseEvent, MouseEventKind};
use ratatui::{
    Frame,
    layout::Rect,
    widgets::{Paragraph, Wrap},
};

use super::chrome::{panel_block, render_scrollbar};
use super::{App, MouseHandler, PanelFocus, View};

pub(super) struct LogView {
    area: Rect,
    measurement: RefCell<Option<LogMeasurement>>,
}

#[derive(Clone, Copy)]
struct LogMeasurement {
    revision: u64,
    width: u16,
    total_lines: u16,
}

impl LogView {
    pub(super) fn new() -> Self {
        Self {
            area: Rect::default(),
            measurement: RefCell::new(None),
        }
    }
}

impl View for LogView {
    fn area(&self) -> Rect {
        self.area
    }

    fn set_area(&mut self, area: Rect) {
        self.area = area;
    }

    fn render(&self, frame: &mut Frame, app: &mut App) {
        let focused = app.is_panel_focused(PanelFocus::Log);
        let revision = app.log_panel.revision();
        let measurement = *self.measurement.borrow();
        let total_lines = match measurement {
            Some(measurement)
                if measurement.revision == revision && measurement.width == self.area.width =>
            {
                measurement.total_lines
            }
            _ => {
                let paragraph = Paragraph::new(app.log_panel.render_text())
                    .wrap(Wrap { trim: false })
                    .block(panel_block("Logs", focused));
                let total_lines =
                    u16::try_from(paragraph.line_count(self.area.width)).unwrap_or(u16::MAX);
                *self.measurement.borrow_mut() = Some(LogMeasurement {
                    revision,
                    width: self.area.width,
                    total_lines,
                });
                total_lines
            }
        };
        app.log_panel.scroll.max_offset = total_lines.saturating_sub(self.area.height);
        let max_offset = app.log_panel.scroll.max_offset;
        let offset = app.log_panel.scroll.offset.min(max_offset);
        let paragraph = Paragraph::new(app.log_panel.render_text())
            .wrap(Wrap { trim: false })
            .block(panel_block("Logs", focused))
            .scroll((offset, 0));

        frame.render_widget(paragraph, self.area);
        render_scrollbar(frame, self.area, max_offset, offset);
    }
}

impl MouseHandler for LogView {
    fn handle_mouse(&self, mouse: MouseEvent, app: &mut App) -> bool {
        if !self.contains_mouse(mouse) {
            return false;
        }
        match mouse.kind {
            MouseEventKind::ScrollDown => {
                app.log_panel.scroll.scroll_down();
                true
            }
            MouseEventKind::ScrollUp => {
                app.log_panel.scroll.scroll_up();
                true
            }
            MouseEventKind::Down(_) => {
                app.focus_panel(PanelFocus::Log);
                true
            }
            _ => false,
        }
    }
}
