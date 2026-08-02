use super::{detail_content_area, detail_panel_block};
use crate::app::{App, BODY_LOADING_TEXT, BODY_TEXT_TAB_WIDTH, BodyRenderText};
use edtui::{EditorStatusLine, EditorTheme, EditorView};
use ratatui::{
    Frame,
    buffer::Buffer,
    layout::{Position, Rect},
    style::{Color, Modifier, Style},
    text::Text,
    widgets::{Paragraph, Wrap},
};
use std::collections::HashSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct JumpCandidate {
    label: char,
    position: Position,
}

pub(super) fn detail_text_paragraph<'a>(text: impl Into<Text<'a>>, focused: bool) -> Paragraph<'a> {
    Paragraph::new(text)
        .block(detail_panel_block(focused))
        .wrap(Wrap { trim: false })
}

pub(super) fn body_render_text(body_text: BodyRenderText<'_>) -> &str {
    match body_text {
        BodyRenderText::Loading => BODY_LOADING_TEXT,
        BodyRenderText::Text(text) => text,
    }
}

pub(super) fn should_render_active_body_editor(app: &mut App) -> bool {
    if !app.detail_panel.body_viewer.is_active() || !app.detail_panel.active_tab.is_body() {
        return false;
    }

    let Some(key) = app.current_body_viewer_key() else {
        return false;
    };

    app.ensure_current_body_viewer_content() && app.detail_panel.body_viewer.has_content_for(key)
}

pub(super) fn render_body_editor(frame: &mut Frame, app: &mut App, focused: bool, area: Rect) {
    app.detail_panel.suspend_scroll_bounds();

    if let Some(editor) = app.detail_panel.body_viewer.editor_mut() {
        frame.render_widget(
            EditorView::new(editor)
                .theme(body_editor_theme(focused))
                .wrap(true)
                .tab_width(BODY_TEXT_TAB_WIDTH),
            area,
        );
    }

    render_jump_overlay(frame.buffer_mut(), app, body_editor_text_area(area));
}

fn body_editor_theme(focused: bool) -> EditorTheme<'static> {
    EditorTheme::default()
        .base(Style::default().fg(Color::Reset))
        .cursor_style(Style::default().bg(Color::Green).fg(Color::Black))
        .selection_style(Style::default().bg(Color::White).fg(Color::DarkGray))
        .block(detail_panel_block(focused))
        .status_line(
            EditorStatusLine::default()
                .style_text(
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::Green)
                        .add_modifier(Modifier::BOLD),
                )
                .style_line(Style::default().fg(Color::Reset)),
        )
}

pub(in crate::ui) fn body_editor_text_area(area: Rect) -> Rect {
    let content_area = detail_content_area(area);
    Rect {
        height: content_area.height.saturating_sub(1),
        ..content_area
    }
}

fn render_jump_overlay(buffer: &mut Buffer, app: &mut App, area: Rect) {
    let Some(query) = app
        .detail_panel
        .body_viewer
        .jump_overlay_query(area)
        .map(str::to_string)
    else {
        render_jump_targets(
            buffer,
            app.detail_panel.body_viewer.rendered_jump_targets(area),
        );
        return;
    };

    let candidates = visible_jump_candidates(buffer, area, &query);
    render_jump_candidates(buffer, &candidates.labeled);
    app.detail_panel.body_viewer.replace_visible_jump_targets(
        &query,
        area,
        candidates
            .labeled
            .iter()
            .map(|candidate| (candidate.label, candidate.position))
            .collect(),
        candidates.has_matches,
    );
}

fn visible_jump_candidates(buffer: &Buffer, area: Rect, query: &str) -> VisibleJumpCandidates {
    if area.is_empty() || query.is_empty() {
        return VisibleJumpCandidates::default();
    }

    let query = query
        .chars()
        .map(|ch| ch.to_ascii_lowercase())
        .collect::<Vec<_>>();
    let query_width = query.len();
    if query_width == 0 || query_width > area.width as usize {
        return VisibleJumpCandidates::default();
    }

    let mut matches = Vec::with_capacity(JUMP_LABEL_COUNT);
    let mut ambiguous_labels = HashSet::with_capacity(JUMP_LABEL_COUNT);
    let mut has_matches = false;
    for y in area.y..area.bottom() {
        let row = (area.x..area.right())
            .map(|x| {
                buffer[(x, y)]
                    .symbol()
                    .chars()
                    .next()
                    .unwrap_or(' ')
                    .to_ascii_lowercase()
            })
            .collect::<Vec<_>>();

        for column in 0..=row.len().saturating_sub(query_width) {
            if row[column..column + query_width] != query {
                continue;
            }
            has_matches = true;
            if let Some(next_ch) = row
                .get(column + query_width)
                .filter(|next_ch| JUMP_LABELS.contains(**next_ch))
            {
                ambiguous_labels.insert(*next_ch);
            }
            if matches.len() >= JUMP_LABEL_COUNT {
                continue;
            }
            let Ok(column) = u16::try_from(column) else {
                continue;
            };
            matches.push(Position::new(area.x.saturating_add(column), y));
        }
    }

    let labeled = JUMP_LABELS
        .chars()
        .filter(|label| !ambiguous_labels.contains(&label.to_ascii_lowercase()))
        .zip(matches.iter().copied())
        .map(|(label, position)| JumpCandidate { label, position })
        .collect();

    VisibleJumpCandidates {
        has_matches,
        labeled,
    }
}

fn render_jump_candidates(buffer: &mut Buffer, candidates: &[JumpCandidate]) {
    let style = jump_label_style();
    for candidate in candidates {
        render_jump_label(buffer, candidate.label, candidate.position, style);
    }
}

fn render_jump_targets(buffer: &mut Buffer, targets: &[(char, Position)]) {
    let style = jump_label_style();
    for (label, position) in targets {
        render_jump_label(buffer, *label, *position, style);
    }
}

fn render_jump_label(buffer: &mut Buffer, label: char, position: Position, style: Style) {
    buffer[position].set_char(label).set_style(style);
}

fn jump_label_style() -> Style {
    Style::default().fg(Color::Black).bg(Color::Green)
}

const JUMP_LABELS: &str = "asdfghjklqwertyuiopzxcvbnm";
const JUMP_LABEL_COUNT: usize = JUMP_LABELS.len();

#[derive(Default)]
struct VisibleJumpCandidates {
    has_matches: bool,
    labeled: Vec<JumpCandidate>,
}
