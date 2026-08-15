//! Narrow public seams used only by Criterion's external benchmark target.

use crate::{
    app::RequestTreeModel,
    capture::{CaptureRecord, CapturedExchange},
    request_search::{
        SearchJobOutcome, SearchRequest, SearchRequestKey, run_search, start_request_search_service,
    },
};
use std::sync::{Arc, atomic::AtomicBool};
use tokio_util::sync::CancellationToken;

pub fn ordered_capture_store(captures: Vec<CapturedExchange>) -> usize {
    crate::capture::benchmark_ordered_store(captures)
}

pub fn request_tree_build_and_snapshot(captures: Vec<CapturedExchange>) -> usize {
    crate::app::benchmark_request_tree(captures)
}

pub fn retained_log_join(record_count: usize, record_bytes: usize) -> usize {
    crate::app::benchmark_retained_log_join(record_count, record_bytes)
}

pub fn yaml_semantic_preservation(rule_count: usize) -> usize {
    crate::settings::benchmark_yaml_semantic_preservation(rule_count)
}

pub struct RequestSearchFixture {
    tree: Arc<RequestTreeModel>,
}

pub fn request_search_fixture(captures: Vec<CapturedExchange>) -> RequestSearchFixture {
    let summaries = captures
        .into_iter()
        .map(CaptureRecord::from_completed)
        .map(|capture| capture.summary());
    RequestSearchFixture {
        tree: Arc::new(RequestTreeModel::from_requests(summaries)),
    }
}

pub fn request_tree_search(fixture: &RequestSearchFixture, query: &str) -> usize {
    let request = SearchRequest {
        key: SearchRequestKey {
            generation: 1,
            tree_revision: 1,
        },
        query: Arc::from(query),
        tree: Arc::clone(&fixture.tree),
    };
    match run_search(&request, &AtomicBool::new(false)) {
        SearchJobOutcome::Completed { results, .. } => results.len(),
        SearchJobOutcome::Cancelled { .. } | SearchJobOutcome::Failed { .. } => 0,
    }
}

pub fn request_search_rapid_supersession(
    fixture: &RequestSearchFixture,
    replacements: usize,
) -> u64 {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("benchmark runtime should build");
    runtime.block_on(async {
        let shutdown = CancellationToken::new();
        let service = start_request_search_service(shutdown.clone());
        let client = service.client;
        let mut results = service.results;
        let task = service.task;
        let replacements = replacements.max(1);
        let search_request = |generation| SearchRequest {
            key: SearchRequestKey {
                generation,
                tree_revision: 1,
            },
            query: Arc::from(format!("query{generation}")),
            tree: Arc::clone(&fixture.tree),
        };

        client.submit(search_request(0));
        tokio::task::yield_now().await;
        for generation in 1..replacements as u64 {
            client.submit(search_request(generation));
        }
        let latest_generation = replacements as u64 - 1;
        loop {
            results
                .changed()
                .await
                .expect("benchmark search service should remain available");
            let outcome = results.borrow_and_update().clone();
            if outcome
                .as_deref()
                .is_some_and(|outcome| outcome.key().generation == latest_generation)
            {
                break;
            }
        }

        shutdown.cancel();
        task.await
            .expect("benchmark search task should join")
            .expect("benchmark search service should stop cleanly");
        latest_generation
    })
}
