use crate::{
    capture::CaptureSequence,
    control::settings::mapping::{
        MappingMutationPreview, MappingReadScope, MappingSettingsView, validate_preview_urls,
    },
    control::{
        BodyWorkRuntimeStatus, CaptureStoreRuntimeStatus, ControlRpcRuntimeStatus,
        InstanceRuntimeMetrics, MappingRuntimeStatus, SearchWorkRuntimeStatus,
        WaitForCaptureRequest, WaitForCaptureResult,
        audit::InstanceAuditSnapshot,
        body::{
            BodyContentRequest, BodyPage, ExtractCaptureBodyRequest, ExtractCaptureBodyResult,
            SearchCaptureBodyRequest, SearchCaptureBodyResult, SelectionContentRequest,
            SelectionPage,
        },
        capture_query::{CaptureDetail, CaptureQuery, CaptureSearchCursor, CompactCapture},
        json_walk::{
            FindJsonPointersRequest, FindJsonPointersResult, ProbeJsonPointerPatternRequest,
            ProbeJsonPointerPatternResult,
        },
    },
    instance::RunId,
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
use schemars::JsonSchema;
use serde::{
    Deserialize, Deserializer, Serialize, Serializer, de::DeserializeOwned, ser::SerializeMap,
};
use serde_json::{Value, value::RawValue};
use std::{
    net::SocketAddr,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::OwnedSemaphorePermit;

pub(crate) const RPC_VERSION: u16 = 2;
const MAX_IDENTIFIER_BYTES: usize = 128;
const ORDINARY_MAX_DEADLINE: Duration = Duration::from_secs(30);
const WAIT_MAX_DEADLINE: Duration = Duration::from_secs(330);

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DeclaredClient {
    pub(crate) name: String,
    pub(crate) version: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ControlOperationKind {
    DescribeInstance,
    GetStatus,
    SetRecordingEnabled,
    SearchCaptures,
    GetCapture,
    WaitForCapture,
    ReadCaptureBody,
    SearchCaptureBody,
    ExtractCaptureBody,
    ReadSelectedBody,
    FindJsonPointers,
    ProbeJsonPointerPattern,
    GetMappingSettings,
    ValidateMappingSettings,
    ExplainMapping,
    PreviewMappingMutation,
    MutateMapping,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum MutationAuditOperationKind {
    SetRecordingEnabled,
    CreatePreset,
    RenamePreset,
    DeletePreset,
    SetActivePreset,
    SetMappingGate,
    CreateMappingRule,
    UpdateMappingRule,
    DeleteMappingRule,
    MoveMappingRule,
    SetMappingRuleEnabled,
}

impl ControlOperationKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::DescribeInstance => "describe_instance",
            Self::GetStatus => "get_status",
            Self::SetRecordingEnabled => "set_recording_enabled",
            Self::SearchCaptures => "search_captures",
            Self::GetCapture => "get_capture",
            Self::WaitForCapture => "wait_for_capture",
            Self::ReadCaptureBody => "read_capture_body",
            Self::SearchCaptureBody => "search_capture_body",
            Self::ExtractCaptureBody => "extract_capture_body",
            Self::ReadSelectedBody => "read_selected_body",
            Self::FindJsonPointers => "find_json_pointers",
            Self::ProbeJsonPointerPattern => "probe_json_pointer_pattern",
            Self::GetMappingSettings => "get_mapping_settings",
            Self::ValidateMappingSettings => "validate_mapping_settings",
            Self::ExplainMapping => "explain_mapping",
            Self::PreviewMappingMutation => "preview_mapping_mutation",
            Self::MutateMapping => "mutate_mapping",
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RequestEnvelope {
    pub(crate) protocol_version: u16,
    pub(crate) request_id: String,
    pub(crate) run_id: RunId,
    pub(crate) deadline_ms: u64,
    pub(crate) client: DeclaredClient,
    pub(crate) operation: ControlOperationKind,
    pub(crate) arguments: Box<RawValue>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ControlRequest {
    pub(crate) request_id: String,
    pub(crate) run_id: RunId,
    pub(crate) deadline: Instant,
    pub(crate) client: DeclaredClient,
    pub(crate) operation: ControlOperation,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum ControlOperation {
    DescribeInstance,
    GetStatus,
    SetRecordingEnabled {
        enabled: bool,
    },
    SearchCaptures {
        query: Box<CaptureQuery>,
        cursor: Option<CaptureSearchCursor>,
        limit: Option<usize>,
    },
    GetCapture {
        capture_id: CaptureSequence,
        expected_revision: Option<u64>,
    },
    WaitForCapture(Box<WaitForCaptureRequest>),
    ReadCaptureBody(Box<BodyContentRequest>),
    SearchCaptureBody(Box<SearchCaptureBodyRequest>),
    ExtractCaptureBody(Box<ExtractCaptureBodyRequest>),
    ReadSelectedBody(Box<SelectionContentRequest>),
    FindJsonPointers(Box<FindJsonPointersRequest>),
    ProbeJsonPointerPattern(Box<ProbeJsonPointerPatternRequest>),
    GetMappingSettings {
        scope: MappingReadScope,
    },
    ValidateMappingSettings {
        proxy: Box<ProxySettings>,
    },
    ExplainMapping {
        url: String,
        proposed_proxy: Option<Box<ProxySettings>>,
    },
    PreviewMappingMutation {
        expected_revision: SettingsRevision,
        mutation: Box<MappingMutation>,
        urls: Vec<String>,
    },
    MutateMapping {
        expected_revision: SettingsRevision,
        mutation: Box<MappingMutation>,
    },
}

impl ControlOperation {
    pub(crate) fn kind(&self) -> ControlOperationKind {
        match self {
            Self::DescribeInstance => ControlOperationKind::DescribeInstance,
            Self::GetStatus => ControlOperationKind::GetStatus,
            Self::SetRecordingEnabled { .. } => ControlOperationKind::SetRecordingEnabled,
            Self::SearchCaptures { .. } => ControlOperationKind::SearchCaptures,
            Self::GetCapture { .. } => ControlOperationKind::GetCapture,
            Self::WaitForCapture(_) => ControlOperationKind::WaitForCapture,
            Self::ReadCaptureBody(_) => ControlOperationKind::ReadCaptureBody,
            Self::SearchCaptureBody(_) => ControlOperationKind::SearchCaptureBody,
            Self::ExtractCaptureBody(_) => ControlOperationKind::ExtractCaptureBody,
            Self::ReadSelectedBody(_) => ControlOperationKind::ReadSelectedBody,
            Self::FindJsonPointers(_) => ControlOperationKind::FindJsonPointers,
            Self::ProbeJsonPointerPattern(_) => ControlOperationKind::ProbeJsonPointerPattern,
            Self::GetMappingSettings { .. } => ControlOperationKind::GetMappingSettings,
            Self::ValidateMappingSettings { .. } => ControlOperationKind::ValidateMappingSettings,
            Self::ExplainMapping { .. } => ControlOperationKind::ExplainMapping,
            Self::PreviewMappingMutation { .. } => ControlOperationKind::PreviewMappingMutation,
            Self::MutateMapping { .. } => ControlOperationKind::MutateMapping,
        }
    }
}

#[derive(Serialize)]
pub(crate) struct OutboundRequestEnvelope<'a> {
    protocol_version: u16,
    request_id: &'a str,
    run_id: &'a RunId,
    deadline_ms: u64,
    client: &'a DeclaredClient,
    operation: ControlOperationKind,
    arguments: ControlOperationArguments<'a>,
}

impl<'a> OutboundRequestEnvelope<'a> {
    pub(crate) fn new(
        request_id: &'a str,
        run_id: &'a RunId,
        deadline_ms: u64,
        client: &'a DeclaredClient,
        operation: &'a ControlOperation,
    ) -> Self {
        Self {
            protocol_version: RPC_VERSION,
            request_id,
            run_id,
            deadline_ms,
            client,
            operation: operation.kind(),
            arguments: ControlOperationArguments(operation),
        }
    }
}

struct ControlOperationArguments<'a>(&'a ControlOperation);

impl Serialize for ControlOperationArguments<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self.0 {
            ControlOperation::DescribeInstance | ControlOperation::GetStatus => {
                serializer.serialize_map(Some(0))?.end()
            }
            ControlOperation::GetMappingSettings { scope } => scope.serialize(serializer),
            ControlOperation::SetRecordingEnabled { enabled } => {
                let mut map = serializer.serialize_map(Some(1))?;
                map.serialize_entry("enabled", enabled)?;
                map.end()
            }
            ControlOperation::SearchCaptures {
                query,
                cursor,
                limit,
            } => {
                let mut map = serializer.serialize_map(Some(3))?;
                map.serialize_entry("query", query)?;
                map.serialize_entry("cursor", cursor)?;
                map.serialize_entry("limit", limit)?;
                map.end()
            }
            ControlOperation::GetCapture {
                capture_id,
                expected_revision,
            } => {
                let mut map = serializer.serialize_map(Some(2))?;
                map.serialize_entry("capture_id", capture_id)?;
                map.serialize_entry("expected_revision", expected_revision)?;
                map.end()
            }
            ControlOperation::WaitForCapture(request) => request.serialize(serializer),
            ControlOperation::ReadCaptureBody(request) => request.serialize(serializer),
            ControlOperation::SearchCaptureBody(request) => request.serialize(serializer),
            ControlOperation::ExtractCaptureBody(request) => request.serialize(serializer),
            ControlOperation::ReadSelectedBody(request) => request.serialize(serializer),
            ControlOperation::FindJsonPointers(request) => request.serialize(serializer),
            ControlOperation::ProbeJsonPointerPattern(request) => request.serialize(serializer),
            ControlOperation::ValidateMappingSettings { proxy } => {
                let mut map = serializer.serialize_map(Some(1))?;
                map.serialize_entry("proxy", proxy)?;
                map.end()
            }
            ControlOperation::ExplainMapping {
                url,
                proposed_proxy,
            } => {
                let mut map = serializer.serialize_map(Some(2))?;
                map.serialize_entry("url", url)?;
                map.serialize_entry("proposed_proxy", proposed_proxy)?;
                map.end()
            }
            ControlOperation::PreviewMappingMutation {
                expected_revision,
                mutation,
                urls,
            } => {
                let mut map = serializer.serialize_map(Some(3))?;
                map.serialize_entry("expected_revision", expected_revision)?;
                map.serialize_entry("mutation", mutation)?;
                map.serialize_entry("urls", urls)?;
                map.end()
            }
            ControlOperation::MutateMapping {
                expected_revision,
                mutation,
            } => {
                let mut map = serializer.serialize_map(Some(2))?;
                map.serialize_entry("expected_revision", expected_revision)?;
                map.serialize_entry("mutation", mutation)?;
                map.end()
            }
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct DescribeInstanceArguments {}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct GetStatusArguments {}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SetRecordingEnabledArguments {
    enabled: bool,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SearchCapturesArguments {
    query: CaptureQuery,
    #[serde(default)]
    cursor: Option<CaptureSearchCursor>,
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct GetCaptureArguments {
    capture_id: CaptureSequence,
    #[serde(default)]
    expected_revision: Option<u64>,
}

fn mapping_default_enabled() -> bool {
    true
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StrictMappingRule {
    from: String,
    to: String,
    #[serde(default = "mapping_default_enabled")]
    enable: bool,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
struct StrictMappingRuleTable {
    #[serde(default = "mapping_default_enabled")]
    enable: bool,
    rules: Vec<StrictMappingRule>,
}

impl Default for StrictMappingRuleTable {
    fn default() -> Self {
        Self {
            enable: true,
            rules: Vec::new(),
        }
    }
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
struct StrictProxyPresetSettings {
    name: String,
    map_remote: StrictMappingRuleTable,
    map_local: StrictMappingRuleTable,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
struct StrictProxySettings {
    #[serde(default = "mapping_default_enabled")]
    enable: bool,
    active_preset: Option<String>,
    presets: Vec<StrictProxyPresetSettings>,
}

impl Default for StrictProxySettings {
    fn default() -> Self {
        Self {
            enable: true,
            active_preset: None,
            presets: Vec::new(),
        }
    }
}

impl From<ProxyMapRemoteRule> for StrictMappingRule {
    fn from(rule: ProxyMapRemoteRule) -> Self {
        Self {
            from: rule.from,
            to: rule.to,
            enable: rule.enable,
        }
    }
}

impl From<ProxyMapLocalRule> for StrictMappingRule {
    fn from(rule: ProxyMapLocalRule) -> Self {
        Self {
            from: rule.from,
            to: rule.to,
            enable: rule.enable,
        }
    }
}

impl From<StrictMappingRule> for ProxyMapRemoteRule {
    fn from(rule: StrictMappingRule) -> Self {
        Self {
            from: rule.from,
            to: rule.to,
            enable: rule.enable,
        }
    }
}

impl From<StrictMappingRule> for ProxyMapLocalRule {
    fn from(rule: StrictMappingRule) -> Self {
        Self {
            from: rule.from,
            to: rule.to,
            enable: rule.enable,
        }
    }
}

impl From<ProxyPresetSettings> for StrictProxyPresetSettings {
    fn from(preset: ProxyPresetSettings) -> Self {
        Self {
            name: preset.name,
            map_remote: StrictMappingRuleTable {
                enable: preset.map_remote.enable,
                rules: preset
                    .map_remote
                    .rules
                    .into_iter()
                    .map(Into::into)
                    .collect(),
            },
            map_local: StrictMappingRuleTable {
                enable: preset.map_local.enable,
                rules: preset.map_local.rules.into_iter().map(Into::into).collect(),
            },
        }
    }
}

impl From<StrictProxyPresetSettings> for ProxyPresetSettings {
    fn from(preset: StrictProxyPresetSettings) -> Self {
        Self {
            name: preset.name,
            map_remote: ProxyMapRemoteSettings {
                enable: preset.map_remote.enable,
                rules: preset
                    .map_remote
                    .rules
                    .into_iter()
                    .map(Into::into)
                    .collect(),
            },
            map_local: ProxyMapLocalSettings {
                enable: preset.map_local.enable,
                rules: preset.map_local.rules.into_iter().map(Into::into).collect(),
            },
        }
    }
}

impl From<ProxySettings> for StrictProxySettings {
    fn from(proxy: ProxySettings) -> Self {
        Self {
            enable: proxy.enable,
            active_preset: proxy.active_preset,
            presets: proxy.presets.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<StrictProxySettings> for ProxySettings {
    fn from(proxy: StrictProxySettings) -> Self {
        Self {
            enable: proxy.enable,
            active_preset: proxy.active_preset,
            presets: proxy.presets.into_iter().map(Into::into).collect(),
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum StrictMappingMutation {
    CreatePreset {
        name: String,
        #[serde(default)]
        initial: Option<StrictProxyPresetSettings>,
    },
    RenamePreset {
        name: String,
        new_name: String,
    },
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
    AppendRemoteRule {
        preset: String,
        rule: StrictMappingRule,
    },
    InsertRemoteRule {
        preset: String,
        index: usize,
        rule: StrictMappingRule,
    },
    UpdateRemoteRule {
        preset: String,
        index: usize,
        from: String,
        to: String,
    },
    AppendLocalRule {
        preset: String,
        rule: StrictMappingRule,
    },
    InsertLocalRule {
        preset: String,
        index: usize,
        rule: StrictMappingRule,
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
impl From<MappingMutation> for StrictMappingMutation {
    fn from(mutation: MappingMutation) -> Self {
        match mutation {
            MappingMutation::CreatePreset { name, initial } => Self::CreatePreset {
                name,
                initial: initial.map(Into::into),
            },
            MappingMutation::RenamePreset { name, new_name } => {
                Self::RenamePreset { name, new_name }
            }
            MappingMutation::DeletePreset { name } => Self::DeletePreset { name },
            MappingMutation::SetActivePreset { name } => Self::SetActivePreset { name },
            MappingMutation::SetGlobalEnabled { enabled } => Self::SetGlobalEnabled { enabled },
            MappingMutation::SetTableEnabled {
                preset,
                table,
                enabled,
            } => Self::SetTableEnabled {
                preset,
                table,
                enabled,
            },
            MappingMutation::AppendRemoteRule { preset, rule } => Self::AppendRemoteRule {
                preset,
                rule: rule.into(),
            },
            MappingMutation::InsertRemoteRule {
                preset,
                index,
                rule,
            } => Self::InsertRemoteRule {
                preset,
                index,
                rule: rule.into(),
            },
            MappingMutation::UpdateRemoteRule {
                preset,
                index,
                from,
                to,
            } => Self::UpdateRemoteRule {
                preset,
                index,
                from,
                to,
            },
            MappingMutation::AppendLocalRule { preset, rule } => Self::AppendLocalRule {
                preset,
                rule: rule.into(),
            },
            MappingMutation::InsertLocalRule {
                preset,
                index,
                rule,
            } => Self::InsertLocalRule {
                preset,
                index,
                rule: rule.into(),
            },
            MappingMutation::UpdateLocalRule {
                preset,
                index,
                from,
                to,
            } => Self::UpdateLocalRule {
                preset,
                index,
                from,
                to,
            },
            MappingMutation::DeleteRule {
                preset,
                table,
                index,
            } => Self::DeleteRule {
                preset,
                table,
                index,
            },
            MappingMutation::MoveRule {
                preset,
                table,
                from,
                to,
            } => Self::MoveRule {
                preset,
                table,
                from,
                to,
            },
            MappingMutation::SetRuleEnabled {
                preset,
                table,
                index,
                enabled,
            } => Self::SetRuleEnabled {
                preset,
                table,
                index,
                enabled,
            },
        }
    }
}

impl From<StrictMappingMutation> for MappingMutation {
    fn from(mutation: StrictMappingMutation) -> Self {
        match mutation {
            StrictMappingMutation::CreatePreset { name, initial } => Self::CreatePreset {
                name,
                initial: initial.map(Into::into),
            },
            StrictMappingMutation::RenamePreset { name, new_name } => {
                Self::RenamePreset { name, new_name }
            }
            StrictMappingMutation::DeletePreset { name } => Self::DeletePreset { name },
            StrictMappingMutation::SetActivePreset { name } => Self::SetActivePreset { name },
            StrictMappingMutation::SetGlobalEnabled { enabled } => {
                Self::SetGlobalEnabled { enabled }
            }
            StrictMappingMutation::SetTableEnabled {
                preset,
                table,
                enabled,
            } => Self::SetTableEnabled {
                preset,
                table,
                enabled,
            },
            StrictMappingMutation::AppendRemoteRule { preset, rule } => Self::AppendRemoteRule {
                preset,
                rule: rule.into(),
            },
            StrictMappingMutation::InsertRemoteRule {
                preset,
                index,
                rule,
            } => Self::InsertRemoteRule {
                preset,
                index,
                rule: rule.into(),
            },
            StrictMappingMutation::UpdateRemoteRule {
                preset,
                index,
                from,
                to,
            } => Self::UpdateRemoteRule {
                preset,
                index,
                from,
                to,
            },
            StrictMappingMutation::AppendLocalRule { preset, rule } => Self::AppendLocalRule {
                preset,
                rule: rule.into(),
            },
            StrictMappingMutation::InsertLocalRule {
                preset,
                index,
                rule,
            } => Self::InsertLocalRule {
                preset,
                index,
                rule: rule.into(),
            },
            StrictMappingMutation::UpdateLocalRule {
                preset,
                index,
                from,
                to,
            } => Self::UpdateLocalRule {
                preset,
                index,
                from,
                to,
            },
            StrictMappingMutation::DeleteRule {
                preset,
                table,
                index,
            } => Self::DeleteRule {
                preset,
                table,
                index,
            },
            StrictMappingMutation::MoveRule {
                preset,
                table,
                from,
                to,
            } => Self::MoveRule {
                preset,
                table,
                from,
                to,
            },
            StrictMappingMutation::SetRuleEnabled {
                preset,
                table,
                index,
                enabled,
            } => Self::SetRuleEnabled {
                preset,
                table,
                index,
                enabled,
            },
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ValidateMappingSettingsArguments {
    proxy: StrictProxySettings,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ExplainMappingArguments {
    url: String,
    #[serde(default)]
    proposed_proxy: Option<StrictProxySettings>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct MutateMappingArguments {
    expected_revision: SettingsRevision,
    mutation: StrictMappingMutation,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PreviewMappingMutationArguments {
    expected_revision: SettingsRevision,
    mutation: StrictMappingMutation,
    urls: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct InstanceScope {
    pub(crate) proxy_endpoint: SocketAddr,
    pub(crate) run_id: RunId,
}

#[derive(Clone, Debug)]
pub(crate) struct MappingSettingsPayload {
    mapping: MappingSettingsView,
    _worker_permit: Option<Arc<OwnedSemaphorePermit>>,
}

impl MappingSettingsPayload {
    pub(crate) fn from_snapshot(
        mapping: MappingSettingsView,
        worker_permit: Arc<OwnedSemaphorePermit>,
    ) -> Self {
        Self {
            mapping,
            _worker_permit: Some(worker_permit),
        }
    }

    #[cfg(test)]
    pub(crate) fn from_proxy(proxy: ProxySettings) -> Self {
        Self {
            mapping: MappingSettingsView::scoped(&proxy, MappingReadScope::default())
                .expect("unfiltered mapping view"),
            _worker_permit: None,
        }
    }

    pub(crate) fn into_mapping(self) -> MappingSettingsView {
        self.mapping
    }
}

impl PartialEq for MappingSettingsPayload {
    fn eq(&self, other: &Self) -> bool {
        self.mapping == other.mapping
    }
}

impl Serialize for MappingSettingsPayload {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.mapping.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for MappingSettingsPayload {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let mapping = MappingSettingsView::deserialize(deserializer)?;
        Ok(Self {
            mapping,
            _worker_permit: None,
        })
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum ControlResult {
    DescribeInstance {
        instance: InstanceScope,
        config_mode: ConfigMode,
        persistence: PersistenceMode,
        recording_enabled: bool,
        retained_capture_count: usize,
        settings_revision: u64,
    },
    GetStatus {
        instance: InstanceScope,
        local_proxy_url: String,
        fluxcope_version: String,
        rpc_version: u16,
        config_source: Option<PathBuf>,
        config_mode: ConfigMode,
        persistence: PersistenceMode,
        recording_enabled: bool,
        retained_capture_count: usize,
        settings_revision: u64,
        mapping: MappingRuntimeStatus,
        capture_store: CaptureStoreRuntimeStatus,
        capture_change_epoch: u64,
        metrics: Box<InstanceRuntimeMetrics>,
        private_rpc: ControlRpcRuntimeStatus,
        body_work: Box<BodyWorkRuntimeStatus>,
        search_work: SearchWorkRuntimeStatus,
        audit: Box<InstanceAuditSnapshot>,
    },
    SetRecordingEnabled {
        instance: InstanceScope,
        previous: bool,
        current: bool,
    },
    SearchCaptures {
        instance: InstanceScope,
        captures: Vec<CompactCapture>,
        next_cursor: Option<CaptureSearchCursor>,
    },
    GetCapture {
        instance: InstanceScope,
        capture: Box<CaptureDetail>,
    },
    WaitForCapture {
        instance: InstanceScope,
        #[serde(flatten)]
        result: WaitForCaptureResult,
    },
    ReadCaptureBody {
        instance: InstanceScope,
        page: Box<BodyPage>,
    },
    SearchCaptureBody {
        instance: InstanceScope,
        #[serde(flatten)]
        result: Box<SearchCaptureBodyResult>,
    },
    ExtractCaptureBody {
        instance: InstanceScope,
        #[serde(flatten)]
        result: Box<ExtractCaptureBodyResult>,
    },
    ReadSelectedBody {
        instance: InstanceScope,
        page: Box<SelectionPage>,
    },
    FindJsonPointers {
        instance: InstanceScope,
        #[serde(flatten)]
        result: Box<FindJsonPointersResult>,
    },
    ProbeJsonPointerPattern {
        instance: InstanceScope,
        #[serde(flatten)]
        result: Box<ProbeJsonPointerPatternResult>,
    },
    GetMappingSettings {
        instance: InstanceScope,
        settings_revision: SettingsRevision,
        config_mode: ConfigMode,
        persistence: PersistenceMode,
        proxy: MappingSettingsPayload,
    },
    ValidateMappingSettings {
        instance: InstanceScope,
        settings_revision: SettingsRevision,
        config_mode: ConfigMode,
        persistence: PersistenceMode,
        validation: Box<MappingValidationResult>,
    },
    ExplainMapping {
        instance: InstanceScope,
        settings_revision: SettingsRevision,
        config_mode: ConfigMode,
        persistence: PersistenceMode,
        explanation: Box<MappingExplanation>,
    },
    PreviewMappingMutation {
        instance: InstanceScope,
        settings_revision: SettingsRevision,
        config_mode: ConfigMode,
        persistence: PersistenceMode,
        preview: Box<MappingMutationPreview>,
    },
    MutateMapping {
        instance: InstanceScope,
        settings_revision: SettingsRevision,
        config_mode: ConfigMode,
        persistence: PersistenceMode,
        outcome: SettingsTransactionOutcome,
        affected: MappingObjectRef,
    },
}

impl ControlResult {
    pub(crate) fn instance_scope(&self) -> &InstanceScope {
        match self {
            Self::DescribeInstance { instance, .. }
            | Self::GetStatus { instance, .. }
            | Self::SetRecordingEnabled { instance, .. }
            | Self::SearchCaptures { instance, .. }
            | Self::GetCapture { instance, .. }
            | Self::WaitForCapture { instance, .. }
            | Self::ReadCaptureBody { instance, .. }
            | Self::SearchCaptureBody { instance, .. }
            | Self::ExtractCaptureBody { instance, .. }
            | Self::ReadSelectedBody { instance, .. }
            | Self::FindJsonPointers { instance, .. }
            | Self::ProbeJsonPointerPattern { instance, .. }
            | Self::GetMappingSettings { instance, .. }
            | Self::ValidateMappingSettings { instance, .. }
            | Self::ExplainMapping { instance, .. }
            | Self::PreviewMappingMutation { instance, .. }
            | Self::MutateMapping { instance, .. } => instance,
        }
    }

    pub(crate) fn kind(&self) -> ControlOperationKind {
        match self {
            Self::DescribeInstance { .. } => ControlOperationKind::DescribeInstance,
            Self::GetStatus { .. } => ControlOperationKind::GetStatus,
            Self::SetRecordingEnabled { .. } => ControlOperationKind::SetRecordingEnabled,
            Self::SearchCaptures { .. } => ControlOperationKind::SearchCaptures,
            Self::GetCapture { .. } => ControlOperationKind::GetCapture,
            Self::WaitForCapture { .. } => ControlOperationKind::WaitForCapture,
            Self::ReadCaptureBody { .. } => ControlOperationKind::ReadCaptureBody,
            Self::SearchCaptureBody { .. } => ControlOperationKind::SearchCaptureBody,
            Self::ExtractCaptureBody { .. } => ControlOperationKind::ExtractCaptureBody,
            Self::ReadSelectedBody { .. } => ControlOperationKind::ReadSelectedBody,
            Self::FindJsonPointers { .. } => ControlOperationKind::FindJsonPointers,
            Self::ProbeJsonPointerPattern { .. } => ControlOperationKind::ProbeJsonPointerPattern,
            Self::GetMappingSettings { .. } => ControlOperationKind::GetMappingSettings,
            Self::ValidateMappingSettings { .. } => ControlOperationKind::ValidateMappingSettings,
            Self::ExplainMapping { .. } => ControlOperationKind::ExplainMapping,
            Self::PreviewMappingMutation { .. } => ControlOperationKind::PreviewMappingMutation,
            Self::MutateMapping { .. } => ControlOperationKind::MutateMapping,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ControlErrorCode {
    InvalidArgument,
    NoInstances,
    InstanceRequired,
    InstanceNotFound,
    InstanceGenerationConflict,
    InstanceUnavailable,
    RpcVersionMismatch,
    RpcFrameTooLarge,
    ResourceLimit,
    CaptureNotFound,
    CaptureRevisionConflict,
    SettingsRevisionConflict,
    MappingValidationFailed,
    TuiDraftConflict,
    ServiceUnavailable,
    UnsupportedBodyEncoding,
    DeadlineExceeded,
    Cancelled,
    UndecodableBody,
    BodyNotTextual,
    NotFound,
    MalformedJson,
    JsonDepthLimit,
    JsonSizeLimit,
    InternalError,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum LocalTransportCause {
    DefinitiveStaleConnect,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ControlError {
    pub(crate) code: ControlErrorCode,
    pub(crate) message: String,
    pub(crate) retryable: bool,
    pub(crate) details: Value,
    #[serde(skip)]
    pub(crate) local_transport_cause: Option<LocalTransportCause>,
}

impl ControlErrorCode {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::InvalidArgument => "invalid_argument",
            Self::NoInstances => "no_instances",
            Self::InstanceRequired => "instance_required",
            Self::InstanceNotFound => "instance_not_found",
            Self::InstanceGenerationConflict => "instance_generation_conflict",
            Self::InstanceUnavailable => "instance_unavailable",
            Self::RpcVersionMismatch => "rpc_version_mismatch",
            Self::RpcFrameTooLarge => "rpc_frame_too_large",
            Self::ResourceLimit => "resource_limit",
            Self::CaptureNotFound => "capture_not_found",
            Self::CaptureRevisionConflict => "capture_revision_conflict",
            Self::ServiceUnavailable => "service_unavailable",
            Self::SettingsRevisionConflict => "settings_revision_conflict",
            Self::MappingValidationFailed => "mapping_validation_failed",
            Self::TuiDraftConflict => "tui_draft_conflict",
            Self::UnsupportedBodyEncoding => "unsupported_body_encoding",
            Self::DeadlineExceeded => "deadline_exceeded",
            Self::Cancelled => "cancelled",
            Self::UndecodableBody => "undecodable_body",
            Self::BodyNotTextual => "body_not_textual",
            Self::NotFound => "not_found",
            Self::MalformedJson => "malformed_json",
            Self::JsonDepthLimit => "json_depth_limit",
            Self::JsonSizeLimit => "json_size_limit",
            Self::InternalError => "internal_error",
        }
    }
}

impl ControlError {
    pub(crate) fn new(
        code: ControlErrorCode,
        message: impl Into<String>,
        retryable: bool,
        details: Value,
    ) -> Self {
        Self {
            code,
            message: message.into(),
            retryable,
            details,
            local_transport_cause: None,
        }
    }
    pub(crate) fn code(&self) -> ControlErrorCode {
        self.code
    }

    pub(crate) fn message(&self) -> &str {
        &self.message
    }

    pub(crate) fn retryable(&self) -> bool {
        self.retryable
    }

    pub(crate) fn details(&self) -> &Value {
        &self.details
    }

    pub(crate) fn is_definitive_stale_connect(&self) -> bool {
        self.local_transport_cause == Some(LocalTransportCause::DefinitiveStaleConnect)
    }

    pub(crate) fn with_local_transport_cause(mut self, cause: LocalTransportCause) -> Self {
        self.local_transport_cause = Some(cause);
        self
    }

    pub(crate) fn invalid_argument(message: impl Into<String>) -> Self {
        Self::new(
            ControlErrorCode::InvalidArgument,
            message,
            false,
            Value::Object(Default::default()),
        )
    }
    pub(crate) fn no_instances() -> Self {
        Self::new(
            ControlErrorCode::NoInstances,
            "no live Fluxcope instances are available",
            true,
            Value::Object(Default::default()),
        )
    }

    pub(crate) fn instance_required(message: impl Into<String>, details: Value) -> Self {
        Self::new(ControlErrorCode::InstanceRequired, message, false, details)
    }

    pub(crate) fn instance_not_found(message: impl Into<String>) -> Self {
        Self::new(
            ControlErrorCode::InstanceNotFound,
            message,
            false,
            Value::Object(Default::default()),
        )
    }

    pub(crate) fn instance_unavailable(message: impl Into<String>) -> Self {
        Self::new(
            ControlErrorCode::InstanceUnavailable,
            message,
            true,
            Value::Object(Default::default()),
        )
    }
    pub(crate) fn service_unavailable(message: impl Into<String>) -> Self {
        Self::new(
            ControlErrorCode::ServiceUnavailable,
            message,
            true,
            Value::Object(Default::default()),
        )
    }

    pub(crate) fn deadline_exceeded(message: impl Into<String>) -> Self {
        Self::new(
            ControlErrorCode::DeadlineExceeded,
            message,
            true,
            Value::Object(Default::default()),
        )
    }

    pub(crate) fn cancelled(message: impl Into<String>) -> Self {
        Self::new(
            ControlErrorCode::Cancelled,
            message,
            true,
            Value::Object(Default::default()),
        )
    }

    pub(crate) fn internal(message: impl Into<String>) -> Self {
        Self::new(
            ControlErrorCode::InternalError,
            message,
            false,
            Value::Object(Default::default()),
        )
    }

    pub(crate) fn frame_too_large(max_bytes: usize) -> Self {
        Self::new(
            ControlErrorCode::RpcFrameTooLarge,
            "private RPC frame exceeds its byte limit",
            false,
            serde_json::json!({"max_bytes": max_bytes}),
        )
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ResponseEnvelope {
    pub(crate) protocol_version: u16,
    pub(crate) request_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) result: Option<ControlResult>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) error: Option<ControlError>,
}

impl RequestEnvelope {
    #[cfg(test)]
    pub(crate) fn new(
        request_id: String,
        run_id: RunId,
        deadline_ms: u64,
        client: DeclaredClient,
        operation: ControlOperation,
    ) -> Result<Self, ControlError> {
        let arguments = serialize_arguments(&ControlOperationArguments(&operation))?;
        let operation = operation.kind();
        Ok(Self {
            protocol_version: RPC_VERSION,
            request_id,
            run_id,
            deadline_ms,
            client,
            operation,
            arguments,
        })
    }

    pub(crate) fn clamped_deadline(&self, received_at: Instant) -> Instant {
        let maximum = match self.operation {
            ControlOperationKind::WaitForCapture => WAIT_MAX_DEADLINE,
            ControlOperationKind::DescribeInstance
            | ControlOperationKind::GetStatus
            | ControlOperationKind::SetRecordingEnabled
            | ControlOperationKind::SearchCaptures
            | ControlOperationKind::GetCapture
            | ControlOperationKind::ReadCaptureBody
            | ControlOperationKind::SearchCaptureBody
            | ControlOperationKind::ExtractCaptureBody
            | ControlOperationKind::ReadSelectedBody
            | ControlOperationKind::FindJsonPointers
            | ControlOperationKind::ProbeJsonPointerPattern
            | ControlOperationKind::GetMappingSettings
            | ControlOperationKind::ValidateMappingSettings
            | ControlOperationKind::ExplainMapping
            | ControlOperationKind::PreviewMappingMutation
            | ControlOperationKind::MutateMapping => ORDINARY_MAX_DEADLINE,
        };
        received_at + Duration::from_millis(self.deadline_ms).min(maximum)
    }

    pub(crate) fn validate(self, received_at: Instant) -> Result<ControlRequest, ControlError> {
        if self.protocol_version != RPC_VERSION {
            return Err(ControlError::new(
                ControlErrorCode::RpcVersionMismatch,
                "private RPC protocol version does not match",
                false,
                serde_json::json!({
                    "expected": RPC_VERSION,
                    "received": self.protocol_version,
                }),
            ));
        }
        validate_identifier("request_id", &self.request_id)?;
        validate_identifier("client.name", &self.client.name)?;
        validate_identifier("client.version", &self.client.version)?;

        #[cfg(test)]
        crate::control_rpc::test_support::notify_argument_parse_probe(&self.request_id);

        let operation = match self.operation {
            ControlOperationKind::DescribeInstance => {
                parse_arguments::<DescribeInstanceArguments>(&self.arguments)?;
                ControlOperation::DescribeInstance
            }
            ControlOperationKind::GetStatus => {
                parse_arguments::<GetStatusArguments>(&self.arguments)?;
                ControlOperation::GetStatus
            }
            ControlOperationKind::SetRecordingEnabled => {
                let arguments = parse_arguments::<SetRecordingEnabledArguments>(&self.arguments)?;
                ControlOperation::SetRecordingEnabled {
                    enabled: arguments.enabled,
                }
            }
            ControlOperationKind::SearchCaptures => {
                let arguments = parse_arguments::<SearchCapturesArguments>(&self.arguments)?;
                ControlOperation::SearchCaptures {
                    query: Box::new(arguments.query),
                    cursor: arguments.cursor,
                    limit: arguments.limit,
                }
            }
            ControlOperationKind::GetCapture => {
                let arguments = parse_arguments::<GetCaptureArguments>(&self.arguments)?;
                ControlOperation::GetCapture {
                    capture_id: arguments.capture_id,
                    expected_revision: arguments.expected_revision,
                }
            }
            ControlOperationKind::WaitForCapture => ControlOperation::WaitForCapture(Box::new(
                parse_arguments::<WaitForCaptureRequest>(&self.arguments)?,
            )),
            ControlOperationKind::ReadCaptureBody => {
                let request = parse_arguments::<BodyContentRequest>(&self.arguments)?;
                request.validate()?;
                ControlOperation::ReadCaptureBody(Box::new(request))
            }
            ControlOperationKind::SearchCaptureBody => {
                let request = parse_arguments::<SearchCaptureBodyRequest>(&self.arguments)?;
                request.validate()?;
                ControlOperation::SearchCaptureBody(Box::new(request))
            }
            ControlOperationKind::ExtractCaptureBody => {
                let request = parse_arguments::<ExtractCaptureBodyRequest>(&self.arguments)?;
                request.validate()?;
                ControlOperation::ExtractCaptureBody(Box::new(request))
            }
            ControlOperationKind::ReadSelectedBody => {
                let request = parse_arguments::<SelectionContentRequest>(&self.arguments)?;
                request.validate()?;
                ControlOperation::ReadSelectedBody(Box::new(request))
            }
            ControlOperationKind::FindJsonPointers => {
                let request = parse_arguments::<FindJsonPointersRequest>(&self.arguments)?;
                request.validate()?;
                ControlOperation::FindJsonPointers(Box::new(request))
            }
            ControlOperationKind::ProbeJsonPointerPattern => {
                let request = parse_arguments::<ProbeJsonPointerPatternRequest>(&self.arguments)?;
                request.validate()?;
                ControlOperation::ProbeJsonPointerPattern(Box::new(request))
            }
            ControlOperationKind::GetMappingSettings => {
                let scope = parse_arguments::<MappingReadScope>(&self.arguments)?;
                ControlOperation::GetMappingSettings { scope }
            }
            ControlOperationKind::ValidateMappingSettings => {
                let arguments =
                    parse_arguments::<ValidateMappingSettingsArguments>(&self.arguments)?;
                ControlOperation::ValidateMappingSettings {
                    proxy: Box::new(arguments.proxy.into()),
                }
            }
            ControlOperationKind::ExplainMapping => {
                let arguments = parse_arguments::<ExplainMappingArguments>(&self.arguments)?;
                ControlOperation::ExplainMapping {
                    url: arguments.url,
                    proposed_proxy: arguments.proposed_proxy.map(|proxy| Box::new(proxy.into())),
                }
            }
            ControlOperationKind::PreviewMappingMutation => {
                let arguments =
                    parse_arguments::<PreviewMappingMutationArguments>(&self.arguments)?;
                validate_preview_urls(&arguments.urls)?;
                ControlOperation::PreviewMappingMutation {
                    expected_revision: arguments.expected_revision,
                    mutation: Box::new(arguments.mutation.into()),
                    urls: arguments.urls,
                }
            }
            ControlOperationKind::MutateMapping => {
                let arguments = parse_arguments::<MutateMappingArguments>(&self.arguments)?;
                ControlOperation::MutateMapping {
                    expected_revision: arguments.expected_revision,
                    mutation: Box::new(arguments.mutation.into()),
                }
            }
        };
        let deadline = self.clamped_deadline(received_at);
        Ok(ControlRequest {
            request_id: self.request_id,
            run_id: self.run_id,
            deadline,
            client: self.client,
            operation,
        })
    }
}

impl ResponseEnvelope {
    pub(crate) fn success(request_id: String, result: ControlResult) -> Self {
        Self {
            protocol_version: RPC_VERSION,
            request_id,
            result: Some(result),
            error: None,
        }
    }

    pub(crate) fn error(request_id: String, error: ControlError) -> Self {
        Self {
            protocol_version: RPC_VERSION,
            request_id,
            result: None,
            error: Some(error),
        }
    }

    pub(crate) fn validate(
        self,
        expected_request_id: &str,
        expected_scope: &InstanceScope,
        expected_kind: ControlOperationKind,
    ) -> Result<ControlResult, ControlError> {
        if self.protocol_version != RPC_VERSION {
            return Err(ControlError::new(
                ControlErrorCode::RpcVersionMismatch,
                "private RPC response protocol version does not match",
                false,
                serde_json::json!({
                    "expected": RPC_VERSION,
                    "received": self.protocol_version,
                }),
            ));
        }
        validate_identifier("request_id", &self.request_id)?;
        if self.request_id != expected_request_id {
            return Err(ControlError::invalid_argument(
                "private RPC response request ID does not match",
            ));
        }
        match (self.result, self.error) {
            (Some(result), None) => {
                if result.kind() != expected_kind {
                    return Err(ControlError::invalid_argument(
                        "private RPC response operation does not match the request",
                    ));
                }
                if let ControlResult::WaitForCapture { result, .. } = &result
                    && result.matched != result.capture.is_some()
                {
                    return Err(ControlError::invalid_argument(
                        "wait result matched and capture fields must agree",
                    ));
                }
                if result.instance_scope() != expected_scope {
                    return Err(ControlError::new(
                        ControlErrorCode::InstanceGenerationConflict,
                        "private RPC response belongs to another instance generation",
                        false,
                        serde_json::json!({
                            "expected_identity": expected_scope,
                            "received_identity": result.instance_scope(),
                        }),
                    ));
                }
                Ok(result)
            }
            (None, Some(error)) => Err(error),
            _ => Err(ControlError::invalid_argument(
                "private RPC response must contain exactly one result or error",
            )),
        }
    }
}

#[cfg(test)]
pub(crate) fn decode_request_payload(
    payload: &[u8],
    received_at: Instant,
) -> Result<ControlRequest, ControlError> {
    strict_from_slice::<RequestEnvelope>(payload)?.validate(received_at)
}

#[cfg(test)]
pub(crate) fn decode_response_payload(
    payload: &[u8],
    expected_request_id: &str,
    expected_scope: &InstanceScope,
    expected_kind: ControlOperationKind,
) -> Result<ControlResult, ControlError> {
    strict_from_slice::<ResponseEnvelope>(payload)?.validate(
        expected_request_id,
        expected_scope,
        expected_kind,
    )
}

pub(crate) fn strict_from_slice<T>(payload: &[u8]) -> Result<T, ControlError>
where
    T: DeserializeOwned,
{
    let mut deserializer = serde_json::Deserializer::from_slice(payload);
    let value = T::deserialize(&mut deserializer)
        .map_err(|error| ControlError::invalid_argument(error.to_string()))?;
    deserializer
        .end()
        .map_err(|error| ControlError::invalid_argument(error.to_string()))?;
    Ok(value)
}

#[cfg(test)]
fn serialize_arguments<T>(arguments: &T) -> Result<Box<RawValue>, ControlError>
where
    T: Serialize,
{
    let value = serde_json::to_string(arguments)
        .map_err(|error| ControlError::invalid_argument(error.to_string()))?;
    RawValue::from_string(value).map_err(|error| ControlError::invalid_argument(error.to_string()))
}

fn parse_arguments<T>(arguments: &RawValue) -> Result<T, ControlError>
where
    T: DeserializeOwned,
{
    strict_from_slice(arguments.get().as_bytes())
}

fn validate_identifier(field: &str, value: &str) -> Result<(), ControlError> {
    if value.is_empty() || value.len() > MAX_IDENTIFIER_BYTES {
        return Err(ControlError::new(
            ControlErrorCode::InvalidArgument,
            "private RPC identifier must contain between 1 and 128 UTF-8 bytes",
            false,
            serde_json::json!({"field": field, "max_bytes": MAX_IDENTIFIER_BYTES}),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod capture_wait_tests;

#[cfg(test)]
mod body_read_tests;
#[cfg(test)]
mod mapping_tests;
#[cfg(test)]
mod search_extract_tests;

#[cfg(test)]
mod mapping_preview_tests;
