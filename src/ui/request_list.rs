use std::collections::HashSet;

use crossterm::event::{MouseEvent, MouseEventKind};
use ratatui::{
    Frame,
    layout::{Alignment, Margin, Position, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, Paragraph, Scrollbar, ScrollbarOrientation},
};
use tui_tree_widget::{Tree, TreeItem};
use unicode_segmentation::UnicodeSegmentation;

use super::chrome::panel_block_owned;
use super::terminal_text::{terminal_grapheme_width, text_width, truncate_text_to_width};
use super::{App, MouseHandler, PanelFocus, RequestTreeNodeSnapshot, View};
use crate::app::SearchTitleStatus;
use crate::text_input::{InputCursorMap, TextInput};

pub(super) struct RequestListView {
    area: Rect,
    cache: RequestTreeRenderCache,
    search_field: Option<SearchInputHitRegion>,
}

struct SearchInputHitRegion {
    field: Rect,
    input: InputCursorMap,
    revision: u64,
    cursor: usize,
}

#[derive(Default)]
struct RequestTreeRenderCache {
    revision: Option<u64>,
    items: Vec<TreeItem<'static, String>>,
    opened: HashSet<Vec<String>>,
    visible_rows: usize,
}

impl RequestListView {
    pub(super) fn new() -> Self {
        Self {
            area: Rect::default(),
            cache: RequestTreeRenderCache::default(),
            search_field: None,
        }
    }
}

impl View for RequestListView {
    fn area(&self) -> Rect {
        self.area
    }

    fn set_area(&mut self, area: Rect) {
        if self.area != area {
            self.search_field = None;
        }
        self.area = area;
    }

    fn render(&mut self, frame: &mut Frame, app: &mut App) {
        let revision = app.request_tree_revision();
        let tree_changed = self.cache.revision != Some(revision);
        if tree_changed {
            self.cache.revision = Some(revision);
            self.cache.items = build_request_tree_items(app);
        }
        let opened_changed = self.cache.opened != *app.request_list.state.opened();
        if tree_changed || opened_changed {
            self.cache.visible_rows = app.request_list.state.flatten(&self.cache.items).len();
            self.cache
                .opened
                .clone_from(app.request_list.state.opened());
        }
        let focused = app.is_panel_focused(PanelFocus::RequestList);
        let title_width = usize::from(self.area.width.saturating_sub(4));
        let block = panel_block_owned(request_panel_title(app, title_width), focused);
        let tree_area = block.inner(self.area);
        let viewport_rows = usize::from(block.inner(self.area).height);
        app.request_list
            .update_scroll_bounds(self.cache.visible_rows, viewport_rows);
        let tree = Tree::new(&self.cache.items)
            .expect("request tree identifiers are unique")
            .block(block)
            .highlight_style(Style::default().bg(Color::White).fg(Color::DarkGray))
            .node_closed_symbol("▶ ")
            .node_open_symbol("▼ ")
            .node_no_children_symbol("  ")
            .experimental_scrollbar(Some(
                Scrollbar::default()
                    .orientation(ScrollbarOrientation::VerticalRight)
                    .begin_symbol(Some("↑"))
                    .end_symbol(Some("↓")),
            ));
        frame.render_stateful_widget(tree, self.area, &mut app.request_list.state);
        // Every request-tree item is one terminal row. `rendered_at` maps blank
        // space to the last item, so never ask it about rows beyond this count.
        let remaining_rows = self
            .cache
            .visible_rows
            .saturating_sub(app.request_list.state.get_offset());
        let rendered_rows = tree_area
            .height
            .min(u16::try_from(remaining_rows).unwrap_or(u16::MAX));
        render_search_highlights(frame, app, tree_area, rendered_rows);
        self.search_field = render_search_overlay(frame, app, self.area);
    }
}

impl MouseHandler for RequestListView {
    fn handle_mouse(&mut self, mouse: MouseEvent, app: &mut App) -> bool {
        if app.is_request_search_editing() {
            if let (MouseEventKind::Down(_), Some(region)) = (mouse.kind, &self.search_field)
                && region.revision == app.request_search_input_revision()
                && app
                    .request_search_input()
                    .is_some_and(|input| input.cursor() == region.cursor)
                && region.field.contains((mouse.column, mouse.row).into())
            {
                // The prefix behaves like the first visible input column, even
                // when the query is horizontally scrolled.
                let position = Position::new(mouse.column.max(region.input.area().x), mouse.row);
                if let Some(cursor) = region.input.cursor_at(position) {
                    app.set_request_search_cursor(cursor);
                }
            }
            return true;
        }
        if !self.contains_mouse(mouse) {
            return false;
        }
        match mouse.kind {
            MouseEventKind::ScrollDown => {
                app.request_list.scroll_down();
                true
            }
            MouseEventKind::ScrollUp => {
                app.request_list.scroll_up();
                true
            }
            MouseEventKind::Down(_) => {
                app.focus_panel(PanelFocus::RequestList);
                let changed = app
                    .request_list
                    .click_at(Position::new(mouse.column, mouse.row));
                app.apply_request_list_change(changed);
                true
            }
            _ => false,
        }
    }
}

fn request_panel_title(app: &App, max_width: usize) -> String {
    if app.request_search_query().is_none() && !app.is_request_search_editing() {
        return truncate_text_to_width("Requests", max_width);
    }

    let prefix = "Requests • Searching";
    let status = match app.request_search_title_status() {
        SearchTitleStatus::None => None,
        SearchTitleStatus::Pending => Some("…".to_string()),
        SearchTitleStatus::Matches { selected, total } => Some(match selected {
            Some(selected) => format!("{selected}/{total}"),
            None => format!("–/{total}"),
        }),
        SearchTitleStatus::NoMatches => Some("No matches".to_string()),
        SearchTitleStatus::Failed => Some("Search failed".to_string()),
        SearchTitleStatus::QueryLimitReached => Some("Query limit reached".to_string()),
    };
    let status_suffix = status
        .as_deref()
        .map_or_else(String::new, |status| format!(" • {status}"));
    let refresh_suffix = if app.request_search_refreshing() {
        " • ↻"
    } else {
        ""
    };

    let query = app.request_search_query();
    let full = query.map_or_else(
        || format!("{prefix}{status_suffix}{refresh_suffix}"),
        |query| format!("{prefix} [{query}]{status_suffix}{refresh_suffix}"),
    );
    if usize::from(text_width(&full)) <= max_width {
        return full;
    }

    let without_refresh = query.map_or_else(
        || format!("{prefix}{status_suffix}"),
        |query| format!("{prefix} [{query}]{status_suffix}"),
    );
    if usize::from(text_width(&without_refresh)) <= max_width {
        return without_refresh;
    }

    let priority_only = format!("{prefix}{status_suffix}");
    let fixed_width = usize::from(text_width(prefix))
        .saturating_add(3)
        .saturating_add(usize::from(text_width(&status_suffix)));
    let Some(query) = query.filter(|_| fixed_width < max_width) else {
        return truncate_text_to_width(&priority_only, max_width);
    };
    let fitted_query = truncate_text_to_width(query, max_width - fixed_width);
    format!("{prefix} [{fitted_query}]{status_suffix}")
}

fn render_search_highlights(frame: &mut Frame, app: &App, tree_area: Rect, rendered_rows: u16) {
    let Some(results) = app.request_search_results() else {
        return;
    };
    for screen_row in 0..rendered_rows {
        let y = tree_area.y.saturating_add(screen_row);
        let Some(path) = app
            .request_list
            .state
            .rendered_at(Position::new(tree_area.x, y))
        else {
            continue;
        };
        let Some(search_match) = results.match_for_path(path) else {
            continue;
        };
        let depth = path.len().saturating_sub(1);
        let label_x = tree_area
            .x
            .saturating_add((depth.saturating_mul(2)).try_into().unwrap_or(u16::MAX))
            .saturating_add(2);
        let mut graphemes = search_match.label.grapheme_indices(true).peekable();
        let mut cell_offset = 0_usize;
        for range in search_match.ranges.iter() {
            while graphemes
                .peek()
                .is_some_and(|(byte_start, _)| *byte_start < range.start)
            {
                let (_, grapheme) = graphemes.next().expect("peeked grapheme exists");
                cell_offset = cell_offset.saturating_add(terminal_grapheme_width(grapheme));
            }
            let range_start = cell_offset;
            while graphemes
                .peek()
                .is_some_and(|(byte_start, _)| *byte_start < range.end)
            {
                let (_, grapheme) = graphemes.next().expect("peeked grapheme exists");
                cell_offset = cell_offset.saturating_add(terminal_grapheme_width(grapheme));
            }
            let start = u16::try_from(range_start).unwrap_or(u16::MAX);
            let range_width = cell_offset.saturating_sub(range_start);
            let x = label_x.saturating_add(start);
            let width = u16::try_from(range_width)
                .unwrap_or(u16::MAX)
                .min(tree_area.right().saturating_sub(x));
            if width > 0 && x < tree_area.right() {
                frame.buffer_mut().set_style(
                    Rect::new(x, y, width, 1),
                    Style::default().bg(Color::Yellow).fg(Color::Black),
                );
            }
            if x >= tree_area.right() {
                break;
            }
        }
    }
}

fn render_search_overlay(
    frame: &mut Frame,
    app: &App,
    panel_area: Rect,
) -> Option<SearchInputHitRegion> {
    if !app.is_request_search_editing() || panel_area.width < 3 || panel_area.height < 3 {
        return None;
    }

    let (overlay, field, bordered) = if panel_area.width >= 6 && panel_area.height >= 5 {
        let overlay = Rect::new(
            panel_area.x.saturating_add(1),
            panel_area.y.saturating_add(1),
            panel_area.width.saturating_sub(2),
            3,
        );
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .title("Search")
            .title_alignment(Alignment::Center);
        let field = block.inner(overlay);
        frame.render_widget(Clear, overlay);
        frame.render_widget(block, overlay);
        (overlay, field, true)
    } else {
        let field = panel_area.inner(Margin {
            horizontal: 1,
            vertical: 1,
        });
        (field, Rect::new(field.x, field.y, field.width, 1), false)
    };

    if !bordered {
        frame.render_widget(Clear, overlay);
    }
    let input = app.request_search_input()?;
    let prefix_width = field.width.min(2);
    frame.render_widget(
        Paragraph::new("/ "),
        Rect::new(field.x, field.y, prefix_width, field.height),
    );
    let input_area = Rect::new(
        field.x.saturating_add(prefix_width),
        field.y,
        field.width.saturating_sub(prefix_width),
        field.height,
    );
    let cursor_map = TextInput::new(input.text(), Some(input.cursor()))
        .render_with_cursor_map(input_area, frame.buffer_mut());
    Some(SearchInputHitRegion {
        field,
        input: cursor_map,
        revision: app.request_search_input_revision(),
        cursor: input.cursor(),
    })
}

pub(super) fn build_request_tree_items(app: &mut App) -> Vec<TreeItem<'static, String>> {
    app.request_tree_snapshot()
        .into_iter()
        .map(request_tree_item)
        .collect()
}

fn request_tree_item(node: RequestTreeNodeSnapshot) -> TreeItem<'static, String> {
    if node.branch {
        TreeItem::new(
            node.identifier,
            subtree_label_with_count(subtree_label(&node.label), node.leaf_count),
            node.children.into_iter().map(request_tree_item).collect(),
        )
        .expect("request tree node child identifiers are unique")
    } else {
        TreeItem::new_leaf(node.identifier, node.label)
    }
}

fn subtree_label(label: &str) -> String {
    if label.ends_with('/') {
        label.to_string()
    } else {
        format!("{label}/")
    }
}

fn subtree_label_with_count(label: String, leaf_count: usize) -> Line<'static> {
    Line::from(vec![
        Span::raw(label),
        Span::styled(
            format!(" {leaf_count}"),
            Style::default().fg(Color::DarkGray),
        ),
    ])
}
