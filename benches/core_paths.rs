use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group};
use hyper::Method;
use wirelens::capture::{CaptureSequence, CapturedExchange};

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

criterion_group!(benches, capture_insertion);

fn main() {
    // `cargo test --all-targets` builds this harness without release optimizations.
    // The quadratic baseline is useful only under `cargo bench` and is prohibitively
    // noisy in a debug test build.
    if !cfg!(debug_assertions) {
        benches();
    }
}
