use super::{
    SearchCapturesInput, SearchCapturesResult, SetRecordingEnabledInput, SetRecordingEnabledResult,
};
use crate::{
    control::capture_query::{CaptureQuery, CaptureSearchCursor},
    control_rpc::protocol::{ControlError, ControlErrorCode},
    instance::RunId,
    mcp::{
        broker::{Broker, to_mcp_error},
        schema::InstanceSelector,
    },
};
use rmcp::{
    ServiceExt,
    model::{ClientCapabilities, ClientInfo, Implementation, ProtocolVersion},
};
use schemars::{JsonSchema, schema_for};
use serde_json::{Value, json};
use std::str::FromStr;
use tokio::io::duplex;

const RUN_ID: &str = "AAAAAAAAAAAAAAAAAAAAAA";

fn selected_instance() -> InstanceSelector {
    InstanceSelector {
        proxy_endpoint: Some("127.0.0.1:19040".parse().expect("endpoint")),
        run_id: Some(RunId::from_str(RUN_ID).expect("run ID")),
    }
}

fn schema_json<T: JsonSchema>() -> Value {
    serde_json::to_value(schema_for!(T)).expect("schema JSON")
}

#[test]
fn capture_tool_inputs_are_closed_and_reject_malformed_query_enums_and_limits() {
    for schema in [
        schema_json::<SetRecordingEnabledInput>(),
        schema_json::<SearchCapturesInput>(),
    ] {
        assert_eq!(schema["type"], "object");
        assert_eq!(schema["additionalProperties"], false);
    }

    assert!(
        serde_json::from_value::<SetRecordingEnabledInput>(json!({
            "instance": {
                "proxy_endpoint": "127.0.0.1:19040",
                "run_id": RUN_ID
            }
        }))
        .is_err(),
        "enabled is explicit and required"
    );
    assert!(
        serde_json::from_value::<SetRecordingEnabledInput>(json!({
            "instance": {
                "proxy_endpoint": "127.0.0.1:19040",
                "run_id": RUN_ID
            },
            "enabled": true,
            "unexpected": true
        }))
        .is_err()
    );
    for invalid in [
        json!({"query": {"mapping_path": "remote-local"}}),
        json!({"query": {"lifecycle": "done"}}),
        json!({"query": {"original_url": {"mode": "auto", "value": "*"}}}),
        json!({"query": {}, "cursor": "30"}),
        json!({"query": {}, "limit": 0}),
        json!({"query": {}, "limit": 101}),
        json!({"query": {}, "unexpected": true}),
    ] {
        assert!(
            serde_json::from_value::<SearchCapturesInput>(invalid).is_err(),
            "strict search input must reject malformed values"
        );
    }
}

#[test]
fn mutation_input_requires_endpoint_run_id_and_explicit_boolean() {
    for value in [
        json!({"instance": {}, "enabled": true}),
        json!({
            "instance": {"proxy_endpoint": "127.0.0.1:19040"},
            "enabled": true
        }),
        json!({"instance": {"run_id": RUN_ID}, "enabled": true}),
    ] {
        let input: SetRecordingEnabledInput =
            serde_json::from_value(value).expect("syntactically valid selector");
        let error = input.validate().expect_err("mutation selector incomplete");
        assert_eq!(error.code, ControlErrorCode::InvalidArgument);
    }

    let valid = SetRecordingEnabledInput {
        instance: selected_instance(),
        enabled: false,
    };
    valid.validate().expect("complete mutation selector");
}

#[test]
fn snapshot_search_allows_an_omitted_selector_but_keeps_query_cursor_and_limit_strict() {
    let omitted: SearchCapturesInput =
        serde_json::from_value(json!({})).expect("omitted snapshot selector");
    assert_eq!(omitted.instance, InstanceSelector::default());
    assert_eq!(omitted.query, CaptureQuery::default());
    assert_eq!(omitted.cursor, None);
    assert_eq!(omitted.limit, None);

    let explicit: SearchCapturesInput = serde_json::from_value(json!({
        "instance": {"proxy_endpoint": "127.0.0.1:19040", "run_id": RUN_ID},
        "query": {"method": "POST"},
        "cursor": {"next_older_sequence": 30},
        "limit": 10
    }))
    .expect("explicit search");
    assert_eq!(explicit.query.method.as_deref(), Some("POST"));
    assert_eq!(
        explicit.cursor,
        Some(CaptureSearchCursor::new(
            crate::capture::CaptureSequence::new(30)
        ))
    );
    assert_eq!(explicit.limit, Some(10));
}

#[test]
fn mutation_and_search_results_repeat_resolved_identity_with_stable_shapes() {
    let recording = SetRecordingEnabledResult {
        instance: selected_instance(),
        previous: false,
        current: true,
    };
    assert_eq!(
        serde_json::to_value(recording).expect("recording result"),
        json!({
            "instance": {
                "proxy_endpoint": "127.0.0.1:19040",
                "run_id": RUN_ID
            },
            "previous": false,
            "current": true
        })
    );

    let search = SearchCapturesResult {
        instance: selected_instance(),
        captures: Vec::new(),
        next_cursor: None,
    };
    assert_eq!(
        serde_json::to_value(search).expect("search result"),
        json!({
            "instance": {
                "proxy_endpoint": "127.0.0.1:19040",
                "run_id": RUN_ID
            },
            "captures": [],
            "next_cursor": null
        })
    );
    for schema in [
        schema_json::<SetRecordingEnabledResult>(),
        schema_json::<SearchCapturesResult>(),
    ] {
        assert_eq!(schema["type"], "object");
        assert_eq!(schema["additionalProperties"], false);
    }
}

#[test]
fn typed_control_failures_retain_code_retryability_and_structured_details_in_mcp_data() {
    let error = to_mcp_error(ControlError::new(
        ControlErrorCode::CaptureRevisionConflict,
        "capture revision changed",
        false,
        json!({
            "capture_id": 7,
            "expected_revision": 2,
            "current_revision": 3
        }),
    ));
    let data = error.data.expect("structured MCP error data");

    assert_eq!(data["code"], "capture_revision_conflict");
    assert_eq!(data["retryable"], false);
    assert_eq!(
        data["details"],
        json!({
            "capture_id": 7,
            "expected_revision": 2,
            "current_revision": 3
        })
    );
}

#[tokio::test]
async fn real_broker_transport_advertises_only_the_task8_public_capture_tools() {
    let home = tempfile::tempdir().expect("temporary home");
    let broker = Broker::new(home.path()).expect("real broker");
    let (client_transport, server_transport) = duplex(64 * 1024);
    let server = tokio::spawn(async move {
        let service = broker.serve(server_transport).await.expect("serve broker");
        service.waiting().await.expect("broker shutdown");
    });
    let client_info = ClientInfo::new(
        ClientCapabilities::default(),
        Implementation::new("capture-contract", "1.0"),
    )
    .with_protocol_version(ProtocolVersion::LATEST);
    let client = client_info
        .serve(client_transport)
        .await
        .expect("initialize client");
    let tools = client.list_all_tools().await.expect("list tools");

    for (name, read_only) in [
        ("get_status", true),
        ("set_recording_enabled", false),
        ("search_captures", true),
    ] {
        let tool = tools
            .iter()
            .find(|tool| tool.name == name)
            .unwrap_or_else(|| panic!("missing {name}"));
        let annotations = tool.annotations.as_ref().expect("tool annotations");
        assert_eq!(annotations.read_only_hint, Some(read_only));
        assert_eq!(annotations.destructive_hint, Some(false));
        assert_eq!(annotations.idempotent_hint, Some(true));
        assert_eq!(annotations.open_world_hint, Some(false));
        assert_eq!(tool.input_schema["type"], json!("object"));
        assert_eq!(tool.input_schema["additionalProperties"], false);
    }
    assert!(tools.iter().all(|tool| tool.name != "get_capture"));

    client.cancel().await.expect("close client");
    server.await.expect("server task");
}
