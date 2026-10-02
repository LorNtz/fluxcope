use std::time::{Duration, Instant};

use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};

use crate::app::{App, ProxyRuleTable, SettingsClickTarget, SettingsPopup};

use super::content::SettingsTableHit;
use super::hit_regions::SettingsHitRegions;

const DOUBLE_CLICK_INTERVAL: Duration = Duration::from_millis(400);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TableRow {
    Prefilter(usize),
    Rule(ProxyRuleTable, usize),
}

impl TableRow {
    fn from_target(target: SettingsClickTarget) -> Option<Self> {
        match target {
            SettingsClickTarget::PrefilterRow {
                index,
                toggle: false,
            } => Some(Self::Prefilter(index)),
            SettingsClickTarget::RuleRow {
                table,
                index,
                toggle: false,
                ..
            } => Some(Self::Rule(table, index)),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ClickContext {
    revision: u64,
    root: Rect,
    viewport: Rect,
    content_scroll: Position,
    table_scroll: [usize; 3],
}

impl ClickContext {
    fn new(popup: &SettingsPopup, regions: &SettingsHitRegions) -> Self {
        Self {
            revision: popup.presentation_revision(),
            root: regions.root_area(),
            viewport: regions.viewport(),
            content_scroll: popup.scroll.offset(),
            table_scroll: [
                popup.prefilter_table_scroll_offset(),
                popup.rule_table_scroll_offset(ProxyRuleTable::Remote),
                popup.rule_table_scroll_offset(ProxyRuleTable::Local),
            ],
        }
    }
}

#[derive(Default)]
pub(super) struct SettingsMouseState {
    last_click: Option<(TableRow, Instant, ClickContext)>,
}

impl SettingsMouseState {
    pub(super) fn reset(&mut self) {
        self.last_click = None;
    }

    fn is_double_click(&mut self, row: TableRow, now: Instant, context: ClickContext) -> bool {
        self.last_click
            .take()
            .is_some_and(|(previous_row, at, previous_context)| {
                row == previous_row
                    && context == previous_context
                    && now.saturating_duration_since(at) <= DOUBLE_CLICK_INTERVAL
            })
    }
}

pub(super) fn handle_settings_popup_mouse(
    mouse: MouseEvent,
    app: &mut App,
    hit_regions: &SettingsHitRegions,
    state: &mut SettingsMouseState,
    now: Instant,
) {
    match mouse.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            let Some(target) = hit_regions.click_target(mouse) else {
                state.reset();
                return;
            };
            let row = TableRow::from_target(target);
            let double_click = row.is_some_and(|row| {
                state.is_double_click(
                    row,
                    now,
                    ClickContext::new(&app.settings_popup, hit_regions),
                )
            });
            state.reset();
            let pending = app.settings_transaction_pending();
            let action = app
                .settings_popup
                .handle_click(target, double_click, pending);
            app.handle_settings_popup_action(action);
            if !double_click
                && !pending
                && let Some(row) = row
            {
                state.last_click = Some((
                    row,
                    now,
                    ClickContext::new(&app.settings_popup, hit_regions),
                ));
            }
        }
        MouseEventKind::ScrollDown | MouseEventKind::ScrollUp => {
            state.reset();
            let position = Position::new(mouse.column, mouse.row);
            if hit_regions.blocks_content_scroll() {
                if hit_regions.viewport().contains(position)
                    && hit_regions.active_select().is_some_and(|select| {
                        select.dropdown_contains(hit_regions.local_position(mouse))
                    })
                {
                    if mouse.kind == MouseEventKind::ScrollDown {
                        app.settings_popup.scroll_active_select_down();
                    } else {
                        app.settings_popup.scroll_active_select_up();
                    }
                }
                return;
            }
            handle_settings_content_scroll_mouse(mouse, app, hit_regions);
        }
        MouseEventKind::ScrollLeft | MouseEventKind::ScrollRight | MouseEventKind::Down(_) => {
            state.reset()
        }
        _ => {}
    }
}

fn handle_settings_content_scroll_mouse(
    mouse: MouseEvent,
    app: &mut App,
    hit_regions: &SettingsHitRegions,
) {
    let position = Position::new(mouse.column, mouse.row);
    if !hit_regions.viewport().contains(position) {
        return;
    }
    let scroll_down = matches!(mouse.kind, MouseEventKind::ScrollDown);
    if let Some(table) = hit_regions.table_hit(hit_regions.local_position(mouse)) {
        let changed = match (table, scroll_down) {
            (SettingsTableHit::Prefilter, true) => app.settings_popup.scroll_prefilter_table_down(),
            (SettingsTableHit::Prefilter, false) => app.settings_popup.scroll_prefilter_table_up(),
            (SettingsTableHit::Proxy(table), true) => {
                app.settings_popup.scroll_rule_table_down(table)
            }
            (SettingsTableHit::Proxy(table), false) => {
                app.settings_popup.scroll_rule_table_up(table)
            }
        };
        if changed {
            return;
        }
    }
    if hit_regions.scrolling_enabled() {
        if scroll_down {
            app.settings_popup.scroll.scroll_down();
        } else {
            app.settings_popup.scroll.scroll_up();
        }
    }
}
