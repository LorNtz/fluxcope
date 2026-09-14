use super::*;
use crate::control::settings::mapping::{MappingMutationPreview, validate_preview_urls};

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum MappingPreviewOperation {
    CreatePreset {
        name: String,
        #[serde(default)]
        initial: Option<MappingPresetBodyInput>,
    },
    RenamePreset {
        name: String,
        new_name: String,
    },
    DeletePreset {
        name: String,
    },
    SetActivePreset {
        #[schemars(required)]
        #[serde(deserialize_with = "deserialize_required_option")]
        name: Option<String>,
    },
    SetMappingGate {
        gate: MappingGateTarget,
        enabled: bool,
    },
    CreateMappingRule {
        preset: String,
        table: ProxyRuleTable,
        #[serde(default)]
        index: Option<usize>,
        rule: MappingRuleInput,
    },
    UpdateMappingRule {
        preset: String,
        table: ProxyRuleTable,
        index: usize,
        from: String,
        to: String,
    },
    DeleteMappingRule {
        preset: String,
        table: ProxyRuleTable,
        index: usize,
    },
    MoveMappingRule {
        preset: String,
        table: ProxyRuleTable,
        from: usize,
        to: usize,
    },
    SetMappingRuleEnabled {
        preset: String,
        table: ProxyRuleTable,
        index: usize,
        enabled: bool,
    },
}

impl MappingPreviewOperation {
    fn into_mutation(
        self,
        instance: RequiredInstanceSelector,
        expected_settings_revision: SettingsRevision,
    ) -> MappingMutation {
        match self {
            Self::CreatePreset { name, initial } => CreatePresetInput {
                instance,
                expected_settings_revision,
                name,
                initial,
            }
            .into_mutation(),
            Self::RenamePreset { name, new_name } => RenamePresetInput {
                instance,
                expected_settings_revision,
                name,
                new_name,
            }
            .into_mutation(),
            Self::DeletePreset { name } => DeletePresetInput {
                instance,
                expected_settings_revision,
                name,
            }
            .into_mutation(),
            Self::SetActivePreset { name } => SetActivePresetInput {
                instance,
                expected_settings_revision,
                name,
            }
            .into_mutation(),
            Self::SetMappingGate { gate, enabled } => SetMappingGateInput {
                instance,
                expected_settings_revision,
                gate,
                enabled,
            }
            .into_mutation(),
            Self::CreateMappingRule {
                preset,
                table,
                index,
                rule,
            } => CreateMappingRuleInput {
                instance,
                expected_settings_revision,
                preset,
                table,
                index,
                rule,
            }
            .into_mutation(),
            Self::UpdateMappingRule {
                preset,
                table,
                index,
                from,
                to,
            } => UpdateMappingRuleInput {
                instance,
                expected_settings_revision,
                preset,
                table,
                index,
                from,
                to,
            }
            .into_mutation(),
            Self::DeleteMappingRule {
                preset,
                table,
                index,
            } => DeleteMappingRuleInput {
                instance,
                expected_settings_revision,
                preset,
                table,
                index,
            }
            .into_mutation(),
            Self::MoveMappingRule {
                preset,
                table,
                from,
                to,
            } => MoveMappingRuleInput {
                instance,
                expected_settings_revision,
                preset,
                table,
                from,
                to,
            }
            .into_mutation(),
            Self::SetMappingRuleEnabled {
                preset,
                table,
                index,
                enabled,
            } => SetMappingRuleEnabledInput {
                instance,
                expected_settings_revision,
                preset,
                table,
                index,
                enabled,
            }
            .into_mutation(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PreviewMappingMutationInput {
    pub(crate) instance: RequiredInstanceSelector,
    pub(crate) expected_settings_revision: SettingsRevision,
    pub(crate) operation: MappingPreviewOperation,
    /// At most 16 URLs, each containing 1..=65536 UTF-8 bytes. Selection is remote then local.
    #[schemars(schema_with = "preview_urls_schema")]
    pub(crate) urls: Vec<String>,
}

fn preview_urls_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schemars::json_schema!({
        "type": "array",
        "maxItems": crate::control::settings::mapping::MAX_PREVIEW_URLS,
        "items": {
            "type": "string",
            "minLength": 1,
            "maxLength": crate::control::settings::mapping::MAX_PREVIEW_URL_BYTES
        }
    })
}

impl PreviewMappingMutationInput {
    pub(crate) fn into_operation(self) -> Result<ControlOperation, ControlError> {
        validate_preview_urls(&self.urls)?;
        Ok(ControlOperation::PreviewMappingMutation {
            expected_revision: self.expected_settings_revision,
            mutation: Box::new(
                self.operation
                    .into_mutation(self.instance, self.expected_settings_revision),
            ),
            urls: self.urls,
        })
    }
}

#[derive(Clone, Debug, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PreviewMappingMutationResult {
    pub(crate) instance: InstanceSelector,
    pub(crate) settings_revision: SettingsRevision,
    pub(crate) config_mode: ConfigMode,
    pub(crate) persistence: PersistenceMode,
    pub(crate) preview: MappingMutationPreview,
}

pub(crate) fn preview_mapping_result(
    result: ControlResult,
) -> Result<PreviewMappingMutationResult, ControlError> {
    match result {
        ControlResult::PreviewMappingMutation {
            instance,
            settings_revision,
            config_mode,
            persistence,
            preview,
        } => Ok(PreviewMappingMutationResult {
            instance: selector(instance),
            settings_revision,
            config_mode,
            persistence,
            preview: *preview,
        }),
        _ => Err(ControlError::internal(
            "private RPC returned an unexpected mapping preview result",
        )),
    }
}

#[cfg(test)]
#[path = "preview_tests.rs"]
mod tests;
