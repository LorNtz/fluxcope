use crossterm::event::MouseEvent;
use ratatui::layout::{Margin, Position, Rect};

use super::content::{
    SettingsContentItem, SettingsContentLayout, SettingsSelectLayout, SettingsTableHit,
    SettingsTableScrollRegion,
};
use crate::app::{ProxyRuleTable, SettingsClickTarget, SettingsPopup, SettingsTopic};
use crate::text_input::InputCursorMap;

pub(super) struct ClickRegion {
    area: Rect,
    target: SettingsClickTarget,
    input: Option<InputCursorMap>,
}

impl ClickRegion {
    pub(super) fn new(area: Rect, target: SettingsClickTarget) -> Self {
        Self {
            area,
            target,
            input: None,
        }
    }

    pub(super) fn input(area: Rect, target: SettingsClickTarget, input: InputCursorMap) -> Self {
        Self {
            area,
            target,
            input: Some(input),
        }
    }

    pub(super) fn set_rule_field(&mut self, field: crate::app::RuleEditField) {
        self.target = SettingsClickTarget::RuleField {
            field,
            cursor: None,
        };
    }

    fn target_at(&self, position: Position) -> Option<SettingsClickTarget> {
        if !self.area.contains(position) {
            return None;
        }
        let cursor = self
            .input
            .as_ref()
            .and_then(|input| input.cursor_at(position));
        Some(match self.target {
            SettingsClickTarget::Field { row, .. } => SettingsClickTarget::Field { row, cursor },
            SettingsClickTarget::PrefilterInput { index, .. } => {
                SettingsClickTarget::PrefilterInput { index, cursor }
            }
            SettingsClickTarget::SelectFilter { .. } => {
                SettingsClickTarget::SelectFilter { cursor }
            }
            SettingsClickTarget::RuleField { field, .. } => {
                SettingsClickTarget::RuleField { field, cursor }
            }
            target => target,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SettingsHitRegionKey {
    root_area: Rect,
    topic: SettingsTopic,
    selected_row: usize,
    presentation_revision: u64,
    scroll_y: u16,
    prefilter_scroll: usize,
    remote_rule_scroll: usize,
    local_rule_scroll: usize,
}

impl SettingsHitRegionKey {
    fn from_popup(root_area: Rect, popup: &SettingsPopup, scroll_y: u16) -> Self {
        Self {
            root_area,
            topic: popup.topic,
            selected_row: popup.selected_row,
            presentation_revision: popup.presentation_revision(),
            scroll_y,
            prefilter_scroll: popup.prefilter_table_scroll_offset(),
            remote_rule_scroll: popup.rule_table_scroll_offset(ProxyRuleTable::Remote),
            local_rule_scroll: popup.rule_table_scroll_offset(ProxyRuleTable::Local),
        }
    }
}

pub(super) struct SettingsHitRegions {
    key: SettingsHitRegionKey,
    popup_area: Rect,
    topics: Rect,
    viewport: Rect,
    scroll_y: u16,
    scrolling_enabled: bool,
    table_scroll_regions: Vec<SettingsTableScrollRegion>,
    controls: Vec<ClickRegion>,
    active_select: Option<SettingsSelectLayout>,
    modal_active: bool,
    overlay: Option<(Rect, Vec<ClickRegion>)>,
}

impl SettingsHitRegions {
    pub(super) fn empty(root_area: Rect, popup: &SettingsPopup, viewport: Rect) -> Self {
        let layout = super::settings_popup_layout(root_area);
        Self {
            key: SettingsHitRegionKey::from_popup(root_area, popup, 0),
            popup_area: layout.area,
            topics: layout.topics.inner(Margin {
                horizontal: 1,
                vertical: 1,
            }),
            viewport,
            scroll_y: 0,
            scrolling_enabled: false,
            table_scroll_regions: Vec::new(),
            controls: Vec::new(),
            active_select: None,
            modal_active: popup.active_select_target().is_some()
                || popup.rule_editor().is_some()
                || popup.is_confirming_unsaved(),
            overlay: None,
        }
    }

    pub(super) fn from_rendered(
        root_area: Rect,
        popup: &SettingsPopup,
        viewport: Rect,
        content_layout: &SettingsContentLayout<'_, '_>,
        controls: Vec<ClickRegion>,
    ) -> Self {
        let mut regions = Self::empty(root_area, popup, viewport);
        regions.controls = controls;
        regions.scrolling_enabled = content_layout.scrolling_enabled;
        for (item, area) in content_layout.item_areas() {
            if let SettingsContentItem::FullWidthTable { table, .. } = item {
                if !regions.modal_active
                    && let Some(region) = table.scroll_region(area)
                {
                    regions.table_scroll_regions.push(region);
                }
            } else {
                let bounds = Rect::new(
                    0,
                    area.y,
                    content_layout.content_width,
                    content_layout.buffer_height.saturating_sub(area.y),
                );
                if let Some(select) =
                    item.select_layout(None, area, content_layout.field_layout, bounds)
                    && popup.active_select_target() == Some(select.target)
                    && select.layout.box_area.width >= 3
                    && select.layout.box_area.height >= 3
                {
                    regions.active_select = Some(select);
                }
            }
        }
        regions.set_scroll_y(if regions.scrolling_enabled {
            popup.scroll.offset().y
        } else {
            0
        });
        regions
    }

    pub(super) fn set_overlay(&mut self, area: Rect, controls: Vec<ClickRegion>) {
        self.overlay = Some((area.intersection(self.key.root_area), controls));
    }

    pub(super) fn click_target(&self, mouse: MouseEvent) -> Option<SettingsClickTarget> {
        let position = Position::new(mouse.column, mouse.row);
        if let Some((area, controls)) = &self.overlay {
            return if area.contains(position) {
                controls
                    .iter()
                    .rev()
                    .find_map(|region| region.target_at(position))
            } else {
                Some(SettingsClickTarget::Outside)
            };
        }
        if self.modal_active {
            if let Some(select) = self.active_select
                && self.viewport.contains(position)
            {
                let local = self.local_position(mouse);
                if select.box_contains(local) {
                    return self
                        .controls
                        .iter()
                        .rev()
                        .filter(|region| {
                            matches!(region.target, SettingsClickTarget::SelectFilter { .. })
                        })
                        .find_map(|region| region.target_at(local));
                }
                if select.dropdown_contains(local) {
                    return select
                        .layout
                        .option_at(local)
                        .map(|index| SettingsClickTarget::SelectOption { index });
                }
            }
            return Some(SettingsClickTarget::Outside);
        }
        if !self.popup_area.contains(position) {
            return Some(SettingsClickTarget::Outside);
        }
        if self.topics.contains(position) {
            return Some(
                SettingsTopic::all()
                    .get(usize::from(position.y - self.topics.y))
                    .copied()
                    .map_or(SettingsClickTarget::Background, SettingsClickTarget::Topic),
            );
        }
        if !self.viewport.contains(position) {
            return Some(SettingsClickTarget::Background);
        }
        let local = self.local_position(mouse);
        self.controls
            .iter()
            .rev()
            .find_map(|region| region.target_at(local))
            .or(Some(SettingsClickTarget::Background))
    }

    pub(super) fn blocks_content_scroll(&self) -> bool {
        self.modal_active
    }
    pub(super) fn root_area(&self) -> Rect {
        self.key.root_area
    }

    pub(super) fn matches(&self, root_area: Rect, popup: &SettingsPopup) -> bool {
        let scroll_y = if self.scrolling_enabled {
            popup.scroll.offset().y
        } else {
            0
        };
        self.key == SettingsHitRegionKey::from_popup(root_area, popup, scroll_y)
    }

    pub(super) fn set_scroll_y(&mut self, scroll_y: u16) {
        self.scroll_y = scroll_y;
        self.key.scroll_y = scroll_y;
    }
    pub(super) fn viewport(&self) -> Rect {
        self.viewport
    }
    pub(super) fn scrolling_enabled(&self) -> bool {
        self.scrolling_enabled
    }
    pub(super) fn local_position(&self, mouse: MouseEvent) -> Position {
        Position::new(
            mouse.column.saturating_sub(self.viewport.x),
            mouse
                .row
                .saturating_sub(self.viewport.y)
                .saturating_add(self.scroll_y),
        )
    }
    pub(super) fn table_hit(&self, position: Position) -> Option<SettingsTableHit> {
        self.table_scroll_regions
            .iter()
            .find_map(|region| region.hit_at(position))
    }
    pub(super) fn active_select(&self) -> Option<SettingsSelectLayout> {
        self.active_select
    }
}
