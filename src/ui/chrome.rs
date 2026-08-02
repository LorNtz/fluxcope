use ratatui::{
    Frame,
    layout::{Margin, Rect},
    style::{Color, Style},
    widgets::{Block, BorderType, Borders, Scrollbar, ScrollbarOrientation, ScrollbarState},
};

pub(super) fn render_scrollbar(frame: &mut Frame, area: Rect, max_offset: u16, offset: u16) {
    let scrollbar = Scrollbar::default()
        .orientation(ScrollbarOrientation::VerticalRight)
        .begin_symbol(Some("↑"))
        .end_symbol(Some("↓"));

    let mut state = ScrollbarState::new(max_offset as usize).position(offset as usize);
    frame.render_stateful_widget(
        scrollbar,
        area.inner(Margin {
            vertical: 1,
            horizontal: 0,
        }),
        &mut state,
    );
}

pub(super) fn panel_block(title: &'static str, focused: bool) -> Block<'static> {
    base_panel_block(focused).title(title)
}

pub(super) fn base_panel_block(focused: bool) -> Block<'static> {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded);
    apply_focus_border(block, focused)
}

fn apply_focus_border(block: Block<'static>, focused: bool) -> Block<'static> {
    if focused {
        block.border_style(Style::default().fg(Color::Green))
    } else {
        block
    }
}

pub(super) fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    let x = area.x + area.width.saturating_sub(width) / 2;
    let y = area.y + area.height.saturating_sub(height) / 2;

    Rect {
        x,
        y,
        width,
        height,
    }
}
