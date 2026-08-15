use std::sync::Arc;
#[cfg(test)]
use std::sync::atomic::AtomicBool;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::{
    app::{
        App,
        single_line_input::{InputEditOutcome, InputViewport, SingleLineInput},
    },
    request_search::{RequestSearchDispatch, SearchJobOutcome, SearchRequest, SearchResults},
};

use super::{
    DispatchIntent, MAX_STALE_MATCH_PROBES, OutcomeApplication, SearchTitleStatus,
    TransientOpeningReason,
};

impl App {
    pub(crate) fn begin_request_search(&mut self) {
        if self.request_search.is_editing() {
            return;
        }
        self.request_search.begin_editing(
            self.request_list.state.selected().to_vec(),
            self.request_list.state.get_offset(),
        );
    }

    pub(crate) fn handle_request_search_edit_key(&mut self, key: KeyEvent) {
        if !self.request_search.is_editing() {
            return;
        }
        match key.code {
            KeyCode::Esc => self.cancel_request_search_editing(),
            KeyCode::Enter => {
                if self
                    .request_search
                    .input()
                    .is_some_and(|input| input.text().is_empty())
                {
                    self.cancel_request_search_editing();
                } else {
                    self.request_search.commit_editing();
                }
            }
            KeyCode::Left if key.modifiers.is_empty() => {
                self.request_search
                    .input_mut()
                    .map(SingleLineInput::move_left);
            }
            KeyCode::Right if key.modifiers.is_empty() => {
                self.request_search
                    .input_mut()
                    .map(SingleLineInput::move_right);
            }
            KeyCode::Home if key.modifiers.is_empty() => {
                self.request_search
                    .input_mut()
                    .map(SingleLineInput::move_home);
            }
            KeyCode::End if key.modifiers.is_empty() => {
                self.request_search
                    .input_mut()
                    .map(SingleLineInput::move_end);
            }
            KeyCode::Backspace if key.modifiers.is_empty() => {
                let outcome = self
                    .request_search
                    .input_mut()
                    .map_or(InputEditOutcome::Unchanged, SingleLineInput::backspace);
                self.apply_request_search_input_edit(outcome);
            }
            KeyCode::Delete if key.modifiers.is_empty() => {
                let outcome = self
                    .request_search
                    .input_mut()
                    .map_or(InputEditOutcome::Unchanged, SingleLineInput::delete);
                self.apply_request_search_input_edit(outcome);
            }
            KeyCode::Char(character)
                if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT =>
            {
                let outcome = self
                    .request_search
                    .input_mut()
                    .map_or(InputEditOutcome::Unchanged, |input| {
                        input.insert_char(character)
                    });
                self.apply_request_search_input_edit(outcome);
            }
            _ => {}
        }
    }

    pub(crate) fn handle_request_search_paste(&mut self, pasted: &str) -> bool {
        let Some(input) = self.request_search.input_mut() else {
            return false;
        };
        let outcome = input.paste(pasted);
        self.apply_request_search_input_edit(outcome);
        true
    }

    fn apply_request_search_input_edit(&mut self, outcome: InputEditOutcome) {
        match outcome {
            InputEditOutcome::Unchanged => {}
            InputEditOutcome::LimitReached => self.request_search.mark_limit_reached(),
            InputEditOutcome::Changed => {
                self.restore_request_search_editing_baseline();
                self.ensure_request_tree_model();
                self.request_search
                    .activate_edited_query(self.request_tree_revision);
            }
        }
    }

    fn cancel_request_search_editing(&mut self) {
        let Some(mut restore) = self.request_search.cancel_editing() else {
            return;
        };
        restore.paths_to_close.sort_by_key(Vec::len);
        for path in restore.paths_to_close.into_iter().rev() {
            self.request_list.state.close(&path);
        }
        self.restore_request_search_selection(&restore.selected_path);
        self.request_list
            .restore_scroll_offset(restore.scroll_offset);
        self.ensure_request_tree_model();
        self.request_search
            .refresh_for_tree(self.request_tree_revision);
    }

    fn restore_request_search_editing_baseline(&mut self) {
        let mut paths_to_close = self.request_search.clear_match_reveal_reasons();
        paths_to_close.sort_by_key(Vec::len);
        for path in paths_to_close.into_iter().rev() {
            self.request_list.state.close(&path);
        }
        let Some((selected, offset)) = self
            .request_search
            .rollback_selection_and_offset()
            .map(|(selected, offset)| (selected.to_vec(), offset))
        else {
            return;
        };
        self.restore_request_search_selection(&selected);
        self.request_list.restore_scroll_offset(offset);
    }

    fn restore_request_search_selection(&mut self, selected: &[String]) {
        self.ensure_request_tree_model();
        let changed = if selected.is_empty() || self.request_tree.contains(selected) {
            self.request_list.state.select(selected.to_vec())
        } else if !self
            .request_tree
            .contains(self.request_list.state.selected())
        {
            self.request_list.state.select(Vec::new())
        } else {
            false
        };
        self.apply_request_list_change(changed);
    }

    pub(crate) fn clear_committed_request_search(&mut self) -> bool {
        self.request_search.clear_committed()
    }

    pub(crate) fn take_request_search_dispatch(&mut self) -> Option<RequestSearchDispatch> {
        if self.request_search.is_active() && self.captures.revision() != self.request_tree_revision
        {
            self.ensure_request_tree_model();
        }
        match self.request_search.take_dispatch_intent()? {
            DispatchIntent::Cancel => Some(RequestSearchDispatch::Cancel),
            DispatchIntent::Run { key, query } => Some(RequestSearchDispatch::Run(SearchRequest {
                key,
                query,
                tree: Arc::clone(&self.request_tree),
            })),
        }
    }

    pub(crate) fn apply_request_search_outcome(&mut self, outcome: &SearchJobOutcome) -> bool {
        let OutcomeApplication::Applied { select_first } =
            self.request_search.apply_outcome(outcome)
        else {
            return false;
        };
        if !select_first {
            return true;
        }
        let Some(results) = self.request_search.results() else {
            return true;
        };
        if let Some(first_match) = results
            .matches()
            .iter()
            .find(|search_match| self.request_tree.contains(&search_match.path))
        {
            self.reveal_request_search_match(&first_match.path, true);
        }
        true
    }

    fn reveal_request_search_match(&mut self, path: &[String], editing_reveal: bool) {
        if editing_reveal {
            for path in self
                .request_search
                .clear_match_reveal_reasons()
                .into_iter()
                .rev()
            {
                self.request_list.state.close(&path);
            }
        }
        for length in 1..path.len() {
            let ancestor = path[..length].to_vec();
            let newly_opened = self.request_list.state.open(ancestor.clone());
            if editing_reveal {
                self.request_search.note_transient_opening(
                    ancestor,
                    TransientOpeningReason::MatchReveal,
                    newly_opened,
                );
            }
        }
        let changed = self.request_list.state.select(path.to_vec());
        self.apply_request_list_change(changed);
    }

    pub(crate) fn note_auto_expand_opening(&mut self, path: Vec<String>) {
        let newly_opened = self.request_list.state.open(path.clone());
        self.request_search.note_transient_opening(
            path,
            TransientOpeningReason::AutoExpand,
            newly_opened,
        );
    }

    pub(crate) fn navigate_request_search(&mut self, forward: bool) -> bool {
        if !self.request_search.is_committed() {
            return false;
        }
        self.ensure_request_tree_model();
        if self.request_tree.is_empty() {
            return false;
        }
        let Some(results) = self.request_search.results() else {
            return false;
        };
        let matches = results.matches();
        if matches.is_empty() {
            return false;
        }
        let selected_position = self
            .request_tree
            .logical_position(self.request_list.state.selected());
        let start = match (forward, selected_position) {
            (true, Some(position)) => {
                matches.partition_point(|search_match| search_match.logical_position <= position)
            }
            (false, Some(position)) => matches
                .partition_point(|search_match| search_match.logical_position < position)
                .checked_sub(1)
                .unwrap_or(matches.len() - 1),
            (true, None) => 0,
            (false, None) => matches.len() - 1,
        };
        // A same-query refresh normally replaces stale results quickly. Bound the
        // fallback scan so a large deleted subtree cannot turn a key press into
        // work proportional to every previous match on the UI thread.
        for step in 0..matches.len().min(MAX_STALE_MATCH_PROBES) {
            let index = if forward {
                (start + step) % matches.len()
            } else {
                (start + matches.len() - step) % matches.len()
            };
            let candidate = &matches[index];
            if self.request_tree.contains(&candidate.path) {
                self.reveal_request_search_match(&candidate.path, false);
                return true;
            }
        }
        false
    }

    pub(crate) fn request_search_results(&self) -> Option<Arc<SearchResults>> {
        self.request_search.results()
    }

    pub(crate) fn is_request_search_editing(&self) -> bool {
        self.request_search.is_editing()
    }

    pub(crate) fn request_search_query(&self) -> Option<&str> {
        self.request_search.query()
    }

    pub(crate) fn request_search_title_status(&self) -> SearchTitleStatus {
        self.request_search
            .title_status(self.request_list.state.selected())
    }

    pub(crate) fn request_search_refreshing(&self) -> bool {
        self.request_search.is_refreshing()
    }

    pub(crate) fn request_search_input_viewport(&self, width: usize) -> Option<InputViewport<'_>> {
        self.request_search
            .input()
            .map(|input| input.viewport(width))
    }

    pub(crate) fn set_request_search_cursor_from_column(
        &mut self,
        width: usize,
        column: usize,
    ) -> bool {
        self.request_search
            .input_mut()
            .is_some_and(|input| input.set_cursor_from_column(width, column))
    }

    #[cfg(test)]
    pub(crate) fn complete_pending_request_search(&mut self) {
        let Some(RequestSearchDispatch::Run(request)) = self.take_request_search_dispatch() else {
            return;
        };
        let outcome = crate::request_search::run_search(&request, &AtomicBool::new(false));
        self.apply_request_search_outcome(&outcome);
    }
}
