use rmcp::model::{ErrorData, GetPromptResult, JsonObject, Prompt, PromptMessage, Role};

const DEBUG_HTTP_FLOW: &str = "debug_http_flow";
const CONFIGURE_MAPPING: &str = "configure_mapping";

const DEBUG_HTTP_FLOW_TEXT: &str = r#"Debug one HTTP flow with Wirelens using bounded, retention-aware steps:
1. Call `get_broker_status`, then `list_instances`; if several instances are live, make an explicit endpoint/run choice and keep that exact identity on every later call. Treat status warnings and reported limits as the operating boundary.
2. Call `get_status` and confirm recording is enabled and the selected instance has sufficient capture/body capacity. Use `set_recording_enabled` only when the user asked you to change it.
3. Trigger the target app action outside Wirelens when needed, then call `wait_for_capture` with a narrow query and milestone. Fall back to narrowly paginated `search_captures`; never broad-scan by default.
4. Call `get_capture` for compact request/response metadata. Respect capture retention, body preview truncation, live-stream state, and revision conflicts.
5. Before reading body windows, locate structure with `find_json_pointers`, `probe_json_pointer_pattern`, `search_capture_body`, or `extract_capture_body`.
6. Read only the necessary raw or decoded resource page. Treat unsupported decode, non-textual content, and configured decode/output limits as observable results; do not infer missing bytes.
7. Report the selected instance identity, capture ID/revision, evidence, and any retention, truncation, or decode limitation."#;

const CONFIGURE_MAPPING_TEXT: &str = r#"Configure Wirelens mappings with one revision-safe transaction per requested change:
1. Call `get_broker_status`, then `list_instances`; choose one explicit endpoint/run identity and never retarget a supplied run ID. Treat status warnings and reported limits as the operating boundary.
2. Call `get_mapping_settings`; record `settings_revision`, config mode, and persistence. Explain whether the result is persistent or ephemeral.
3. Build only the requested candidate change. Call `validate_mapping_settings`, then `explain_mapping` for affected URLs without reading local files.
4. Apply only the requested typed mutation (`create_preset`, `rename_preset`, `delete_preset`, `set_active_preset`, `set_mapping_gate`, `create_mapping_rule`, `update_mapping_rule`, `delete_mapping_rule`, `move_mapping_rule`, or `set_mapping_rule_enabled`) with `expected_settings_revision`.
5. After every successful write, refresh with `get_mapping_settings` and use the returned revision for the next write. Stop on revision, TUI-draft, validation, persistence, or instance-generation conflict rather than retrying blindly.
6. When applicable, verify the mapping through new traffic using `wait_for_capture`, `search_captures`, and `get_capture`; do not claim an unexercised mapping works."#;

#[cfg(test)]
const REFERENCES: &[&str] = &[
    "get_broker_status",
    "list_instances",
    "get_status",
    "set_recording_enabled",
    "wait_for_capture",
    "search_captures",
    "get_capture",
    "find_json_pointers",
    "probe_json_pointer_pattern",
    "search_capture_body",
    "extract_capture_body",
    "get_mapping_settings",
    "validate_mapping_settings",
    "explain_mapping",
    "create_preset",
    "rename_preset",
    "delete_preset",
    "set_active_preset",
    "set_mapping_gate",
    "create_mapping_rule",
    "update_mapping_rule",
    "delete_mapping_rule",
    "move_mapping_rule",
    "set_mapping_rule_enabled",
];

pub(crate) fn list() -> Vec<Prompt> {
    vec![
        Prompt::new(
            DEBUG_HTTP_FLOW,
            Some("Debug one captured HTTP flow with bounded, retention-aware inspection"),
            None,
        ),
        Prompt::new(
            CONFIGURE_MAPPING,
            Some("Validate and apply revision-safe Wirelens mapping changes"),
            None,
        ),
    ]
}

pub(crate) fn get(
    name: &str,
    arguments: Option<&JsonObject>,
) -> Result<GetPromptResult, ErrorData> {
    if arguments.is_some_and(|arguments| !arguments.is_empty()) {
        return Err(ErrorData::invalid_params(
            format!("prompt '{name}' does not accept arguments"),
            None,
        ));
    }
    let (description, text) = match name {
        DEBUG_HTTP_FLOW => (
            "A bounded workflow for finding and inspecting one HTTP exchange",
            DEBUG_HTTP_FLOW_TEXT,
        ),
        CONFIGURE_MAPPING => (
            "A revision-safe workflow for reading, validating, mutating, and verifying mappings",
            CONFIGURE_MAPPING_TEXT,
        ),
        _ => {
            return Err(ErrorData::invalid_params(
                format!("prompt '{name}' not found"),
                None,
            ));
        }
    };
    Ok(
        GetPromptResult::new(vec![PromptMessage::new_text(Role::User, text)])
            .with_description(description),
    )
}

#[cfg(test)]
pub(crate) fn references() -> &'static [&'static str] {
    REFERENCES
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use serde_json::Value;

    use super::*;

    const ADVERTISED_TOOLS: &[&str] = &[
        "get_broker_status",
        "list_instances",
        "get_status",
        "set_recording_enabled",
        "search_captures",
        "get_capture",
        "wait_for_capture",
        "find_json_pointers",
        "probe_json_pointer_pattern",
        "search_capture_body",
        "extract_capture_body",
        "get_mapping_settings",
        "validate_mapping_settings",
        "explain_mapping",
        "create_preset",
        "rename_preset",
        "delete_preset",
        "set_active_preset",
        "set_mapping_gate",
        "create_mapping_rule",
        "update_mapping_rule",
        "delete_mapping_rule",
        "move_mapping_rule",
        "set_mapping_rule_enabled",
    ];

    const ADVERTISED_TEMPLATES: &[&str] = &[
        "wirelens://instances/{proxy_endpoint}/{run_id}/captures/{capture_id}/{side}/{representation}?revision={revision}&start={start}&length={length}",
        "wirelens://instances/{proxy_endpoint}/{run_id}/captures/{capture_id}/{side}/json?revision={revision}&pointer={pointer}&start={start}&length={length}",
        "wirelens://instances/{proxy_endpoint}/{run_id}/captures/{capture_id}/{side}/form?revision={revision}&name={name}&occurrence={occurrence}&start={start}&length={length}",
    ];

    fn prompt_text(name: &str) -> String {
        let result = get(name, None).expect("known prompt");
        let value = serde_json::to_value(result).expect("prompt result JSON");
        value["messages"]
            .as_array()
            .expect("messages")
            .iter()
            .filter_map(|message| message["content"]["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn lists_exactly_the_two_approved_prompts() {
        let prompts = list();
        assert_eq!(prompts.len(), 2);
        assert_eq!(prompts[0].name, "debug_http_flow");
        assert_eq!(prompts[1].name, "configure_mapping");
        assert!(prompts.iter().all(|prompt| prompt.arguments.is_none()));
    }

    #[test]
    fn prompts_reference_only_advertised_surface() {
        let tools = ADVERTISED_TOOLS.iter().copied().collect::<HashSet<_>>();
        let templates = ADVERTISED_TEMPLATES.iter().copied().collect::<HashSet<_>>();
        for reference in references() {
            assert!(
                tools.contains(reference) || templates.contains(reference),
                "prompt references unadvertised surface {reference}"
            );
        }
    }

    #[test]
    fn debug_prompt_encodes_safe_bounded_workflow() {
        let text = prompt_text("debug_http_flow");
        for required in [
            "get_broker_status",
            "list_instances",
            "explicit",
            "get_status",
            "wait_for_capture",
            "search_captures",
            "get_capture",
            "find_json_pointers",
            "retention",
            "truncation",
            "decode",
        ] {
            assert!(text.contains(required), "missing {required}: {text}");
        }
        assert!(!text.contains("Authorization"));
    }

    #[test]
    fn mapping_prompt_requires_revision_safe_validation_and_verification() {
        let text = prompt_text("configure_mapping");
        for required in [
            "get_broker_status",
            "list_instances",
            "explicit",
            "get_mapping_settings",
            "persistence",
            "validate_mapping_settings",
            "explain_mapping",
            "expected_settings_revision",
            "refresh",
            "new traffic",
        ] {
            assert!(text.contains(required), "missing {required}: {text}");
        }
    }

    #[test]
    fn unknown_prompt_is_a_stable_invalid_argument() {
        let error = get("missing", Some(&serde_json::Map::<String, Value>::new()))
            .expect_err("unknown prompt");
        assert_eq!(error.code, rmcp::model::ErrorCode::INVALID_PARAMS);
        assert!(error.message.contains("missing"));
    }
}
