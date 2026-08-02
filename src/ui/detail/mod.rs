use super::chrome::{base_panel_block, render_scrollbar};
use super::terminal_text::text_width;
use super::{MouseHandler, View};
use crate::app::{App, BodyRenderText, BodyViewerKey, MainDisplayTab, PanelFocus};
use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::{
    Frame,
    layout::{Margin, Position, Rect},
    style::{Color, Modifier, Style},
    symbols,
    widgets::{Block, Paragraph, Tabs},
};
use std::cell::RefCell;

mod body;
pub(super) use body::body_editor_text_area;
use body::{
    body_render_text, detail_text_paragraph, render_body_editor, should_render_active_body_editor,
};

mod header_table;
use header_table::{
    borrowed_lines, header_table_render, header_table_row_at_position, request_header_rows,
    response_header_rows,
};

pub(super) struct DetailView {
    area: Rect,
    body_measurement: RefCell<Option<DetailBodyMeasurement>>,
    header_rows: RefCell<Option<DetailHeaderRows>>,
}

#[derive(Clone, Copy)]
struct DetailBodyMeasurement {
    key: BodyViewerKey,
    text_revision: u64,
    width: u16,
    total_lines: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct DetailHeaderKey {
    sequence: crate::capture::CaptureSequence,
    revision: u64,
    tab: MainDisplayTab,
}

struct DetailHeaderRows {
    key: DetailHeaderKey,
    rows: Vec<header_table::TableRow>,
    render_width: Option<u16>,
    render_selection: Option<usize>,
    render: Option<header_table::HeaderTableRender>,
}

impl DetailView {
    pub(super) fn new() -> Self {
        Self {
            area: Rect::default(),
            body_measurement: RefCell::new(None),
            header_rows: RefCell::new(None),
        }
    }
}

impl View for DetailView {
    fn area(&self) -> Rect {
        self.area
    }

    fn set_area(&mut self, area: Rect) {
        self.area = area;
    }

    fn render(&self, frame: &mut Frame, app: &mut App) {
        let focused = app.is_panel_focused(PanelFocus::Detail);
        if should_render_active_body_editor(app) {
            render_body_editor(frame, app, focused, self.area());
            frame.render_widget(
                detail_tabs(app.detail_panel.active_tab),
                detail_tabs_area(self.area()),
            );
            return;
        }

        match build_detail_content(app) {
            DetailContent::Text(detail_text) => {
                let mut paragraph = detail_text_paragraph(detail_text, focused);

                let total_lines: u16 = paragraph
                    .line_count(self.area().width)
                    .try_into()
                    .unwrap_or(u16::MAX);
                let offset = app
                    .detail_panel
                    .update_scroll_bounds(total_lines.saturating_sub(self.area().height));
                paragraph = paragraph.scroll((offset, 0));

                frame.render_widget(paragraph, self.area());
            }
            DetailContent::Body(body_key) => {
                if app.body_render_text(body_key).is_some() {
                    let body_is_loading = matches!(
                        app.body_render_text(body_key),
                        Some(BodyRenderText::Loading)
                    );
                    let text_revision = app.body_text_revision();
                    let cached = *self.body_measurement.borrow();
                    let total_lines = match cached {
                        Some(cached)
                            if cached.key == body_key
                                && cached.text_revision == text_revision
                                && cached.width == self.area().width =>
                        {
                            cached.total_lines
                        }
                        _ => {
                            let total_lines = app
                                .body_render_text(body_key)
                                .map(|body_text| {
                                    detail_text_paragraph(body_render_text(body_text), focused)
                                        .line_count(self.area().width)
                                })
                                .unwrap_or_default()
                                .try_into()
                                .unwrap_or(u16::MAX);
                            *self.body_measurement.borrow_mut() = Some(DetailBodyMeasurement {
                                key: body_key,
                                text_revision,
                                width: self.area().width,
                                total_lines,
                            });
                            total_lines
                        }
                    };
                    let offset = if body_is_loading {
                        app.detail_panel.suspend_scroll_bounds();
                        0
                    } else {
                        app.detail_panel
                            .update_scroll_bounds(total_lines.saturating_sub(self.area().height))
                    };

                    if let Some(body_text) = app.body_render_text(body_key) {
                        frame.render_widget(
                            detail_text_paragraph(body_render_text(body_text), focused)
                                .scroll((offset, 0)),
                            self.area(),
                        );
                    }
                }
            }
            DetailContent::Table(req) => {
                let key = DetailHeaderKey {
                    sequence: req.sequence,
                    revision: req.revision,
                    tab: app.detail_panel.active_tab,
                };
                if self.header_rows.borrow().as_ref().map(|cached| cached.key) != Some(key) {
                    let rows = match key.tab {
                        MainDisplayTab::RequestHeader => request_header_rows(&req),
                        MainDisplayTab::ResponseHeader => response_header_rows(&req),
                        MainDisplayTab::RequestBody | MainDisplayTab::ResponseBody => {
                            unreachable!()
                        }
                    };
                    *self.header_rows.borrow_mut() = Some(DetailHeaderRows {
                        key,
                        rows,
                        render_width: None,
                        render_selection: None,
                        render: None,
                    });
                }
                let table_area = detail_content_area(self.area());
                let selected_row = app.detail_panel.selected_header_row;
                {
                    let mut cache = self.header_rows.borrow_mut();
                    let cache = cache.as_mut().expect("header rows should be cached");
                    if cache.render_width != Some(table_area.width)
                        || cache.render_selection != selected_row
                    {
                        cache.render = Some(header_table_render(
                            &cache.rows,
                            table_area.width,
                            selected_row,
                        ));
                        cache.render_width = Some(table_area.width);
                        cache.render_selection = selected_row;
                    }
                }
                let cache = self.header_rows.borrow();
                let table = cache
                    .as_ref()
                    .and_then(|cache| cache.render.as_ref())
                    .expect("header render should be cached");
                let total_lines: u16 = table.lines.len().try_into().unwrap_or(u16::MAX);
                let offset = app
                    .detail_panel
                    .update_scroll_bounds(total_lines.saturating_sub(table_area.height));

                frame.render_widget(
                    Paragraph::new(borrowed_lines(&table.lines))
                        .block(detail_panel_block(focused))
                        .scroll((offset, 0)),
                    self.area(),
                );
            }
        }
        frame.render_widget(
            detail_tabs(app.detail_panel.active_tab),
            detail_tabs_area(self.area()),
        );
        render_scrollbar(
            frame,
            self.area(),
            app.detail_panel.scroll.max_offset,
            app.detail_panel
                .scroll
                .offset
                .min(app.detail_panel.scroll.max_offset),
        );
    }
}

impl MouseHandler for DetailView {
    fn handle_mouse(&self, mouse: MouseEvent, app: &mut App) -> bool {
        if !self.contains_mouse(mouse) {
            return false;
        }

        match mouse.kind {
            MouseEventKind::ScrollDown => {
                if app.detail_panel.body_viewer.is_active() {
                    app.detail_panel.body_viewer.scroll_down();
                } else {
                    app.detail_panel.scroll_down();
                }
                true
            }
            MouseEventKind::ScrollUp => {
                if app.detail_panel.body_viewer.is_active() {
                    app.detail_panel.body_viewer.scroll_up();
                } else {
                    app.detail_panel.scroll_up();
                }
                true
            }
            MouseEventKind::Down(button) => {
                app.focus_panel(PanelFocus::Detail);
                if let Some(tab) =
                    detail_tab_at_position(self.area(), Position::new(mouse.column, mouse.row))
                {
                    app.detail_panel.select_tab(tab);
                } else if app.detail_panel.body_viewer.is_active()
                    && button == MouseButton::Left
                    && body_editor_text_area(self.area())
                        .contains(Position::new(mouse.column, mouse.row))
                {
                    app.detail_panel.body_viewer.handle_mouse(mouse);
                } else if button == MouseButton::Left {
                    app.detail_panel.selected_header_row = header_table_row_at_position(
                        app,
                        self.area(),
                        Position::new(mouse.column, mouse.row),
                    );
                }
                true
            }
            MouseEventKind::Drag(MouseButton::Left) | MouseEventKind::Up(MouseButton::Left)
                if app.detail_panel.body_viewer.is_active()
                    && body_editor_text_area(self.area())
                        .contains(Position::new(mouse.column, mouse.row)) =>
            {
                app.detail_panel.body_viewer.handle_mouse(mouse);
                true
            }
            _ => false,
        }
    }
}

fn detail_panel_block(focused: bool) -> Block<'static> {
    base_panel_block(focused)
}

fn detail_tabs(active_tab: MainDisplayTab) -> Tabs<'static> {
    let titles = MainDisplayTab::all()
        .iter()
        .map(|tab| tab.title())
        .collect::<Vec<_>>();

    Tabs::new(titles)
        .style(Style::default().fg(Color::Reset))
        .divider(symbols::line::VERTICAL)
        .highlight_style(
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        )
        .select(active_tab.index())
}

fn detail_tabs_area(area: Rect) -> Rect {
    Rect {
        x: area.x.saturating_add(1),
        y: area.y,
        width: area.width.saturating_sub(2),
        height: u16::from(area.height > 0 && area.width > 2),
    }
}

fn detail_tab_at_position(area: Rect, position: Position) -> Option<MainDisplayTab> {
    let tabs_area = detail_tabs_area(area);
    if tabs_area.is_empty() || position.y != tabs_area.y {
        return None;
    }

    if position.x < tabs_area.x || position.x >= tabs_area.right() {
        return None;
    }

    let mut cursor = tabs_area.x;
    for (index, tab) in MainDisplayTab::all().iter().copied().enumerate() {
        if index > 0 {
            cursor = cursor.saturating_add(1);
        }
        if cursor >= tabs_area.right() {
            return None;
        }

        let width = text_width(tab.title()).saturating_add(2);
        let tab_end = cursor.saturating_add(width).min(tabs_area.right());
        if position.x >= cursor && position.x < tab_end {
            return Some(tab);
        }
        cursor = cursor.saturating_add(width);
    }

    None
}

pub(in crate::ui) fn detail_content_area(area: Rect) -> Rect {
    area.inner(Margin {
        vertical: 1,
        horizontal: 1,
    })
}

enum DetailContent {
    Text(&'static str),
    Body(BodyViewerKey),
    Table(crate::capture::CaptureSummary),
}

fn build_detail_content(app: &App) -> DetailContent {
    if app.detail_panel.active_tab.is_body() {
        return app
            .current_body_viewer_key()
            .map(DetailContent::Body)
            .unwrap_or(DetailContent::Text("Select a request"));
    }

    let Some(req) = app.selected_request() else {
        return DetailContent::Text("Select a request");
    };

    match app.detail_panel.active_tab {
        MainDisplayTab::RequestHeader | MainDisplayTab::ResponseHeader => DetailContent::Table(req),
        MainDisplayTab::RequestBody | MainDisplayTab::ResponseBody => unreachable!(),
    }
}
