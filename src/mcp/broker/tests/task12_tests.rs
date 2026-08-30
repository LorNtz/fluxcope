use super::*;
use crate::{
    capture::{BodySide, BodyStreamState, CaptureSequence},
    control::{
        body::{
            BodyPageSource, BodyRange, ExtractCaptureBodyResult, ExtractSelector,
            ExtractSelectorKind, SearchCaptureBodyResult, SelectionPage,
        },
        json_walk::JsonPointer,
    },
    control_rpc::protocol::{ControlOperation, ControlResult, InstanceScope},
    mcp::{
        body::{ExtractCaptureBodyInput, SearchCaptureBodyInput, SelectionResourceUri},
        capture::RequiredInstanceSelector,
    },
};
use bytes::Bytes;
use serde_json::json;

struct Task12DispatchProbe {
    calls: Mutex<Vec<ControlOperation>>,
}

fn source() -> BodyPageSource {
    BodyPageSource {
        stream: BodyStreamState::Complete,
        observed_bytes: 16,
        retained_bytes: 16,
        truncated: false,
        truncation_reason: None,
        decoded_encoding_chain: Vec::new(),
        decoded_output_limited: false,
    }
}

impl InstanceProbe for Task12DispatchProbe {
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
            .expect("Task 12 dispatch calls")
            .push(operation.clone());
        let instance = InstanceScope {
            proxy_endpoint: descriptor.proxy_endpoint(),
            run_id: descriptor.run_id().clone(),
        };
        Box::pin(async move {
            match operation {
                ControlOperation::SearchCaptureBody(request) => {
                    Ok(ControlResult::SearchCaptureBody {
                        instance,
                        result: Box::new(SearchCaptureBodyResult {
                            capture_revision: request.capture_revision,
                            total_matches: 0,
                            omitted_matches: 0,
                            decoded_bytes_inspected: 16,
                            matches: Vec::new(),
                            stream: BodyStreamState::Complete,
                            observed_bytes: 16,
                            retained_bytes: 16,
                            source_truncated: false,
                            source_truncation_reason: None,
                            decoded_encoding_chain: Vec::new(),
                        }),
                    })
                }
                ControlOperation::ExtractCaptureBody(request) => {
                    Ok(ControlResult::ExtractCaptureBody {
                        instance,
                        result: Box::new(ExtractCaptureBodyResult {
                            capture_revision: request.capture_revision,
                            selector_kind: request.selector.kind(),
                            selected_bytes: 1,
                            media_type: "application/json".to_owned(),
                            inline: Some("7".to_owned()),
                            resource_uri: None,
                            stream: BodyStreamState::Complete,
                            observed_bytes: 16,
                            retained_bytes: 16,
                            source_truncated: false,
                            source_truncation_reason: None,
                            decoded_encoding_chain: Vec::new(),
                        }),
                    })
                }
                ControlOperation::ReadSelectedBody(request) => {
                    Ok(ControlResult::ReadSelectedBody {
                        instance,
                        page: Box::new(SelectionPage {
                            content: Bytes::from_static(br#"["snow"]"#),
                            media_type: "application/json".to_owned(),
                            selected_bytes: 8,
                            requested_range: BodyRange {
                                offset: request.offset,
                                length: request.length,
                            },
                            actual_range: BodyRange {
                                offset: request.offset,
                                length: 8,
                            },
                            next_offset: None,
                            next_uri: None,
                            source: source(),
                        }),
                    })
                }
                other => panic!("unexpected Task 12 operation: {other:?}"),
            }
        })
    }
}

#[tokio::test]
async fn broker_dispatches_search_extract_and_selected_resource_with_exact_identity() {
    let descriptor = descriptor(19012, RUN_A);
    let probe = Arc::new(Task12DispatchProbe {
        calls: Mutex::new(Vec::new()),
    });
    let broker = Broker::with_dependencies(
        PathBuf::from("/test/.wirelens/run/instances"),
        FakeRegistry::new(vec![descriptor.clone()]),
        Arc::clone(&probe),
    );
    let instance = RequiredInstanceSelector {
        proxy_endpoint: descriptor.proxy_endpoint(),
        run_id: descriptor.run_id().clone(),
    };

    let searched = broker
        .search_capture_body_impl(
            SearchCaptureBodyInput {
                instance: instance.clone(),
                capture_id: CaptureSequence::new(41),
                capture_revision: 7,
                side: BodySide::Response,
                query: "needle".to_owned(),
                limit: Some(2),
                context_bytes: Some(9),
            },
            client(),
            Instant::now() + Duration::from_secs(1),
            CancellationToken::new(),
        )
        .await
        .expect("search dispatch");
    assert_eq!(searched.result.capture_revision, 7);
    assert_eq!(
        searched.instance.proxy_endpoint,
        Some(descriptor.proxy_endpoint())
    );
    assert_eq!(searched.instance.run_id.as_ref(), Some(descriptor.run_id()));

    let extracted = broker
        .extract_capture_body_impl(
            ExtractCaptureBodyInput {
                instance,
                capture_id: CaptureSequence::new(41),
                capture_revision: 7,
                side: BodySide::Request,
                selector: ExtractSelector::JsonPointer {
                    pointer: JsonPointer::parse("/value").expect("pointer"),
                },
            },
            client(),
            Instant::now() + Duration::from_secs(1),
            CancellationToken::new(),
        )
        .await
        .expect("extract dispatch");
    assert_eq!(
        extracted.result.selector_kind,
        ExtractSelectorKind::JsonPointer
    );
    assert_eq!(
        extracted.instance.run_id.as_ref(),
        Some(descriptor.run_id())
    );

    let uri = format!(
        "wirelens://{}/runs/{}/captures/41/revisions/7/bodies/response/extract/json-pointer?pointer=%2Fvalue&offset=0&length=8192",
        descriptor.proxy_endpoint(),
        descriptor.run_id(),
    );
    let resource = broker
        .read_selection_resource_impl(
            SelectionResourceUri::parse(&uri).expect("selection URI"),
            client(),
            Instant::now() + Duration::from_secs(1),
            CancellationToken::new(),
        )
        .await
        .expect("selection resource dispatch");
    assert_eq!(resource.contents.len(), 1);
    let content = serde_json::to_value(&resource.contents[0]).expect("resource JSON");
    assert_eq!(content["uri"], json!(uri));
    assert_eq!(content["text"], json!(r#"["snow"]"#));
    assert_eq!(content["_meta"]["wirelens"]["capture_revision"], json!(7));

    let calls = probe.calls.lock().expect("Task 12 dispatch calls");
    assert!(matches!(
        &calls[0],
        ControlOperation::SearchCaptureBody(request)
            if request.capture_id == CaptureSequence::new(41)
                && request.capture_revision == 7
                && request.side == BodySide::Response
                && request.query == "needle"
                && request.limit == 2
                && request.context_bytes == 9
    ));
    assert!(matches!(
        &calls[1],
        ControlOperation::ExtractCaptureBody(request)
            if request.capture_id == CaptureSequence::new(41)
                && request.capture_revision == 7
                && request.side == BodySide::Request
                && matches!(&request.selector, ExtractSelector::JsonPointer { pointer } if pointer.as_str() == "/value")
    ));
    assert!(matches!(
        &calls[2],
        ControlOperation::ReadSelectedBody(request)
            if request.capture_id == CaptureSequence::new(41)
                && request.capture_revision == 7
                && request.side == BodySide::Response
                && request.offset == 0
                && request.length == 8192
    ));
}
