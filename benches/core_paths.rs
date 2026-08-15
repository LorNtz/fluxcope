use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group};
use hyper::Method;
use wirelens::{
    benchmark_support::{
        ordered_capture_store, request_search_fixture, request_search_rapid_supersession,
        request_tree_build_and_snapshot, request_tree_search, retained_log_join,
        yaml_semantic_preservation,
    },
    capture::{CaptureSequence, CapturedExchange},
};

fn capture(sequence: u64) -> CapturedExchange {
    CapturedExchange {
        sequence: CaptureSequence::new(sequence),
        method: Method::GET,
        uri: format!("https://example.com/api/items/{sequence}"),
        mapped_uri: None,
        local_path: None,
        status: Some(200),
        req_headers: Vec::new(),
        res_headers: Vec::new(),
        req_body: None,
        res_body: None,
    }
}

fn search_capture(sequence: u64) -> CapturedExchange {
    CapturedExchange {
        uri: format!("https://example.com/api/items/item{sequence}"),
        ..capture(sequence)
    }
}

fn unicode_capture(sequence: u64) -> CapturedExchange {
    CapturedExchange {
        uri: format!("/unicode/Straße{sequence}"),
        req_headers: vec![("host".to_string(), "example.com".to_string())],
        ..capture(sequence)
    }
}

fn insert_ordered(captures: impl Iterator<Item = CapturedExchange>) -> Vec<CapturedExchange> {
    let mut ordered: Vec<CapturedExchange> = Vec::new();
    for capture in captures {
        let position = ordered.partition_point(|existing| existing.sequence <= capture.sequence);
        ordered.insert(position, capture);
    }
    ordered
}

fn capture_insertion(c: &mut Criterion) {
    let mut group = c.benchmark_group("capture insertion baseline");
    group.sample_size(10);
    group.measurement_time(std::time::Duration::from_secs(2));

    for size in [1_000_u64, 10_000] {
        group.throughput(Throughput::Elements(size));
        group.bench_with_input(BenchmarkId::new("ascending", size), &size, |b, &size| {
            b.iter(|| insert_ordered((0..size).map(capture)))
        });
        group.bench_with_input(BenchmarkId::new("reverse", size), &size, |b, &size| {
            b.iter(|| black_box(insert_ordered((0..size).rev().map(capture))))
        });
    }

    group.finish();
}

fn refactored_core_paths(c: &mut Criterion) {
    let mut capture_group = c.benchmark_group("ordered capture store");
    capture_group.sample_size(10);
    capture_group.measurement_time(std::time::Duration::from_secs(2));
    for size in [1_000_u64, 10_000] {
        capture_group.throughput(Throughput::Elements(size));
        capture_group.bench_with_input(BenchmarkId::new("ascending", size), &size, |b, &size| {
            b.iter(|| ordered_capture_store((0..size).map(capture).collect()))
        });
        capture_group.bench_with_input(BenchmarkId::new("reverse", size), &size, |b, &size| {
            b.iter(|| ordered_capture_store((0..size).rev().map(capture).collect()))
        });
    }
    capture_group.finish();

    let mut tree_group = c.benchmark_group("request tree build and snapshot");
    tree_group.sample_size(10);
    tree_group.measurement_time(std::time::Duration::from_secs(2));
    for size in [1_000_u64, 10_000] {
        tree_group.throughput(Throughput::Elements(size));
        tree_group.bench_with_input(BenchmarkId::from_parameter(size), &size, |b, &size| {
            b.iter(|| request_tree_build_and_snapshot((0..size).map(capture).collect()))
        });
    }
    tree_group.finish();

    let mut log_group = c.benchmark_group("retained log join");
    log_group.sample_size(10);
    log_group.measurement_time(std::time::Duration::from_secs(2));
    for records in [1_000_usize, 5_000] {
        log_group.bench_with_input(
            BenchmarkId::from_parameter(records),
            &records,
            |b, &records| b.iter(|| retained_log_join(records, 128)),
        );
    }
    log_group.finish();

    let mut yaml_group = c.benchmark_group("yaml semantic preservation");
    yaml_group.sample_size(10);
    yaml_group.measurement_time(std::time::Duration::from_secs(2));
    for rules in [100_usize, 1_000] {
        yaml_group.bench_with_input(BenchmarkId::from_parameter(rules), &rules, |b, &rules| {
            b.iter(|| yaml_semantic_preservation(rules))
        });
    }
    yaml_group.finish();

    let mut search_group = c.benchmark_group("request tree search");
    search_group.sample_size(10);
    search_group.measurement_time(std::time::Duration::from_secs(2));
    for size in [1_000_u64, 10_000] {
        let fixture = request_search_fixture((0..size).map(search_capture).collect());
        let unicode_fixture = request_search_fixture((0..size).map(unicode_capture).collect());
        search_group.throughput(Throughput::Elements(size));
        search_group.bench_with_input(BenchmarkId::new("no matches", size), &size, |b, _| {
            b.iter(|| request_tree_search(black_box(&fixture), black_box("absent")))
        });
        search_group.bench_with_input(BenchmarkId::new("sparse", size), &size, |b, &size| {
            let query = (size - 1).to_string();
            b.iter(|| request_tree_search(black_box(&fixture), black_box(&query)))
        });
        search_group.bench_with_input(BenchmarkId::new("all leaves", size), &size, |b, _| {
            b.iter(|| request_tree_search(black_box(&fixture), black_box("item")))
        });
        search_group.bench_with_input(BenchmarkId::new("unicode folding", size), &size, |b, _| {
            b.iter(|| request_tree_search(black_box(&unicode_fixture), black_box("STRASSE")))
        });
        search_group.bench_with_input(
            BenchmarkId::new("rapid supersession", size),
            &size,
            |b, _| {
                b.iter(|| request_search_rapid_supersession(black_box(&fixture), black_box(100)))
            },
        );
    }
    search_group.finish();
}

criterion_group!(benches, capture_insertion, refactored_core_paths);

fn main() {
    // `cargo test --all-targets` builds this harness without release optimizations.
    // The quadratic baseline is useful only under `cargo bench` and is prohibitively
    // noisy in a debug test build.
    if !cfg!(debug_assertions) {
        benches();
    }
}
