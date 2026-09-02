//! Release-performance harness for MCP-critical bounded paths.
//!
//! Release gate:
//! `cargo bench --features benchmark --bench core_paths -- --sample-size 10`.
//! Record Criterion output on the release machine and compare like-for-like baselines; this
//! harness reports trends, while unit tests enforce hard admission and memory limits.
//!
//! Diagnostic-only subsets may append a Criterion filter, for example
//! `cargo bench --features benchmark --bench core_paths -- registry_and_control`.

use criterion::{
    BatchSize, BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main,
};
use std::{sync::Arc, time::Duration};
use fluxcope::{
    benchmark_support as support,
    capture::{CaptureSequence, CapturedExchange},
};
use hyper::Method;

const KIB: usize = 1024;
const MIB: usize = 1024 * KIB;

fn capture(sequence: u64) -> CapturedExchange {
    CapturedExchange {
        sequence: CaptureSequence::new(sequence),
        method: hyper::Method::GET,
        uri: format!("https://example.com/item/{sequence}?query={sequence}"),
        mapped_uri: None,
        local_path: None,
        status: Some(200),
        req_headers: vec![
            ("x-benchmark".to_owned(), format!("value-{sequence}")),
            ("accept".to_owned(), "application/json".to_owned()),
        ],
        res_headers: vec![("content-type".to_owned(), "application/json".to_owned())],
        req_body: None,
        res_body: None,
    }
}

fn unicode_capture(sequence: u64) -> CapturedExchange {
    let mut capture = capture(sequence);
    capture.uri = format!("https://example.com/straße/{sequence}");
    capture
}

fn captures(count: usize) -> Vec<CapturedExchange> {
    (0..count as u64).map(capture).collect()
}

fn search_captures(count: usize) -> Vec<CapturedExchange> {
    (0..count as u64)
        .map(|sequence| {
            if sequence % 997 == 0 {
                unicode_capture(sequence)
            } else {
                capture(sequence)
            }
        })
        .collect()
}

fn body_exchange(bytes: usize) -> CapturedExchange {
    let mut body = "x".repeat(bytes.saturating_sub(32));
    body.push_str(" target-needle STRASSE 结束");
    CapturedExchange {
        sequence: CaptureSequence::new(1),
        method: hyper::Method::POST,
        uri: "https://example.com/body".to_owned(),
        mapped_uri: None,
        local_path: None,
        status: Some(200),
        req_headers: Vec::new(),
        res_headers: vec![(
            "content-type".to_owned(),
            "text/plain; charset=utf-8".to_owned(),
        )],
        req_body: None,
        res_body: Some(body),
    }
}

fn json_bytes(target_bytes: usize) -> Vec<u8> {
    let prefix = br#"{"padding":""#;
    let suffix = br#"","items":[{"id":1,"needle":"value"},{"id":2,"needle":"other"}]}"#;
    let padding = target_bytes.saturating_sub(prefix.len() + suffix.len());
    let mut json = Vec::with_capacity(prefix.len() + padding + suffix.len());
    json.extend_from_slice(prefix);
    json.resize(prefix.len() + padding, b'x');
    json.extend_from_slice(suffix);
    json
}

fn bench_capture_store_and_tree(c: &mut Criterion) {
    let mut group = c.benchmark_group("capture_store_and_tree");
    for size in [1_000_usize, 10_000] {
        group.throughput(Throughput::Elements(size as u64));
        let fixture = captures(size);
        group.bench_with_input(
            BenchmarkId::new("ordered_store", size),
            &fixture,
            |b, fixture| {
                b.iter_batched(
                    || fixture.clone(),
                    |captures| black_box(support::ordered_capture_store(captures)),
                    BatchSize::SmallInput,
                );
            },
        );
        group.bench_with_input(
            BenchmarkId::new("request_tree_build_snapshot", size),
            &fixture,
            |b, fixture| {
                b.iter_batched(
                    || fixture.clone(),
                    |captures| black_box(support::request_tree_build_and_snapshot(captures)),
                    BatchSize::SmallInput,
                );
            },
        );
    }
    group.finish();
}

fn bench_capture_query(c: &mut Criterion) {
    let mut group = c.benchmark_group("capture_query_10k");
    group.throughput(Throughput::Elements(10_000));
    let fixture = support::capture_query_fixture(search_captures(10_000));
    for scenario in [
        support::CaptureQueryScenario::Absent,
        support::CaptureQueryScenario::Sparse,
        support::CaptureQueryScenario::FullPage,
        support::CaptureQueryScenario::Header,
        support::CaptureQueryScenario::Unicode,
    ] {
        group.bench_with_input(
            BenchmarkId::from_parameter(format!("{scenario:?}")),
            &scenario,
            |b, scenario| {
                b.iter(|| black_box(support::capture_query_search(&fixture, *scenario)));
            },
        );
    }

    let mut max_header = capture(0);
    max_header.req_headers = vec![(
        "x-benchmark".to_owned(),
        format!("{}needle", "x".repeat(256 * KIB - 6)),
    )];
    let max_header_fixture = support::capture_query_fixture(vec![max_header]);
    group.throughput(Throughput::Bytes((256 * KIB) as u64));
    group.bench_function("maximum_header_match", |b| {
        b.iter(|| {
            black_box(support::capture_query_search(
                &max_header_fixture,
                support::CaptureQueryScenario::Header,
            ))
        });
    });
    group.finish();
}

fn bench_request_search(c: &mut Criterion) {
    let mut group = c.benchmark_group("request_tree_search");
    for size in [1_000_usize, 10_000] {
        group.throughput(Throughput::Elements(size as u64));
        let fixture = support::request_search_fixture(captures(size));
        for (name, query) in [
            ("absent", "definitely-absent"),
            ("sparse", "item/997"),
            ("all_leaves", "example.com"),
        ] {
            group.bench_with_input(BenchmarkId::new(name, size), &query, |b, query| {
                b.iter(|| black_box(support::request_tree_search(&fixture, query)));
            });
        }
    }

    let unicode_fixture = support::request_search_fixture(vec![
        unicode_capture(0),
        CapturedExchange {
            uri: "https://example.com/Μάϊος/東京".to_owned(),
            ..capture(1)
        },
    ]);
    group.throughput(Throughput::Elements(2));
    group.bench_function("unicode_casefold", |b| {
        b.iter(|| black_box(support::request_tree_search(&unicode_fixture, "STRASSE")));
    });

    let supersession_fixture = support::request_search_fixture(captures(10_000));
    let runtime = tokio::runtime::Runtime::new().expect("benchmark runtime");
    group.throughput(Throughput::Elements(64));
    group.bench_function("rapid_supersession_64", |b| {
        b.to_async(&runtime).iter(|| async {
            black_box(support::request_search_rapid_supersession(&supersession_fixture, 64).await)
        });
    });
    group.finish();
}

fn bench_body_and_json(c: &mut Criterion) {
    let mut group = c.benchmark_group("body_and_json");
    let body = support::body_fixture(body_exchange(8 * MIB));
    group.throughput(Throughput::Bytes((8 * MIB) as u64));
    group.bench_function("body_search_near_end", |b| {
        b.iter(|| black_box(support::search_body(&body, "target-needle", KIB)));
    });
    group.bench_function("body_search_unicode_casefold", |b| {
        b.iter(|| black_box(support::search_body(&body, "strasse", KIB)));
    });

    group.throughput(Throughput::Bytes((64 * KIB) as u64));
    group.bench_function("private_raw_base64_relay_64k", |b| {
        b.iter(|| {
            black_box(support::private_body_page_relay(
                &body,
                8 * MIB - 64 * KIB,
                64 * KIB,
            ))
        });
    });

    let gzip = support::gzip_body_fixture(8 * MIB);
    group.throughput(Throughput::Bytes((8 * MIB) as u64));
    group.bench_function("gzip_decode_8m", |b| {
        b.iter(|| black_box(support::decode_gzip_body(&gzip)));
    });
    group.bench_function("gzip_decode_page_base64_8m", |b| {
        b.iter(|| {
            black_box(support::decode_page_and_base64(
                &gzip,
                8 * MIB - 64 * KIB,
                64 * KIB,
            ))
        });
    });

    let json = support::json_fixture(json_bytes(16 * MIB - 1));
    group.throughput(Throughput::Bytes((16 * MIB - 1) as u64));
    group.bench_function("json_field_discovery_16m", |b| {
        b.iter(|| black_box(support::find_json_fields(&json, "needle")));
    });
    group.bench_function("json_pointer_probe_16m", |b| {
        b.iter(|| black_box(support::probe_json_pattern(&json, "/items/*/id")));
    });
    group.finish();
}

fn bench_streaming_and_admission(c: &mut Criterion) {
    let mut group = c.benchmark_group("streaming_and_admission");
    group.throughput(Throughput::Bytes((4 * MIB) as u64));
    group.bench_function("fragmented_live_body_snapshots", |b| {
        b.iter(|| black_box(support::fragmented_live_body(1_024, 4 * KIB)));
    });

    let runtime = tokio::runtime::Runtime::new().expect("benchmark runtime");
    let feed = Arc::new(tokio::sync::Mutex::new(support::capture_feed_fixture()));
    group.throughput(Throughput::Elements(1));
    group.bench_function("capture_feed_publish_wakeup", |b| {
        b.to_async(&runtime).iter(|| {
            let feed = Arc::clone(&feed);
            async move {
                let mut feed = feed.lock().await;
                black_box(support::capture_feed_wakeup(&mut feed).await)
            }
        });
    });
    group.bench_function("capture_feed_cancel_wakeup", |b| {
        b.to_async(&runtime)
            .iter(|| async { black_box(support::capture_feed_cancel_wakeup().await) });
    });
    group.bench_function("body_and_rpc_admission_saturation", |b| {
        b.to_async(&runtime)
            .iter(|| async { black_box(support::body_and_rpc_admission_saturation().await) });
    });
    group.finish();
}

#[cfg(unix)]
fn bench_registry_and_control(c: &mut Criterion) {
    let mut group = c.benchmark_group("registry_and_control");
    group.sample_size(10);
    group.measurement_time(Duration::from_secs(10));
    let fixture = support::live_instance_fixture(256);
    let runtime = tokio::runtime::Runtime::new().expect("benchmark runtime");

    group.throughput(Throughput::Elements(256));
    group.bench_function("full_registry_scan_256", |b| {
        b.iter(|| black_box(support::registry_full_scan(&fixture)));
    });
    group.throughput(Throughput::Elements(1));
    group.bench_function("targeted_registry_lookup_256", |b| {
        b.iter(|| black_box(support::registry_targeted_lookup(&fixture, 127)));
    });
    group.throughput(Throughput::Elements(256));
    group.bench_function("broker_live_discovery_256", |b| {
        b.to_async(&runtime)
            .iter(|| async { black_box(support::broker_full_discovery(&fixture).await) });
    });
    group.throughput(Throughput::Elements(1));
    group.bench_function("private_rpc_round_trip", |b| {
        b.to_async(&runtime)
            .iter(|| async { black_box(support::private_rpc_round_trip(&fixture, 0).await) });
    });
    group.throughput(Throughput::Elements(32));
    group.bench_function("concurrent_routing_32", |b| {
        b.to_async(&runtime)
            .iter(|| async { black_box(support::concurrent_private_routing(&fixture, 32).await) });
    });
    group.finish();
}

#[cfg(unix)]
fn bench_persistence_and_logging(c: &mut Criterion) {
    let mut group = c.benchmark_group("persistence_and_logging");
    group.sample_size(10);
    let lock = support::default_lock_fixture();
    group.throughput(Throughput::Elements(1));
    group.bench_function("contended_default_lock", |b| {
        b.iter(|| black_box(support::contended_default_lock(&lock)));
    });

    let settings = support::settings_write_fixture(1_000);
    group.throughput(Throughput::Elements(1_000));
    group.bench_function("atomic_settings_write_1000_rules", |b| {
        b.iter(|| black_box(support::atomic_settings_write(&settings)));
    });

    group.throughput(Throughput::Elements(1_000));
    group.bench_function("endpoint_log_format_1000", |b| {
        b.iter(|| black_box(support::format_endpoint_logs(1_000, 512)));
    });

    let rotation = support::log_rotation_fixture(8);
    let runtime = tokio::runtime::Runtime::new().expect("benchmark runtime");
    group.throughput(Throughput::Elements(8));
    group.bench_function("endpoint_log_rotation_8", |b| {
        b.to_async(&runtime)
            .iter(|| async { black_box(support::rotate_endpoint_log(&rotation).await) });
    });
    group.finish();
}

fn bench_mapping_audit_and_serialization(c: &mut Criterion) {
    let mut group = c.benchmark_group("mapping_audit_and_serialization");
    let mapping = support::mapping_fixture(1_000);
    group.throughput(Throughput::Elements(1_000));
    group.bench_function("mapping_compile_1000", |b| {
        b.iter(|| black_box(support::compile_mapping(&mapping)));
    });
    group.bench_function("mapping_validate_1000", |b| {
        b.iter(|| black_box(support::validate_mapping(&mapping)));
    });
    group.bench_function("mapping_explain_1000", |b| {
        b.iter(|| {
            black_box(support::explain_mapping(
                &mapping,
                "https://source999.example/path",
            ))
        });
    });
    group.bench_function("mapping_mutation_1000", |b| {
        b.iter(|| black_box(support::mutate_mapping(&mapping, 999)));
    });
    group.bench_function("mapping_validation_serialization", |b| {
        b.iter(|| black_box(support::validate_mapping_serialization(&mapping)));
    });

    let audit = support::audit_fixture(10_000);
    group.throughput(Throughput::Elements(10_000));
    group.bench_function("audit_aggregation_10000", |b| {
        b.iter(|| black_box(support::aggregate_audit(&audit)));
    });

    group.throughput(Throughput::Bytes((7 * MIB) as u64));
    group.bench_function("private_frame_encode_7m", |b| {
        b.iter(|| black_box(support::encode_private_frame(7 * MIB)));
    });
    let serialization = support::serialization_fixture(7 * MIB);
    let runtime = tokio::runtime::Runtime::new().expect("benchmark runtime");
    group.bench_function("private_response_worker_7m", |b| {
        b.to_async(&runtime).iter(|| async {
            black_box(support::serialize_private_response(&serialization).await)
        });
    });
    group.throughput(Throughput::Bytes((32 * 7 * MIB) as u64));
    group.bench_function("private_response_workers_32x7m", |b| {
        b.to_async(&runtime).iter(|| async {
            black_box(support::serialize_private_responses(&serialization, 32).await)
        });
    });
    group.finish();
}

fn bench_legacy_regressions(c: &mut Criterion) {
    let mut group = c.benchmark_group("legacy_regressions");
    group.throughput(Throughput::Elements(10_000));
    group.bench_function("retained_log_join", |b| {
        b.iter(|| black_box(support::retained_log_join(10_000, 128)));
    });
    group.throughput(Throughput::Elements(1_000));
    group.bench_function("yaml_semantic_preservation", |b| {
        b.iter(|| black_box(support::yaml_semantic_preservation(1_000)));
    });
    group.finish();
}

fn benchmarks(c: &mut Criterion) {
    if cfg!(debug_assertions) {
        eprintln!("Wirelens Criterion benchmarks must run with `cargo bench --features benchmark`");
        return;
    }
    bench_capture_store_and_tree(c);
    bench_capture_query(c);
    bench_request_search(c);
    bench_body_and_json(c);
    bench_streaming_and_admission(c);
    #[cfg(unix)]
    bench_registry_and_control(c);
    #[cfg(unix)]
    bench_persistence_and_logging(c);
    bench_mapping_audit_and_serialization(c);
    bench_legacy_regressions(c);
}

criterion_group! {
    name = core_paths;
    config = Criterion::default()
        .sample_size(20)
        .measurement_time(Duration::from_secs(5));
    targets = benchmarks
}
criterion_main!(core_paths);
