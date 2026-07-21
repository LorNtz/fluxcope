//! Narrow public seams used only by Criterion's external benchmark target.

use crate::capture::CapturedExchange;

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
