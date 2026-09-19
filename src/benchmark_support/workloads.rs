use crate::{
    capture::{
        BodySide, BodyWorkAdmission, CaptureChangeFeed, CaptureChangeKind,
        CaptureChangeSubscription, CapturePolicy, CapturePublisher, CaptureSequence,
        CaptureSnapshotMode, RequestCaptureInput, ResponseCaptureInput,
    },
    control::audit::InstanceAudit,
    control_rpc::{
        framing::{
            BenchmarkSerializationFixture, CallAdmission, RESPONSE_MAX_BYTES, encode_json_frame,
        },
        protocol::{ControlOperationKind, DeclaredClient, InstanceScope},
        server::{ControlCallAudit, ControlCallOutcome},
    },
    instance::RunId,
    request_policy::RequestPolicy,
    settings::{
        AppSettings, ConfigMode, PersistenceMode, ProxyMapRemoteRule, ProxyMapRemoteSettings,
        ProxyPresetSettings, ProxySettings,
        mapping_ops::{
            MappingMutation, ProxyRuleTable, apply_mapping_mutation, explain_mapping_candidate,
            validate_mapping_candidate,
        },
    },
};
use futures::{StreamExt as _, stream::FuturesUnordered};
use hyper::{HeaderMap, Method};
use serde::Serialize;
use std::{
    str::FromStr,
    time::{Duration, Instant},
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

pub fn fragmented_live_body(chunk_count: usize, chunk_bytes: usize) -> usize {
    let (tx, mut rx) = mpsc::channel(1);
    let publisher = CapturePublisher::new(
        tx,
        CapturePolicy {
            response_preview_bytes: chunk_count.saturating_mul(chunk_bytes),
            total_retained_bytes: chunk_count
                .saturating_mul(chunk_bytes)
                .saturating_add(1024 * 1024),
            ..CapturePolicy::default()
        },
    );
    let headers = HeaderMap::new();
    let handle = publisher
        .try_start(RequestCaptureInput {
            method: Method::GET,
            original_uri: "https://benchmark.example/live",
            effective_uri: "https://benchmark.example/live",
            local_path: None,
            headers: &headers,
        })
        .expect("benchmark capture should be admitted");
    let record = rx
        .try_recv()
        .expect("benchmark capture should be immediately published");
    handle.set_response(ResponseCaptureInput {
        status: 200,
        headers: &headers,
    });
    let chunk = vec![b'x'; chunk_bytes];
    let mut observed = 0_usize;
    for _ in 0..chunk_count {
        handle.append(BodySide::Response, &chunk);
        observed = observed.saturating_add(
            record
                .snapshot(CaptureSnapshotMode::WithBodyPreviews)
                .response_body
                .preview
                .len(),
        );
    }
    observed
}

pub struct CaptureFeedFixture {
    feed: CaptureChangeFeed,
    subscription: CaptureChangeSubscription,
    sequence: u64,
}

pub fn capture_feed_fixture() -> CaptureFeedFixture {
    let feed = CaptureChangeFeed::new();
    let subscription = feed.subscribe();
    CaptureFeedFixture {
        feed,
        subscription,
        sequence: 0,
    }
}

pub async fn capture_feed_wakeup(fixture: &mut CaptureFeedFixture) -> u64 {
    fixture.sequence = fixture.sequence.saturating_add(1);
    fixture.feed.publish(
        CaptureSequence::new(fixture.sequence),
        fixture.sequence,
        CaptureChangeKind::RecordUpdated,
    );
    fixture
        .subscription
        .recv()
        .await
        .expect("benchmark capture feed should deliver")
        .epoch
}

pub async fn capture_feed_cancel_wakeup() -> bool {
    let feed = CaptureChangeFeed::new();
    let mut subscription = feed.subscribe();
    let cancelled = CancellationToken::new();
    let trigger = cancelled.clone();
    let trigger_task = tokio::spawn(async move {
        tokio::task::yield_now().await;
        trigger.cancel();
    });
    let cancelled_first = tokio::select! {
        biased;
        () = cancelled.cancelled() => true,
        _ = subscription.recv() => false,
    };
    trigger_task
        .await
        .expect("benchmark cancellation trigger should join");
    cancelled_first
}

pub async fn body_and_rpc_admission_saturation() -> usize {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let cancelled = CancellationToken::new();
    let body = BodyWorkAdmission::new();
    let mut queued = Vec::with_capacity(8);
    for _ in 0..8 {
        queued.push(
            body.try_admit_mcp_until(4 * 1024 * 1024, deadline, &cancelled)
                .expect("benchmark body job should queue"),
        );
    }
    let first = queued.remove(0);
    let second = queued.remove(0);
    let active_a = first
        .acquire_active(deadline, cancelled.child_token())
        .await
        .expect("first body job should activate");
    let active_b = second
        .acquire_active(deadline, cancelled.child_token())
        .await
        .expect("second body job should activate");

    let admission = CallAdmission::new(32);
    let mut calls = Vec::with_capacity(32);
    for _ in 0..32 {
        calls.push(
            admission
                .acquire()
                .await
                .expect("benchmark RPC call should be admitted"),
        );
    }
    let snapshot = body.snapshot();
    let score = snapshot
        .active
        .saturating_add(snapshot.queued)
        .saturating_add(calls.len());
    drop((active_a, active_b, queued, calls));
    score
}

pub struct SerializationFixture {
    inner: BenchmarkSerializationFixture,
}

pub fn serialization_fixture(payload_bytes: usize) -> SerializationFixture {
    SerializationFixture {
        inner: BenchmarkSerializationFixture::new(payload_bytes),
    }
}

pub async fn serialize_private_response(fixture: &SerializationFixture) -> usize {
    fixture
        .inner
        .serialize()
        .await
        .expect("benchmark response should serialize")
}

pub async fn serialize_private_responses(
    fixture: &SerializationFixture,
    concurrent_calls: usize,
) -> usize {
    let mut pending = (0..concurrent_calls)
        .map(|_| fixture.inner.serialize())
        .collect::<FuturesUnordered<_>>();
    let mut bytes = 0_usize;
    while let Some(result) = pending.next().await {
        bytes = bytes.saturating_add(result.expect("benchmark response should serialize"));
    }
    bytes
}

#[derive(Serialize)]
struct LargeResponse<'a> {
    operation: &'static str,
    payload: &'a str,
}

pub fn encode_private_frame(payload_bytes: usize) -> usize {
    let payload = "x".repeat(payload_bytes);
    encode_json_frame(
        &LargeResponse {
            operation: "benchmark",
            payload: &payload,
        },
        RESPONSE_MAX_BYTES,
    )
    .expect("benchmark private frame should encode")
    .len()
}

pub struct AuditFixture {
    audit: InstanceAudit,
    now: u64,
}

pub fn audit_fixture(record_count: usize) -> AuditFixture {
    let scope = InstanceScope {
        proxy_endpoint: "127.0.0.1:8989".parse().expect("benchmark endpoint"),
        run_id: RunId::from_str("AAAAAAAAAAAAAAAAAAAAAA").expect("benchmark run ID"),
    };
    let audit = InstanceAudit::new(
        scope.clone(),
        ConfigMode::Temporary,
        PersistenceMode::Ephemeral,
    );
    let now = 1_800_000_000_u64;
    for index in 0..record_count {
        audit.record(ControlCallAudit {
            client: DeclaredClient {
                name: format!("agent-{}", index % 64),
                version: "1".to_owned(),
            },
            operation: ControlOperationKind::SearchCaptures,
            mutation: None,
            outcome: ControlCallOutcome::Success {
                instance: Some(scope.clone()),
                config_mode: Some(ConfigMode::Temporary),
                persistence: Some(PersistenceMode::Ephemeral),
                settings_revision: None,
                affected: None,
            },
            duration: Duration::from_micros(50),
            response_bytes: 1024,
            response_delivery_failed: false,
            completed_unix_seconds: now.saturating_sub((index % 300) as u64),
        });
    }
    AuditFixture { audit, now }
}

pub fn aggregate_audit(fixture: &AuditFixture) -> usize {
    let snapshot = fixture.audit.snapshot(fixture.now);
    snapshot
        .reads
        .len()
        .saturating_add(snapshot.mutations.len())
}

pub struct MappingFixture {
    settings: AppSettings,
}

pub fn mapping_fixture(rule_count: usize) -> MappingFixture {
    let rules = (0..rule_count)
        .map(|index| ProxyMapRemoteRule {
            from: format!("https://source{index}.example/**"),
            to: format!("https://target{index}.example/"),
            enable: true,
        })
        .collect();
    let settings = AppSettings {
        proxy: Some(ProxySettings {
            active_preset: Some("benchmark".to_owned()),
            presets: vec![ProxyPresetSettings {
                name: "benchmark".to_owned(),
                map_remote: ProxyMapRemoteSettings {
                    rules,
                    ..ProxyMapRemoteSettings::default()
                },
                ..ProxyPresetSettings::default()
            }],
            ..ProxySettings::default()
        }),
        ..AppSettings::default()
    };
    MappingFixture { settings }
}

pub fn compile_mapping(fixture: &MappingFixture) -> usize {
    let compilation = RequestPolicy::compile(&fixture.settings);
    compilation.diagnostics.len()
}

pub fn validate_mapping(fixture: &MappingFixture) -> usize {
    let result = validate_mapping_candidate(fixture.settings.proxy.as_ref());
    result
        .diagnostics
        .len()
        .saturating_add(result.diagnostics_total)
}

pub fn explain_mapping(fixture: &MappingFixture, url: &str) -> usize {
    let explanation = explain_mapping_candidate(fixture.settings.proxy.as_ref(), url);
    explanation
        .diagnostics
        .len()
        .saturating_add(explanation.diagnostics_total)
}

pub fn mutate_mapping(fixture: &MappingFixture, index: usize) -> usize {
    let mut settings = fixture.settings.clone();
    let rule_count = settings.proxy.as_ref().expect("proxy fixture").presets[0]
        .map_remote
        .rules
        .len();
    let result = apply_mapping_mutation(
        &mut settings,
        MappingMutation::SetRuleEnabled {
            preset: "benchmark".to_owned(),
            table: ProxyRuleTable::Remote,
            index: index % rule_count,
            enabled: false,
        },
    )
    .expect("benchmark mapping mutation should apply");
    serde_json::to_vec(&result)
        .expect("benchmark mutation result should serialize")
        .len()
}

pub fn validate_mapping_serialization(fixture: &MappingFixture) -> usize {
    let validation = validate_mapping_candidate(fixture.settings.proxy.as_ref());
    encode_json_frame(&validation, RESPONSE_MAX_BYTES)
        .expect("benchmark mapping validation should serialize")
        .len()
}

pub fn monotonic_now() -> Instant {
    Instant::now()
}
