use super::*;
use crate::{
    capture::{BodySide, CaptureSequence},
    control::json_walk::{
        FieldMatchMode, FindJsonPointersResult, JsonInspectionStatus, JsonPointerPattern,
        JsonTypeCounts, ProbeJsonPointerPatternResult,
    },
    control_rpc::protocol::{ControlOperation, ControlResult, InstanceScope},
    mcp::{
        body::{FindJsonPointersInput, ProbeJsonPointerPatternInput},
        capture::RequiredInstanceSelector,
    },
};

struct JsonDispatchProbe {
    calls: Mutex<Vec<ControlOperation>>,
}

impl InstanceProbe for JsonDispatchProbe {
    fn describe<'a>(
        &'a self,
        descriptor: &'a InstanceDescriptor,
        _client: DeclaredClient,
        _deadline: Instant,
        _cancelled: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = Result<ControlResult, ControlError>> + Send + 'a>> {
        Box::pin(async move { Ok(describe(descriptor, 1)) })
    }

    fn call<'a>(
        &'a self,
        descriptor: &'a InstanceDescriptor,
        operation: ControlOperation,
        _client: DeclaredClient,
        _deadline: Instant,
        _cancelled: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = Result<ControlResult, ControlError>> + Send + 'a>> {
        self.calls
            .lock()
            .expect("JSON dispatch calls")
            .push(operation.clone());
        let instance = InstanceScope {
            proxy_endpoint: descriptor.proxy_endpoint(),
            run_id: descriptor.run_id().clone(),
        };
        Box::pin(async move {
            match operation {
                ControlOperation::FindJsonPointers(request) => {
                    Ok(ControlResult::FindJsonPointers {
                        instance,
                        result: Box::new(FindJsonPointersResult {
                            status: JsonInspectionStatus::NoMatch,
                            capture_revision: request.capture_revision,
                            matches: Vec::new(),
                            total_matches: 0,
                            omitted_matches: 0,
                            inspected_bytes: 2,
                            source_truncated: false,
                            truncated: false,
                        }),
                    })
                }
                ControlOperation::ProbeJsonPointerPattern(request) => {
                    Ok(ControlResult::ProbeJsonPointerPattern {
                        instance,
                        result: Box::new(ProbeJsonPointerPatternResult {
                            status: JsonInspectionStatus::NoMatch,
                            capture_revision: request.capture_revision,
                            match_count: 0,
                            type_counts: JsonTypeCounts::default(),
                            examples: Vec::new(),
                            omitted_examples: 0,
                            object_key_summaries: Vec::new(),
                            omitted_object_summaries: 0,
                            array_length_range: None,
                            miss_hint: None,
                            inspected_bytes: 2,
                            source_truncated: false,
                            truncated: false,
                        }),
                    })
                }
                other => panic!("unexpected JSON dispatch operation: {other:?}"),
            }
        })
    }
}

#[tokio::test]
async fn broker_dispatches_both_json_tools_with_exact_revision_pinned_operations() {
    let descriptor = descriptor(19011, RUN_A);
    let probe = Arc::new(JsonDispatchProbe {
        calls: Mutex::new(Vec::new()),
    });
    let broker = Broker::with_dependencies(
        PathBuf::from("/test/.fluxcope/run/instances"),
        FakeRegistry::new(vec![descriptor.clone()]),
        Arc::clone(&probe),
    );
    let instance = RequiredInstanceSelector {
        proxy_endpoint: descriptor.proxy_endpoint(),
        run_id: descriptor.run_id().clone(),
    };

    let found = broker
        .find_json_pointers_impl(
            FindJsonPointersInput {
                instance: instance.clone(),
                capture_id: CaptureSequence::new(41),
                capture_revision: 7,
                side: BodySide::Response,
                field_name: "userId".to_owned(),
                match_mode: FieldMatchMode::UnicodeCasefoldExact,
                limit: Some(20),
            },
            client(),
            Instant::now() + Duration::from_secs(1),
            CancellationToken::new(),
        )
        .await
        .expect("field dispatch");
    assert_eq!(found.result.capture_revision, 7);

    let probed = broker
        .probe_json_pointer_pattern_impl(
            ProbeJsonPointerPatternInput {
                instance,
                capture_id: CaptureSequence::new(41),
                capture_revision: 7,
                side: BodySide::Request,
                pattern: JsonPointerPattern::parse("/orders/*/userId").expect("pattern"),
            },
            client(),
            Instant::now() + Duration::from_secs(1),
            CancellationToken::new(),
        )
        .await
        .expect("pattern dispatch");
    assert_eq!(probed.result.capture_revision, 7);

    let calls = probe.calls.lock().expect("JSON dispatch calls");
    assert!(matches!(
        &calls[0],
        ControlOperation::FindJsonPointers(request)
            if request.capture_id == CaptureSequence::new(41)
                && request.capture_revision == 7
                && request.side == BodySide::Response
                && request.limit == 20
    ));
    assert!(matches!(
        &calls[1],
        ControlOperation::ProbeJsonPointerPattern(request)
            if request.capture_id == CaptureSequence::new(41)
                && request.capture_revision == 7
                && request.side == BodySide::Request
                && request.pattern.as_str() == "/orders/*/userId"
    ));
}
