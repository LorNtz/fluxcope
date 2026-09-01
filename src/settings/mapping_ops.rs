use crate::{
    mapping::{
        DiagnosticSeverity, MappingDiagnostic, MappingDiagnosticCode, MappingEngine, MappingField,
        MappingRuleLocation, MappingTable, validate_proxy_settings_capped, validate_rule_values,
    },
    settings::{
        AppSettings, ProxyMapLocalRule, ProxyMapRemoteRule, ProxyPresetSettings, ProxySettings,
    },
};
use http::Uri;
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _};
use std::{error::Error, fmt, path::PathBuf};

pub(crate) const MAX_MAPPING_DIAGNOSTICS: usize = 256;

#[derive(Clone, Copy, Debug, Deserialize, JsonSchema, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ProxyRuleTable {
    Remote,
    Local,
}

impl From<ProxyRuleTable> for MappingTable {
    fn from(table: ProxyRuleTable) -> Self {
        match table {
            ProxyRuleTable::Remote => Self::Remote,
            ProxyRuleTable::Local => Self::Local,
        }
    }
}

impl From<MappingTable> for ProxyRuleTable {
    fn from(table: MappingTable) -> Self {
        match table {
            MappingTable::Remote => Self::Remote,
            MappingTable::Local => Self::Local,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, JsonSchema, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum MappingRuleField {
    From,
    To,
}

impl From<MappingField> for MappingRuleField {
    fn from(field: MappingField) -> Self {
        match field {
            MappingField::From => Self::From,
            MappingField::To => Self::To,
        }
    }
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum MappingMutation {
    #[allow(dead_code, reason = "consumed by Task 14 MCP preset creation")]
    CreatePreset {
        name: String,
        initial: Option<ProxyPresetSettings>,
    },
    RenamePreset {
        name: String,
        new_name: String,
    },
    #[allow(dead_code, reason = "consumed by Task 14 MCP preset deletion")]
    DeletePreset {
        name: String,
    },
    SetActivePreset {
        name: Option<String>,
    },
    SetGlobalEnabled {
        enabled: bool,
    },
    SetTableEnabled {
        preset: String,
        table: ProxyRuleTable,
        enabled: bool,
    },
    #[allow(dead_code, reason = "consumed by Task 14 MCP remote-rule creation")]
    AppendRemoteRule {
        preset: String,
        rule: ProxyMapRemoteRule,
    },
    InsertRemoteRule {
        preset: String,
        index: usize,
        rule: ProxyMapRemoteRule,
    },
    UpdateRemoteRule {
        preset: String,
        index: usize,
        from: String,
        to: String,
    },
    #[allow(dead_code, reason = "consumed by Task 14 MCP local-rule creation")]
    AppendLocalRule {
        preset: String,
        rule: ProxyMapLocalRule,
    },
    InsertLocalRule {
        preset: String,
        index: usize,
        rule: ProxyMapLocalRule,
    },
    UpdateLocalRule {
        preset: String,
        index: usize,
        from: String,
        to: String,
    },
    DeleteRule {
        preset: String,
        table: ProxyRuleTable,
        index: usize,
    },
    MoveRule {
        preset: String,
        table: ProxyRuleTable,
        from: usize,
        to: usize,
    },
    SetRuleEnabled {
        preset: String,
        table: ProxyRuleTable,
        index: usize,
        enabled: bool,
    },
}

impl MappingMutation {
    pub(crate) fn audit_target_bounded(
        &self,
        maximum_name_bytes: usize,
    ) -> (MappingObjectRef, bool) {
        match self {
            Self::CreatePreset { name, .. }
            | Self::RenamePreset { name, .. }
            | Self::DeletePreset { name } => {
                let (name, truncated) = bounded_mapping_name(name, maximum_name_bytes);
                (MappingObjectRef::Preset { name }, truncated)
            }
            Self::SetActivePreset { .. } => (MappingObjectRef::ActivePreset, false),
            Self::SetGlobalEnabled { .. } => (MappingObjectRef::Proxy, false),
            Self::SetTableEnabled { preset, table, .. } => {
                let (preset, truncated) = bounded_mapping_name(preset, maximum_name_bytes);
                (
                    MappingObjectRef::Table {
                        preset,
                        table: *table,
                    },
                    truncated,
                )
            }
            Self::AppendRemoteRule { preset, .. } => {
                let (preset, truncated) = bounded_mapping_name(preset, maximum_name_bytes);
                (
                    MappingObjectRef::Table {
                        preset,
                        table: ProxyRuleTable::Remote,
                    },
                    truncated,
                )
            }
            Self::AppendLocalRule { preset, .. } => {
                let (preset, truncated) = bounded_mapping_name(preset, maximum_name_bytes);
                (
                    MappingObjectRef::Table {
                        preset,
                        table: ProxyRuleTable::Local,
                    },
                    truncated,
                )
            }
            Self::InsertRemoteRule { preset, index, .. }
            | Self::UpdateRemoteRule { preset, index, .. } => {
                let (preset, truncated) = bounded_mapping_name(preset, maximum_name_bytes);
                (
                    MappingObjectRef::Rule {
                        preset,
                        table: ProxyRuleTable::Remote,
                        index: *index,
                    },
                    truncated,
                )
            }
            Self::InsertLocalRule { preset, index, .. }
            | Self::UpdateLocalRule { preset, index, .. } => {
                let (preset, truncated) = bounded_mapping_name(preset, maximum_name_bytes);
                (
                    MappingObjectRef::Rule {
                        preset,
                        table: ProxyRuleTable::Local,
                        index: *index,
                    },
                    truncated,
                )
            }
            Self::DeleteRule {
                preset,
                table,
                index,
            }
            | Self::SetRuleEnabled {
                preset,
                table,
                index,
                ..
            } => {
                let (preset, truncated) = bounded_mapping_name(preset, maximum_name_bytes);
                (
                    MappingObjectRef::Rule {
                        preset,
                        table: *table,
                        index: *index,
                    },
                    truncated,
                )
            }
            Self::MoveRule {
                preset,
                table,
                from,
                ..
            } => {
                let (preset, truncated) = bounded_mapping_name(preset, maximum_name_bytes);
                (
                    MappingObjectRef::Rule {
                        preset,
                        table: *table,
                        index: *from,
                    },
                    truncated,
                )
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, JsonSchema, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum MutationEffect {
    Changed,
    Unchanged,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum MappingObjectRef {
    Proxy,
    ActivePreset,
    Preset {
        name: String,
    },
    PresetName {
        name: String,
    },
    Table {
        preset: String,
        table: ProxyRuleTable,
    },
    Rule {
        preset: String,
        table: ProxyRuleTable,
        index: usize,
    },
    RuleField {
        preset: String,
        table: ProxyRuleTable,
        index: usize,
        field: MappingRuleField,
    },
}

impl MappingObjectRef {
    pub(crate) fn bounded_clone(&self, maximum_name_bytes: usize) -> (Self, bool) {
        match self {
            Self::Proxy => (Self::Proxy, false),
            Self::ActivePreset => (Self::ActivePreset, false),
            Self::Preset { name } => {
                let (name, truncated) = bounded_mapping_name(name, maximum_name_bytes);
                (Self::Preset { name }, truncated)
            }
            Self::PresetName { name } => {
                let (name, truncated) = bounded_mapping_name(name, maximum_name_bytes);
                (Self::PresetName { name }, truncated)
            }
            Self::Table { preset, table } => {
                let (preset, truncated) = bounded_mapping_name(preset, maximum_name_bytes);
                (
                    Self::Table {
                        preset,
                        table: *table,
                    },
                    truncated,
                )
            }
            Self::Rule {
                preset,
                table,
                index,
            } => {
                let (preset, truncated) = bounded_mapping_name(preset, maximum_name_bytes);
                (
                    Self::Rule {
                        preset,
                        table: *table,
                        index: *index,
                    },
                    truncated,
                )
            }
            Self::RuleField {
                preset,
                table,
                index,
                field,
            } => {
                let (preset, truncated) = bounded_mapping_name(preset, maximum_name_bytes);
                (
                    Self::RuleField {
                        preset,
                        table: *table,
                        index: *index,
                        field: *field,
                    },
                    truncated,
                )
            }
        }
    }
}

fn bounded_mapping_name(value: &str, maximum_bytes: usize) -> (String, bool) {
    if value.len() <= maximum_bytes {
        return (value.to_owned(), false);
    }
    let mut end = maximum_bytes.min(value.len());
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    (value[..end].to_owned(), true)
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Eq, Serialize)]
pub(crate) struct MappingMutationResult {
    pub affected: MappingObjectRef,
    pub effect: MutationEffect,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MappingMutationErrorKind {
    ProxyNotFound,
    PresetNotFound,
    DuplicatePresetName,
    RuleNotFound,
    InvalidField,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MappingMutationError {
    pub kind: MappingMutationErrorKind,
    pub location: MappingObjectRef,
    pub message: String,
}

impl fmt::Display for MappingMutationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for MappingMutationError {}

#[derive(Clone, Debug, Default, Deserialize, JsonSchema, PartialEq, Eq, Serialize)]
pub(crate) struct MappingGateState {
    pub proxy_present: bool,
    pub global_enabled: bool,
    pub active_preset: Option<MappingPresetRef>,
    pub remote_enabled: Option<bool>,
    pub local_enabled: Option<bool>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Eq, Serialize)]
pub(crate) struct MappingPresetRef {
    pub name: String,
    pub index: usize,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Eq, Serialize)]
pub(crate) struct MappingValidationResult {
    pub gates: MappingGateState,
    pub diagnostics: Vec<MappingDiagnostic>,
    pub diagnostics_total: usize,
    pub diagnostics_omitted: usize,
}

#[cfg(test)]
impl MappingValidationResult {
    pub(crate) fn is_valid(&self) -> bool {
        !self
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == DiagnosticSeverity::Error)
    }
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Eq, Serialize)]
#[allow(dead_code, reason = "consumed by Task 14 mapping explanation RPC")]
pub(crate) struct MappingRuleMatch {
    pub preset: String,
    pub preset_index: usize,
    pub table: ProxyRuleTable,
    pub index: usize,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Eq, Serialize)]
#[allow(dead_code, reason = "consumed by Task 14 mapping explanation RPC")]
pub(crate) struct MappingExplanation {
    #[schemars(with = "Option<String>")]
    #[serde(
        serialize_with = "serialize_optional_uri",
        deserialize_with = "deserialize_optional_uri"
    )]
    pub original_url: Option<Uri>,
    #[schemars(with = "Option<String>")]
    #[serde(
        serialize_with = "serialize_optional_uri",
        deserialize_with = "deserialize_optional_uri"
    )]
    pub effective_url: Option<Uri>,
    pub local_path: Option<PathBuf>,
    pub matches: Vec<MappingRuleMatch>,
    pub gates: MappingGateState,
    pub diagnostics: Vec<MappingDiagnostic>,
    pub diagnostics_total: usize,
    pub diagnostics_omitted: usize,
}

fn serialize_optional_uri<S>(value: &Option<Uri>, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    value
        .as_ref()
        .map(ToString::to_string)
        .serialize(serializer)
}

fn deserialize_optional_uri<'de, D>(deserializer: D) -> Result<Option<Uri>, D::Error>
where
    D: Deserializer<'de>,
{
    Option::<String>::deserialize(deserializer)?
        .map(|value| value.parse().map_err(D::Error::custom))
        .transpose()
}

pub(crate) fn apply_mapping_mutation(
    settings: &mut AppSettings,
    mutation: MappingMutation,
) -> Result<MappingMutationResult, MappingMutationError> {
    let mut candidate = settings.clone();
    let result = apply_to_candidate(&mut candidate, mutation)?;
    if result.effect == MutationEffect::Changed {
        *settings = candidate;
    }
    Ok(result)
}

pub(crate) fn apply_mapping_mutation_owned(
    mut settings: AppSettings,
    mutation: MappingMutation,
) -> Result<(AppSettings, MappingMutationResult), MappingMutationError> {
    let result = apply_to_candidate(&mut settings, mutation)?;
    Ok((settings, result))
}

fn mutation_result(affected: MappingObjectRef, changed: bool) -> MappingMutationResult {
    MappingMutationResult {
        affected,
        effect: if changed {
            MutationEffect::Changed
        } else {
            MutationEffect::Unchanged
        },
    }
}

fn apply_to_candidate(
    settings: &mut AppSettings,
    mutation: MappingMutation,
) -> Result<MappingMutationResult, MappingMutationError> {
    match mutation {
        MappingMutation::CreatePreset { name, initial } => {
            let name = normalized_name(&name)?;
            if settings
                .proxy
                .as_ref()
                .is_some_and(|proxy| find_mapping_preset_index(proxy, &name).is_some())
            {
                return Err(duplicate_name_error(name));
            }
            let mut preset = initial.unwrap_or_default();
            preset.name.clone_from(&name);
            validate_initial_preset(&preset)?;
            settings
                .proxy
                .get_or_insert_with(ProxySettings::default)
                .presets
                .push(preset);
            Ok(mutation_result(MappingObjectRef::Preset { name }, true))
        }
        MappingMutation::RenamePreset { name, new_name } => {
            let name = normalized_name(&name)?;
            let new_name = normalized_name(&new_name)?;
            let proxy = require_proxy(settings, MappingObjectRef::Preset { name: name.clone() })?;
            let index = require_preset_index(proxy, &name)?;
            if name != new_name
                && proxy.presets.iter().enumerate().any(|(candidate, preset)| {
                    candidate != index && preset.name.trim() == new_name.as_str()
                })
            {
                return Err(duplicate_name_error(new_name));
            }
            let active_matches =
                proxy.active_preset.as_deref().map(str::trim) == Some(name.as_str());
            let changed = proxy.presets[index].name != new_name
                || (active_matches && proxy.active_preset.as_deref() != Some(new_name.as_str()));
            proxy.presets[index].name.clone_from(&new_name);
            if active_matches {
                proxy.active_preset = Some(new_name.clone());
            }
            Ok(mutation_result(
                MappingObjectRef::PresetName { name: new_name },
                changed,
            ))
        }
        MappingMutation::DeletePreset { name } => {
            let name = normalized_name(&name)?;
            let proxy = require_proxy(settings, MappingObjectRef::Preset { name: name.clone() })?;
            let index = require_preset_index(proxy, &name)?;
            proxy.presets.remove(index);
            if proxy.active_preset.as_deref().map(str::trim) == Some(name.as_str()) {
                proxy.active_preset = None;
            }
            Ok(mutation_result(MappingObjectRef::Preset { name }, true))
        }
        MappingMutation::SetActivePreset { name } => {
            let location = MappingObjectRef::ActivePreset;
            let proxy = require_proxy(settings, location.clone())?;
            let next = match name {
                Some(name) => {
                    let name = normalized_name(&name)?;
                    require_preset_index(proxy, &name)?;
                    Some(name)
                }
                None => None,
            };
            let changed = proxy.active_preset != next;
            proxy.active_preset = next;
            Ok(mutation_result(location, changed))
        }
        MappingMutation::SetGlobalEnabled { enabled } => {
            let location = MappingObjectRef::Proxy;
            let proxy = require_proxy(settings, location.clone())?;
            let changed = proxy.enable != enabled;
            proxy.enable = enabled;
            Ok(mutation_result(location, changed))
        }
        MappingMutation::SetTableEnabled {
            preset,
            table,
            enabled,
        } => {
            let preset = normalized_name(&preset)?;
            let proxy = require_proxy(
                settings,
                MappingObjectRef::Table {
                    preset: preset.clone(),
                    table,
                },
            )?;
            let index = require_preset_index(proxy, &preset)?;
            let changed = match table {
                ProxyRuleTable::Remote => {
                    let changed = proxy.presets[index].map_remote.enable != enabled;
                    proxy.presets[index].map_remote.enable = enabled;
                    changed
                }
                ProxyRuleTable::Local => {
                    let changed = proxy.presets[index].map_local.enable != enabled;
                    proxy.presets[index].map_local.enable = enabled;
                    changed
                }
            };
            Ok(mutation_result(
                MappingObjectRef::Table { preset, table },
                changed,
            ))
        }
        MappingMutation::AppendRemoteRule { preset, rule } => {
            insert_remote_rule(settings, preset, None, rule)
                .map(|affected| mutation_result(affected, true))
        }
        MappingMutation::InsertRemoteRule {
            preset,
            index,
            rule,
        } => insert_remote_rule(settings, preset, Some(index), rule)
            .map(|affected| mutation_result(affected, true)),
        MappingMutation::AppendLocalRule { preset, rule } => {
            insert_local_rule(settings, preset, None, rule)
                .map(|affected| mutation_result(affected, true))
        }
        MappingMutation::InsertLocalRule {
            preset,
            index,
            rule,
        } => insert_local_rule(settings, preset, Some(index), rule)
            .map(|affected| mutation_result(affected, true)),
        MappingMutation::UpdateRemoteRule {
            preset,
            index,
            from,
            to,
        } => update_remote_rule(settings, preset, index, from, to),
        MappingMutation::UpdateLocalRule {
            preset,
            index,
            from,
            to,
        } => update_local_rule(settings, preset, index, from, to),
        MappingMutation::DeleteRule {
            preset,
            table,
            index,
        } => delete_rule(settings, preset, table, index)
            .map(|affected| mutation_result(affected, true)),
        MappingMutation::MoveRule {
            preset,
            table,
            from,
            to,
        } => move_rule(settings, preset, table, from, to),
        MappingMutation::SetRuleEnabled {
            preset,
            table,
            index,
            enabled,
        } => set_rule_enabled(settings, preset, table, index, enabled),
    }
}

fn insert_remote_rule(
    settings: &mut AppSettings,
    preset: String,
    index: Option<usize>,
    rule: ProxyMapRemoteRule,
) -> Result<MappingObjectRef, MappingMutationError> {
    let preset = normalized_name(&preset)?;
    let proxy = require_proxy(settings, table_ref(&preset, ProxyRuleTable::Remote))?;
    let preset_index = require_preset_index(proxy, &preset)?;
    let index = index.unwrap_or(proxy.presets[preset_index].map_remote.rules.len());
    validate_rule_fields(&preset, ProxyRuleTable::Remote, index, &rule.from, &rule.to)?;
    if index > proxy.presets[preset_index].map_remote.rules.len() {
        return Err(rule_not_found(&preset, ProxyRuleTable::Remote, index));
    }
    proxy.presets[preset_index]
        .map_remote
        .rules
        .insert(index, rule);
    Ok(rule_ref(&preset, ProxyRuleTable::Remote, index))
}

fn insert_local_rule(
    settings: &mut AppSettings,
    preset: String,
    index: Option<usize>,
    rule: ProxyMapLocalRule,
) -> Result<MappingObjectRef, MappingMutationError> {
    let preset = normalized_name(&preset)?;
    let proxy = require_proxy(settings, table_ref(&preset, ProxyRuleTable::Local))?;
    let preset_index = require_preset_index(proxy, &preset)?;
    let index = index.unwrap_or(proxy.presets[preset_index].map_local.rules.len());
    validate_rule_fields(&preset, ProxyRuleTable::Local, index, &rule.from, &rule.to)?;
    if index > proxy.presets[preset_index].map_local.rules.len() {
        return Err(rule_not_found(&preset, ProxyRuleTable::Local, index));
    }
    proxy.presets[preset_index]
        .map_local
        .rules
        .insert(index, rule);
    Ok(rule_ref(&preset, ProxyRuleTable::Local, index))
}

fn update_remote_rule(
    settings: &mut AppSettings,
    preset: String,
    index: usize,
    from: String,
    to: String,
) -> Result<MappingMutationResult, MappingMutationError> {
    let preset = normalized_name(&preset)?;
    validate_rule_fields(&preset, ProxyRuleTable::Remote, index, &from, &to)?;
    let proxy = require_proxy(settings, rule_ref(&preset, ProxyRuleTable::Remote, index))?;
    let preset_index = require_preset_index(proxy, &preset)?;
    let rule = proxy.presets[preset_index]
        .map_remote
        .rules
        .get_mut(index)
        .ok_or_else(|| rule_not_found(&preset, ProxyRuleTable::Remote, index))?;
    let changed = rule.from != from || rule.to != to;
    rule.from = from;
    rule.to = to;
    Ok(mutation_result(
        rule_ref(&preset, ProxyRuleTable::Remote, index),
        changed,
    ))
}

fn update_local_rule(
    settings: &mut AppSettings,
    preset: String,
    index: usize,
    from: String,
    to: String,
) -> Result<MappingMutationResult, MappingMutationError> {
    let preset = normalized_name(&preset)?;
    validate_rule_fields(&preset, ProxyRuleTable::Local, index, &from, &to)?;
    let proxy = require_proxy(settings, rule_ref(&preset, ProxyRuleTable::Local, index))?;
    let preset_index = require_preset_index(proxy, &preset)?;
    let rule = proxy.presets[preset_index]
        .map_local
        .rules
        .get_mut(index)
        .ok_or_else(|| rule_not_found(&preset, ProxyRuleTable::Local, index))?;
    let changed = rule.from != from || rule.to != to;
    rule.from = from;
    rule.to = to;
    Ok(mutation_result(
        rule_ref(&preset, ProxyRuleTable::Local, index),
        changed,
    ))
}

fn delete_rule(
    settings: &mut AppSettings,
    preset: String,
    table: ProxyRuleTable,
    index: usize,
) -> Result<MappingObjectRef, MappingMutationError> {
    let preset = normalized_name(&preset)?;
    let proxy = require_proxy(settings, rule_ref(&preset, table, index))?;
    let preset_index = require_preset_index(proxy, &preset)?;
    match table {
        ProxyRuleTable::Remote => {
            if index >= proxy.presets[preset_index].map_remote.rules.len() {
                return Err(rule_not_found(&preset, table, index));
            }
            proxy.presets[preset_index].map_remote.rules.remove(index);
        }
        ProxyRuleTable::Local => {
            if index >= proxy.presets[preset_index].map_local.rules.len() {
                return Err(rule_not_found(&preset, table, index));
            }
            proxy.presets[preset_index].map_local.rules.remove(index);
        }
    }
    Ok(rule_ref(&preset, table, index))
}

fn move_rule(
    settings: &mut AppSettings,
    preset: String,
    table: ProxyRuleTable,
    from: usize,
    to: usize,
) -> Result<MappingMutationResult, MappingMutationError> {
    let preset = normalized_name(&preset)?;
    let proxy = require_proxy(settings, rule_ref(&preset, table, from))?;
    let preset_index = require_preset_index(proxy, &preset)?;
    match table {
        ProxyRuleTable::Remote => move_item(
            &mut proxy.presets[preset_index].map_remote.rules,
            &preset,
            table,
            from,
            to,
        )?,
        ProxyRuleTable::Local => move_item(
            &mut proxy.presets[preset_index].map_local.rules,
            &preset,
            table,
            from,
            to,
        )?,
    }
    Ok(mutation_result(rule_ref(&preset, table, to), from != to))
}

fn move_item<T>(
    rules: &mut Vec<T>,
    preset: &str,
    table: ProxyRuleTable,
    from: usize,
    to: usize,
) -> Result<(), MappingMutationError> {
    if from >= rules.len() {
        return Err(rule_not_found(preset, table, from));
    }
    if to >= rules.len() {
        return Err(rule_not_found(preset, table, to));
    }
    if from != to {
        let rule = rules.remove(from);
        rules.insert(to, rule);
    }
    Ok(())
}

fn set_rule_enabled(
    settings: &mut AppSettings,
    preset: String,
    table: ProxyRuleTable,
    index: usize,
    enabled: bool,
) -> Result<MappingMutationResult, MappingMutationError> {
    let preset = normalized_name(&preset)?;
    let proxy = require_proxy(settings, rule_ref(&preset, table, index))?;
    let preset_index = require_preset_index(proxy, &preset)?;
    let changed = match table {
        ProxyRuleTable::Remote => {
            let rule = proxy.presets[preset_index]
                .map_remote
                .rules
                .get_mut(index)
                .ok_or_else(|| rule_not_found(&preset, table, index))?;
            let changed = rule.enable != enabled;
            rule.enable = enabled;
            changed
        }
        ProxyRuleTable::Local => {
            let rule = proxy.presets[preset_index]
                .map_local
                .rules
                .get_mut(index)
                .ok_or_else(|| rule_not_found(&preset, table, index))?;
            let changed = rule.enable != enabled;
            rule.enable = enabled;
            changed
        }
    };
    Ok(mutation_result(rule_ref(&preset, table, index), changed))
}

fn normalized_name(name: &str) -> Result<String, MappingMutationError> {
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err(MappingMutationError {
            kind: MappingMutationErrorKind::InvalidField,
            location: MappingObjectRef::PresetName { name },
            message: "preset name cannot be empty".to_string(),
        });
    }
    Ok(name)
}

fn require_proxy(
    settings: &mut AppSettings,
    location: MappingObjectRef,
) -> Result<&mut ProxySettings, MappingMutationError> {
    settings.proxy.as_mut().ok_or_else(|| MappingMutationError {
        kind: MappingMutationErrorKind::ProxyNotFound,
        location,
        message: "proxy settings do not exist".to_string(),
    })
}

pub(crate) fn find_mapping_preset_index(proxy: &ProxySettings, name: &str) -> Option<usize> {
    let name = name.trim();
    proxy
        .presets
        .iter()
        .position(|preset| preset.name.trim() == name)
}

fn require_preset_index(proxy: &ProxySettings, name: &str) -> Result<usize, MappingMutationError> {
    find_mapping_preset_index(proxy, name).ok_or_else(|| MappingMutationError {
        kind: MappingMutationErrorKind::PresetNotFound,
        location: MappingObjectRef::Preset {
            name: name.to_string(),
        },
        message: format!("proxy preset '{name}' was not found"),
    })
}

fn duplicate_name_error(name: String) -> MappingMutationError {
    MappingMutationError {
        kind: MappingMutationErrorKind::DuplicatePresetName,
        location: MappingObjectRef::PresetName { name },
        message: "preset name already exists".to_string(),
    }
}

fn table_ref(preset: &str, table: ProxyRuleTable) -> MappingObjectRef {
    MappingObjectRef::Table {
        preset: preset.to_string(),
        table,
    }
}

fn rule_ref(preset: &str, table: ProxyRuleTable, index: usize) -> MappingObjectRef {
    MappingObjectRef::Rule {
        preset: preset.to_string(),
        table,
        index,
    }
}

fn rule_not_found(preset: &str, table: ProxyRuleTable, index: usize) -> MappingMutationError {
    MappingMutationError {
        kind: MappingMutationErrorKind::RuleNotFound,
        location: rule_ref(preset, table, index),
        message: format!("mapping rule {index} was not found"),
    }
}

fn validate_rule_fields(
    preset: &str,
    table: ProxyRuleTable,
    index: usize,
    from: &str,
    to: &str,
) -> Result<(), MappingMutationError> {
    if let Some((field, message)) = validate_rule_values(table.into(), from, to)
        .into_iter()
        .next()
    {
        return Err(MappingMutationError {
            kind: MappingMutationErrorKind::InvalidField,
            location: MappingObjectRef::RuleField {
                preset: preset.to_string(),
                table,
                index,
                field: field.into(),
            },
            message,
        });
    }
    Ok(())
}

fn validate_initial_preset(preset: &ProxyPresetSettings) -> Result<(), MappingMutationError> {
    let proxy = ProxySettings {
        active_preset: Some(preset.name.clone()),
        presets: vec![preset.clone()],
        ..ProxySettings::default()
    };
    if let Some(diagnostic) = validate_proxy_settings_capped(&proxy, 1)
        .0
        .into_iter()
        .next()
    {
        let location = diagnostic.location.map_or_else(
            || MappingObjectRef::PresetName {
                name: preset.name.clone(),
            },
            |location| MappingObjectRef::RuleField {
                preset: preset.name.clone(),
                table: location.table.into(),
                index: location.rule_index,
                field: location.field.into(),
            },
        );
        return Err(MappingMutationError {
            kind: MappingMutationErrorKind::InvalidField,
            location,
            message: diagnostic.message,
        });
    }
    Ok(())
}

pub(crate) fn validate_mapping_candidate(proxy: Option<&ProxySettings>) -> MappingValidationResult {
    let (diagnostics, diagnostics_total) = proxy.map_or_else(
        || (Vec::new(), 0),
        |proxy| validate_proxy_settings_capped(proxy, MAX_MAPPING_DIAGNOSTICS),
    );
    MappingValidationResult {
        gates: mapping_gates(proxy),
        diagnostics,
        diagnostics_total,
        diagnostics_omitted: diagnostics_total.saturating_sub(MAX_MAPPING_DIAGNOSTICS),
    }
}

#[allow(dead_code, reason = "consumed by Task 14 mapping explanation RPC")]
pub(crate) fn explain_mapping_candidate(
    proxy: Option<&ProxySettings>,
    url: &str,
) -> MappingExplanation {
    let validation = validate_mapping_candidate(proxy);
    let mut diagnostics = validation.diagnostics;
    let mut diagnostics_total = validation.diagnostics_total;
    let parsed = url.parse::<Uri>().ok().filter(|uri| {
        matches!(uri.scheme_str(), Some("http" | "https")) && uri.authority().is_some()
    });
    let Some(uri) = parsed else {
        diagnostics_total = diagnostics_total.saturating_add(1);
        if diagnostics.len() < MAX_MAPPING_DIAGNOSTICS {
            diagnostics.push(MappingDiagnostic {
                severity: DiagnosticSeverity::Error,
                code: MappingDiagnosticCode::InvalidRequestUrl,
                location: None,
                message: "request URL must be an absolute http or https URL".to_string(),
            });
        }
        return MappingExplanation {
            original_url: None,
            effective_url: None,
            local_path: None,
            matches: Vec::new(),
            gates: validation.gates,
            diagnostics,
            diagnostics_total,
            diagnostics_omitted: diagnostics_total.saturating_sub(MAX_MAPPING_DIAGNOSTICS),
        };
    };

    let trace = MappingEngine::compile(proxy).trace_request(&uri);
    let matches = proxy
        .map(|proxy| {
            [trace.remote_match, trace.local_match]
                .into_iter()
                .flatten()
                .filter_map(|location| rule_match(proxy, location))
                .collect()
        })
        .unwrap_or_default();
    MappingExplanation {
        original_url: Some(uri),
        effective_url: trace.effective_uri,
        local_path: trace.decision.local_path,
        matches,
        gates: validation.gates,
        diagnostics,
        diagnostics_total,
        diagnostics_omitted: diagnostics_total.saturating_sub(MAX_MAPPING_DIAGNOSTICS),
    }
}

#[allow(dead_code, reason = "consumed by Task 14 mapping explanation RPC")]
fn mapping_gates(proxy: Option<&ProxySettings>) -> MappingGateState {
    let Some(proxy) = proxy else {
        return MappingGateState::default();
    };
    let active = proxy
        .active_preset
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .and_then(|name| {
            find_mapping_preset_index(proxy, name).map(|index| MappingPresetRef {
                name: proxy.presets[index].name.clone(),
                index,
            })
        });
    let (remote_enabled, local_enabled) = active.as_ref().map_or((None, None), |active| {
        let preset = &proxy.presets[active.index];
        (
            Some(preset.map_remote.enable),
            Some(preset.map_local.enable),
        )
    });
    MappingGateState {
        proxy_present: true,
        global_enabled: proxy.enable,
        active_preset: active,
        remote_enabled,
        local_enabled,
    }
}

#[allow(dead_code, reason = "consumed by Task 14 mapping explanation RPC")]
fn rule_match(proxy: &ProxySettings, location: MappingRuleLocation) -> Option<MappingRuleMatch> {
    let preset = proxy.presets.get(location.preset_index)?;
    Some(MappingRuleMatch {
        preset: preset.name.clone(),
        preset_index: location.preset_index,
        table: location.table.into(),
        index: location.rule_index,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        mapping::{DiagnosticSeverity, MappingDiagnosticCode, MappingField, MappingTable},
        settings::{
            AppSettings, ProxyMapLocalRule, ProxyMapLocalSettings, ProxyMapRemoteRule,
            ProxyMapRemoteSettings, ProxyPresetSettings, ProxySettings,
        },
    };
    use std::path::PathBuf;

    fn remote(from: &str, to: &str) -> ProxyMapRemoteRule {
        ProxyMapRemoteRule {
            from: from.into(),
            to: to.into(),
            enable: true,
        }
    }

    fn local(from: &str, to: &str) -> ProxyMapLocalRule {
        ProxyMapLocalRule {
            from: from.into(),
            to: to.into(),
            enable: true,
        }
    }

    fn preset(name: &str) -> ProxyPresetSettings {
        ProxyPresetSettings {
            name: name.into(),
            ..ProxyPresetSettings::default()
        }
    }

    fn active_settings() -> AppSettings {
        AppSettings {
            proxy: Some(ProxySettings {
                active_preset: Some("dev".into()),
                presets: vec![preset("dev"), preset("qa")],
                ..ProxySettings::default()
            }),
            ..AppSettings::default()
        }
    }

    fn settings_with_rules() -> AppSettings {
        let mut settings = active_settings();
        let preset = &mut settings.proxy.as_mut().unwrap().presets[0];
        preset.map_remote.rules = vec![
            remote("https://a.example", "http://localhost:3001"),
            remote("https://b.example", "http://localhost:3002"),
            remote("https://c.example", "http://localhost:3003"),
        ];
        preset.map_local.rules = vec![
            local("https://a.example", "/tmp/a.json"),
            local("https://b.example", "/tmp/b.json"),
        ];
        settings
    }

    fn apply(settings: &mut AppSettings, mutation: MappingMutation) -> MappingMutationResult {
        apply_mapping_mutation(settings, mutation).expect("mapping mutation")
    }

    #[test]
    fn creates_default_preset_without_activation() {
        let mut settings = AppSettings::default();
        let result = apply(
            &mut settings,
            MappingMutation::CreatePreset {
                name: "  dev  ".into(),
                initial: None,
            },
        );
        let proxy = settings.proxy.unwrap();
        assert_eq!(proxy.active_preset, None);
        assert_eq!(proxy.presets, vec![preset("dev")]);
        assert_eq!(result.effect, MutationEffect::Changed);
        assert_eq!(
            result.affected,
            MappingObjectRef::Preset { name: "dev".into() }
        );
    }

    #[test]
    fn complete_preset_is_validated_renamed_and_not_activated() {
        let mut settings = AppSettings::default();
        let initial = ProxyPresetSettings {
            name: "payload".into(),
            map_remote: ProxyMapRemoteSettings {
                enable: false,
                rules: vec![remote("https://api.example/v1", "http://localhost:9000")],
            },
            map_local: ProxyMapLocalSettings {
                enable: false,
                rules: vec![local("https://static.example", "/tmp/static.json")],
            },
        };
        apply(
            &mut settings,
            MappingMutation::CreatePreset {
                name: " staging ".into(),
                initial: Some(initial),
            },
        );
        let proxy = settings.proxy.unwrap();
        assert_eq!(proxy.active_preset, None);
        assert_eq!(proxy.presets[0].name, "staging");
        assert!(!proxy.presets[0].map_remote.enable);
        assert!(!proxy.presets[0].map_local.enable);
        assert_eq!(proxy.presets[0].map_remote.rules.len(), 1);
        assert_eq!(proxy.presets[0].map_local.rules.len(), 1);
    }

    #[test]
    fn names_are_trimmed_unique_and_case_sensitive() {
        let mut settings = active_settings();
        apply(
            &mut settings,
            MappingMutation::CreatePreset {
                name: " Dev ".into(),
                initial: None,
            },
        );
        let before = settings.clone();
        let error = apply_mapping_mutation(
            &mut settings,
            MappingMutation::CreatePreset {
                name: "  Dev  ".into(),
                initial: None,
            },
        )
        .unwrap_err();
        assert_eq!(error.kind, MappingMutationErrorKind::DuplicatePresetName);
        assert_eq!(
            error.location,
            MappingObjectRef::PresetName { name: "Dev".into() }
        );
        assert_eq!(settings, before);
    }

    #[test]
    fn invalid_complete_preset_is_atomic_and_located() {
        let mut settings = AppSettings::default();
        let before = settings.clone();
        let initial = ProxyPresetSettings {
            name: "ignored".into(),
            map_remote: ProxyMapRemoteSettings {
                rules: vec![remote("bad", "http://localhost:3000")],
                ..ProxyMapRemoteSettings::default()
            },
            ..ProxyPresetSettings::default()
        };
        let error = apply_mapping_mutation(
            &mut settings,
            MappingMutation::CreatePreset {
                name: "dev".into(),
                initial: Some(initial),
            },
        )
        .unwrap_err();
        assert_eq!(error.kind, MappingMutationErrorKind::InvalidField);
        assert_eq!(
            error.location,
            MappingObjectRef::RuleField {
                preset: "dev".into(),
                table: ProxyRuleTable::Remote,
                index: 0,
                field: MappingRuleField::From,
            }
        );
        assert_eq!(settings, before);
    }

    #[test]
    fn active_rename_delete_null_and_stale_paths_are_explicit() {
        let mut settings = active_settings();
        apply(
            &mut settings,
            MappingMutation::RenamePreset {
                name: "dev".into(),
                new_name: " staging ".into(),
            },
        );
        assert_eq!(
            settings.proxy.as_ref().unwrap().active_preset.as_deref(),
            Some("staging")
        );
        apply(
            &mut settings,
            MappingMutation::SetActivePreset {
                name: Some("qa".into()),
            },
        );
        apply(
            &mut settings,
            MappingMutation::DeletePreset { name: "qa".into() },
        );
        assert_eq!(settings.proxy.as_ref().unwrap().active_preset, None);
        settings.proxy.as_mut().unwrap().active_preset = Some("stale".into());
        let before = settings.clone();
        let error = apply_mapping_mutation(
            &mut settings,
            MappingMutation::SetActivePreset {
                name: Some("missing".into()),
            },
        )
        .unwrap_err();
        assert_eq!(error.kind, MappingMutationErrorKind::PresetNotFound);
        assert_eq!(settings, before);
        apply(
            &mut settings,
            MappingMutation::SetActivePreset { name: None },
        );
        assert_eq!(settings.proxy.unwrap().active_preset, None);
    }

    #[test]
    fn global_remote_and_local_gates_are_mutated() {
        let mut settings = active_settings();
        apply(
            &mut settings,
            MappingMutation::SetGlobalEnabled { enabled: false },
        );
        for table in [ProxyRuleTable::Remote, ProxyRuleTable::Local] {
            apply(
                &mut settings,
                MappingMutation::SetTableEnabled {
                    preset: "dev".into(),
                    table,
                    enabled: false,
                },
            );
        }
        let preset = &settings.proxy.as_ref().unwrap().presets[0];
        assert!(!settings.proxy.as_ref().unwrap().enable);
        assert!(!preset.map_remote.enable);
        assert!(!preset.map_local.enable);
    }

    #[test]
    fn append_insert_keep_remote_and_local_types_separate() {
        let mut settings = active_settings();
        apply(
            &mut settings,
            MappingMutation::AppendRemoteRule {
                preset: "dev".into(),
                rule: remote("https://b.example", "http://localhost:2"),
            },
        );
        apply(
            &mut settings,
            MappingMutation::InsertRemoteRule {
                preset: "dev".into(),
                index: 0,
                rule: remote("https://a.example", "http://localhost:1"),
            },
        );
        apply(
            &mut settings,
            MappingMutation::AppendLocalRule {
                preset: "dev".into(),
                rule: local("https://b.example", "/tmp/b"),
            },
        );
        apply(
            &mut settings,
            MappingMutation::InsertLocalRule {
                preset: "dev".into(),
                index: 0,
                rule: local("https://a.example", "/tmp/a"),
            },
        );
        let preset = &settings.proxy.unwrap().presets[0];
        assert_eq!(preset.map_remote.rules.len(), 2);
        assert_eq!(preset.map_local.rules.len(), 2);
        assert_eq!(preset.map_remote.rules[0].to, "http://localhost:1");
        assert_eq!(preset.map_local.rules[0].to, "/tmp/a");
    }

    #[test]
    fn update_delete_enable_and_move_cover_both_rule_tables() {
        let mut settings = settings_with_rules();
        apply(
            &mut settings,
            MappingMutation::UpdateRemoteRule {
                preset: "dev".into(),
                index: 0,
                from: "https://updated.example/v1".into(),
                to: "http://localhost:4000".into(),
            },
        );
        apply(
            &mut settings,
            MappingMutation::UpdateLocalRule {
                preset: "dev".into(),
                index: 0,
                from: "https://updated.example/static".into(),
                to: "/tmp/updated".into(),
            },
        );
        apply(
            &mut settings,
            MappingMutation::SetRuleEnabled {
                preset: "dev".into(),
                table: ProxyRuleTable::Remote,
                index: 0,
                enabled: false,
            },
        );
        apply(
            &mut settings,
            MappingMutation::SetRuleEnabled {
                preset: "dev".into(),
                table: ProxyRuleTable::Local,
                index: 1,
                enabled: false,
            },
        );
        let moved = apply(
            &mut settings,
            MappingMutation::MoveRule {
                preset: "dev".into(),
                table: ProxyRuleTable::Remote,
                from: 0,
                to: 2,
            },
        );
        apply(
            &mut settings,
            MappingMutation::MoveRule {
                preset: "dev".into(),
                table: ProxyRuleTable::Local,
                from: 1,
                to: 0,
            },
        );
        apply(
            &mut settings,
            MappingMutation::DeleteRule {
                preset: "dev".into(),
                table: ProxyRuleTable::Remote,
                index: 1,
            },
        );
        apply(
            &mut settings,
            MappingMutation::DeleteRule {
                preset: "dev".into(),
                table: ProxyRuleTable::Local,
                index: 1,
            },
        );
        let preset = &settings.proxy.unwrap().presets[0];
        assert_eq!(
            preset
                .map_remote
                .rules
                .iter()
                .map(|r| r.from.as_str())
                .collect::<Vec<_>>(),
            vec!["https://b.example", "https://updated.example/v1"]
        );
        assert!(!preset.map_remote.rules[1].enable);
        assert_eq!(preset.map_local.rules[0].to, "/tmp/b.json");
        assert!(!preset.map_local.rules[0].enable);
        assert_eq!(preset.map_local.rules.len(), 1);
        assert_eq!(
            moved.affected,
            MappingObjectRef::Rule {
                preset: "dev".into(),
                table: ProxyRuleTable::Remote,
                index: 2,
            }
        );
    }

    #[test]
    fn same_index_move_is_unchanged() {
        let mut settings = settings_with_rules();
        let before = settings.clone();
        let result = apply(
            &mut settings,
            MappingMutation::MoveRule {
                preset: "dev".into(),
                table: ProxyRuleTable::Remote,
                from: 1,
                to: 1,
            },
        );
        assert_eq!(result.effect, MutationEffect::Unchanged);
        assert_eq!(settings, before);
    }

    #[test]
    fn invalid_proxy_preset_table_index_and_field_are_atomic() {
        let cases = [
            (
                AppSettings::default(),
                MappingMutation::SetGlobalEnabled { enabled: false },
                MappingMutationErrorKind::ProxyNotFound,
            ),
            (
                active_settings(),
                MappingMutation::DeletePreset {
                    name: "missing".into(),
                },
                MappingMutationErrorKind::PresetNotFound,
            ),
            (
                active_settings(),
                MappingMutation::SetTableEnabled {
                    preset: "missing".into(),
                    table: ProxyRuleTable::Remote,
                    enabled: false,
                },
                MappingMutationErrorKind::PresetNotFound,
            ),
            (
                settings_with_rules(),
                MappingMutation::DeleteRule {
                    preset: "dev".into(),
                    table: ProxyRuleTable::Remote,
                    index: 99,
                },
                MappingMutationErrorKind::RuleNotFound,
            ),
            (
                settings_with_rules(),
                MappingMutation::UpdateRemoteRule {
                    preset: "dev".into(),
                    index: 0,
                    from: "bad".into(),
                    to: "http://localhost:1".into(),
                },
                MappingMutationErrorKind::InvalidField,
            ),
        ];
        for (mut settings, mutation, kind) in cases {
            let before = settings.clone();
            let error = apply_mapping_mutation(&mut settings, mutation).unwrap_err();
            assert_eq!(error.kind, kind);
            assert_eq!(settings, before);
        }
    }

    #[test]
    fn validation_reports_invalid_urls_rules_gates_and_locations() {
        let proxy = ProxySettings {
            enable: false,
            active_preset: Some("dev".into()),
            presets: vec![ProxyPresetSettings {
                name: "dev".into(),
                map_remote: ProxyMapRemoteSettings {
                    enable: false,
                    rules: vec![remote("bad", "bad")],
                },
                map_local: ProxyMapLocalSettings {
                    enable: false,
                    rules: vec![local("bad", "")],
                },
            }],
        };
        let result = validate_mapping_candidate(Some(&proxy));
        assert!(!result.is_valid());
        assert!(!result.gates.global_enabled);
        assert_eq!(result.gates.remote_enabled, Some(false));
        assert_eq!(result.gates.local_enabled, Some(false));
        assert!(
            result
                .diagnostics
                .iter()
                .all(|d| d.severity == DiagnosticSeverity::Error)
        );
        assert!(result.diagnostics.iter().any(|d| d.code
            == MappingDiagnosticCode::InvalidRuleSource
            && d.location.is_some_and(|l| l.preset_index == 0
                && l.table == MappingTable::Remote
                && l.rule_index == 0
                && l.field == MappingField::From)));
        assert!(result.diagnostics.iter().any(|d| {
            d.code == MappingDiagnosticCode::InvalidRuleTarget
                && d.location
                    .is_some_and(|l| l.table == MappingTable::Local && l.field == MappingField::To)
        }));
    }

    #[test]
    fn absent_candidate_is_valid_with_closed_gates() {
        let result = validate_mapping_candidate(None);
        assert!(result.is_valid());
        assert_eq!(result.gates, MappingGateState::default());
        assert!(result.diagnostics.is_empty());
    }

    #[test]
    fn validation_rejects_preset_names_that_collide_after_trimming() {
        let proxy = ProxySettings {
            active_preset: Some("dev".into()),
            presets: vec![preset("dev"), preset(" dev ")],
            ..ProxySettings::default()
        };

        let result = validate_mapping_candidate(Some(&proxy));

        assert!(result.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == MappingDiagnosticCode::DuplicatePresetName
                && diagnostic.severity == DiagnosticSeverity::Error
        }));
    }

    #[test]
    fn explanation_reports_effective_destinations_matches_gates_and_diagnostics_without_reads() {
        let proxy = ProxySettings {
            active_preset: Some("dev".into()),
            presets: vec![ProxyPresetSettings {
                name: "dev".into(),
                map_remote: ProxyMapRemoteSettings {
                    rules: vec![remote(
                        "https://api.example/v1",
                        "http://localhost:8080/backend",
                    )],
                    ..ProxyMapRemoteSettings::default()
                },
                map_local: ProxyMapLocalSettings {
                    rules: vec![local(
                        "http://localhost:8080/backend",
                        "/definitely/not/read.json",
                    )],
                    ..ProxyMapLocalSettings::default()
                },
            }],
            ..ProxySettings::default()
        };
        let result = explain_mapping_candidate(Some(&proxy), "https://api.example/v1");
        assert_eq!(
            result
                .original_url
                .as_ref()
                .map(ToString::to_string)
                .as_deref(),
            Some("https://api.example/v1")
        );
        assert_eq!(
            result
                .effective_url
                .as_ref()
                .map(ToString::to_string)
                .as_deref(),
            Some("http://localhost:8080/backend")
        );
        assert_eq!(
            result.local_path,
            Some(PathBuf::from("/definitely/not/read.json"))
        );
        assert_eq!(
            result.matches,
            vec![
                MappingRuleMatch {
                    preset: "dev".into(),
                    preset_index: 0,
                    table: ProxyRuleTable::Remote,
                    index: 0
                },
                MappingRuleMatch {
                    preset: "dev".into(),
                    preset_index: 0,
                    table: ProxyRuleTable::Local,
                    index: 0
                },
            ]
        );
        assert!(result.gates.global_enabled);
        assert_eq!(result.gates.remote_enabled, Some(true));
        assert_eq!(result.gates.local_enabled, Some(true));
        assert!(result.diagnostics.is_empty());
    }

    #[test]
    fn explanation_handles_invalid_request_candidate_and_disabled_gate() {
        let mut proxy = settings_with_rules().proxy.unwrap();
        proxy.presets[0].map_remote.rules[0].from = "bad".into();
        let invalid = explain_mapping_candidate(Some(&proxy), "not a request url");
        assert_eq!(invalid.original_url, None);
        assert_eq!(invalid.effective_url, None);
        assert!(invalid.matches.is_empty());
        assert!(
            invalid
                .diagnostics
                .iter()
                .any(|d| d.code == MappingDiagnosticCode::InvalidRequestUrl)
        );
        assert!(
            invalid
                .diagnostics
                .iter()
                .any(|d| d.code == MappingDiagnosticCode::InvalidRuleSource)
        );

        proxy.enable = false;
        let disabled = explain_mapping_candidate(Some(&proxy), "https://b.example/path");
        assert_eq!(
            disabled.effective_url.unwrap().to_string(),
            "https://b.example/path"
        );
        assert_eq!(disabled.local_path, None);
        assert!(disabled.matches.is_empty());
        assert!(!disabled.gates.global_enabled);
    }
}
