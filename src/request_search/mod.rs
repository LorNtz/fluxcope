use std::{
    collections::HashMap,
    ops::{ControlFlow, Range},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use unicode_casefold::UnicodeCaseFold;
use unicode_segmentation::UnicodeSegmentation;

use crate::app::RequestTreeModel;

mod service;

pub(crate) use service::{RequestSearchClient, start_request_search_service};

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub(crate) struct SearchRequestKey {
    pub generation: u64,
    pub tree_revision: u64,
}

#[derive(Clone, Debug)]
pub(crate) struct SearchRequest {
    pub key: SearchRequestKey,
    pub query: Arc<str>,
    pub tree: Arc<RequestTreeModel>,
}

#[derive(Clone, Debug)]
pub(crate) enum RequestSearchDispatch {
    Run(SearchRequest),
    Cancel,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SearchMatch {
    pub path: Arc<[String]>,
    pub label: Arc<str>,
    pub logical_position: usize,
    pub ranges: Arc<[Range<usize>]>,
}

#[derive(Clone, Debug)]
pub(crate) struct SearchResults {
    matches: Box<[SearchMatch]>,
    ordinal_by_path: HashMap<Arc<[String]>, usize>,
}

impl SearchResults {
    fn new(matches: Vec<SearchMatch>) -> Self {
        let ordinal_by_path = matches
            .iter()
            .enumerate()
            .map(|(ordinal, search_match)| (Arc::clone(&search_match.path), ordinal))
            .collect();
        Self {
            matches: matches.into_boxed_slice(),
            ordinal_by_path,
        }
    }

    pub(crate) fn matches(&self) -> &[SearchMatch] {
        &self.matches
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.matches.is_empty()
    }

    pub(crate) fn len(&self) -> usize {
        self.matches.len()
    }

    pub(crate) fn ordinal_for_path(&self, path: &[String]) -> Option<usize> {
        self.ordinal_by_path.get(path).copied()
    }

    pub(crate) fn match_for_path(&self, path: &[String]) -> Option<&SearchMatch> {
        self.ordinal_for_path(path)
            .and_then(|ordinal| self.matches.get(ordinal))
    }
}

#[derive(Clone, Debug)]
pub(crate) enum SearchJobOutcome {
    Completed {
        key: SearchRequestKey,
        results: Arc<SearchResults>,
    },
    Cancelled {
        key: SearchRequestKey,
    },
    Failed {
        key: SearchRequestKey,
        message: Arc<str>,
    },
}

impl SearchJobOutcome {
    pub(crate) fn key(&self) -> SearchRequestKey {
        match self {
            Self::Completed { key, .. } | Self::Cancelled { key } | Self::Failed { key, .. } => {
                *key
            }
        }
    }
}

pub(crate) fn run_search(request: &SearchRequest, cancelled: &AtomicBool) -> SearchJobOutcome {
    if cancelled.load(Ordering::Relaxed) {
        return SearchJobOutcome::Cancelled { key: request.key };
    }

    let folded_query = request.query.as_ref().case_fold().collect::<String>();
    if folded_query.is_empty() {
        return SearchJobOutcome::Completed {
            key: request.key,
            results: Arc::new(SearchResults::new(Vec::new())),
        };
    }

    let mut matches = Vec::new();
    let completed = request
        .tree
        .visit_logical_nodes(|logical_position, label, path| {
            if cancelled.load(Ordering::Relaxed) {
                return ControlFlow::Break(());
            }
            let Some(ranges) = matching_original_ranges(label, &folded_query, cancelled) else {
                return ControlFlow::Break(());
            };
            if !ranges.is_empty() {
                matches.push(SearchMatch {
                    path: path
                        .iter()
                        .map(|identifier| identifier.to_string())
                        .collect::<Vec<_>>()
                        .into(),
                    label: Arc::clone(label),
                    logical_position,
                    ranges: ranges.into(),
                });
            }
            ControlFlow::Continue(())
        });

    if !completed || cancelled.load(Ordering::Relaxed) {
        SearchJobOutcome::Cancelled { key: request.key }
    } else {
        SearchJobOutcome::Completed {
            key: request.key,
            results: Arc::new(SearchResults::new(matches)),
        }
    }
}

#[derive(Clone, Debug)]
struct FoldedUnit {
    folded: Range<usize>,
    original: Range<usize>,
}

fn matching_original_ranges(
    label: &str,
    folded_query: &str,
    cancelled: &AtomicBool,
) -> Option<Vec<Range<usize>>> {
    let mut folded = String::with_capacity(label.len());
    let mut units = Vec::new();
    for (index, (original_start, grapheme)) in label.grapheme_indices(true).enumerate() {
        if index.is_multiple_of(64) && cancelled.load(Ordering::Relaxed) {
            return None;
        }
        let folded_start = folded.len();
        folded.extend(grapheme.case_fold());
        let folded_end = folded.len();
        units.push(FoldedUnit {
            folded: folded_start..folded_end,
            original: original_start..original_start + grapheme.len(),
        });
    }

    let mut ranges = Vec::<Range<usize>>::new();
    let mut search_start = 0_usize;
    let mut occurrence_count = 0_usize;
    while search_start <= folded.len() {
        if occurrence_count.is_multiple_of(64) && cancelled.load(Ordering::Relaxed) {
            return None;
        }
        let Some(relative_start) = folded[search_start..].find(folded_query) else {
            break;
        };
        let match_start = search_start + relative_start;
        let match_end = match_start + folded_query.len();
        occurrence_count = occurrence_count.saturating_add(1);
        if let Some(original) = original_range_for_folded(&units, match_start..match_end) {
            if let Some(previous) = ranges.last_mut()
                && original.start <= previous.end
            {
                previous.end = previous.end.max(original.end);
            } else {
                ranges.push(original);
            }
        }

        let step = folded[match_start..]
            .chars()
            .next()
            .map_or(1, char::len_utf8);
        search_start = match_start.saturating_add(step);
    }
    Some(ranges)
}

fn original_range_for_folded(
    units: &[FoldedUnit],
    folded_range: Range<usize>,
) -> Option<Range<usize>> {
    let first = units.partition_point(|unit| unit.folded.end <= folded_range.start);
    let after_last = units.partition_point(|unit| unit.folded.start < folded_range.end);
    let last = after_last.checked_sub(1)?;
    if first >= units.len() || first > last {
        return None;
    }
    Some(units[first].original.start..units[last].original.end)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::{CaptureRecord, CaptureSequence, CapturedExchange};
    use http::Method;

    fn ranges(label: &str, query: &str) -> Vec<Range<usize>> {
        let folded_query = query.case_fold().collect::<String>();
        matching_original_ranges(label, &folded_query, &AtomicBool::new(false))
            .expect("matching should not be cancelled")
    }

    fn request(uris: &[&str], query: &str) -> SearchRequest {
        let summaries = uris.iter().enumerate().map(|(index, uri)| {
            CaptureRecord::from_completed(CapturedExchange {
                sequence: CaptureSequence::new(index as u64),
                method: Method::GET,
                uri: (*uri).to_string(),
                mapped_uri: None,
                local_path: None,
                status: None,
                req_headers: Vec::new(),
                res_headers: Vec::new(),
                req_body: None,
                res_body: None,
            })
            .summary()
        });
        SearchRequest {
            key: SearchRequestKey {
                generation: 1,
                tree_revision: 1,
            },
            query: Arc::from(query),
            tree: Arc::new(RequestTreeModel::from_requests(summaries)),
        }
    }

    fn completed(request: &SearchRequest) -> Arc<SearchResults> {
        match run_search(request, &AtomicBool::new(false)) {
            SearchJobOutcome::Completed { results, .. } => results,
            outcome => panic!("expected completed search, got {outcome:?}"),
        }
    }

    #[test]
    fn full_case_folding_maps_expansions_back_to_original_graphemes() {
        assert_eq!(ranges("Straße", "STRASSE"), vec![0..7]);
        assert_eq!(ranges("ß", "s"), vec![0..2]);
    }

    #[test]
    fn overlapping_matches_are_merged_for_highlighting() {
        assert_eq!(ranges("banana", "ana"), vec![1..6]);
    }

    #[test]
    fn matching_does_not_normalize_unicode() {
        assert!(ranges("é", "e\u{301}").is_empty());
        assert_eq!(ranges("e\u{301}", "E\u{301}"), vec![0..3]);
    }

    #[test]
    fn matcher_visits_origins_branches_and_duplicate_leaves_in_logical_order() {
        let request = request(
            &[
                "https://needle.example/api/needle",
                "https://needle.example/api/needle",
                "https://other.example/needle/final",
            ],
            "NeEdLe",
        );
        let results = completed(&request);
        let labels = results
            .matches()
            .iter()
            .map(|search_match| search_match.label.as_ref())
            .collect::<Vec<_>>();

        assert_eq!(
            labels,
            ["https://needle.example", "needle", "needle", "needle",]
        );
        assert!(
            results
                .matches()
                .windows(2)
                .all(|pair| pair[0].logical_position < pair[1].logical_position)
        );
    }

    #[test]
    fn matcher_does_not_decode_urls_or_match_generated_branch_text() {
        let encoded = request(&["https://a.com/api/hello%20world"], "hello world");
        assert!(completed(&encoded).is_empty());

        let slash = request(&["https://a.com/api/item"], "api/");
        assert!(completed(&slash).is_empty());

        let count = request(
            &["https://a.com/api/item", "https://a.com/api/item2"],
            "api 2",
        );
        assert!(completed(&count).is_empty());
    }

    #[test]
    fn matcher_honors_cancellation() {
        let request = request(&["https://a.com/api/item"], "item");
        let cancelled = AtomicBool::new(true);
        assert!(matches!(
            run_search(&request, &cancelled),
            SearchJobOutcome::Cancelled { key } if key == request.key
        ));
    }
}
