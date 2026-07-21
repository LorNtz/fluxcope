use std::cell::RefCell;

use crossterm::event::{MouseEvent, MouseEventKind};
use ratatui::{
    Frame,
    layout::{Position, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Scrollbar, ScrollbarOrientation},
};
use tui_tree_widget::{Tree, TreeItem};

use super::{App, MouseHandler, PanelFocus, RequestTreeNodeSnapshot, View, panel_block};

pub(super) struct RequestListView {
    area: Rect,
    cache: RefCell<RequestTreeRenderCache>,
}

#[derive(Default)]
struct RequestTreeRenderCache {
    revision: Option<u64>,
    items: Vec<TreeItem<'static, String>>,
}

impl RequestListView {
    pub(super) fn new() -> Self {
        Self {
            area: Rect::default(),
            cache: RefCell::new(RequestTreeRenderCache::default()),
        }
    }
}

impl View for RequestListView {
    fn area(&self) -> Rect {
        self.area
    }

    fn set_area(&mut self, area: Rect) {
        self.area = area;
    }

    fn render(&self, frame: &mut Frame, app: &mut App) {
        let revision = app.request_tree_revision();
        if self.cache.borrow().revision != Some(revision) {
            let items = build_request_tree_items(app);
            *self.cache.borrow_mut() = RequestTreeRenderCache {
                revision: Some(revision),
                items,
            };
        }
        let cache = self.cache.borrow();
        let focused = app.is_panel_focused(PanelFocus::RequestList);
        let tree = Tree::new(&cache.items)
            .expect("request tree identifiers are unique")
            .block(panel_block("Requests", focused))
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
    }
}

impl MouseHandler for RequestListView {
    fn handle_mouse(&self, mouse: MouseEvent, app: &mut App) -> bool {
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
