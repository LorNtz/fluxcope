use crossterm::event::MouseEvent;
use ratatui::layout::{Position, Rect};

use crate::app::{ProxyRuleTable, SettingsPopup, SettingsTopic};

use super::content::{
    SettingsContentItem, SettingsContentLayout, SettingsSelectLayout, SettingsTableHit,
    SettingsTableScrollRegion,
};
use super::prefilter::PrefilterTableHitRegion;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SettingsHitRegionKey {
    root_area: Rect,
    topic: SettingsTopic,
    selected_row: usize,
    // Content and row-count mutations advance this revision at their mutation boundary.
    presentation_revision: u64,
    scroll_y: u16,
    // Table offsets are UI-derived state, so they are matched explicitly.
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
    viewport: Rect,
    scroll_y: u16,
    scrolling_enabled: bool,
    table_scroll_regions: Vec<SettingsTableScrollRegion>,
    prefilter_region: Option<PrefilterTableHitRegion>,
    select_regions: Vec<SettingsSelectLayout>,
    active_select: Option<SettingsSelectLayout>,
}

impl SettingsHitRegions {
    pub(super) fn from_rendered(
        root_area: Rect,
        popup: &SettingsPopup,
        viewport: Rect,
        content_layout: &SettingsContentLayout<'_, '_>,
    ) -> Self {
        let allow_table_scroll =
            matches!(popup.topic, SettingsTopic::Recording | SettingsTopic::Proxy)
                && popup.active_select_target().is_none()
                && popup.unsaved_dialog().is_none()
                && popup.rule_editor().is_none();
        let allow_prefilter_click = popup.topic == SettingsTopic::Recording
            && popup.unsaved_dialog().is_none()
            && popup.prefilter_pattern_edit().is_none();
        let mut table_scroll_regions = Vec::new();
        let mut prefilter_region = None;
        let mut select_regions = Vec::new();
        let active_select_target = popup.active_select_target();
        let mut active_select = None;

        for (item, area) in content_layout.item_areas() {
            let SettingsContentItem::FullWidthTable { table, .. } = item else {
                let overlay_bounds = Rect::new(
                    0,
                    area.y,
                    content_layout.content_width,
                    content_layout.buffer_height.saturating_sub(area.y),
                );
                if let Some(select) =
                    item.select_layout(None, area, content_layout.field_layout, overlay_bounds)
                {
                    if active_select_target == Some(select.target) {
                        active_select = Some(select);
                    }
                    select_regions.push(select);
                }
                continue;
            };

            if allow_table_scroll && let Some(region) = table.scroll_region(area) {
                table_scroll_regions.push(region);
            }
            if allow_prefilter_click {
                prefilter_region = table.prefilter_hit_region(area).or(prefilter_region);
            }
        }

        let scroll_y = if content_layout.scrolling_enabled {
            popup.scroll.offset().y
        } else {
            0
        };

        Self {
            key: SettingsHitRegionKey::from_popup(root_area, popup, scroll_y),
            viewport,
            scroll_y,
            scrolling_enabled: content_layout.scrolling_enabled,
            table_scroll_regions,
            prefilter_region,
            select_regions,
            active_select,
        }
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

    pub(super) fn prefilter_hit(&self, position: Position) -> Option<(usize, bool)> {
        self.prefilter_region?.pattern_hit(position)
    }

    pub(super) fn active_select(&self) -> Option<SettingsSelectLayout> {
        self.active_select
    }

    pub(super) fn select_at(&self, position: Position) -> Option<SettingsSelectLayout> {
        self.select_regions
            .iter()
            .copied()
            .find(|select| select.box_contains(position))
    }
}
