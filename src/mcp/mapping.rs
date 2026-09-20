use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize};

use super::{capture::RequiredInstanceSelector, schema::InstanceSelector};
#[path = "mapping/preview.rs"]
mod preview;
use crate::control::settings::mapping::{MappingReadScope, MappingSettingsView};
use crate::{
    control_rpc::protocol::{ControlError, ControlOperation, ControlResult},
    runtime::settings::{SettingsRevision, SettingsTransactionOutcome},
    settings::{
        ConfigMode, PersistenceMode, ProxyMapLocalRule, ProxyMapLocalSettings, ProxyMapRemoteRule,
        ProxyMapRemoteSettings, ProxyPresetSettings, ProxySettings,
        mapping_ops::{
            MappingExplanation, MappingMutation, MappingObjectRef, MappingValidationResult,
            ProxyRuleTable,
        },
    },
};
pub(crate) use preview::{
    PreviewMappingMutationInput, PreviewMappingMutationResult, preview_mapping_result,
};

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct GetMappingSettingsInput {
    #[serde(default)]
    #[schemars(default)]
    pub(crate) instance: InstanceSelector,
    #[serde(default)]
    pub(crate) preset: Option<String>,
    #[serde(default)]
    pub(crate) table: Option<ProxyRuleTable>,
}

impl GetMappingSettingsInput {
    pub(crate) fn into_operation(self) -> ControlOperation {
        ControlOperation::GetMappingSettings {
            scope: MappingReadScope {
                preset: self.preset,
                table: self.table,
            },
        }
    }
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ValidateMappingSettingsInput {
    #[serde(default)]
    #[schemars(default)]
    pub(crate) instance: InstanceSelector,
    pub(crate) proxy: MappingSettingsConfigInput,
}

impl ValidateMappingSettingsInput {
    pub(crate) fn into_operation(self) -> ControlOperation {
        ControlOperation::ValidateMappingSettings {
            proxy: Box::new(self.proxy.into()),
        }
    }
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ExplainMappingInput {
    #[serde(default)]
    #[schemars(default)]
    pub(crate) instance: InstanceSelector,
    pub(crate) url: String,
    #[serde(default)]
    pub(crate) proposed_proxy: Option<MappingSettingsConfigInput>,
}

impl ExplainMappingInput {
    pub(crate) fn into_operation(self) -> ControlOperation {
        ControlOperation::ExplainMapping {
            url: self.url,
            proposed_proxy: self.proposed_proxy.map(|proxy| Box::new(proxy.into())),
        }
    }
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CreatePresetInput {
    pub(crate) instance: RequiredInstanceSelector,
    pub(crate) expected_settings_revision: SettingsRevision,
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) initial: Option<MappingPresetBodyInput>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RenamePresetInput {
    pub(crate) instance: RequiredInstanceSelector,
    pub(crate) expected_settings_revision: SettingsRevision,
    pub(crate) name: String,
    pub(crate) new_name: String,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DeletePresetInput {
    pub(crate) instance: RequiredInstanceSelector,
    pub(crate) expected_settings_revision: SettingsRevision,
    pub(crate) name: String,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SetActivePresetInput {
    pub(crate) instance: RequiredInstanceSelector,
    pub(crate) expected_settings_revision: SettingsRevision,
    #[schemars(required)]
    #[serde(deserialize_with = "deserialize_required_option")]
    pub(crate) name: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum MappingGateTarget {
    Global,
    Table {
        preset: String,
        table: ProxyRuleTable,
    },
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SetMappingGateInput {
    pub(crate) instance: RequiredInstanceSelector,
    pub(crate) expected_settings_revision: SettingsRevision,
    pub(crate) gate: MappingGateTarget,
    pub(crate) enabled: bool,
}

fn default_enabled() -> bool {
    true
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct MappingRuleTableConfigInput {
    #[serde(default = "default_enabled")]
    pub(crate) enabled: bool,
    #[serde(default)]
    pub(crate) rules: Vec<MappingRuleInput>,
}

impl Default for MappingRuleTableConfigInput {
    fn default() -> Self {
        Self {
            enabled: true,
            rules: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct MappingPresetBodyInput {
    #[serde(default)]
    pub(crate) map_remote: MappingRuleTableConfigInput,
    #[serde(default)]
    pub(crate) map_local: MappingRuleTableConfigInput,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct MappingPresetConfigInput {
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) map_remote: MappingRuleTableConfigInput,
    #[serde(default)]
    pub(crate) map_local: MappingRuleTableConfigInput,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct MappingSettingsConfigInput {
    #[serde(default = "default_enabled")]
    pub(crate) enabled: bool,
    #[serde(default)]
    pub(crate) active_preset: Option<String>,
    #[serde(default)]
    pub(crate) presets: Vec<MappingPresetConfigInput>,
}

impl From<MappingRuleTableConfigInput> for ProxyMapRemoteSettings {
    fn from(table: MappingRuleTableConfigInput) -> Self {
        Self {
            enable: table.enabled,
            rules: table
                .rules
                .into_iter()
                .map(|rule| ProxyMapRemoteRule {
                    from: rule.from,
                    to: rule.to,
                    enable: rule.enabled,
                })
                .collect(),
        }
    }
}

impl From<MappingRuleTableConfigInput> for ProxyMapLocalSettings {
    fn from(table: MappingRuleTableConfigInput) -> Self {
        Self {
            enable: table.enabled,
            rules: table
                .rules
                .into_iter()
                .map(|rule| ProxyMapLocalRule {
                    from: rule.from,
                    to: rule.to,
                    enable: rule.enabled,
                })
                .collect(),
        }
    }
}

impl From<MappingPresetBodyInput> for ProxyPresetSettings {
    fn from(preset: MappingPresetBodyInput) -> Self {
        Self {
            name: String::new(),
            map_remote: preset.map_remote.into(),
            map_local: preset.map_local.into(),
        }
    }
}

impl From<MappingPresetConfigInput> for ProxyPresetSettings {
    fn from(preset: MappingPresetConfigInput) -> Self {
        Self {
            name: preset.name,
            map_remote: preset.map_remote.into(),
            map_local: preset.map_local.into(),
        }
    }
}

impl From<MappingSettingsConfigInput> for ProxySettings {
    fn from(proxy: MappingSettingsConfigInput) -> Self {
        Self {
            enable: proxy.enabled,
            active_preset: proxy.active_preset,
            presets: proxy.presets.into_iter().map(Into::into).collect(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct MappingRuleInput {
    pub(crate) from: String,
    pub(crate) to: String,
    #[serde(default = "default_enabled")]
    pub(crate) enabled: bool,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CreateMappingRuleInput {
    pub(crate) instance: RequiredInstanceSelector,
    pub(crate) expected_settings_revision: SettingsRevision,
    pub(crate) preset: String,
    pub(crate) table: ProxyRuleTable,
    #[serde(default)]
    pub(crate) index: Option<usize>,
    pub(crate) rule: MappingRuleInput,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct UpdateMappingRuleInput {
    pub(crate) instance: RequiredInstanceSelector,
    pub(crate) expected_settings_revision: SettingsRevision,
    pub(crate) preset: String,
    pub(crate) table: ProxyRuleTable,
    pub(crate) index: usize,
    pub(crate) from: String,
    pub(crate) to: String,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DeleteMappingRuleInput {
    pub(crate) instance: RequiredInstanceSelector,
    pub(crate) expected_settings_revision: SettingsRevision,
    pub(crate) preset: String,
    pub(crate) table: ProxyRuleTable,
    pub(crate) index: usize,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct MoveMappingRuleInput {
    pub(crate) instance: RequiredInstanceSelector,
    pub(crate) expected_settings_revision: SettingsRevision,
    pub(crate) preset: String,
    pub(crate) table: ProxyRuleTable,
    pub(crate) from: usize,
    pub(crate) to: usize,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SetMappingRuleEnabledInput {
    pub(crate) instance: RequiredInstanceSelector,
    pub(crate) expected_settings_revision: SettingsRevision,
    pub(crate) preset: String,
    pub(crate) table: ProxyRuleTable,
    pub(crate) index: usize,
    pub(crate) enabled: bool,
}

macro_rules! operation_impl {
    ($type:ty, $body:expr) => {
        impl $type {
            pub(crate) fn into_operation(self) -> ControlOperation {
                ControlOperation::MutateMapping {
                    expected_revision: self.expected_settings_revision,
                    mutation: Box::new(self.into_mutation()),
                }
            }
            fn into_mutation(self) -> MappingMutation {
                ($body)(self)
            }
        }
    };
}

operation_impl!(CreatePresetInput, |value: CreatePresetInput| {
    MappingMutation::CreatePreset {
        name: value.name,
        initial: value.initial.map(Into::into),
    }
});
operation_impl!(RenamePresetInput, |value: RenamePresetInput| {
    MappingMutation::RenamePreset {
        name: value.name,
        new_name: value.new_name,
    }
});
operation_impl!(DeletePresetInput, |value: DeletePresetInput| {
    MappingMutation::DeletePreset { name: value.name }
});
operation_impl!(SetActivePresetInput, |value: SetActivePresetInput| {
    MappingMutation::SetActivePreset { name: value.name }
});
operation_impl!(
    SetMappingGateInput,
    |value: SetMappingGateInput| match value.gate {
        MappingGateTarget::Global => MappingMutation::SetGlobalEnabled {
            enabled: value.enabled
        },
        MappingGateTarget::Table { preset, table } => MappingMutation::SetTableEnabled {
            preset,
            table,
            enabled: value.enabled,
        },
    }
);
operation_impl!(CreateMappingRuleInput, |value: CreateMappingRuleInput| {
    match (value.table, value.index) {
        (ProxyRuleTable::Remote, Some(index)) => MappingMutation::InsertRemoteRule {
            preset: value.preset,
            index,
            rule: ProxyMapRemoteRule {
                from: value.rule.from,
                to: value.rule.to,
                enable: value.rule.enabled,
            },
        },
        (ProxyRuleTable::Remote, None) => MappingMutation::AppendRemoteRule {
            preset: value.preset,
            rule: ProxyMapRemoteRule {
                from: value.rule.from,
                to: value.rule.to,
                enable: value.rule.enabled,
            },
        },
        (ProxyRuleTable::Local, Some(index)) => MappingMutation::InsertLocalRule {
            preset: value.preset,
            index,
            rule: ProxyMapLocalRule {
                from: value.rule.from,
                to: value.rule.to,
                enable: value.rule.enabled,
            },
        },
        (ProxyRuleTable::Local, None) => MappingMutation::AppendLocalRule {
            preset: value.preset,
            rule: ProxyMapLocalRule {
                from: value.rule.from,
                to: value.rule.to,
                enable: value.rule.enabled,
            },
        },
    }
});
operation_impl!(
    UpdateMappingRuleInput,
    |value: UpdateMappingRuleInput| match value.table {
        ProxyRuleTable::Remote => MappingMutation::UpdateRemoteRule {
            preset: value.preset,
            index: value.index,
            from: value.from,
            to: value.to,
        },
        ProxyRuleTable::Local => MappingMutation::UpdateLocalRule {
            preset: value.preset,
            index: value.index,
            from: value.from,
            to: value.to,
        },
    }
);
operation_impl!(DeleteMappingRuleInput, |value: DeleteMappingRuleInput| {
    MappingMutation::DeleteRule {
        preset: value.preset,
        table: value.table,
        index: value.index,
    }
});
operation_impl!(MoveMappingRuleInput, |value: MoveMappingRuleInput| {
    MappingMutation::MoveRule {
        preset: value.preset,
        table: value.table,
        from: value.from,
        to: value.to,
    }
});
operation_impl!(
    SetMappingRuleEnabledInput,
    |value: SetMappingRuleEnabledInput| MappingMutation::SetRuleEnabled {
        preset: value.preset,
        table: value.table,
        index: value.index,
        enabled: value.enabled,
    }
);

pub(crate) trait MappingToolInput {
    fn instance(&self) -> &RequiredInstanceSelector;
    fn into_operation(self) -> ControlOperation;
}

macro_rules! mapping_tool_input {
    ($type:ty) => {
        impl MappingToolInput for $type {
            fn instance(&self) -> &RequiredInstanceSelector {
                &self.instance
            }
            fn into_operation(self) -> ControlOperation {
                <$type>::into_operation(self)
            }
        }
    };
}

mapping_tool_input!(CreatePresetInput);
mapping_tool_input!(RenamePresetInput);
mapping_tool_input!(DeletePresetInput);
mapping_tool_input!(SetActivePresetInput);
mapping_tool_input!(SetMappingGateInput);
mapping_tool_input!(CreateMappingRuleInput);
mapping_tool_input!(UpdateMappingRuleInput);
mapping_tool_input!(DeleteMappingRuleInput);
mapping_tool_input!(MoveMappingRuleInput);
mapping_tool_input!(SetMappingRuleEnabledInput);

#[derive(Clone, Debug, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct GetMappingSettingsResult {
    pub(crate) instance: InstanceSelector,
    pub(crate) settings_revision: SettingsRevision,
    pub(crate) config_mode: ConfigMode,
    pub(crate) persistence: PersistenceMode,
    pub(crate) mapping: MappingSettingsView,
}

#[derive(Clone, Debug, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ValidateMappingSettingsResult {
    pub(crate) instance: InstanceSelector,
    pub(crate) settings_revision: SettingsRevision,
    pub(crate) config_mode: ConfigMode,
    pub(crate) persistence: PersistenceMode,
    pub(crate) validation: MappingValidationResult,
}

#[derive(Clone, Debug, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ExplainMappingResult {
    pub(crate) instance: InstanceSelector,
    pub(crate) settings_revision: SettingsRevision,
    pub(crate) config_mode: ConfigMode,
    pub(crate) persistence: PersistenceMode,
    pub(crate) explanation: MappingExplanation,
}

#[derive(Clone, Debug, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct MappingMutationOutput {
    pub(crate) instance: InstanceSelector,
    pub(crate) settings_revision: SettingsRevision,
    pub(crate) config_mode: ConfigMode,
    pub(crate) persistence: PersistenceMode,
    pub(crate) outcome: SettingsTransactionOutcome,
    pub(crate) affected: MappingObjectRef,
}

fn selector(scope: crate::control_rpc::protocol::InstanceScope) -> InstanceSelector {
    InstanceSelector {
        proxy_endpoint: Some(scope.proxy_endpoint),
        run_id: Some(scope.run_id),
    }
}

pub(crate) fn mapping_mutation_result(
    result: ControlResult,
) -> Result<MappingMutationOutput, ControlError> {
    match result {
        ControlResult::MutateMapping {
            instance,
            settings_revision,
            config_mode,
            persistence,
            outcome,
            affected,
        } => Ok(MappingMutationOutput {
            instance: selector(instance),
            settings_revision,
            config_mode,
            persistence,
            outcome,
            affected,
        }),
        _ => Err(ControlError::internal(
            "private RPC returned an unexpected mapping mutation result",
        )),
    }
}

pub(crate) fn get_mapping_result(
    result: ControlResult,
) -> Result<GetMappingSettingsResult, ControlError> {
    match result {
        ControlResult::GetMappingSettings {
            instance,
            settings_revision,
            config_mode,
            persistence,
            proxy,
        } => Ok(GetMappingSettingsResult {
            instance: selector(instance),
            settings_revision,
            config_mode,
            persistence,
            mapping: proxy.into_mapping(),
        }),
        _ => Err(ControlError::internal(
            "private RPC returned an unexpected mapping settings result",
        )),
    }
}

pub(crate) fn validate_mapping_result(
    result: ControlResult,
) -> Result<ValidateMappingSettingsResult, ControlError> {
    match result {
        ControlResult::ValidateMappingSettings {
            instance,
            settings_revision,
            config_mode,
            persistence,
            validation,
        } => Ok(ValidateMappingSettingsResult {
            instance: selector(instance),
            settings_revision,
            config_mode,
            persistence,
            validation: *validation,
        }),
        _ => Err(ControlError::internal(
            "private RPC returned an unexpected mapping validation result",
        )),
    }
}

pub(crate) fn explain_mapping_result(
    result: ControlResult,
) -> Result<ExplainMappingResult, ControlError> {
    match result {
        ControlResult::ExplainMapping {
            instance,
            settings_revision,
            config_mode,
            persistence,
            explanation,
        } => Ok(ExplainMappingResult {
            instance: selector(instance),
            settings_revision,
            config_mode,
            persistence,
            explanation: *explanation,
        }),
        _ => Err(ControlError::internal(
            "private RPC returned an unexpected mapping explanation result",
        )),
    }
}

fn deserialize_required_option<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::deserialize(deserializer)
}

#[cfg(test)]
mod tests;
