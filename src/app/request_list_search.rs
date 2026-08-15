use std::{collections::HashMap, sync::Arc};

use crate::request_search::{SearchJobOutcome, SearchRequestKey, SearchResults};

use super::single_line_input::SingleLineInput;

mod controller;

#[derive(Clone, Debug)]
enum ActiveSearchProgress {
    Pending,
    Ready(Arc<SearchResults>),
    Refreshing(Arc<SearchResults>),
    Failed(Option<Arc<SearchResults>>),
}

#[derive(Clone, Debug)]
struct ActiveSearch {
    query: Arc<str>,
    key: SearchRequestKey,
    progress: ActiveSearchProgress,
    select_first_when_ready: bool,
}

impl ActiveSearch {
    fn results(&self) -> Option<&Arc<SearchResults>> {
        match &self.progress {
            ActiveSearchProgress::Pending | ActiveSearchProgress::Failed(None) => None,
            ActiveSearchProgress::Ready(results)
            | ActiveSearchProgress::Refreshing(results)
            | ActiveSearchProgress::Failed(Some(results)) => Some(results),
        }
    }
}

#[derive(Clone, Debug)]
struct CommittedSearch {
    active: ActiveSearch,
}

#[derive(Clone, Debug)]
struct EditingRollback {
    selected_path: Vec<String>,
    scroll_offset: usize,
    prior_committed: Option<CommittedSearch>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct TransientReasons {
    match_reveal: bool,
    auto_expand: bool,
}

#[derive(Clone, Debug)]
struct EditingSearch {
    input: SingleLineInput,
    active: Option<ActiveSearch>,
    rollback: EditingRollback,
    transient_openings: HashMap<Vec<String>, TransientReasons>,
    query_limit_reached: bool,
}

#[derive(Clone, Debug, Default)]
enum RequestSearchMode {
    #[default]
    Idle,
    Editing(EditingSearch),
    Committed(CommittedSearch),
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum DispatchIntent {
    Run {
        key: SearchRequestKey,
        query: Arc<str>,
    },
    Cancel,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TransientOpeningReason {
    MatchReveal,
    AutoExpand,
}

#[derive(Debug)]
struct EditingRestore {
    selected_path: Vec<String>,
    scroll_offset: usize,
    paths_to_close: Vec<Vec<String>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OutcomeApplication {
    Ignored,
    Applied { select_first: bool },
}

const MAX_STALE_MATCH_PROBES: usize = 64;

#[derive(Debug, Default)]
pub(crate) struct RequestListSearch {
    mode: RequestSearchMode,
    next_generation: u64,
    dispatch: Option<DispatchIntent>,
}

impl RequestListSearch {
    pub(in crate::app) fn is_editing(&self) -> bool {
        matches!(self.mode, RequestSearchMode::Editing(_))
    }

    pub(crate) fn is_committed(&self) -> bool {
        matches!(self.mode, RequestSearchMode::Committed(_))
    }

    pub(in crate::app) fn is_active(&self) -> bool {
        !matches!(self.mode, RequestSearchMode::Idle)
    }

    fn begin_editing(&mut self, selected_path: Vec<String>, scroll_offset: usize) {
        let prior_committed = match std::mem::take(&mut self.mode) {
            RequestSearchMode::Committed(committed) => Some(committed),
            RequestSearchMode::Idle => None,
            RequestSearchMode::Editing(editing) => {
                self.mode = RequestSearchMode::Editing(editing);
                return;
            }
        };
        self.mode = RequestSearchMode::Editing(EditingSearch {
            input: SingleLineInput::default(),
            active: None,
            rollback: EditingRollback {
                selected_path,
                scroll_offset,
                prior_committed,
            },
            transient_openings: HashMap::new(),
            query_limit_reached: false,
        });
        self.dispatch = Some(DispatchIntent::Cancel);
    }

    fn input_mut(&mut self) -> Option<&mut SingleLineInput> {
        match &mut self.mode {
            RequestSearchMode::Editing(editing) => Some(&mut editing.input),
            RequestSearchMode::Idle | RequestSearchMode::Committed(_) => None,
        }
    }

    fn input(&self) -> Option<&SingleLineInput> {
        match &self.mode {
            RequestSearchMode::Editing(editing) => Some(&editing.input),
            RequestSearchMode::Idle | RequestSearchMode::Committed(_) => None,
        }
    }

    fn mark_limit_reached(&mut self) {
        if let RequestSearchMode::Editing(editing) = &mut self.mode {
            editing.query_limit_reached = true;
        }
    }

    fn activate_edited_query(&mut self, tree_revision: u64) {
        let RequestSearchMode::Editing(editing) = &mut self.mode else {
            return;
        };
        editing.query_limit_reached = false;
        self.next_generation = self.next_generation.wrapping_add(1);
        if editing.input.text().is_empty() {
            editing.active = None;
            self.dispatch = Some(DispatchIntent::Cancel);
            return;
        }
        editing.active = Some(ActiveSearch {
            query: Arc::from(editing.input.text()),
            key: SearchRequestKey {
                generation: self.next_generation,
                tree_revision,
            },
            progress: ActiveSearchProgress::Pending,
            select_first_when_ready: true,
        });
        self.dispatch = editing.active.as_ref().map(|active| DispatchIntent::Run {
            key: active.key,
            query: Arc::clone(&active.query),
        });
    }

    fn active(&self) -> Option<&ActiveSearch> {
        match &self.mode {
            RequestSearchMode::Idle => None,
            RequestSearchMode::Editing(editing) => editing.active.as_ref(),
            RequestSearchMode::Committed(committed) => Some(&committed.active),
        }
    }

    fn active_mut(&mut self) -> Option<&mut ActiveSearch> {
        match &mut self.mode {
            RequestSearchMode::Idle => None,
            RequestSearchMode::Editing(editing) => editing.active.as_mut(),
            RequestSearchMode::Committed(committed) => Some(&mut committed.active),
        }
    }

    fn query(&self) -> Option<&str> {
        match &self.mode {
            RequestSearchMode::Idle => None,
            RequestSearchMode::Editing(editing) if editing.input.text().is_empty() => None,
            RequestSearchMode::Editing(editing) => Some(editing.input.text()),
            RequestSearchMode::Committed(committed) => Some(&committed.active.query),
        }
    }

    pub(in crate::app) fn refresh_for_tree(&mut self, tree_revision: u64) {
        let Some(active) = self.active_mut() else {
            return;
        };
        if active.key.tree_revision == tree_revision {
            return;
        }
        active.key.tree_revision = tree_revision;
        active.progress =
            match std::mem::replace(&mut active.progress, ActiveSearchProgress::Pending) {
                ActiveSearchProgress::Ready(results)
                | ActiveSearchProgress::Refreshing(results)
                | ActiveSearchProgress::Failed(Some(results)) => {
                    ActiveSearchProgress::Refreshing(results)
                }
                ActiveSearchProgress::Pending | ActiveSearchProgress::Failed(None) => {
                    ActiveSearchProgress::Pending
                }
            };
        active.select_first_when_ready = false;
        self.dispatch = Some(DispatchIntent::Run {
            key: active.key,
            query: Arc::clone(&active.query),
        });
    }

    fn take_dispatch_intent(&mut self) -> Option<DispatchIntent> {
        self.dispatch.take()
    }

    fn apply_outcome(&mut self, outcome: &SearchJobOutcome) -> OutcomeApplication {
        let Some(active) = self.active_mut() else {
            return OutcomeApplication::Ignored;
        };
        if active.key != outcome.key() {
            return OutcomeApplication::Ignored;
        }
        match outcome {
            SearchJobOutcome::Completed { results, .. } => {
                active.progress = ActiveSearchProgress::Ready(Arc::clone(results));
                OutcomeApplication::Applied {
                    select_first: std::mem::take(&mut active.select_first_when_ready),
                }
            }
            SearchJobOutcome::Cancelled { .. } => OutcomeApplication::Applied {
                select_first: false,
            },
            SearchJobOutcome::Failed { message, .. } => {
                log::error!("request search failed: {message}");
                let previous = active.results().cloned();
                active.progress = ActiveSearchProgress::Failed(previous);
                active.select_first_when_ready = false;
                OutcomeApplication::Applied {
                    select_first: false,
                }
            }
        }
    }

    fn commit_editing(&mut self) -> bool {
        let RequestSearchMode::Editing(editing) = std::mem::take(&mut self.mode) else {
            return false;
        };
        let Some(active) = editing.active else {
            self.mode = RequestSearchMode::Editing(editing);
            return false;
        };
        self.mode = RequestSearchMode::Committed(CommittedSearch { active });
        true
    }

    fn cancel_editing(&mut self) -> Option<EditingRestore> {
        let RequestSearchMode::Editing(editing) = std::mem::take(&mut self.mode) else {
            return None;
        };
        let EditingSearch {
            rollback,
            transient_openings,
            ..
        } = editing;
        self.mode = rollback
            .prior_committed
            .map_or(RequestSearchMode::Idle, RequestSearchMode::Committed);
        self.dispatch = Some(DispatchIntent::Cancel);
        Some(EditingRestore {
            selected_path: rollback.selected_path,
            scroll_offset: rollback.scroll_offset,
            paths_to_close: transient_openings.into_keys().collect(),
        })
    }

    fn clear_committed(&mut self) -> bool {
        if !self.is_committed() {
            return false;
        }
        self.mode = RequestSearchMode::Idle;
        self.dispatch = Some(DispatchIntent::Cancel);
        true
    }

    fn rollback_selection_and_offset(&self) -> Option<(&[String], usize)> {
        match &self.mode {
            RequestSearchMode::Editing(editing) => Some((
                &editing.rollback.selected_path,
                editing.rollback.scroll_offset,
            )),
            RequestSearchMode::Idle | RequestSearchMode::Committed(_) => None,
        }
    }

    fn clear_match_reveal_reasons(&mut self) -> Vec<Vec<String>> {
        let RequestSearchMode::Editing(editing) = &mut self.mode else {
            return Vec::new();
        };
        let mut paths_to_close = Vec::new();
        editing.transient_openings.retain(|path, reasons| {
            reasons.match_reveal = false;
            if reasons.auto_expand {
                true
            } else {
                paths_to_close.push(path.clone());
                false
            }
        });
        paths_to_close
    }

    fn note_transient_opening(
        &mut self,
        path: Vec<String>,
        reason: TransientOpeningReason,
        newly_opened: bool,
    ) {
        let RequestSearchMode::Editing(editing) = &mut self.mode else {
            return;
        };
        let already_tracked = editing.transient_openings.contains_key(&path);
        if !newly_opened && !already_tracked {
            return;
        }
        let reasons = editing.transient_openings.entry(path).or_default();
        match reason {
            TransientOpeningReason::MatchReveal => reasons.match_reveal = true,
            TransientOpeningReason::AutoExpand => reasons.auto_expand = true,
        }
    }

    pub(crate) fn results(&self) -> Option<Arc<SearchResults>> {
        self.active()?.results().cloned()
    }

    pub(crate) fn is_refreshing(&self) -> bool {
        self.active()
            .is_some_and(|active| matches!(active.progress, ActiveSearchProgress::Refreshing(_)))
    }

    fn title_status(&self, selected_path: &[String]) -> SearchTitleStatus {
        let RequestSearchMode::Editing(editing) = &self.mode else {
            return self.active_title_status(selected_path);
        };
        if editing.query_limit_reached {
            SearchTitleStatus::QueryLimitReached
        } else {
            self.active_title_status(selected_path)
        }
    }

    fn active_title_status(&self, selected_path: &[String]) -> SearchTitleStatus {
        let Some(active) = self.active() else {
            return SearchTitleStatus::None;
        };
        match &active.progress {
            ActiveSearchProgress::Pending => SearchTitleStatus::Pending,
            ActiveSearchProgress::Failed(_) => SearchTitleStatus::Failed,
            ActiveSearchProgress::Ready(results) | ActiveSearchProgress::Refreshing(results) => {
                let results = results.as_ref();
                if results.is_empty() {
                    SearchTitleStatus::NoMatches
                } else {
                    SearchTitleStatus::Matches {
                        selected: results
                            .ordinal_for_path(selected_path)
                            .map(|ordinal| ordinal + 1),
                        total: results.len(),
                    }
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SearchTitleStatus {
    None,
    Pending,
    Matches {
        selected: Option<usize>,
        total: usize,
    },
    NoMatches,
    Failed,
    QueryLimitReached,
}
