use rmcp::model::{ErrorData, GetPromptResult, JsonObject, Prompt, PromptMessage, Role};

const DEBUG_HTTP_FLOW: &str = "debug_http_flow";
const CONFIGURE_MAPPING: &str = "configure_mapping";

const DEBUG_HTTP_FLOW_TEXT: &str = r#"Debug one HTTP flow with Wirelens using bounded, retention-aware steps:
1. Call `list_instances`; if several instances are live, make an explicit endpoint/run choice and keep that exact identity on every later call. Inspect discovery diagnostics; use `get_broker_status` when diagnosing connection or resource limits. Empty discovery is not evidence of an empty capture store.
2. Call `get_status` and confirm recording is enabled and the selected instance has sufficient capture/body capacity. Use `set_recording_enabled` only when the user asked you to change it.
3. Trigger the target app action outside Wirelens when needed, then call `wait_for_capture` with a narrow query and milestone. Fall back to narrowly paginated `search_captures`; never broad-scan by default.
4. Call `get_capture` for compact request/response metadata. Respect capture retention, body preview truncation, live-stream state, and revision conflicts.
5. Before reading body windows, locate structure with `find_json_pointers`, `probe_json_pointer_pattern`, `search_capture_body`, or `extract_capture_body`.
6. Read only the necessary raw or decoded resource page. Treat unsupported decode, non-textual content, and configured decode/output limits as observable results; do not infer missing bytes.
7. Report the selected instance identity, capture ID/revision, evidence, and any retention, truncation, or decode limitation."#;

const CONFIGURE_MAPPING_TEXT: &str = r#"Configure Wirelens mappings with one revision-safe transaction per requested change:
1. Prefer native Wirelens MCP tools. Otherwise use `wirelens mcp tools [NAME]`, `wirelens mcp call TOOL --arguments @FILE`, and `wirelens mcp read URI`; do not improvise JSON-RPC pipelines or use a terminal for the broker. The CLI manages initialization, deadlines, output normalization, and shutdown. Discover only needed input schemas.
2. Call `list_instances`; choose one explicit endpoint/run identity and never retarget a supplied run ID. Inspect discovery diagnostics; use `get_broker_status` or `get_status` when relevant to diagnosis, not as mandatory full dumps.
3. Call `get_mapping_settings`, scoped to the requested `preset` and `table` when known; record `settings_revision`, config mode, persistence, active selection, gates, and scope/omissions. Rule indexes preserve table order, including disabled variants. Do not reconstruct a complete configuration from a scoped result.
4. Call `preview_mapping_mutation` with the exact instance, `expected_settings_revision`, requested typed `operation`, and affected `urls`. Inspect validation, affected object, gates, and original/effective mapping decisions. Preview changes no state and checks neither file readability nor traffic. Use `validate_mapping_settings` only for an independently prepared complete configuration, or `explain_mapping` for current-state decisions.
5. Apply the same requested typed mutation (`create_preset`, `rename_preset`, `delete_preset`, `set_active_preset`, `set_mapping_gate`, `create_mapping_rule`, `update_mapping_rule`, `delete_mapping_rule`, `move_mapping_rule`, or `set_mapping_rule_enabled`) with the preview's revision as `expected_settings_revision`. Preview is not a reservation: intervening edits and TUI drafts still block commit. Never enable unrelated gates.
6. Inspect the mutation outcome and persistence; use its returned revision for subsequent writes rather than assuming an increment or unconditionally rereading all settings. Refresh the affected scope when current indexes or reconciliation are needed. Stop on revision, TUI-draft, validation, persistence, or instance-generation conflict. After a lost acknowledgment, reread the same run's state before considering another write; do not blindly retry.
7. When authorized, verify through new traffic using `wait_for_capture`, `search_captures`, and `get_capture`. Report configuration validity, predicted selection, file/payload checks, actual traffic and response comparison separately. Effective-URL HTTP success does not establish original-URL HTTPS/TLS success or app/UI behavior. Keep fixture bodies and comparisons local."#;

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
mod tests {
    use serde_json::Value;

    use super::*;

    #[test]
    fn lists_exactly_the_two_approved_prompts() {
        let prompts = list();
        assert_eq!(prompts.len(), 2);
        assert_eq!(prompts[0].name, "debug_http_flow");
        assert_eq!(prompts[1].name, "configure_mapping");
        assert!(prompts.iter().all(|prompt| prompt.arguments.is_none()));
    }

    #[test]
    fn unknown_prompt_is_a_stable_invalid_argument() {
        let error = get("missing", Some(&serde_json::Map::<String, Value>::new()))
            .expect_err("unknown prompt");
        assert_eq!(error.code, rmcp::model::ErrorCode::INVALID_PARAMS);
        assert!(error.message.contains("missing"));
    }
}
