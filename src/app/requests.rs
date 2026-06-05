use tui_tree_widget::TreeState;

use super::{
    App,
    event::AppEvent,
    request_tree::{
        DeleteSelectionContext, RequestPathTree, RequestTreeEntry, selected_request_sequence,
    },
};
use crate::proxy_handler::CapturedData;

impl App {
    pub fn add_request(&mut self, req: CapturedData) {
        let was_empty = self.requests.is_empty();
        let tree_entry = RequestTreeEntry::from(&req);
        let insert_pos = self
            .requests
            .partition_point(|existing| existing.sequence <= req.sequence);
        let uri = req.uri.clone();
        self.requests.insert(insert_pos, req);
        log::info!("request received: {}", uri);

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

    pub fn selected_request(&self) -> Option<&CapturedData> {
        selected_request_sequence(self.request_list.state.selected())
            .and_then(|sequence| self.requests.iter().find(|req| req.sequence == sequence))
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

        let tree_before_delete = RequestPathTree::from_requests(&self.requests);
        let len_before_deletion = self.requests.len();

        // deletion
        self.requests.retain(|req| {
            let request_path = RequestTreeEntry::from(req).request_path();
            !request_path.starts_with(&selected_path)
        });
        let removed_count = len_before_deletion - self.requests.len();

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
        let removed_count = self.requests.len();
        self.requests = Vec::new();
        self.request_list.state = TreeState::default();
        self.detail_panel.reset_content_position();
        if removed_count > 0 {
            log::info!("cleared {removed_count} request(s) from request tree");
        }
    }

    pub fn handle_app_event(&mut self, event: AppEvent) {
        match event {
            AppEvent::NetworkRequest(req) => self.add_request(req),
            AppEvent::LogMessage(msg) => self.log_panel.add_log(msg),
            AppEvent::CertificateDownloadReady(download_url) => {
                self.certificate_popup.set_download_url(download_url)
            }
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
        tree_before_delete: &RequestPathTree,
    ) {
        let tree_after_delete = RequestPathTree::from_requests(&self.requests);
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
}
