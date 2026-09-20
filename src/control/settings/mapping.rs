use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    control_rpc::protocol::{ControlError, ControlErrorCode},
    settings::{
        ProxySettings,
        mapping_ops::{
            MappingExplanation, MappingObjectRef, MappingValidationResult, MutationEffect,
            ProxyRuleTable,
        },
    },
};

#[derive(Clone, Debug, Default, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct MappingReadScope {
    pub(crate) preset: Option<String>,
    pub(crate) table: Option<ProxyRuleTable>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct MappingRuleView {
    pub(crate) index: usize,
    pub(crate) from: String,
    pub(crate) to: String,
    pub(crate) enabled: bool,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct MappingRuleTableView {
    pub(crate) enabled: bool,
    pub(crate) included: bool,
    pub(crate) rules_omitted: usize,
    pub(crate) rules: Vec<MappingRuleView>,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct MappingPresetView {
    pub(crate) index: usize,
    pub(crate) name: String,
    pub(crate) map_remote: MappingRuleTableView,
    pub(crate) map_local: MappingRuleTableView,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct MappingSettingsView {
    pub(crate) enabled: bool,
    pub(crate) active_preset: Option<String>,
    pub(crate) scope: MappingReadScope,
    pub(crate) presets_total: usize,
    pub(crate) presets_omitted: usize,
    pub(crate) presets: Vec<MappingPresetView>,
}

impl MappingSettingsView {
    pub(crate) fn scoped(
        proxy: &ProxySettings,
        scope: MappingReadScope,
    ) -> Result<Self, ControlError> {
        if let Some(name) = &scope.preset
            && !proxy.presets.iter().any(|preset| &preset.name == name)
        {
            return Err(ControlError::new(
                ControlErrorCode::InvalidArgument,
                "mapping preset was not found",
                false,
                serde_json::json!({"preset": name}),
            ));
        }
        // Bound the owned projection before copying strings/rules. Framing separately
        // checks encoded JSON size, including escaping and response-envelope overhead.
        let mut remaining = crate::control_rpc::framing::RESPONSE_MAX_BYTES;
        admit_projection_bytes(
            &mut remaining,
            std::mem::size_of::<Self>()
                .saturating_add(proxy.active_preset.as_ref().map_or(0, String::len))
                .saturating_add(scope.preset.as_ref().map_or(0, String::len)),
        )?;
        let presets = proxy
            .presets
            .iter()
            .enumerate()
            .filter(|(_, preset)| {
                scope
                    .preset
                    .as_ref()
                    .is_none_or(|name| &preset.name == name)
            })
            .map(|(index, preset)| {
                admit_projection_bytes(
                    &mut remaining,
                    std::mem::size_of::<MappingPresetView>().saturating_add(preset.name.len()),
                )?;
                let remote = scope.table != Some(ProxyRuleTable::Local);
                let local = scope.table != Some(ProxyRuleTable::Remote);
                Ok(MappingPresetView {
                    index,
                    name: preset.name.clone(),
                    map_remote: MappingRuleTableView {
                        enabled: preset.map_remote.enable,
                        included: remote,
                        rules_omitted: if remote {
                            0
                        } else {
                            preset.map_remote.rules.len()
                        },
                        rules: if remote {
                            preset
                                .map_remote
                                .rules
                                .iter()
                                .enumerate()
                                .map(|(index, rule)| {
                                    admit_projection_bytes(
                                        &mut remaining,
                                        std::mem::size_of::<MappingRuleView>()
                                            .saturating_add(rule.from.len())
                                            .saturating_add(rule.to.len()),
                                    )?;
                                    Ok(MappingRuleView {
                                        index,
                                        from: rule.from.clone(),
                                        to: rule.to.clone(),
                                        enabled: rule.enable,
                                    })
                                })
                                .collect::<Result<Vec<_>, ControlError>>()?
                        } else {
                            Vec::new()
                        },
                    },
                    map_local: MappingRuleTableView {
                        enabled: preset.map_local.enable,
                        included: local,
                        rules_omitted: if local {
                            0
                        } else {
                            preset.map_local.rules.len()
                        },
                        rules: if local {
                            preset
                                .map_local
                                .rules
                                .iter()
                                .enumerate()
                                .map(|(index, rule)| {
                                    admit_projection_bytes(
                                        &mut remaining,
                                        std::mem::size_of::<MappingRuleView>()
                                            .saturating_add(rule.from.len())
                                            .saturating_add(rule.to.len()),
                                    )?;
                                    Ok(MappingRuleView {
                                        index,
                                        from: rule.from.clone(),
                                        to: rule.to.clone(),
                                        enabled: rule.enable,
                                    })
                                })
                                .collect::<Result<Vec<_>, ControlError>>()?
                        } else {
                            Vec::new()
                        },
                    },
                })
            })
            .collect::<Result<Vec<_>, ControlError>>()?;
        Ok(Self {
            enabled: proxy.enable,
            active_preset: proxy.active_preset.clone(),
            presets_total: proxy.presets.len(),
            presets_omitted: proxy.presets.len() - presets.len(),
            presets,
            scope,
        })
    }
}

fn admit_projection_bytes(remaining: &mut usize, bytes: usize) -> Result<(), ControlError> {
    *remaining = remaining.checked_sub(bytes).ok_or_else(|| {
        ControlError::new(
            ControlErrorCode::ResourceLimit,
            "mapping settings projection exceeds the response budget; narrow the preset or table",
            false,
            serde_json::json!({"max_projection_bytes": crate::control_rpc::framing::RESPONSE_MAX_BYTES}),
        )
    })?;
    Ok(())
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct MappingMutationPreview {
    pub(crate) affected: MappingObjectRef,
    pub(crate) effect: MutationEffect,
    pub(crate) validation: MappingValidationResult,
    pub(crate) explanations: Vec<MappingExplanation>,
}

pub(crate) const MAX_PREVIEW_URLS: usize = 16;
pub(crate) const MAX_PREVIEW_URL_BYTES: usize =
    crate::control::capture_query::MAX_CAPTURE_PATTERN_BYTES;

pub(crate) fn validate_preview_urls(urls: &[String]) -> Result<(), ControlError> {
    if urls.len() > MAX_PREVIEW_URLS {
        return Err(ControlError::new(
            ControlErrorCode::InvalidArgument,
            "too many mapping preview URLs",
            false,
            serde_json::json!({"maximum": MAX_PREVIEW_URLS}),
        ));
    }
    for (index, url) in urls.iter().enumerate() {
        if url.is_empty() || url.len() > MAX_PREVIEW_URL_BYTES {
            return Err(ControlError::new(
                ControlErrorCode::InvalidArgument,
                "mapping preview URL length is out of range",
                false,
                serde_json::json!({"index": index, "minimum_bytes": 1, "maximum_bytes": MAX_PREVIEW_URL_BYTES}),
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "mapping_tests.rs"]
mod tests;
