use tui_tree_widget::TreeState;

use super::{
    App,
    request_tree::{
        DeleteSelectionContext, RequestTreeEntry, RequestTreeModel, RequestTreeNodeSnapshot,
        selected_request_sequence,
    },
};
#[cfg(test)]
use crate::capture::CapturedExchange;
use crate::capture::{CaptureRecord, CaptureSummary};
use std::sync::Arc;

impl App {
    #[cfg(test)]
    pub fn add_request(&mut self, req: CapturedExchange) {
        self.add_capture(CaptureRecord::from_completed(req));
    }

    pub fn add_capture(&mut self, record: Arc<CaptureRecord>) {
        let was_empty = self.captures.is_empty();
        let summary = record.summary();
        let tree_entry = RequestTreeEntry::from(&summary);
        let uri = summary.request.original_uri.clone();
        let evicted = self.captures.insert(record);
        log::info!("request received: {}", uri);

        if !evicted.is_empty() {
            self.repair_request_list_after_retention_eviction();
        }

        if self.request_list.auto_expand {
            self.request_list.open_entry(&tree_entry);
        }
        if was_empty {
            if self.request_list.auto_expand {
                self.request_list.select_entry(&tree_entry);
            } else {
                self.request_list.select_origin(&tree_entry);
            }
        }
    }

    pub fn next(&mut self) {
        let changed = self.request_list.state.key_down();
        self.apply_request_list_change(changed);
    }

    pub fn previous(&mut self) {
        let changed = self.request_list.state.key_up();
        self.apply_request_list_change(changed);
    }

    pub fn toggle_selected_request_subtree(&mut self) -> bool {
        let Some(selected_path) = self.selected_request_subtree_path() else {
            return false;
        };

        let changed = self.request_list.state.toggle(selected_path);
        self.apply_request_list_change(changed);
        changed
    }

    pub fn open_selected_request_subtree(&mut self) -> bool {
        let Some(selected_path) = self.selected_request_subtree_path() else {
            return false;
        };

        let changed = self.request_list.state.open(selected_path);
        self.apply_request_list_change(changed);
        changed
    }

    pub fn expand_selected_request_subtree(&mut self) -> bool {
        let Some(selected_path) = self.selected_request_subtree_path() else {
            return false;
        };

        let changed =
            self.open_matching_request_tree_branch_paths(|path| path.starts_with(&selected_path));
        self.apply_request_list_change(changed);
        changed
    }

    pub fn expand_all_request_subtrees(&mut self) -> bool {
        let changed = self.open_matching_request_tree_branch_paths(|_| true);
        self.apply_request_list_change(changed);
        changed
    }

    pub fn collapse_all_request_subtrees(&mut self) -> bool {
        let selected_origin = self.request_list.state.selected().first().cloned();
        let mut changed = self.request_list.state.close_all();
        if let Some(origin) = selected_origin {
            changed |= self.request_list.state.select(vec![origin]);
        }

        self.apply_request_list_change(changed);
        changed
    }

    pub fn collapse_selected_request_subtree_children(&mut self) -> bool {
        let Some(selected_path) = self.selected_request_subtree_path() else {
            return false;
        };

        let child_opened_paths = self
            .request_list
            .state
            .opened()
            .iter()
            .filter(|path| path.len() > selected_path.len() && path.starts_with(&selected_path))
            .cloned()
            .collect::<Vec<_>>();

        let mut changed = false;
        for path in child_opened_paths {
            changed |= self.request_list.state.close(&path);
        }
        changed |= self.request_list.state.open(selected_path);

        self.apply_request_list_change(changed);
        changed
    }

    fn open_matching_request_tree_branch_paths(
        &mut self,
        mut should_open: impl FnMut(&[String]) -> bool,
    ) -> bool {
        self.ensure_request_tree_model();
        let paths = self.request_tree.branch_paths();
        let mut changed = false;
        for path in paths {
            if should_open(&path) {
                changed |= self.request_list.state.open(path);
            }
        }
        changed
    }

    fn selected_request_subtree_path(&self) -> Option<Vec<String>> {
        let selected_path = self.request_list.state.selected();
        if selected_path.is_empty() || selected_request_sequence(selected_path).is_some() {
            None
        } else {
            Some(selected_path.to_vec())
        }
    }

    pub fn selected_request_leaf(&self) -> bool {
        self.selected_request_sequence().is_some()
    }

    pub fn selected_request_sequence(&self) -> Option<crate::capture::CaptureSequence> {
        selected_request_sequence(self.request_list.state.selected())
    }

    pub fn selected_request(&self) -> Option<CaptureSummary> {
        self.selected_request_sequence()
            .and_then(|sequence| self.captures.get(sequence))
            .map(|record| record.summary())
    }

    pub(crate) fn selected_capture_record(&self) -> Option<Arc<CaptureRecord>> {
        self.selected_request_sequence()
            .and_then(|sequence| self.captures.get(sequence))
    }

    /// Deletes the selected request leaf or every request under the selected branch.
    ///
    /// Returns the number of captured requests removed. The selected tree path is
    /// resolved against each request's stable tree path, so selecting an origin or
    /// path segment removes the whole visible subtree represented by that node.
    pub fn delete_selected_requests(&mut self) -> usize {
        let selected_path = self.request_list.state.selected().to_vec();
        if selected_path.is_empty() {
            return 0;
        }

        self.ensure_request_tree_model();
        let tree_before_delete = self.request_tree.clone();
        let sequences = tree_before_delete.sequences_under(&selected_path);
        let removed_count = self.captures.remove_sequences(sequences);

        if removed_count > 0 {
            log::info!("deleted {removed_count} request(s) from request tree");
            self.rebuild_request_list_state_after_delete(&selected_path, &tree_before_delete);
            self.detail_panel.reset_content_position();
        }

        removed_count
    }

    /// Removes all captured requests and resets request-tree UI state.
    ///
    /// Replaces the request vector with a fresh allocation so the memory used by
    /// captured request/response bodies can be released promptly.
    pub fn clear_requests(&mut self) {
        let removed_count = self.captures.clear();
        self.request_list.state = TreeState::default();
        self.detail_panel.reset_content_position();
        if removed_count > 0 {
            log::info!("cleared {removed_count} request(s) from request tree");
        }
    }

    pub fn apply_request_list_change(&mut self, changed: bool) {
        if changed {
            self.detail_panel.reset_content_position();
        }
    }

    /// Rebuilds request-tree UI state after a delete while preserving valid opens.
    ///
    /// The tree state stores paths independently from the captured request list,
    /// so deleted paths and empty ancestors must be discarded before choosing the
    /// next selection.
    fn rebuild_request_list_state_after_delete(
        &mut self,
        deleted_path: &[String],
        tree_before_delete: &RequestTreeModel,
    ) {
        self.ensure_request_tree_model();
        let tree_after_delete = self.request_tree.clone();
        let opened_paths_before_delete = self
            .request_list
            .state
            .opened()
            .iter()
            .cloned()
            .collect::<Vec<_>>();
        let preserved_opened_paths = opened_paths_before_delete
            .iter()
            .filter(|path| tree_after_delete.contains(path))
            .cloned()
            .collect::<Vec<_>>();
        let visible_paths = tree_after_delete.visible_paths(&preserved_opened_paths);
        let path_to_select = DeleteSelectionContext::new(
            deleted_path,
            tree_before_delete,
            &tree_after_delete,
            &visible_paths,
        )
        .selected_path();

        // Keep the current scroll offset; rendering will adjust it only if the
        // newly selected row falls outside the viewport.
        for opened_path in opened_paths_before_delete {
            if !tree_after_delete.contains(&opened_path) {
                self.request_list.state.close(&opened_path);
            }
        }
        self.request_list.state.select(path_to_select);
    }

    pub fn captures(&self) -> impl DoubleEndedIterator<Item = CaptureSummary> + '_ {
        self.captures.iter().map(|capture| capture.summary())
    }

    pub fn capture_count(&self) -> usize {
        self.captures.len()
    }

    pub(crate) fn evict_oldest_capture(&mut self) -> bool {
        if self.captures.evict_oldest().is_none() {
            return false;
        }
        self.repair_request_list_after_retention_eviction();
        true
    }

    #[cfg(test)]
    pub fn capture_retained_bytes(&self) -> usize {
        self.captures.retained_bytes()
    }

    #[cfg(test)]
    pub fn capture_at(&self, index: usize) -> Option<CaptureSummary> {
        self.captures
            .iter()
            .nth(index)
            .map(|capture| capture.summary())
    }

    fn repair_request_list_after_retention_eviction(&mut self) {
        self.ensure_request_tree_model();
        let tree = self.request_tree.clone();
        let opened = self
            .request_list
            .state
            .opened()
            .iter()
            .filter(|path| tree.contains(path))
            .cloned()
            .collect::<Vec<_>>();
        let visible = tree.visible_paths(&opened);
        let selected = self.request_list.state.selected().to_vec();
        if !tree.contains(&selected) {
            self.request_list
                .state
                .select(visible.first().cloned().unwrap_or_default());
            self.detail_panel.reset_content_position();
        }
    }

    pub(crate) fn request_tree_snapshot(&mut self) -> Vec<RequestTreeNodeSnapshot> {
        self.ensure_request_tree_model();
        self.request_tree.snapshot()
    }

    pub(crate) fn request_tree_revision(&mut self) -> u64 {
        self.ensure_request_tree_model();
        self.request_tree_revision
    }

    fn ensure_request_tree_model(&mut self) {
        let revision = self.captures.revision();
        if revision == self.request_tree_revision {
            return;
        }
        self.request_tree = RequestTreeModel::from_requests(self.captures());
        self.request_tree_revision = revision;
    }
}
