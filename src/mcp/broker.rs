use std::{
    future::Future,
    io,
    net::SocketAddr,
    path::{Path, PathBuf},
    pin::Pin,
    sync::Arc,
    time::Duration,
};

use futures::{StreamExt, stream::FuturesUnordered};
use rmcp::{
    ErrorData, RoleServer, ServerHandler,
    handler::server::{
        router::tool::ToolRouter,
        tool::schema_for_type,
        wrapper::{Json, Parameters},
    },
    model::{
        ErrorCode, GetPromptRequestParams, GetPromptResponse, Implementation, ListPromptsResult,
        ListResourceTemplatesResult, ListResourcesResult, ProtocolVersion,
        ReadResourceRequestParams, ReadResourceResponse, ReadResourceResult, ResourceTemplate,
        ServerCapabilities, ServerInfo,
    },
    service::RequestContext,
    tool, tool_handler, tool_router,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use crate::{
    capture::BodySide,
    control::{
        body::{BodyRepresentation, DEFAULT_BODY_PAGE_LENGTH},
        normalize_wait_timeout_ms,
    },
    control_rpc::{
        client::ControlRpcClient,
        framing::{REQUEST_MAX_BYTES, RESPONSE_MAX_BYTES, ensure_json_payload_within_limit},
        protocol::{
            ControlError, ControlErrorCode, ControlOperation, ControlResult, DeclaredClient,
            RPC_VERSION,
        },
    },
    instance::RunId,
    instance_registry::{DiscoveryDiagnostic, InstanceDescriptor, RegistryScan, RegistryScanner},
};

use super::{
    body::{
        BodyResourceUri, CONTENT_RESOURCE_TEMPLATE, ExtractCaptureBodyInput,
        ExtractCaptureBodyOutput, FORM_FIELD_RESOURCE_TEMPLATE, FindJsonPointersInput,
        FindJsonPointersOutput, JSON_POINTER_RESOURCE_TEMPLATE, ProbeJsonPointerPatternInput,
        ProbeJsonPointerPatternOutput, SearchCaptureBodyInput, SearchCaptureBodyOutput,
        SelectionResourceUri, body_page_resource_contents, selection_page_resource_contents,
    },
    capture::{
        BodyRepresentationLinks, CaptureBodyResources, GetCaptureInput, GetCaptureResult,
        SearchCapturesInput, SearchCapturesResult, SetRecordingEnabledInput,
        SetRecordingEnabledResult, WaitForCaptureInput, WaitForCaptureResult, recording_result,
        search_result, wait_result,
    },
    mapping::{
        CreateMappingRuleInput, CreatePresetInput, DeleteMappingRuleInput, DeletePresetInput,
        ExplainMappingInput, ExplainMappingResult, GetMappingSettingsInput,
        GetMappingSettingsResult, MappingMutationOutput, MappingToolInput, MoveMappingRuleInput,
        PreviewMappingMutationInput, PreviewMappingMutationResult, RenamePresetInput,
        SetActivePresetInput, SetMappingGateInput, SetMappingRuleEnabledInput,
        UpdateMappingRuleInput, ValidateMappingSettingsInput, ValidateMappingSettingsResult,
        explain_mapping_result, get_mapping_result, mapping_mutation_result,
        preview_mapping_result, validate_mapping_result,
    },
    schema::{
        BrokerLimits, BrokerStatusResult, BrokerVersions, DiscoverySummary, GetStatusInput,
        GetStatusResult, InstanceSelector, InstanceSummary, ListInstancesResult, RegistryStatus,
        RejectedDescriptor, StatusWarning, TransportStatus,
    },
    telemetry::{ActivityGuard, BrokerTelemetry},
};

pub(super) const PUBLIC_CALL_LIMIT: usize = 32;
const BLOCKING_SCAN_LIMIT: usize = 32;
const MAPPING_CONVERSION_LIMIT: usize = 2;
pub(super) const LIVENESS_PROBE_LIMIT: usize = 16;
const AMBIGUOUS_INSTANCE_LIMIT: usize = 16;
const ORDINARY_DEADLINE: Duration = Duration::from_secs(30);
const STALE_CLEANUP_MAX_WAIT: Duration = Duration::from_millis(250);
const CLIENT_IDENTIFIER_LIMIT: usize = 128;

pub(crate) type McpDomainError = ControlError;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SelectorRequirement {
    SnapshotRead,
    Mutation,
    Wait,
    TargetedBody,
    Resource,
}

impl SelectorRequirement {
    fn requires_run_id(self) -> bool {
        !matches!(self, Self::SnapshotRead)
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ResolvedInstance {
    pub(crate) descriptor: InstanceDescriptor,
    pub(crate) description: ControlResult,
}

#[derive(Debug)]
pub(crate) struct LiveDiscoveryReport {
    pub(crate) live_instances: Vec<ResolvedInstance>,
    pub(crate) stale_count: usize,
    pub(crate) rejected: Vec<DiscoveryDiagnostic>,
    pub(crate) omitted: usize,
}

pub(crate) trait RegistryAccess: Send + Sync + 'static {
    fn scan_all(&self) -> io::Result<RegistryScan>;
    fn read_endpoint(&self, endpoint: SocketAddr) -> io::Result<RegistryScan>;
    fn prune_batch_if_current(
        &self,
        descriptors: &[InstanceDescriptor],
        deadline: Instant,
        cancelled: &CancellationToken,
    ) -> io::Result<usize>;
}

impl RegistryAccess for RegistryScanner {
    fn scan_all(&self) -> io::Result<RegistryScan> {
        RegistryScanner::scan_all(self)
    }

    fn read_endpoint(&self, endpoint: SocketAddr) -> io::Result<RegistryScan> {
        RegistryScanner::read_endpoint(self, endpoint)
    }

    fn prune_batch_if_current(
        &self,
        descriptors: &[InstanceDescriptor],
        deadline: Instant,
        cancelled: &CancellationToken,
    ) -> io::Result<usize> {
        self.remove_stale_batch_if_current(descriptors, deadline.into_std(), cancelled, || {})
    }
}

pub(crate) trait InstanceProbe: Send + Sync + 'static {
    fn describe<'a>(
        &'a self,
        descriptor: &'a InstanceDescriptor,
        client: DeclaredClient,
        deadline: Instant,
        cancelled: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = Result<ControlResult, ControlError>> + Send + 'a>>;

    fn call<'a>(
        &'a self,
        descriptor: &'a InstanceDescriptor,
        operation: ControlOperation,
        client: DeclaredClient,
        deadline: Instant,
        cancelled: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = Result<ControlResult, ControlError>> + Send + 'a>> {
        Box::pin(async move {
            match operation {
                ControlOperation::DescribeInstance => {
                    self.describe(descriptor, client, deadline, cancelled).await
                }
                ControlOperation::GetStatus => {
                    match self
                        .describe(descriptor, client, deadline, cancelled)
                        .await?
                    {
                        ControlResult::DescribeInstance {
                            instance,
                            config_mode,
                            persistence,
                            recording_enabled,
                            retained_capture_count,
                            settings_revision,
                        } => Ok(ControlResult::GetStatus {
                            local_proxy_url: format!("http://{}", instance.proxy_endpoint),
                            fluxcope_version: env!("CARGO_PKG_VERSION").to_owned(),
                            rpc_version: RPC_VERSION,
                            config_source: descriptor
                                .config_source()
                                .map(std::path::Path::to_path_buf),
                            instance,
                            config_mode,
                            persistence,
                            recording_enabled,
                            retained_capture_count,
                            settings_revision,
                            mapping: Default::default(),
                            capture_store: Default::default(),
                            capture_change_epoch: 0,
                            metrics: Default::default(),
                            private_rpc: Default::default(),
                            body_work: Default::default(),
                            search_work: Default::default(),
                            audit: Default::default(),
                        }),
                        _ => Err(ControlError::internal(
                            "instance description returned an unexpected result",
                        )),
                    }
                }
                _ => Err(ControlError::service_unavailable(
                    "instance probe does not implement control calls",
                )),
            }
        })
    }
}

#[derive(Debug)]
struct ControlRpcProbe;

impl InstanceProbe for ControlRpcProbe {
    fn describe<'a>(
        &'a self,
        descriptor: &'a InstanceDescriptor,
        client: DeclaredClient,
        deadline: Instant,
        cancelled: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = Result<ControlResult, ControlError>> + Send + 'a>> {
        Box::pin(async move {
            ControlRpcClient::call(
                descriptor,
                ControlOperation::DescribeInstance,
                deadline.into_std(),
                client,
                cancelled,
            )
            .await
        })
    }

    fn call<'a>(
        &'a self,
        descriptor: &'a InstanceDescriptor,
        operation: ControlOperation,
        client: DeclaredClient,
        deadline: Instant,
        cancelled: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = Result<ControlResult, ControlError>> + Send + 'a>> {
        Box::pin(async move {
            ControlRpcClient::call(
                descriptor,
                operation,
                deadline.into_std(),
                client,
                cancelled,
            )
            .await
        })
    }
}

pub(crate) struct PublicCallPermit {
    _permit: OwnedSemaphorePermit,
    _activity: ActivityGuard,
}

impl std::fmt::Debug for PublicCallPermit {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PublicCallPermit")
            .finish_non_exhaustive()
    }
}
async fn validate_public_search_input(
    input: SearchCapturesInput,
    permit: PublicCallPermit,
    deadline: Instant,
    cancelled: CancellationToken,
) -> Result<(SearchCapturesInput, PublicCallPermit), ControlError> {
    run_public_search_validation(permit, deadline, cancelled, move || {
        input.validate()?;
        Ok(input)
    })
    .await
}

async fn run_public_search_validation<F>(
    permit: PublicCallPermit,
    deadline: Instant,
    cancelled: CancellationToken,
    validate: F,
) -> Result<(SearchCapturesInput, PublicCallPermit), ControlError>
where
    F: FnOnce() -> Result<SearchCapturesInput, ControlError> + Send + 'static,
{
    let worker = tokio::task::spawn_blocking(move || (permit, validate()));
    tokio::select! {
        result = worker => {
            let (permit, input) = result
                .map_err(|_| ControlError::internal("capture search validation worker failed"))?;
            input.map(|input| (input, permit))
        }
        _ = tokio::time::sleep_until(deadline) => {
            Err(ControlError::deadline_exceeded(
                "capture search validation deadline elapsed",
            ))
        }
        _ = cancelled.cancelled() => {
            Err(ControlError::cancelled("capture search validation cancelled"))
        }
    }
}

async fn validate_public_wait_input(
    input: WaitForCaptureInput,
    permit: PublicCallPermit,
    deadline: Instant,
    cancelled: CancellationToken,
) -> Result<(WaitForCaptureInput, PublicCallPermit), ControlError> {
    let worker = tokio::task::spawn_blocking(move || (permit, input.normalize()));
    tokio::select! {
        biased;
        _ = cancelled.cancelled() => {
            Err(ControlError::cancelled("capture wait validation cancelled"))
        }
        _ = tokio::time::sleep_until(deadline) => {
            Err(ControlError::deadline_exceeded(
                "capture wait validation deadline elapsed",
            ))
        }
        result = worker => {
            let (permit, input) = result
                .map_err(|_| ControlError::internal("capture wait validation worker failed"))?;
            input.map(|input| (input, permit))
        }
    }
}

#[derive(Clone)]
pub(crate) struct Broker {
    tool_router: ToolRouter<Self>,
    registry_path: PathBuf,
    registry: Arc<dyn RegistryAccess>,
    probe: Arc<dyn InstanceProbe>,
    call_admission: Arc<Semaphore>,
    probe_admission: Arc<Semaphore>,
    scan_admission: Arc<Semaphore>,
    mapping_conversion_admission: Arc<Semaphore>,
    telemetry: Arc<BrokerTelemetry>,
}

#[cfg(test)]
impl Broker {
    async fn serve<T, E, A>(
        self,
        transport: T,
    ) -> Result<
        rmcp::service::RunningService<RoleServer, Self>,
        Box<rmcp::service::ServerInitializeError>,
    >
    where
        T: rmcp::transport::IntoTransport<RoleServer, E, A>,
        E: std::error::Error + Send + Sync + 'static,
    {
        let cancelled = CancellationToken::new();
        let transport = super::stdio::cancel_on_disconnect(
            rmcp::transport::IntoTransport::into_transport(transport),
            cancelled.clone(),
        );
        rmcp::ServiceExt::serve_with_ct(self, transport, cancelled)
            .await
            .map_err(Box::new)
    }
}

impl std::fmt::Debug for Broker {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Broker")
            .field("registry_path", &self.registry_path)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct EmptyToolInput {}

fn empty_tool_schema() -> Arc<rmcp::model::JsonObject> {
    let mut schema = (*schema_for_type::<EmptyToolInput>()).clone();
    schema.insert("properties".to_owned(), json!({}));
    schema.insert("additionalProperties".to_owned(), json!(false));
    Arc::new(schema)
}

#[tool_router(router = tool_router)]
impl Broker {
    pub(crate) fn new(fluxcope_home: &Path) -> io::Result<Self> {
        let registry = Arc::new(RegistryScanner::initialize(fluxcope_home)?);
        Ok(Self::with_dependencies(
            fluxcope_home.join("run").join("instances"),
            registry,
            Arc::new(ControlRpcProbe),
        ))
    }

    pub(crate) fn with_dependencies<R, P>(
        registry_path: PathBuf,
        registry: Arc<R>,
        probe: Arc<P>,
    ) -> Self
    where
        R: RegistryAccess,
        P: InstanceProbe,
    {
        Self {
            tool_router: Self::tool_router(),
            registry_path,
            registry,
            probe,
            call_admission: Arc::new(Semaphore::new(PUBLIC_CALL_LIMIT)),
            probe_admission: Arc::new(Semaphore::new(LIVENESS_PROBE_LIMIT)),
            scan_admission: Arc::new(Semaphore::new(BLOCKING_SCAN_LIMIT)),
            mapping_conversion_admission: Arc::new(Semaphore::new(MAPPING_CONVERSION_LIMIT)),
            telemetry: Arc::new(BrokerTelemetry::default()),
        }
    }

    pub(crate) fn try_admit_public_call(&self) -> Result<PublicCallPermit, McpDomainError> {
        let permit = Arc::clone(&self.call_admission)
            .try_acquire_owned()
            .map_err(|_| {
                self.telemetry.record_saturated_call();
                ControlError::service_unavailable("broker public call limit reached")
            })?;
        Ok(PublicCallPermit {
            _permit: permit,
            _activity: Arc::clone(&self.telemetry).begin_public_call(),
        })
    }

    pub(crate) async fn discover(
        &self,
        client: DeclaredClient,
        deadline: Instant,
        cancelled: CancellationToken,
    ) -> Result<LiveDiscoveryReport, McpDomainError> {
        let scan = self.scan(None, deadline, cancelled.clone()).await?;
        self.probe_scan(scan, client, deadline, cancelled).await
    }

    #[cfg(test)]
    pub(crate) async fn resolve(
        &self,
        selector: InstanceSelector,
        requirement: SelectorRequirement,
        deadline: Instant,
        cancelled: CancellationToken,
    ) -> Result<ResolvedInstance, McpDomainError> {
        self.resolve_with_client(
            selector,
            requirement,
            DeclaredClient {
                name: "fluxcope-internal".to_owned(),
                version: env!("CARGO_PKG_VERSION").to_owned(),
            },
            deadline,
            cancelled,
        )
        .await
    }

    async fn resolve_with_client(
        &self,
        selector: InstanceSelector,
        requirement: SelectorRequirement,
        client: DeclaredClient,
        deadline: Instant,
        cancelled: CancellationToken,
    ) -> Result<ResolvedInstance, McpDomainError> {
        if requirement.requires_run_id() && selector.run_id.is_none() {
            return Err(ControlError::invalid_argument(
                "run_id is required for this operation",
            ));
        }
        let endpoint = selector.proxy_endpoint;
        let mut scan = self.scan(endpoint, deadline, cancelled.clone()).await?;
        if let Some(run_id) = selector.run_id.as_ref() {
            if let Some(endpoint) = endpoint {
                if let Some(current) = scan.candidates.first()
                    && current.run_id() != run_id
                {
                    return Err(ControlError::new(
                        ControlErrorCode::InstanceGenerationConflict,
                        "selected endpoint belongs to a different run",
                        false,
                        json!({
                            "proxy_endpoint": endpoint,
                            "requested_run_id": run_id,
                            "current_run_id": current.run_id(),
                        }),
                    ));
                }
            } else {
                scan.candidates
                    .retain(|descriptor| descriptor.run_id() == run_id);
            }
        }
        let report = self.probe_scan(scan, client, deadline, cancelled).await?;
        self.select(selector, report, deadline)
    }

    async fn call_instance(
        &self,
        descriptor: &InstanceDescriptor,
        operation: ControlOperation,
        client: DeclaredClient,
        deadline: Instant,
        cancelled: CancellationToken,
    ) -> Result<ControlResult, ControlError> {
        let result = self
            .probe
            .call(descriptor, operation, client, deadline, cancelled)
            .await;
        if let Err(error) = &result {
            if error.code() == ControlErrorCode::Cancelled {
                self.telemetry.record_cancelled_call();
            } else if error.is_definitive_stale_connect() {
                self.telemetry.record_connection_failure();
            }
        }
        result
    }

    fn select(
        &self,
        selector: InstanceSelector,
        report: LiveDiscoveryReport,
        deadline: Instant,
    ) -> Result<ResolvedInstance, McpDomainError> {
        if let Some(endpoint) = selector.proxy_endpoint {
            let Some(current) = report.live_instances.into_iter().next() else {
                if Instant::now() >= deadline {
                    return Err(ControlError::deadline_exceeded(
                        "instance resolution deadline elapsed",
                    ));
                }
                return if report.stale_count > 0 {
                    Err(ControlError::instance_unavailable(
                        "selected instance is unavailable",
                    ))
                } else {
                    Err(ControlError::instance_not_found(
                        "selected endpoint is not registered",
                    ))
                };
            };
            if let Some(run_id) = selector.run_id
                && run_id != *current.descriptor.run_id()
            {
                return Err(ControlError::new(
                    ControlErrorCode::InstanceGenerationConflict,
                    "selected endpoint belongs to a different run",
                    false,
                    json!({
                        "proxy_endpoint": endpoint,
                        "requested_run_id": run_id,
                        "current_run_id": current.descriptor.run_id(),
                    }),
                ));
            }
            return Ok(current);
        }

        if let Some(run_id) = selector.run_id {
            let mut matching = report
                .live_instances
                .into_iter()
                .filter(|instance| instance.descriptor.run_id() == &run_id);
            let Some(selected) = matching.next() else {
                return Err(ControlError::instance_not_found(
                    "selected run is not registered",
                ));
            };
            if matching.next().is_some() {
                return Err(ControlError::instance_required(
                    "run ID is not unique; proxy_endpoint is required",
                    json!({"run_id": run_id}),
                ));
            }
            return Ok(selected);
        }

        match report.live_instances.len() {
            0 => Err(ControlError::no_instances()),
            1 => Ok(report
                .live_instances
                .into_iter()
                .next()
                .expect("singleton report contains one instance")),
            _ => {
                let instances = report
                    .live_instances
                    .iter()
                    .take(AMBIGUOUS_INSTANCE_LIMIT)
                    .map(|instance| {
                        json!({
                            "proxy_endpoint": instance.descriptor.proxy_endpoint(),
                            "run_id": instance.descriptor.run_id(),
                        })
                    })
                    .collect::<Vec<_>>();
                Err(ControlError::instance_required(
                    "multiple live instances require explicit selection",
                    json!({"instances": instances}),
                ))
            }
        }
    }

    async fn scan(
        &self,
        endpoint: Option<SocketAddr>,
        deadline: Instant,
        cancelled: CancellationToken,
    ) -> Result<RegistryScan, McpDomainError> {
        let permit = tokio::select! {
            biased;
            () = cancelled.cancelled() => {
                self.telemetry.record_cancelled_call();
                return Err(ControlError::cancelled("registry scan admission cancelled"));
            }
            () = tokio::time::sleep_until(deadline) => {
                return Err(ControlError::deadline_exceeded("registry scan admission deadline elapsed"));
            }
            permit = Arc::clone(&self.scan_admission).acquire_owned() => permit
                .map_err(|_| ControlError::service_unavailable("registry scan admission closed"))?,
        };
        let registry = Arc::clone(&self.registry);
        let task = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            match endpoint {
                Some(endpoint) => registry.read_endpoint(endpoint),
                None => registry.scan_all(),
            }
        });
        tokio::select! {
            biased;
            () = cancelled.cancelled() => {
                self.telemetry.record_cancelled_call();
                Err(ControlError::cancelled("registry scan cancelled"))
            }
            () = tokio::time::sleep_until(deadline) => {
                Err(ControlError::deadline_exceeded("registry scan deadline elapsed"))
            }
            result = task => result
                .map_err(|_| ControlError::internal("registry scan task failed"))?
                .map_err(|_| ControlError::instance_unavailable("registry scan failed")),
        }
    }

    async fn probe_scan(
        &self,
        mut scan: RegistryScan,
        client: DeclaredClient,
        deadline: Instant,
        cancelled: CancellationToken,
    ) -> Result<LiveDiscoveryReport, McpDomainError> {
        scan.candidates.sort_by(|left, right| {
            left.proxy_endpoint()
                .cmp(&right.proxy_endpoint())
                .then_with(|| left.run_id().as_str().cmp(right.run_id().as_str()))
        });
        let admission = Arc::clone(&self.probe_admission);
        let probe = Arc::clone(&self.probe);
        let telemetry = Arc::clone(&self.telemetry);
        let make_probe = |descriptor: InstanceDescriptor| {
            let admission = Arc::clone(&admission);
            let probe = Arc::clone(&probe);
            let telemetry = Arc::clone(&telemetry);
            let client = client.clone();
            let cancelled = cancelled.clone();
            async move {
                let permit = tokio::select! {
                    biased;
                    () = cancelled.cancelled() => {
                        return Err((descriptor, ControlError::cancelled("probe admission cancelled")));
                    }
                    () = tokio::time::sleep_until(deadline) => {
                        return Err((descriptor, ControlError::deadline_exceeded("probe admission deadline elapsed")));
                    }
                    permit = admission.acquire_owned() => permit,
                };
                let permit = match permit {
                    Ok(permit) => permit,
                    Err(_) => {
                        return Err((
                            descriptor,
                            ControlError::service_unavailable("probe admission closed"),
                        ));
                    }
                };
                let _permit = permit;
                let _activity = Arc::clone(&telemetry).begin_liveness_probe();
                match probe
                    .describe(&descriptor, client, deadline, cancelled)
                    .await
                {
                    Ok(result) => Ok((descriptor, result)),
                    Err(error) => Err((descriptor, error)),
                }
            }
        };
        let mut candidates = scan.candidates.into_iter();
        let mut probes = FuturesUnordered::new();
        for descriptor in candidates.by_ref().take(LIVENESS_PROBE_LIMIT) {
            probes.push(make_probe(descriptor));
        }

        let mut live_instances = Vec::new();
        let mut stale = Vec::new();
        let mut rejected = scan.rejected;
        while let Some(result) = probes.next().await {
            match result {
                Ok((descriptor, description))
                    if matches!(description, ControlResult::DescribeInstance { .. })
                        && description.instance_scope().proxy_endpoint
                            == descriptor.proxy_endpoint()
                        && description.instance_scope().run_id == *descriptor.run_id() =>
                {
                    live_instances.push(ResolvedInstance {
                        descriptor,
                        description,
                    });
                }
                Ok((_descriptor, _description)) => {
                    rejected.push(DiscoveryDiagnostic::from_parts(
                        "probe_identity_mismatch",
                        "DescribeInstance identity does not match its descriptor",
                    ));
                }
                Err((_descriptor, error)) if error.code() == ControlErrorCode::Cancelled => {
                    self.telemetry.record_cancelled_call();
                    return Err(error);
                }
                Err((descriptor, error)) if error.is_definitive_stale_connect() => {
                    self.telemetry.record_connection_failure();
                    stale.push(descriptor);
                }
                Err((_descriptor, error)) if error.code() == ControlErrorCode::DeadlineExceeded => {
                    return Err(error);
                }
                Err((_descriptor, error)) => {
                    rejected.push(DiscoveryDiagnostic::from_parts(
                        error.code().as_str(),
                        error.message(),
                    ));
                }
            }
            if let Some(descriptor) = candidates.next() {
                probes.push(make_probe(descriptor));
            }
        }
        stale.sort_by(|left, right| {
            left.proxy_endpoint()
                .cmp(&right.proxy_endpoint())
                .then_with(|| left.run_id().as_str().cmp(right.run_id().as_str()))
                .then_with(|| left.socket_path().cmp(right.socket_path()))
        });
        stale.dedup_by(|left, right| {
            left.proxy_endpoint() == right.proxy_endpoint()
                && left.run_id() == right.run_id()
                && left.socket_path() == right.socket_path()
        });
        let stale_count = stale.len();
        if !stale.is_empty()
            && let Err(error) = self
                .prune_stale_batch(stale, deadline, cancelled.clone())
                .await
        {
            if matches!(
                error.code(),
                ControlErrorCode::Cancelled | ControlErrorCode::DeadlineExceeded
            ) {
                return Err(error);
            }
            rejected.push(DiscoveryDiagnostic::from_parts(
                "stale_cleanup_failed",
                error.message(),
            ));
        }
        live_instances.sort_by(|left, right| {
            left.descriptor
                .proxy_endpoint()
                .cmp(&right.descriptor.proxy_endpoint())
                .then_with(|| {
                    left.descriptor
                        .run_id()
                        .as_str()
                        .cmp(right.descriptor.run_id().as_str())
                })
        });
        rejected.sort_by(|left, right| {
            left.code()
                .cmp(right.code())
                .then_with(|| left.message().cmp(right.message()))
        });
        Ok(LiveDiscoveryReport {
            live_instances,
            stale_count,
            rejected,
            omitted: scan.omitted,
        })
    }

    async fn prune_stale_batch(
        &self,
        descriptors: Vec<InstanceDescriptor>,
        deadline: Instant,
        cancelled: CancellationToken,
    ) -> Result<(), McpDomainError> {
        let permit = tokio::select! {
            biased;
            () = cancelled.cancelled() => {
                self.telemetry.record_cancelled_call();
                return Err(ControlError::cancelled("stale cleanup admission cancelled"));
            }
            () = tokio::time::sleep_until(deadline) => {
                return Err(ControlError::deadline_exceeded("stale cleanup admission deadline elapsed"));
            }
            permit = Arc::clone(&self.scan_admission).acquire_owned() => permit
                .map_err(|_| ControlError::service_unavailable("stale cleanup admission closed"))?,
        };
        let registry = Arc::clone(&self.registry);
        let cleanup_cancelled = cancelled.clone();
        let cleanup_limit = Instant::now() + STALE_CLEANUP_MAX_WAIT;
        let cleanup_deadline = deadline.min(cleanup_limit);
        let cleanup_uses_call_deadline = deadline <= cleanup_limit;
        let task = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            registry.prune_batch_if_current(&descriptors, cleanup_deadline, &cleanup_cancelled)
        });
        tokio::select! {
            biased;
            () = cancelled.cancelled() => {
                self.telemetry.record_cancelled_call();
                Err(ControlError::cancelled("stale cleanup cancelled"))
            }
            () = tokio::time::sleep_until(cleanup_deadline) => {
                if cleanup_uses_call_deadline {
                    Err(ControlError::deadline_exceeded("stale cleanup deadline elapsed"))
                } else {
                    Err(ControlError::instance_unavailable(
                        "stale cleanup mutation lock wait limit elapsed",
                    ))
                }
            }
            result = task => {
                result
                    .map_err(|_| ControlError::internal("stale cleanup task failed"))?
                    .map_err(|_| ControlError::instance_unavailable("stale cleanup failed"))?;
                Ok(())
            }
        }
    }

    pub(crate) async fn list_instances_impl(
        &self,
        client: DeclaredClient,
        deadline: Instant,
        cancelled: CancellationToken,
    ) -> Result<ListInstancesResult, McpDomainError> {
        let report = self.discover(client, deadline, cancelled).await?;
        let instances = report
            .live_instances
            .into_iter()
            .map(instance_summary)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(ListInstancesResult {
            instances,
            diagnostics: DiscoverySummary {
                stale_count: report.stale_count,
                rejected: report
                    .rejected
                    .into_iter()
                    .map(|diagnostic| RejectedDescriptor {
                        code: diagnostic.code().to_owned(),
                        message: diagnostic.message().to_owned(),
                    })
                    .collect(),
                omitted: report.omitted,
            },
        })
    }

    pub(crate) async fn get_status_impl(
        &self,
        input: GetStatusInput,
        client: DeclaredClient,
        deadline: Instant,
        cancelled: CancellationToken,
    ) -> Result<GetStatusResult, McpDomainError> {
        let resolved = self
            .resolve_with_client(
                input.instance,
                SelectorRequirement::SnapshotRead,
                client.clone(),
                deadline,
                cancelled.clone(),
            )
            .await?;
        let result = self
            .call_instance(
                &resolved.descriptor,
                ControlOperation::GetStatus,
                client,
                deadline,
                cancelled,
            )
            .await?;
        status_result(result)
    }

    pub(crate) async fn set_recording_enabled_impl(
        &self,
        input: SetRecordingEnabledInput,
        client: DeclaredClient,
        deadline: Instant,
        cancelled: CancellationToken,
    ) -> Result<SetRecordingEnabledResult, McpDomainError> {
        input.validate()?;
        let resolved = self
            .resolve_with_client(
                input.instance.selector(),
                SelectorRequirement::Mutation,
                client.clone(),
                deadline,
                cancelled.clone(),
            )
            .await?;
        let result = self
            .call_instance(
                &resolved.descriptor,
                input.operation(),
                client,
                deadline,
                cancelled,
            )
            .await?;
        recording_result(result)
    }
    async fn mapping_call(
        &self,
        instance: InstanceSelector,
        operation: ControlOperation,
        requirement: SelectorRequirement,
        client: DeclaredClient,
        deadline: Instant,
        cancelled: CancellationToken,
    ) -> Result<ControlResult, ControlError> {
        let resolved = self
            .resolve_with_client(
                instance,
                requirement,
                client.clone(),
                deadline,
                cancelled.clone(),
            )
            .await?;
        self.call_instance(&resolved.descriptor, operation, client, deadline, cancelled)
            .await
    }

    async fn run_mapping_worker<T, F>(
        &self,
        deadline: Instant,
        cancelled: CancellationToken,
        work: F,
    ) -> Result<T, ControlError>
    where
        T: Send + 'static,
        F: FnOnce() -> Result<T, ControlError> + Send + 'static,
    {
        let admission = Arc::clone(&self.mapping_conversion_admission);
        let permit = tokio::select! {
            biased;
            _ = cancelled.cancelled() => {
                return Err(ControlError::cancelled("mapping conversion was cancelled"));
            }
            _ = tokio::time::sleep_until(deadline) => {
                return Err(ControlError::deadline_exceeded(
                    "mapping conversion deadline elapsed",
                ));
            }
            permit = admission.acquire_owned() => permit.map_err(|_| {
                ControlError::service_unavailable("mapping conversion admission is closed")
            })?,
        };
        let worker = tokio::task::spawn_blocking(move || (permit, work()));
        tokio::select! {
            biased;
            _ = cancelled.cancelled() => {
                Err(ControlError::cancelled("mapping conversion was cancelled"))
            }
            _ = tokio::time::sleep_until(deadline) => {
                Err(ControlError::deadline_exceeded(
                    "mapping conversion deadline elapsed",
                ))
            }
            result = worker => {
                let (_permit, result) = result.map_err(|_| {
                    ControlError::internal("mapping conversion worker failed")
                })?;
                result
            }
        }
    }

    async fn prepare_mapping_operation<I, F>(
        &self,
        input: I,
        deadline: Instant,
        cancelled: CancellationToken,
        prepare: F,
    ) -> Result<(InstanceSelector, ControlOperation), ControlError>
    where
        I: Serialize + Send + 'static,
        F: FnOnce(I) -> (InstanceSelector, ControlOperation) + Send + 'static,
    {
        self.run_mapping_worker(deadline, cancelled, move || {
            ensure_json_payload_within_limit(&input, REQUEST_MAX_BYTES)?;
            Ok(prepare(input))
        })
        .await
    }

    async fn mapping_mutation_impl<T>(
        &self,
        input: T,
        client: DeclaredClient,
        deadline: Instant,
        cancelled: CancellationToken,
    ) -> Result<MappingMutationOutput, ControlError>
    where
        T: MappingToolInput + Serialize + Send + 'static,
    {
        let (instance, operation) = self
            .prepare_mapping_operation(input, deadline, cancelled.clone(), |input| {
                let instance = input.instance().clone().selector();
                let operation = input.into_operation();
                (instance, operation)
            })
            .await?;
        let result = self
            .mapping_call(
                instance,
                operation,
                SelectorRequirement::Mutation,
                client,
                deadline,
                cancelled,
            )
            .await?;
        mapping_mutation_result(result)
    }

    async fn get_mapping_settings_impl(
        &self,
        input: GetMappingSettingsInput,
        client: DeclaredClient,
        deadline: Instant,
        cancelled: CancellationToken,
    ) -> Result<GetMappingSettingsResult, ControlError> {
        let instance = input.instance.clone();
        let operation = input.into_operation();
        let result = self
            .mapping_call(
                instance,
                operation,
                SelectorRequirement::SnapshotRead,
                client,
                deadline,
                cancelled.clone(),
            )
            .await?;
        self.run_mapping_worker(deadline, cancelled, move || {
            let result = get_mapping_result(result)?;
            ensure_json_payload_within_limit(&result, RESPONSE_MAX_BYTES)?;
            Ok(result)
        })
        .await
    }

    async fn preview_mapping_mutation_impl(
        &self,
        input: PreviewMappingMutationInput,
        client: DeclaredClient,
        deadline: Instant,
        cancelled: CancellationToken,
    ) -> Result<PreviewMappingMutationResult, ControlError> {
        let (instance, operation) = self
            .run_mapping_worker(deadline, cancelled.clone(), move || {
                ensure_json_payload_within_limit(&input, REQUEST_MAX_BYTES)?;
                let instance = input.instance.clone().selector();
                Ok((instance, input.into_operation()?))
            })
            .await?;
        let result = self
            .mapping_call(
                instance,
                operation,
                SelectorRequirement::Mutation,
                client,
                deadline,
                cancelled.clone(),
            )
            .await?;
        self.run_mapping_worker(deadline, cancelled, move || {
            let result = preview_mapping_result(result)?;
            ensure_json_payload_within_limit(&result, RESPONSE_MAX_BYTES)?;
            Ok(result)
        })
        .await
    }

    async fn validate_mapping_settings_impl(
        &self,
        input: ValidateMappingSettingsInput,
        client: DeclaredClient,
        deadline: Instant,
        cancelled: CancellationToken,
    ) -> Result<ValidateMappingSettingsResult, ControlError> {
        let (instance, operation) = self
            .prepare_mapping_operation(input, deadline, cancelled.clone(), |input| {
                let instance = input.instance.clone();
                let operation = input.into_operation();
                (instance, operation)
            })
            .await?;
        let result = self
            .mapping_call(
                instance,
                operation,
                SelectorRequirement::SnapshotRead,
                client,
                deadline,
                cancelled,
            )
            .await?;
        validate_mapping_result(result)
    }

    async fn explain_mapping_impl(
        &self,
        input: ExplainMappingInput,
        client: DeclaredClient,
        deadline: Instant,
        cancelled: CancellationToken,
    ) -> Result<ExplainMappingResult, ControlError> {
        let (instance, operation) = self
            .prepare_mapping_operation(input, deadline, cancelled.clone(), |input| {
                let instance = input.instance.clone();
                let operation = input.into_operation();
                (instance, operation)
            })
            .await?;
        let result = self
            .mapping_call(
                instance,
                operation,
                SelectorRequirement::SnapshotRead,
                client,
                deadline,
                cancelled,
            )
            .await?;
        explain_mapping_result(result)
    }

    pub(crate) async fn search_captures_impl(
        &self,
        input: SearchCapturesInput,
        client: DeclaredClient,
        deadline: Instant,
        cancelled: CancellationToken,
    ) -> Result<SearchCapturesResult, McpDomainError> {
        let resolved = self
            .resolve_with_client(
                input.instance.clone(),
                SelectorRequirement::SnapshotRead,
                client.clone(),
                deadline,
                cancelled.clone(),
            )
            .await?;
        let result = self
            .call_instance(
                &resolved.descriptor,
                input.operation(),
                client,
                deadline,
                cancelled,
            )
            .await?;
        search_result(result)
    }

    pub(crate) async fn get_capture_impl(
        &self,
        input: GetCaptureInput,
        client: DeclaredClient,
        deadline: Instant,
        cancelled: CancellationToken,
    ) -> Result<GetCaptureResult, McpDomainError> {
        input.validate()?;
        let resolved = self
            .resolve_with_client(
                input.instance.selector(),
                SelectorRequirement::TargetedBody,
                client.clone(),
                deadline,
                cancelled.clone(),
            )
            .await?;
        let result = self
            .call_instance(
                &resolved.descriptor,
                input.operation(),
                client,
                deadline,
                cancelled,
            )
            .await?;
        let ControlResult::GetCapture { instance, capture } = result else {
            return Err(ControlError::internal(
                "private RPC returned an unexpected capture detail result",
            ));
        };
        let capture = *capture;
        if instance.proxy_endpoint != resolved.descriptor.proxy_endpoint()
            || instance.run_id != *resolved.descriptor.run_id()
            || capture.capture_sequence != input.capture_id
            || input
                .expected_revision
                .is_some_and(|revision| revision != capture.capture_revision)
        {
            return Err(ControlError::internal(
                "private RPC returned mismatched capture detail identity",
            ));
        }
        let resources = capture_body_resources(&instance, &capture);
        Ok(GetCaptureResult {
            instance: InstanceSelector {
                proxy_endpoint: Some(instance.proxy_endpoint),
                run_id: Some(instance.run_id),
            },
            capture,
            body_resources: resources,
        })
    }

    pub(crate) async fn search_capture_body_impl(
        &self,
        input: SearchCaptureBodyInput,
        client: DeclaredClient,
        deadline: Instant,
        cancelled: CancellationToken,
    ) -> Result<SearchCaptureBodyOutput, McpDomainError> {
        let operation = input.operation()?;
        let resolved = self
            .resolve_with_client(
                input.selector(),
                SelectorRequirement::TargetedBody,
                client.clone(),
                deadline,
                cancelled.clone(),
            )
            .await?;
        let result = self
            .call_instance(&resolved.descriptor, operation, client, deadline, cancelled)
            .await?;
        let ControlResult::SearchCaptureBody { instance, result } = result else {
            return Err(ControlError::internal(
                "private RPC returned an unexpected body search result",
            ));
        };
        let result = *result;
        if instance.proxy_endpoint != resolved.descriptor.proxy_endpoint()
            || instance.run_id != *resolved.descriptor.run_id()
            || result.capture_revision != input.capture_revision
        {
            return Err(ControlError::internal(
                "private RPC returned mismatched body search identity",
            ));
        }
        Ok(SearchCaptureBodyOutput {
            instance: InstanceSelector {
                proxy_endpoint: Some(instance.proxy_endpoint),
                run_id: Some(instance.run_id),
            },
            result,
        })
    }

    pub(crate) async fn extract_capture_body_impl(
        &self,
        input: ExtractCaptureBodyInput,
        client: DeclaredClient,
        deadline: Instant,
        cancelled: CancellationToken,
    ) -> Result<ExtractCaptureBodyOutput, McpDomainError> {
        let operation = input.operation()?;
        let resolved = self
            .resolve_with_client(
                input.selector(),
                SelectorRequirement::TargetedBody,
                client.clone(),
                deadline,
                cancelled.clone(),
            )
            .await?;
        let result = self
            .call_instance(&resolved.descriptor, operation, client, deadline, cancelled)
            .await?;
        let ControlResult::ExtractCaptureBody { instance, result } = result else {
            return Err(ControlError::internal(
                "private RPC returned an unexpected body extraction result",
            ));
        };
        let result = *result;
        if instance.proxy_endpoint != resolved.descriptor.proxy_endpoint()
            || instance.run_id != *resolved.descriptor.run_id()
            || result.capture_revision != input.capture_revision
        {
            return Err(ControlError::internal(
                "private RPC returned mismatched body extraction identity",
            ));
        }
        Ok(ExtractCaptureBodyOutput {
            instance: InstanceSelector {
                proxy_endpoint: Some(instance.proxy_endpoint),
                run_id: Some(instance.run_id),
            },
            result,
        })
    }

    pub(crate) async fn find_json_pointers_impl(
        &self,
        input: FindJsonPointersInput,
        client: DeclaredClient,
        deadline: Instant,
        cancelled: CancellationToken,
    ) -> Result<FindJsonPointersOutput, McpDomainError> {
        let operation = input.operation()?;
        let resolved = self
            .resolve_with_client(
                input.selector(),
                SelectorRequirement::TargetedBody,
                client.clone(),
                deadline,
                cancelled.clone(),
            )
            .await?;
        let result = self
            .call_instance(&resolved.descriptor, operation, client, deadline, cancelled)
            .await?;
        let ControlResult::FindJsonPointers { instance, result } = result else {
            return Err(ControlError::internal(
                "private RPC returned an unexpected JSON field result",
            ));
        };
        let result = *result;
        if instance.proxy_endpoint != resolved.descriptor.proxy_endpoint()
            || instance.run_id != *resolved.descriptor.run_id()
            || result.capture_revision != input.capture_revision
        {
            return Err(ControlError::internal(
                "private RPC returned mismatched JSON field identity",
            ));
        }
        Ok(FindJsonPointersOutput {
            instance: InstanceSelector {
                proxy_endpoint: Some(instance.proxy_endpoint),
                run_id: Some(instance.run_id),
            },
            result,
        })
    }

    pub(crate) async fn probe_json_pointer_pattern_impl(
        &self,
        input: ProbeJsonPointerPatternInput,
        client: DeclaredClient,
        deadline: Instant,
        cancelled: CancellationToken,
    ) -> Result<ProbeJsonPointerPatternOutput, McpDomainError> {
        let operation = input.operation()?;
        let resolved = self
            .resolve_with_client(
                input.selector(),
                SelectorRequirement::TargetedBody,
                client.clone(),
                deadline,
                cancelled.clone(),
            )
            .await?;
        let result = self
            .call_instance(&resolved.descriptor, operation, client, deadline, cancelled)
            .await?;
        let ControlResult::ProbeJsonPointerPattern { instance, result } = result else {
            return Err(ControlError::internal(
                "private RPC returned an unexpected JSON pattern result",
            ));
        };
        let result = *result;
        if instance.proxy_endpoint != resolved.descriptor.proxy_endpoint()
            || instance.run_id != *resolved.descriptor.run_id()
            || result.capture_revision != input.capture_revision
        {
            return Err(ControlError::internal(
                "private RPC returned mismatched JSON pattern identity",
            ));
        }
        Ok(ProbeJsonPointerPatternOutput {
            instance: InstanceSelector {
                proxy_endpoint: Some(instance.proxy_endpoint),
                run_id: Some(instance.run_id),
            },
            result,
        })
    }

    async fn read_body_resource_impl(
        &self,
        requested: BodyResourceUri,
        client: DeclaredClient,
        deadline: Instant,
        cancelled: CancellationToken,
    ) -> Result<ReadResourceResult, McpDomainError> {
        let selector = InstanceSelector {
            proxy_endpoint: Some(requested.proxy_endpoint()),
            run_id: Some(requested.run_id().clone()),
        };
        let resolved = self
            .resolve_with_client(
                selector,
                SelectorRequirement::Resource,
                client.clone(),
                deadline,
                cancelled.clone(),
            )
            .await?;
        let result = self
            .call_instance(
                &resolved.descriptor,
                ControlOperation::ReadCaptureBody(Box::new(requested.request())),
                client,
                deadline,
                cancelled,
            )
            .await?;
        let ControlResult::ReadCaptureBody { instance, page } = result else {
            return Err(ControlError::internal(
                "private RPC returned an unexpected capture body result",
            ));
        };
        if instance.proxy_endpoint != requested.proxy_endpoint()
            || &instance.run_id != requested.run_id()
        {
            return Err(ControlError::internal(
                "private RPC returned mismatched capture body identity",
            ));
        }
        let relayed_bytes = u64::try_from(page.content.len()).unwrap_or(u64::MAX);
        let content = body_page_resource_contents(&requested, *page)?;
        self.telemetry.record_bytes_relayed(relayed_bytes);
        Ok(ReadResourceResult::new(vec![content]))
    }

    async fn read_selection_resource_impl(
        &self,
        requested: SelectionResourceUri,
        client: DeclaredClient,
        deadline: Instant,
        cancelled: CancellationToken,
    ) -> Result<ReadResourceResult, McpDomainError> {
        let selector = InstanceSelector {
            proxy_endpoint: Some(requested.proxy_endpoint()),
            run_id: Some(requested.run_id().clone()),
        };
        let resolved = self
            .resolve_with_client(
                selector,
                SelectorRequirement::Resource,
                client.clone(),
                deadline,
                cancelled.clone(),
            )
            .await?;
        let result = self
            .call_instance(
                &resolved.descriptor,
                ControlOperation::ReadSelectedBody(Box::new(requested.request())),
                client,
                deadline,
                cancelled,
            )
            .await?;
        let ControlResult::ReadSelectedBody { instance, page } = result else {
            return Err(ControlError::internal(
                "private RPC returned an unexpected selected body result",
            ));
        };
        if instance.proxy_endpoint != requested.proxy_endpoint()
            || &instance.run_id != requested.run_id()
        {
            return Err(ControlError::internal(
                "private RPC returned mismatched selected body identity",
            ));
        }
        let relayed_bytes = u64::try_from(page.content.len()).unwrap_or(u64::MAX);
        let content = selection_page_resource_contents(&requested, *page)?;
        self.telemetry.record_bytes_relayed(relayed_bytes);
        Ok(ReadResourceResult::new(vec![content]))
    }

    pub(crate) async fn wait_for_capture_impl(
        &self,
        input: WaitForCaptureInput,
        client: DeclaredClient,
        deadline: Instant,
        cancelled: CancellationToken,
    ) -> Result<WaitForCaptureResult, McpDomainError> {
        let endpoint = input.instance.proxy_endpoint;
        let requested_run_id = input.instance.run_id.clone();
        let resolved = self
            .resolve_with_client(
                input.selector(),
                SelectorRequirement::Wait,
                client.clone(),
                deadline,
                cancelled.clone(),
            )
            .await?;
        let dispatch = self.call_instance(
            &resolved.descriptor,
            input.operation(),
            client,
            deadline,
            cancelled.clone(),
        );
        tokio::pin!(dispatch);
        let result = tokio::select! {
            biased;
            _ = cancelled.cancelled() => {
                self.telemetry.record_cancelled_call();
                return Err(ControlError::cancelled("capture wait dispatch cancelled"));
            }
            _ = tokio::time::sleep_until(deadline) => {
                return Err(ControlError::deadline_exceeded(
                    "capture wait dispatch deadline elapsed",
                ));
            }
            result = &mut dispatch => result,
        };
        match result {
            Ok(result) => wait_result(result),
            Err(error)
                if !cancelled.is_cancelled()
                    && matches!(
                        error.code,
                        ControlErrorCode::InstanceUnavailable
                            | ControlErrorCode::InstanceNotFound
                            | ControlErrorCode::ServiceUnavailable
                            | ControlErrorCode::Cancelled
                    ) =>
            {
                if let Some(conflict) = self
                    .replacement_generation_conflict(
                        endpoint,
                        &requested_run_id,
                        deadline,
                        cancelled,
                    )
                    .await
                {
                    Err(conflict)
                } else {
                    Err(error)
                }
            }
            Err(error) => Err(error),
        }
    }

    async fn replacement_generation_conflict(
        &self,
        endpoint: SocketAddr,
        requested_run_id: &RunId,
        deadline: Instant,
        cancelled: CancellationToken,
    ) -> Option<ControlError> {
        if cancelled.is_cancelled() {
            return None;
        }
        let scan = self.scan(Some(endpoint), deadline, cancelled).await.ok()?;
        let current = scan
            .candidates
            .into_iter()
            .find(|candidate| candidate.run_id() != requested_run_id)?;
        Some(ControlError::new(
            ControlErrorCode::InstanceGenerationConflict,
            "selected endpoint was replaced during capture wait",
            false,
            json!({
                "proxy_endpoint": endpoint,
                "requested_run_id": requested_run_id,
                "current_run_id": current.run_id(),
            }),
        ))
    }

    async fn get_broker_status_impl(
        &self,
        client: DeclaredClient,
        deadline: Instant,
        cancelled: CancellationToken,
    ) -> Result<BrokerStatusResult, McpDomainError> {
        let report = self.discover(client, deadline, cancelled).await?;
        let telemetry = self.telemetry.snapshot();
        let mut warnings = vec![StatusWarning {
            code: "local_unauthenticated_access".to_owned(),
            message: "MCP control is available to processes running as the current OS user"
                .to_owned(),
        }];
        if report.stale_count > 0
            || !report.rejected.is_empty()
            || report.omitted > 0
            || telemetry.connection_failures > 0
        {
            warnings.push(StatusWarning {
                code: "registry_degraded".to_owned(),
                message: "instance discovery encountered stale, rejected, omitted, or unreachable descriptors"
                    .to_owned(),
            });
        }
        if telemetry.saturated_calls > 0 || telemetry.cancelled_calls > 0 {
            warnings.push(StatusWarning {
                code: "broker_transport_limited".to_owned(),
                message: "broker calls were saturated or cancelled".to_owned(),
            });
        }
        Ok(BrokerStatusResult {
            versions: BrokerVersions {
                broker: env!("CARGO_PKG_VERSION").to_owned(),
                mcp: ProtocolVersion::LATEST.to_string(),
                rpc: RPC_VERSION,
            },
            registry: RegistryStatus {
                path: self.registry_path.clone(),
                live_instances: report.live_instances.len(),
                stale_descriptors: report.stale_count,
                rejected_descriptors: report.rejected.len(),
                omitted_descriptors: report.omitted,
                connection_failures: telemetry.connection_failures,
            },
            transport: TransportStatus {
                active_calls: telemetry.active_calls,
                saturated_calls: telemetry.saturated_calls,
                cancelled_calls: telemetry.cancelled_calls,
                active_probes: telemetry.active_probes,
                bytes_relayed: telemetry.bytes_relayed,
            },
            limits: BrokerLimits::default(),
            warnings,
        })
    }

    #[tool(
        name = "list_instances",
        description = "List live MCP-enabled Fluxcope proxy instances",
        input_schema = empty_tool_schema(),
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn list_instances(
        &self,
        context: RequestContext<RoleServer>,
    ) -> Result<Json<ListInstancesResult>, ErrorData> {
        let _call = self.try_admit_public_call().map_err(to_mcp_error)?;
        let client = declared_client(&context).map_err(to_mcp_error)?;
        self.list_instances_impl(
            client,
            Instant::now() + ORDINARY_DEADLINE,
            context.ct.clone(),
        )
        .await
        .map(Json)
        .map_err(to_mcp_error)
    }

    #[tool(
        name = "get_broker_status",
        description = "Get bounded Fluxcope broker discovery and transport status",
        input_schema = empty_tool_schema(),
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn get_broker_status(
        &self,
        context: RequestContext<RoleServer>,
    ) -> Result<Json<BrokerStatusResult>, ErrorData> {
        let _call = self.try_admit_public_call().map_err(to_mcp_error)?;
        let client = declared_client(&context).map_err(to_mcp_error)?;
        self.get_broker_status_impl(
            client,
            Instant::now() + ORDINARY_DEADLINE,
            context.ct.clone(),
        )
        .await
        .map(Json)
        .map_err(to_mcp_error)
    }

    #[tool(
        name = "get_status",
        description = "Get authoritative identity, configuration, recording, mapping, capture-store, worker-resource, metric, warning, and bounded audit status for one Fluxcope instance",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn get_status(
        &self,
        Parameters(input): Parameters<GetStatusInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<Json<GetStatusResult>, ErrorData> {
        let _call = self.try_admit_public_call().map_err(to_mcp_error)?;
        let client = declared_client(&context).map_err(to_mcp_error)?;
        self.get_status_impl(
            input,
            client,
            Instant::now() + ORDINARY_DEADLINE,
            context.ct.clone(),
        )
        .await
        .map(Json)
        .map_err(to_mcp_error)
    }

    #[tool(
        name = "set_recording_enabled",
        description = "Explicitly enable or disable live recording for one Fluxcope instance",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn set_recording_enabled(
        &self,
        Parameters(input): Parameters<SetRecordingEnabledInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<Json<SetRecordingEnabledResult>, ErrorData> {
        let _call = self.try_admit_public_call().map_err(to_mcp_error)?;
        let client = declared_client(&context).map_err(to_mcp_error)?;
        self.set_recording_enabled_impl(
            input,
            client,
            Instant::now() + ORDINARY_DEADLINE,
            context.ct.clone(),
        )
        .await
        .map(Json)
        .map_err(to_mcp_error)
    }

    #[tool(
        name = "get_mapping_settings",
        description = "Get revisioned mapping settings, optionally scoped by exact preset and remote/local table; retain all ordered rules including disabled duplicates, gates, original indexes and explicit omissions",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn get_mapping_settings(
        &self,
        Parameters(input): Parameters<GetMappingSettingsInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<Json<GetMappingSettingsResult>, ErrorData> {
        let _call = self.try_admit_public_call().map_err(to_mcp_error)?;
        let client = declared_client(&context).map_err(to_mcp_error)?;
        self.get_mapping_settings_impl(
            input,
            client,
            Instant::now() + ORDINARY_DEADLINE,
            context.ct.clone(),
        )
        .await
        .map(Json)
        .map_err(to_mcp_error)
    }

    #[tool(
        name = "validate_mapping_settings",
        description = "Validate complete proposed proxy mapping settings without mutation",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn validate_mapping_settings(
        &self,
        Parameters(input): Parameters<ValidateMappingSettingsInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<Json<ValidateMappingSettingsResult>, ErrorData> {
        let _call = self.try_admit_public_call().map_err(to_mcp_error)?;
        let client = declared_client(&context).map_err(to_mcp_error)?;
        self.validate_mapping_settings_impl(
            input,
            client,
            Instant::now() + ORDINARY_DEADLINE,
            context.ct.clone(),
        )
        .await
        .map(Json)
        .map_err(to_mcp_error)
    }

    #[tool(
        name = "explain_mapping",
        description = "Explain a current or proposed proxy mapping decision without traffic or file reads",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn explain_mapping(
        &self,
        Parameters(input): Parameters<ExplainMappingInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<Json<ExplainMappingResult>, ErrorData> {
        let _call = self.try_admit_public_call().map_err(to_mcp_error)?;
        let client = declared_client(&context).map_err(to_mcp_error)?;
        self.explain_mapping_impl(
            input,
            client,
            Instant::now() + ORDINARY_DEADLINE,
            context.ct.clone(),
        )
        .await
        .map(Json)
        .map_err(to_mcp_error)
    }

    #[tool(
        name = "preview_mapping_mutation",
        description = "Preview one typed mutation against an exact instance and expected settings revision without commit, file reads or traffic; return validation, gates, affected change and remote-then-local explanations for at most 16 URLs. Commit with the same revision using the matching named mutation tool; stale revisions and unsaved TUI drafts are rejected",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn preview_mapping_mutation(
        &self,
        Parameters(input): Parameters<PreviewMappingMutationInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<Json<PreviewMappingMutationResult>, ErrorData> {
        let _call = self.try_admit_public_call().map_err(to_mcp_error)?;
        let client = declared_client(&context).map_err(to_mcp_error)?;
        self.preview_mapping_mutation_impl(
            input,
            client,
            Instant::now() + ORDINARY_DEADLINE,
            context.ct.clone(),
        )
        .await
        .map(Json)
        .map_err(to_mcp_error)
    }

    #[tool(
        name = "create_preset",
        description = "Create a mapping preset without activating it",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    async fn create_preset(
        &self,
        Parameters(input): Parameters<CreatePresetInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<Json<MappingMutationOutput>, ErrorData> {
        let _call = self.try_admit_public_call().map_err(to_mcp_error)?;
        let client = declared_client(&context).map_err(to_mcp_error)?;
        self.mapping_mutation_impl(
            input,
            client,
            Instant::now() + ORDINARY_DEADLINE,
            context.ct.clone(),
        )
        .await
        .map(Json)
        .map_err(to_mcp_error)
    }

    #[tool(
        name = "rename_preset",
        description = "Rename a mapping preset",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    async fn rename_preset(
        &self,
        Parameters(input): Parameters<RenamePresetInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<Json<MappingMutationOutput>, ErrorData> {
        let _call = self.try_admit_public_call().map_err(to_mcp_error)?;
        let client = declared_client(&context).map_err(to_mcp_error)?;
        self.mapping_mutation_impl(
            input,
            client,
            Instant::now() + ORDINARY_DEADLINE,
            context.ct.clone(),
        )
        .await
        .map(Json)
        .map_err(to_mcp_error)
    }

    #[tool(
        name = "delete_preset",
        description = "Delete a mapping preset",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    async fn delete_preset(
        &self,
        Parameters(input): Parameters<DeletePresetInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<Json<MappingMutationOutput>, ErrorData> {
        let _call = self.try_admit_public_call().map_err(to_mcp_error)?;
        let client = declared_client(&context).map_err(to_mcp_error)?;
        self.mapping_mutation_impl(
            input,
            client,
            Instant::now() + ORDINARY_DEADLINE,
            context.ct.clone(),
        )
        .await
        .map(Json)
        .map_err(to_mcp_error)
    }

    #[tool(
        name = "set_active_preset",
        description = "Explicitly set or clear the active mapping preset",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn set_active_preset(
        &self,
        Parameters(input): Parameters<SetActivePresetInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<Json<MappingMutationOutput>, ErrorData> {
        let _call = self.try_admit_public_call().map_err(to_mcp_error)?;
        let client = declared_client(&context).map_err(to_mcp_error)?;
        self.mapping_mutation_impl(
            input,
            client,
            Instant::now() + ORDINARY_DEADLINE,
            context.ct.clone(),
        )
        .await
        .map(Json)
        .map_err(to_mcp_error)
    }

    #[tool(
        name = "set_mapping_gate",
        description = "Explicitly enable or disable a mapping gate",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn set_mapping_gate(
        &self,
        Parameters(input): Parameters<SetMappingGateInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<Json<MappingMutationOutput>, ErrorData> {
        let _call = self.try_admit_public_call().map_err(to_mcp_error)?;
        let client = declared_client(&context).map_err(to_mcp_error)?;
        self.mapping_mutation_impl(
            input,
            client,
            Instant::now() + ORDINARY_DEADLINE,
            context.ct.clone(),
        )
        .await
        .map(Json)
        .map_err(to_mcp_error)
    }

    #[tool(
        name = "create_mapping_rule",
        description = "Create a remote or local mapping rule",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    async fn create_mapping_rule(
        &self,
        Parameters(input): Parameters<CreateMappingRuleInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<Json<MappingMutationOutput>, ErrorData> {
        let _call = self.try_admit_public_call().map_err(to_mcp_error)?;
        let client = declared_client(&context).map_err(to_mcp_error)?;
        self.mapping_mutation_impl(
            input,
            client,
            Instant::now() + ORDINARY_DEADLINE,
            context.ct.clone(),
        )
        .await
        .map(Json)
        .map_err(to_mcp_error)
    }

    #[tool(
        name = "update_mapping_rule",
        description = "Update a mapping rule source and target",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    async fn update_mapping_rule(
        &self,
        Parameters(input): Parameters<UpdateMappingRuleInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<Json<MappingMutationOutput>, ErrorData> {
        let _call = self.try_admit_public_call().map_err(to_mcp_error)?;
        let client = declared_client(&context).map_err(to_mcp_error)?;
        self.mapping_mutation_impl(
            input,
            client,
            Instant::now() + ORDINARY_DEADLINE,
            context.ct.clone(),
        )
        .await
        .map(Json)
        .map_err(to_mcp_error)
    }

    #[tool(
        name = "delete_mapping_rule",
        description = "Delete a mapping rule",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    async fn delete_mapping_rule(
        &self,
        Parameters(input): Parameters<DeleteMappingRuleInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<Json<MappingMutationOutput>, ErrorData> {
        let _call = self.try_admit_public_call().map_err(to_mcp_error)?;
        let client = declared_client(&context).map_err(to_mcp_error)?;
        self.mapping_mutation_impl(
            input,
            client,
            Instant::now() + ORDINARY_DEADLINE,
            context.ct.clone(),
        )
        .await
        .map(Json)
        .map_err(to_mcp_error)
    }

    #[tool(
        name = "move_mapping_rule",
        description = "Move a mapping rule to a final post-move index",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    async fn move_mapping_rule(
        &self,
        Parameters(input): Parameters<MoveMappingRuleInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<Json<MappingMutationOutput>, ErrorData> {
        let _call = self.try_admit_public_call().map_err(to_mcp_error)?;
        let client = declared_client(&context).map_err(to_mcp_error)?;
        self.mapping_mutation_impl(
            input,
            client,
            Instant::now() + ORDINARY_DEADLINE,
            context.ct.clone(),
        )
        .await
        .map(Json)
        .map_err(to_mcp_error)
    }

    #[tool(
        name = "set_mapping_rule_enabled",
        description = "Explicitly enable or disable one mapping rule",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn set_mapping_rule_enabled(
        &self,
        Parameters(input): Parameters<SetMappingRuleEnabledInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<Json<MappingMutationOutput>, ErrorData> {
        let _call = self.try_admit_public_call().map_err(to_mcp_error)?;
        let client = declared_client(&context).map_err(to_mcp_error)?;
        self.mapping_mutation_impl(
            input,
            client,
            Instant::now() + ORDINARY_DEADLINE,
            context.ct.clone(),
        )
        .await
        .map(Json)
        .map_err(to_mcp_error)
    }

    #[tool(
        name = "search_captures",
        description = "Search retained capture metadata newest-first with a stable sequence cursor",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn search_captures(
        &self,
        Parameters(input): Parameters<SearchCapturesInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<Json<SearchCapturesResult>, ErrorData> {
        let deadline = Instant::now() + ORDINARY_DEADLINE;
        let call = self.try_admit_public_call().map_err(to_mcp_error)?;
        let cancelled = context.ct.clone();
        let (input, _call) = validate_public_search_input(input, call, deadline, cancelled.clone())
            .await
            .map_err(to_mcp_error)?;
        let client = declared_client(&context).map_err(to_mcp_error)?;
        self.search_captures_impl(input, client, deadline, cancelled)
            .await
            .map(Json)
            .map_err(to_mcp_error)
    }

    #[tool(
        name = "get_capture",
        description = "Get one exact retained capture revision and readable request/response body resource links",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn get_capture(
        &self,
        Parameters(input): Parameters<GetCaptureInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<Json<GetCaptureResult>, ErrorData> {
        let _call = self.try_admit_public_call().map_err(to_mcp_error)?;
        let client = declared_client(&context).map_err(to_mcp_error)?;
        self.get_capture_impl(
            input,
            client,
            Instant::now() + ORDINARY_DEADLINE,
            context.ct.clone(),
        )
        .await
        .map(Json)
        .map_err(to_mcp_error)
    }

    #[tool(
        name = "search_capture_body",
        description = "Search one exact revision-pinned decoded capture body with bounded Unicode-folded matches and context",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn search_capture_body(
        &self,
        Parameters(input): Parameters<SearchCaptureBodyInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<Json<SearchCaptureBodyOutput>, ErrorData> {
        let _call = self.try_admit_public_call().map_err(to_mcp_error)?;
        let client = declared_client(&context).map_err(to_mcp_error)?;
        self.search_capture_body_impl(
            input,
            client,
            Instant::now() + ORDINARY_DEADLINE,
            context.ct.clone(),
        )
        .await
        .map(Json)
        .map_err(to_mcp_error)
    }

    #[tool(
        name = "extract_capture_body",
        description = "Extract one exact JSON pointer or URL-form field from a revision-pinned decoded capture body",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn extract_capture_body(
        &self,
        Parameters(input): Parameters<ExtractCaptureBodyInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<Json<ExtractCaptureBodyOutput>, ErrorData> {
        let _call = self.try_admit_public_call().map_err(to_mcp_error)?;
        let client = declared_client(&context).map_err(to_mcp_error)?;
        self.extract_capture_body_impl(
            input,
            client,
            Instant::now() + ORDINARY_DEADLINE,
            context.ct.clone(),
        )
        .await
        .map(Json)
        .map_err(to_mcp_error)
    }

    #[tool(
        name = "find_json_pointers",
        description = "Find bounded exact JSON field pointers and structural metadata in one revision-pinned decoded capture body without returning values",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn find_json_pointers(
        &self,
        Parameters(input): Parameters<FindJsonPointersInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<Json<FindJsonPointersOutput>, ErrorData> {
        let _call = self.try_admit_public_call().map_err(to_mcp_error)?;
        let client = declared_client(&context).map_err(to_mcp_error)?;
        self.find_json_pointers_impl(
            input,
            client,
            Instant::now() + ORDINARY_DEADLINE,
            context.ct.clone(),
        )
        .await
        .map(Json)
        .map_err(to_mcp_error)
    }

    #[tool(
        name = "probe_json_pointer_pattern",
        description = "Probe one bounded RFC 6901 pointer pattern with single-segment wildcards in a revision-pinned decoded capture body without returning values",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn probe_json_pointer_pattern(
        &self,
        Parameters(input): Parameters<ProbeJsonPointerPatternInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<Json<ProbeJsonPointerPatternOutput>, ErrorData> {
        let _call = self.try_admit_public_call().map_err(to_mcp_error)?;
        let client = declared_client(&context).map_err(to_mcp_error)?;
        self.probe_json_pointer_pattern_impl(
            input,
            client,
            Instant::now() + ORDINARY_DEADLINE,
            context.ct.clone(),
        )
        .await
        .map(Json)
        .map_err(to_mcp_error)
    }

    #[tool(
        name = "wait_for_capture",
        description = "Wait on one exact run for request_seen, response_started, or exchange_terminal capture metadata for at most five minutes; a normal timeout returns an unmatched result",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    async fn wait_for_capture(
        &self,
        Parameters(input): Parameters<WaitForCaptureInput>,
        context: RequestContext<RoleServer>,
    ) -> Result<Json<WaitForCaptureResult>, ErrorData> {
        let started = Instant::now();
        let timeout_ms = normalize_wait_timeout_ms(input.timeout_ms).map_err(to_mcp_error)?;
        let deadline = started + Duration::from_millis(timeout_ms) + ORDINARY_DEADLINE;
        let call = self.try_admit_public_call().map_err(to_mcp_error)?;
        let cancelled = context.ct.clone();
        let (input, _call) = validate_public_wait_input(input, call, deadline, cancelled.clone())
            .await
            .map_err(to_mcp_error)?;
        let client = declared_client(&context).map_err(to_mcp_error)?;
        self.wait_for_capture_impl(input, client, deadline, cancelled)
            .await
            .map(Json)
            .map_err(to_mcp_error)
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for Broker {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(
            ServerCapabilities::builder()
                .enable_resources()
                .enable_tools()
                .enable_prompts()
                .build(),
        )
        .with_server_info(Implementation::new("fluxcope", env!("CARGO_PKG_VERSION")))
        .with_protocol_version(ProtocolVersion::LATEST)
        .with_instructions(
            "Use list_instances first and select an explicit instance when more than one is live.",
        )
    }

    fn list_prompts(
        &self,
        _request: Option<rmcp::model::PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListPromptsResult, ErrorData>> + Send + '_ {
        std::future::ready(Ok(
            ListPromptsResult::with_all_items(super::prompts::list()),
        ))
    }

    fn get_prompt(
        &self,
        request: GetPromptRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<GetPromptResponse, ErrorData>> + Send + '_ {
        std::future::ready(
            super::prompts::get(&request.name, request.arguments.as_ref()).map(Into::into),
        )
    }

    fn list_resources(
        &self,
        _request: Option<rmcp::model::PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListResourcesResult, ErrorData>> + Send + '_ {
        std::future::ready(Ok(ListResourcesResult::with_all_items(Vec::new())))
    }

    fn list_resource_templates(
        &self,
        _request: Option<rmcp::model::PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListResourceTemplatesResult, ErrorData>> + Send + '_ {
        let content = ResourceTemplate::new(CONTENT_RESOURCE_TEMPLATE, "capture_body_content")
            .with_description(
                "A revision-pinned byte page from one retained request or response body",
            );
        let json = ResourceTemplate::new(
            JSON_POINTER_RESOURCE_TEMPLATE,
            "capture_body_json_pointer_selection",
        )
        .with_description("A UTF-8 page from one exact compact JSON-pointer selection");
        let form = ResourceTemplate::new(
            FORM_FIELD_RESOURCE_TEMPLATE,
            "capture_body_form_field_selection",
        )
        .with_description("A UTF-8 page from one exact URL-form field selection");
        std::future::ready(Ok(ListResourceTemplatesResult::with_all_items(vec![
            content, json, form,
        ])))
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, ErrorData> {
        let _call = self.try_admit_public_call().map_err(to_mcp_error)?;
        let client = declared_client(&context).map_err(to_mcp_error)?;
        if request
            .uri
            .split_once('?')
            .map_or(request.uri.as_str(), |(path, _)| path)
            .contains("/extract/")
        {
            let requested = SelectionResourceUri::parse(&request.uri).map_err(to_mcp_error)?;
            self.read_selection_resource_impl(
                requested,
                client,
                Instant::now() + ORDINARY_DEADLINE,
                context.ct.clone(),
            )
            .await
            .map(Into::into)
            .map_err(to_mcp_error)
        } else {
            let requested = BodyResourceUri::parse(&request.uri).map_err(to_mcp_error)?;
            self.read_body_resource_impl(
                requested,
                client,
                Instant::now() + ORDINARY_DEADLINE,
                context.ct.clone(),
            )
            .await
            .map(Into::into)
            .map_err(to_mcp_error)
        }
    }
}

fn instance_summary(resolved: ResolvedInstance) -> Result<InstanceSummary, ControlError> {
    let descriptor = resolved.descriptor;
    let ControlResult::DescribeInstance {
        config_mode,
        persistence,
        recording_enabled,
        retained_capture_count,
        settings_revision,
        ..
    } = resolved.description
    else {
        return Err(ControlError::internal(
            "instance probe returned an unexpected description result",
        ));
    };
    Ok(InstanceSummary {
        instance: InstanceSelector {
            proxy_endpoint: Some(descriptor.proxy_endpoint()),
            run_id: Some(descriptor.run_id().clone()),
        },
        local_proxy_url: descriptor.local_proxy_url().to_owned(),
        started_at: descriptor.started_at().to_rfc3339(),
        fluxcope_version: descriptor.binary_version().to_owned(),
        rpc_version: RPC_VERSION,
        config_mode,
        persistence,
        config_source: descriptor.config_source().map(Path::to_path_buf),
        recording_enabled,
        retained_capture_count,
        settings_revision,
    })
}
fn capture_body_resources(
    instance: &crate::control_rpc::protocol::InstanceScope,
    capture: &crate::control::capture_query::CaptureDetail,
) -> CaptureBodyResources {
    let links = |side| {
        let make = |representation| {
            BodyResourceUri::content(
                instance.proxy_endpoint,
                instance.run_id.clone(),
                capture.capture_sequence,
                capture.capture_revision,
                side,
                representation,
                0,
                DEFAULT_BODY_PAGE_LENGTH,
            )
            .to_string()
        };
        BodyRepresentationLinks {
            raw: make(BodyRepresentation::Raw),
            decoded: make(BodyRepresentation::Decoded),
        }
    };
    CaptureBodyResources {
        request: links(BodySide::Request),
        response: capture.status.map(|_| links(BodySide::Response)),
    }
}

fn status_result(result: ControlResult) -> Result<GetStatusResult, ControlError> {
    let ControlResult::GetStatus {
        instance,
        local_proxy_url,
        fluxcope_version,
        rpc_version,
        config_source,
        config_mode,
        persistence,
        recording_enabled,
        retained_capture_count,
        settings_revision,
        mapping,
        capture_store,
        capture_change_epoch,
        metrics,
        private_rpc,
        body_work,
        search_work,
        audit,
    } = result
    else {
        return Err(ControlError::internal(
            "private RPC returned an unexpected status result",
        ));
    };
    let metrics = *metrics;
    let body_work = *body_work;
    let audit = *audit;
    let warnings = status_warnings(metrics, body_work, &audit);
    Ok(GetStatusResult {
        instance: InstanceSelector {
            proxy_endpoint: Some(instance.proxy_endpoint),
            run_id: Some(instance.run_id),
        },
        local_proxy_url,
        fluxcope_version,
        rpc_version,
        config_source,
        config_mode,
        persistence,
        recording_enabled,
        retained_capture_count,
        settings_revision,
        mapping,
        capture_store,
        capture_change_epoch,
        private_rpc,
        metrics,
        search_work,
        body_work,
        audit,
        limits: BrokerLimits::default(),
        warnings,
    })
}

fn status_warnings(
    metrics: crate::control::InstanceRuntimeMetrics,
    body_work: crate::control::BodyWorkRuntimeStatus,
    audit: &crate::control::audit::InstanceAuditSnapshot,
) -> Vec<StatusWarning> {
    let mut warnings = Vec::with_capacity(6);
    warnings.push(StatusWarning {
        code: "local_unauthenticated_access".to_owned(),
        message: crate::control::MCP_LOCAL_ACCESS_WARNING.to_owned(),
    });
    if metrics.capture.exchanges_not_admitted > 0
        || metrics.capture.memory_pressure > 0
        || metrics.capture.previews_per_body_limited > 0
        || metrics.capture.previews_memory_limited > 0
        || metrics.capture.metadata_truncated > 0
    {
        warnings.push(StatusWarning {
            code: "capture_retention_pressure".to_owned(),
            message: "some captures, metadata, or body previews were limited by capture budgets"
                .to_owned(),
        });
    }
    if metrics.decode.rejected > 0 || metrics.decode.output_limited > 0 || metrics.decode.failed > 0
    {
        warnings.push(StatusWarning {
            code: "body_decode_limited".to_owned(),
            message: "some body decode work was rejected, limited, or failed".to_owned(),
        });
    }
    if metrics.logging.producer_dropped > 0
        || metrics.logging.tui_dropped > 0
        || metrics.logging.records_truncated > 0
    {
        warnings.push(StatusWarning {
            code: "logging_limited".to_owned(),
            message: "some log records were dropped or truncated".to_owned(),
        });
    }
    if body_work.queued > 0 || body_work.rejected > 0 {
        warnings.push(StatusWarning {
            code: "body_work_saturated".to_owned(),
            message: "body processing is queued or has rejected work at its configured limits"
                .to_owned(),
        });
    }
    if audit.response_delivery_failures > 0 {
        warnings.push(StatusWarning {
            code: "response_delivery_failed".to_owned(),
            message: "one or more private RPC responses could not be delivered to their client"
                .to_owned(),
        });
    }
    warnings
}

fn declared_client(context: &RequestContext<RoleServer>) -> Result<DeclaredClient, McpDomainError> {
    let info = context.peer.peer_info().ok_or_else(|| {
        ControlError::invalid_argument("MCP client metadata is unavailable before initialization")
    })?;
    validate_client_identifier("client name", &info.client_info.name)?;
    validate_client_identifier("client version", &info.client_info.version)?;
    Ok(DeclaredClient {
        name: info.client_info.name.clone(),
        version: info.client_info.version.clone(),
    })
}

fn validate_client_identifier(field: &str, value: &str) -> Result<(), McpDomainError> {
    if value.is_empty()
        || value.len() > CLIENT_IDENTIFIER_LIMIT
        || value.chars().any(char::is_control)
    {
        return Err(ControlError::invalid_argument(format!(
            "{field} must be 1 to {CLIENT_IDENTIFIER_LIMIT} bytes without control characters"
        )));
    }
    Ok(())
}

pub(super) fn to_mcp_error(error: McpDomainError) -> ErrorData {
    let data = json!({
        "code": error.code(),
        "retryable": error.retryable(),
        "details": error.details(),
    });
    let code = match error.code() {
        ControlErrorCode::InvalidArgument | ControlErrorCode::MappingValidationFailed => {
            ErrorCode::INVALID_PARAMS
        }
        ControlErrorCode::NoInstances
        | ControlErrorCode::InstanceRequired
        | ControlErrorCode::InstanceNotFound
        | ControlErrorCode::CaptureNotFound
        | ControlErrorCode::NotFound => ErrorCode(-32004),
        ControlErrorCode::InstanceGenerationConflict
        | ControlErrorCode::CaptureRevisionConflict
        | ControlErrorCode::SettingsRevisionConflict
        | ControlErrorCode::TuiDraftConflict => ErrorCode(-32009),
        ControlErrorCode::InstanceUnavailable | ControlErrorCode::ServiceUnavailable => {
            ErrorCode(-32003)
        }
        ControlErrorCode::RpcVersionMismatch => ErrorCode(-32010),
        ControlErrorCode::RpcFrameTooLarge
        | ControlErrorCode::ResourceLimit
        | ControlErrorCode::JsonDepthLimit
        | ControlErrorCode::JsonSizeLimit => ErrorCode(-32005),
        ControlErrorCode::UnsupportedBodyEncoding
        | ControlErrorCode::UndecodableBody
        | ControlErrorCode::BodyNotTextual
        | ControlErrorCode::MalformedJson => ErrorCode(-32022),
        ControlErrorCode::DeadlineExceeded => ErrorCode(-32008),
        ControlErrorCode::Cancelled => ErrorCode(-32007),
        ControlErrorCode::InternalError => ErrorCode::INTERNAL_ERROR,
    };
    ErrorData::new(code, error.message().to_owned(), Some(data))
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashMap,
        io,
        net::{IpAddr, Ipv4Addr, SocketAddr},
        path::PathBuf,
        pin::Pin,
        str::FromStr,
        sync::{
            Arc, Condvar, Mutex,
            atomic::{AtomicBool, AtomicUsize, Ordering},
        },
        time::Duration,
    };

    use futures::Future;
    use rmcp::{
        ServiceExt,
        model::{
            CallToolRequestParams, ClientCapabilities, ClientInfo, Implementation, ProtocolVersion,
        },
        object,
    };
    use serde_json::json;
    use tokio::time::Instant;
    use tokio::{
        io::duplex,
        sync::{Notify, Semaphore, mpsc},
    };
    use tokio_util::sync::CancellationToken;

    use super::{
        Broker, InstanceProbe, McpDomainError, RegistryAccess, SelectorRequirement,
        run_public_search_validation, status_warnings, to_mcp_error,
    };
    use crate::{
        control_rpc::{
            client::connect_failure,
            protocol::{
                ControlError, ControlErrorCode, ControlOperation, ControlResult, DeclaredClient,
                InstanceScope, RPC_VERSION,
            },
        },
        instance::RunId,
        instance_registry::{DiscoveryDiagnostic, InstanceDescriptor, RegistryScan},
        mcp::{
            capture::SearchCapturesInput, mapping::ValidateMappingSettingsInput,
            schema::InstanceSelector,
        },
        settings::{ConfigMode, PersistenceMode},
    };

    const RUN_A: &str = "AAAAAAAAAAAAAAAAAAAAAA";
    const RUN_B: &str = "AQEBAQEBAQEBAQEBAQEBAQ";
    const RUN_STALE: &str = "AgICAgICAgICAgICAgICAg";

    #[derive(Clone)]
    enum ProbePlan {
        Live(Box<ControlResult>),
        Unavailable,
        StaleConnect,
    }

    #[derive(Clone, Debug, Eq, PartialEq)]
    struct ProbeCall {
        endpoint: SocketAddr,
        client: DeclaredClient,
    }

    struct FakeProbe {
        plans: Mutex<HashMap<SocketAddr, ProbePlan>>,
        delay: Duration,
        gate: Option<Arc<Semaphore>>,
        calls: Mutex<Vec<ProbeCall>>,
        active: AtomicUsize,
        maximum_active: AtomicUsize,
    }

    impl FakeProbe {
        fn live(descriptors: &[InstanceDescriptor]) -> Arc<Self> {
            let plans = descriptors
                .iter()
                .map(|descriptor| {
                    (
                        descriptor.proxy_endpoint(),
                        ProbePlan::Live(Box::new(describe(
                            descriptor,
                            descriptor.proxy_endpoint().port() as usize,
                        ))),
                    )
                })
                .collect();
            Arc::new(Self {
                plans: Mutex::new(plans),
                delay: Duration::ZERO,
                gate: None,
                calls: Mutex::new(Vec::new()),
                active: AtomicUsize::new(0),
                maximum_active: AtomicUsize::new(0),
            })
        }

        fn delayed(descriptors: &[InstanceDescriptor], delay: Duration) -> Arc<Self> {
            let mut probe = Arc::try_unwrap(Self::live(descriptors))
                .ok()
                .expect("unique probe");
            probe.delay = delay;
            Arc::new(probe)
        }

        fn gated(descriptors: &[InstanceDescriptor], gate: Arc<Semaphore>) -> Arc<Self> {
            let mut probe = Arc::try_unwrap(Self::live(descriptors))
                .ok()
                .expect("unique probe");
            probe.gate = Some(gate);
            Arc::new(probe)
        }

        fn mark_unavailable(&self, endpoint: SocketAddr) {
            self.plans
                .lock()
                .expect("probe plans")
                .insert(endpoint, ProbePlan::Unavailable);
        }

        fn mark_stale_connect(&self, endpoint: SocketAddr) {
            self.plans
                .lock()
                .expect("probe plans")
                .insert(endpoint, ProbePlan::StaleConnect);
        }

        fn calls(&self) -> Vec<ProbeCall> {
            self.calls.lock().expect("probe calls").clone()
        }

        fn active(&self) -> usize {
            self.active.load(Ordering::SeqCst)
        }

        fn maximum_active(&self) -> usize {
            self.maximum_active.load(Ordering::SeqCst)
        }
    }

    struct ActiveProbe<'a>(&'a AtomicUsize);

    impl Drop for ActiveProbe<'_> {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Ordering::SeqCst);
        }
    }

    impl InstanceProbe for FakeProbe {
        fn describe<'a>(
            &'a self,
            descriptor: &'a InstanceDescriptor,
            client: DeclaredClient,
            deadline: Instant,
            cancelled: CancellationToken,
        ) -> Pin<Box<dyn Future<Output = Result<ControlResult, ControlError>> + Send + 'a>>
        {
            Box::pin(async move {
                self.calls.lock().expect("probe calls").push(ProbeCall {
                    endpoint: descriptor.proxy_endpoint(),
                    client,
                });
                let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
                self.maximum_active.fetch_max(active, Ordering::SeqCst);
                let _active = ActiveProbe(&self.active);
                let operation = async {
                    if let Some(gate) = &self.gate {
                        let permit = gate.acquire().await.map_err(|_| {
                            ControlError::instance_unavailable("test probe gate closed")
                        })?;
                        permit.forget();
                    }
                    tokio::time::sleep(self.delay).await;
                    match self
                        .plans
                        .lock()
                        .expect("probe plans")
                        .get(&descriptor.proxy_endpoint())
                        .cloned()
                    {
                        Some(ProbePlan::Live(result)) => Ok(*result),
                        Some(ProbePlan::StaleConnect) => {
                            Err(connect_failure(io::Error::from(io::ErrorKind::NotFound)))
                        }
                        Some(ProbePlan::Unavailable) | None => Err(
                            ControlError::instance_unavailable("test instance unavailable"),
                        ),
                    }
                };
                tokio::select! {
                    () = cancelled.cancelled() => Err(ControlError::cancelled("probe cancelled")),
                    result = tokio::time::timeout_at(deadline, operation) => result
                        .map_err(|_| ControlError::deadline_exceeded("probe deadline elapsed"))?,
                }
            })
        }
    }

    struct FakeRegistry {
        scan_candidates: Vec<InstanceDescriptor>,
        endpoint_candidates: Vec<InstanceDescriptor>,
        rejected: Vec<DiscoveryDiagnostic>,
        omitted: usize,
        scan_calls: AtomicUsize,
        endpoint_calls: AtomicUsize,
        prune_calls: AtomicUsize,
        prune_error: bool,
    }

    impl FakeRegistry {
        fn new(candidates: Vec<InstanceDescriptor>) -> Arc<Self> {
            Arc::new(Self {
                endpoint_candidates: candidates.clone(),
                scan_candidates: candidates,
                rejected: Vec::new(),
                omitted: 0,
                scan_calls: AtomicUsize::new(0),
                endpoint_calls: AtomicUsize::new(0),
                prune_calls: AtomicUsize::new(0),
                prune_error: false,
            })
        }

        fn with_scan_candidates(
            scan_candidates: Vec<InstanceDescriptor>,
            endpoint_candidates: Vec<InstanceDescriptor>,
        ) -> Arc<Self> {
            Arc::new(Self {
                scan_candidates,
                endpoint_candidates,
                rejected: Vec::new(),
                omitted: 0,
                scan_calls: AtomicUsize::new(0),
                endpoint_calls: AtomicUsize::new(0),
                prune_calls: AtomicUsize::new(0),
                prune_error: false,
            })
        }

        fn with_diagnostics(
            candidates: Vec<InstanceDescriptor>,
            rejected: Vec<DiscoveryDiagnostic>,
            omitted: usize,
        ) -> Arc<Self> {
            Arc::new(Self {
                endpoint_candidates: candidates.clone(),
                scan_candidates: candidates,
                rejected,
                omitted,
                scan_calls: AtomicUsize::new(0),
                endpoint_calls: AtomicUsize::new(0),
                prune_calls: AtomicUsize::new(0),
                prune_error: false,
            })
        }

        fn with_prune_failure(candidates: Vec<InstanceDescriptor>) -> Arc<Self> {
            Arc::new(Self {
                endpoint_candidates: candidates.clone(),
                scan_candidates: candidates,
                rejected: Vec::new(),
                omitted: 0,
                scan_calls: AtomicUsize::new(0),
                endpoint_calls: AtomicUsize::new(0),
                prune_calls: AtomicUsize::new(0),
                prune_error: true,
            })
        }

        fn prune_calls(&self) -> usize {
            self.prune_calls.load(Ordering::SeqCst)
        }
    }

    impl RegistryAccess for FakeRegistry {
        fn scan_all(&self) -> io::Result<RegistryScan> {
            self.scan_calls.fetch_add(1, Ordering::SeqCst);
            Ok(RegistryScan {
                candidates: self.scan_candidates.clone(),
                rejected: self.rejected.clone(),
                omitted: self.omitted,
            })
        }

        fn read_endpoint(&self, endpoint: SocketAddr) -> io::Result<RegistryScan> {
            self.endpoint_calls.fetch_add(1, Ordering::SeqCst);
            Ok(RegistryScan {
                candidates: self
                    .endpoint_candidates
                    .iter()
                    .filter(|descriptor| descriptor.proxy_endpoint() == endpoint)
                    .cloned()
                    .collect(),
                rejected: Vec::new(),
                omitted: 0,
            })
        }

        fn prune_batch_if_current(
            &self,
            descriptors: &[InstanceDescriptor],
            _deadline: Instant,
            _cancelled: &CancellationToken,
        ) -> io::Result<usize> {
            self.prune_calls.fetch_add(1, Ordering::SeqCst);
            if self.prune_error {
                Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "test stale cleanup failure",
                ))
            } else {
                Ok(descriptors.len())
            }
        }
    }

    #[derive(Default)]
    struct BlockingGate {
        open: Mutex<bool>,
        changed: Condvar,
    }

    impl BlockingGate {
        fn wait(&self) {
            let mut open = self.open.lock().expect("blocking gate");
            while !*open {
                open = self.changed.wait(open).expect("blocking gate wait");
            }
        }

        fn release(&self) {
            *self.open.lock().expect("blocking gate") = true;
            self.changed.notify_all();
        }
    }

    struct BlockingScanRegistry {
        gate: Arc<BlockingGate>,
        started: AtomicUsize,
    }

    impl RegistryAccess for BlockingScanRegistry {
        fn scan_all(&self) -> io::Result<RegistryScan> {
            self.started.fetch_add(1, Ordering::SeqCst);
            self.gate.wait();
            Ok(RegistryScan {
                candidates: Vec::new(),
                rejected: Vec::new(),
                omitted: 0,
            })
        }

        fn read_endpoint(&self, _endpoint: SocketAddr) -> io::Result<RegistryScan> {
            self.scan_all()
        }

        fn prune_batch_if_current(
            &self,
            _descriptors: &[InstanceDescriptor],
            _deadline: Instant,
            _cancelled: &CancellationToken,
        ) -> io::Result<usize> {
            Ok(0)
        }
    }

    struct BlockingPruneRegistry {
        descriptor: InstanceDescriptor,
        gate: Arc<BlockingGate>,
        prune_started: AtomicBool,
        prune_committed: AtomicUsize,
    }

    impl RegistryAccess for BlockingPruneRegistry {
        fn scan_all(&self) -> io::Result<RegistryScan> {
            Ok(RegistryScan {
                candidates: vec![self.descriptor.clone()],
                rejected: Vec::new(),
                omitted: 0,
            })
        }

        fn read_endpoint(&self, _endpoint: SocketAddr) -> io::Result<RegistryScan> {
            self.scan_all()
        }

        fn prune_batch_if_current(
            &self,
            descriptors: &[InstanceDescriptor],
            deadline: Instant,
            cancelled: &CancellationToken,
        ) -> io::Result<usize> {
            self.prune_started.store(true, Ordering::SeqCst);
            self.gate.wait();
            if cancelled.is_cancelled() || Instant::now() >= deadline {
                return Ok(0);
            }
            self.prune_committed.fetch_add(1, Ordering::SeqCst);
            Ok(descriptors.len())
        }
    }

    fn descriptor(port: u16, run_id: &str) -> InstanceDescriptor {
        let endpoint = endpoint(port);
        let value = serde_json::json!({
            "schema_version": 1,
            "rpc_version": RPC_VERSION,
            "binary_version": "9.8.7-test",
            "pid": u32::from(port),
            "proxy_endpoint": endpoint,
            "local_proxy_url": format!("http://{endpoint}"),
            "run_id": run_id,
            "started_at": "2026-08-24T00:00:00Z",
            "socket_path": format!("/tmp/fluxcope-mcp-{port}.sock"),
            "config_mode": "temporary",
            "persistence": "ephemeral",
            "config_source": null
        });
        let encoded = serde_json::to_string(&value).expect("serialize test descriptor");
        serde_json::from_str(&encoded).expect("test descriptor")
    }

    fn describe(descriptor: &InstanceDescriptor, retained_capture_count: usize) -> ControlResult {
        ControlResult::DescribeInstance {
            instance: InstanceScope {
                proxy_endpoint: descriptor.proxy_endpoint(),
                run_id: descriptor.run_id().clone(),
            },
            config_mode: descriptor.config_mode(),
            persistence: descriptor.persistence(),
            recording_enabled: true,
            retained_capture_count,
            settings_revision: 41,
        }
    }

    fn endpoint(port: u16) -> SocketAddr {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port)
    }

    fn client() -> DeclaredClient {
        DeclaredClient {
            name: "test-mcp-host".to_owned(),
            version: "4.5.6".to_owned(),
        }
    }

    fn broker(registry: Arc<FakeRegistry>, probe: Arc<FakeProbe>) -> Broker {
        Broker::with_dependencies(
            PathBuf::from("/test/.fluxcope/run/instances"),
            registry,
            probe,
        )
    }

    fn deadline() -> Instant {
        Instant::now() + Duration::from_secs(1)
    }

    async fn resolve(
        broker: &Broker,
        selector: InstanceSelector,
        requirement: SelectorRequirement,
    ) -> Result<super::ResolvedInstance, McpDomainError> {
        broker
            .resolve(selector, requirement, deadline(), CancellationToken::new())
            .await
    }

    async fn wait_for_active(probe: &FakeProbe, expected: usize) {
        tokio::time::timeout(Duration::from_secs(1), async {
            while probe.active() != expected {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("probe activity reached expected bound");
    }

    #[tokio::test]
    async fn omitted_selector_reports_no_instances_when_discovery_is_empty() {
        let registry = FakeRegistry::new(Vec::new());
        let broker = broker(Arc::clone(&registry), FakeProbe::live(&[]));

        let error = resolve(
            &broker,
            InstanceSelector::default(),
            SelectorRequirement::SnapshotRead,
        )
        .await
        .expect_err("no instance");

        assert_eq!(error.code(), ControlErrorCode::NoInstances);
        assert_eq!(registry.scan_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn omitted_selector_selects_the_only_live_instance() {
        let only = descriptor(19001, RUN_A);
        let probe = FakeProbe::live(std::slice::from_ref(&only));
        let broker = broker(FakeRegistry::new(vec![only.clone()]), probe);

        let resolved = resolve(
            &broker,
            InstanceSelector::default(),
            SelectorRequirement::SnapshotRead,
        )
        .await
        .expect("singleton selection");

        assert_eq!(resolved.descriptor.proxy_endpoint(), endpoint(19001));
        assert_eq!(resolved.descriptor.run_id().as_str(), RUN_A);
    }

    #[tokio::test]
    async fn omitted_selector_requires_identity_when_multiple_instances_are_live() {
        let first = descriptor(19002, RUN_B);
        let second = descriptor(19001, RUN_A);
        let candidates = vec![first, second];
        let broker = broker(
            FakeRegistry::new(candidates.clone()),
            FakeProbe::live(&candidates),
        );

        let error = resolve(
            &broker,
            InstanceSelector::default(),
            SelectorRequirement::SnapshotRead,
        )
        .await
        .expect_err("ambiguous");

        assert_eq!(error.code(), ControlErrorCode::InstanceRequired);
        assert_eq!(
            error.details()["instances"],
            serde_json::json!([
                {"proxy_endpoint": "127.0.0.1:19001", "run_id": RUN_A},
                {"proxy_endpoint": "127.0.0.1:19002", "run_id": RUN_B}
            ])
        );
    }

    #[tokio::test]
    async fn endpoint_selector_uses_only_the_deterministic_endpoint_read() {
        let selected = descriptor(19001, RUN_A);
        let unrelated = descriptor(19002, RUN_B);
        let registry = FakeRegistry::new(vec![unrelated.clone(), selected.clone()]);
        let probe = FakeProbe::live(&[selected]);
        let broker = broker(Arc::clone(&registry), Arc::clone(&probe));

        let resolved = resolve(
            &broker,
            InstanceSelector {
                proxy_endpoint: Some(endpoint(19001)),
                run_id: None,
            },
            SelectorRequirement::SnapshotRead,
        )
        .await
        .expect("targeted selection");

        assert_eq!(resolved.descriptor.proxy_endpoint(), endpoint(19001));
        assert_eq!(registry.scan_calls.load(Ordering::SeqCst), 0);
        assert_eq!(registry.endpoint_calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            probe
                .calls()
                .iter()
                .map(|call| call.endpoint)
                .collect::<Vec<_>>(),
            vec![endpoint(19001)]
        );
    }

    #[tokio::test]
    async fn supplied_stale_run_id_is_never_retargeted() {
        let current = descriptor(19001, RUN_A);
        let broker = broker(
            FakeRegistry::new(vec![current.clone()]),
            FakeProbe::live(&[current]),
        );

        let error = resolve(
            &broker,
            InstanceSelector {
                proxy_endpoint: Some(endpoint(19001)),
                run_id: Some(RunId::from_str(RUN_STALE).expect("stale run ID")),
            },
            SelectorRequirement::Mutation,
        )
        .await
        .expect_err("stale generation");

        assert_eq!(error.code(), ControlErrorCode::InstanceGenerationConflict);
        assert_eq!(error.details()["current_run_id"], RUN_A);
    }

    #[tokio::test]
    async fn snapshot_read_without_run_id_selects_the_current_endpoint_generation() {
        let current = descriptor(19001, RUN_B);
        let broker = broker(
            FakeRegistry::new(vec![current.clone()]),
            FakeProbe::live(&[current]),
        );

        let resolved = resolve(
            &broker,
            InstanceSelector {
                proxy_endpoint: Some(endpoint(19001)),
                run_id: None,
            },
            SelectorRequirement::SnapshotRead,
        )
        .await
        .expect("current snapshot generation");

        assert_eq!(resolved.descriptor.run_id().as_str(), RUN_B);
    }

    #[tokio::test]
    async fn every_non_snapshot_requirement_rejects_a_missing_run_id() {
        for requirement in [
            SelectorRequirement::Mutation,
            SelectorRequirement::Wait,
            SelectorRequirement::TargetedBody,
            SelectorRequirement::Resource,
        ] {
            let current = descriptor(19001, RUN_A);
            let broker = broker(
                FakeRegistry::new(vec![current.clone()]),
                FakeProbe::live(&[current]),
            );
            let error = resolve(
                &broker,
                InstanceSelector {
                    proxy_endpoint: Some(endpoint(19001)),
                    run_id: None,
                },
                requirement,
            )
            .await
            .expect_err("run ID required");
            assert_eq!(error.code(), ControlErrorCode::InvalidArgument);
        }
    }

    #[tokio::test]
    async fn discovery_isolates_stale_and_malformed_descriptors_and_sorts_diagnostics() {
        let live_b = descriptor(19002, RUN_B);
        let live_a = descriptor(19001, RUN_A);
        let stale = descriptor(19003, RUN_STALE);
        let candidates = vec![live_b.clone(), stale.clone(), live_a.clone()];
        let registry = FakeRegistry::with_diagnostics(
            candidates.clone(),
            vec![
                DiscoveryDiagnostic::from_parts("z_invalid", "last"),
                DiscoveryDiagnostic::from_parts("a_invalid", "first"),
            ],
            7,
        );
        let probe = FakeProbe::live(&candidates);
        probe.mark_stale_connect(stale.proxy_endpoint());
        let broker = broker(registry, probe);

        let report = broker
            .discover(client(), deadline(), CancellationToken::new())
            .await
            .expect("bounded discovery");

        assert_eq!(
            report
                .live_instances
                .iter()
                .map(|instance| instance.descriptor.proxy_endpoint())
                .collect::<Vec<_>>(),
            vec![endpoint(19001), endpoint(19002)]
        );
        assert_eq!(report.stale_count, 1);
        assert_eq!(
            report
                .rejected
                .iter()
                .map(DiscoveryDiagnostic::code)
                .collect::<Vec<_>>(),
            vec!["a_invalid", "z_invalid"]
        );
        assert_eq!(report.omitted, 7);
    }

    #[tokio::test]
    async fn stale_cleanup_failure_keeps_probed_live_instances_visible() {
        let live = descriptor(19004, RUN_A);
        let stale = descriptor(19005, RUN_STALE);
        let registry = FakeRegistry::with_prune_failure(vec![live.clone(), stale.clone()]);
        let probe = FakeProbe::live(&[live.clone(), stale.clone()]);
        probe.mark_stale_connect(stale.proxy_endpoint());
        let broker = broker(registry, probe);

        let report = broker
            .discover(client(), deadline(), CancellationToken::new())
            .await
            .expect("cleanup failure must not hide live instances");

        assert_eq!(report.live_instances.len(), 1);
        assert_eq!(
            report.live_instances[0].descriptor.proxy_endpoint(),
            live.proxy_endpoint()
        );
        assert_eq!(report.stale_count, 1);
        assert_eq!(report.rejected.len(), 1);
        assert_eq!(report.rejected[0].code(), "stale_cleanup_failed");
    }

    #[tokio::test]
    async fn public_call_admission_is_shared_across_concurrent_broker_clones() {
        let broker = broker(FakeRegistry::new(Vec::new()), FakeProbe::live(&[]));
        let release = Arc::new(Notify::new());
        let (ready_tx, mut ready_rx) = mpsc::channel(32);
        let mut tasks = Vec::new();
        for _ in 0..32 {
            let broker = broker.clone();
            let release = Arc::clone(&release);
            let ready_tx = ready_tx.clone();
            tasks.push(tokio::spawn(async move {
                let _permit = broker.try_admit_public_call().expect("one of 32 calls");
                ready_tx.send(()).await.expect("ready signal");
                release.notified().await;
            }));
        }
        drop(ready_tx);
        for _ in 0..32 {
            ready_rx.recv().await.expect("admitted call");
        }

        let error = broker
            .clone()
            .try_admit_public_call()
            .expect_err("33rd call rejected");
        assert_eq!(error.code(), ControlErrorCode::ServiceUnavailable);
        assert!(error.retryable());
        release.notify_waiters();
        for task in tasks {
            task.await.expect("admission task");
        }
    }

    #[tokio::test]
    async fn public_search_validation_runs_off_thread_and_retains_call_permit_until_worker_exit() {
        let broker = broker(FakeRegistry::new(Vec::new()), FakeProbe::live(&[]));
        let permit = broker.try_admit_public_call().expect("public call permit");
        assert_eq!(broker.call_admission.available_permits(), 31);
        let gate = Arc::new(BlockingGate::default());
        let worker_gate = Arc::clone(&gate);
        let (started_tx, mut started_rx) = mpsc::unbounded_channel();
        let cancelled = CancellationToken::new();
        let task_cancelled = cancelled.clone();
        let task = tokio::spawn(async move {
            run_public_search_validation(
                permit,
                Instant::now() + Duration::from_secs(60),
                task_cancelled,
                move || {
                    started_tx.send(()).expect("validation started");
                    worker_gate.wait();
                    Ok(SearchCapturesInput::default())
                },
            )
            .await
        });

        started_rx.recv().await.expect("blocking worker started");
        cancelled.cancel();
        let error = task
            .await
            .expect("validation task")
            .expect_err("validation cancellation");
        assert_eq!(error.code(), ControlErrorCode::Cancelled);
        assert_eq!(
            broker.call_admission.available_permits(),
            31,
            "detached blocking validation must continue to own the public call permit"
        );

        gate.release();
        tokio::time::timeout(Duration::from_secs(1), async {
            while broker.call_admission.available_permits() != 32 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("validation worker released permit");
    }

    #[tokio::test(start_paused = true)]
    async fn public_search_validation_timeout_uses_outer_deadline_and_retains_permit() {
        let selected = descriptor(19001, RUN_A);
        let registry = FakeRegistry::new(vec![selected.clone()]);
        let probe = FakeProbe::live(&[selected]);
        let broker = broker(Arc::clone(&registry), Arc::clone(&probe));
        let permit = broker.try_admit_public_call().expect("public call permit");
        let gate = Arc::new(BlockingGate::default());
        let worker_gate = Arc::clone(&gate);
        let (started_tx, mut started_rx) = mpsc::unbounded_channel();
        let deadline = Instant::now() + super::ORDINARY_DEADLINE;
        let validation_cancelled = CancellationToken::new();
        let dispatch_cancelled = validation_cancelled.clone();
        let task_broker = broker.clone();
        let task = tokio::spawn(async move {
            let (input, _permit) =
                run_public_search_validation(permit, deadline, validation_cancelled, move || {
                    started_tx.send(()).expect("validation started");
                    worker_gate.wait();
                    Ok(SearchCapturesInput::default())
                })
                .await?;
            task_broker
                .search_captures_impl(input, client(), deadline, dispatch_cancelled)
                .await
        });

        started_rx.recv().await.expect("blocking worker started");
        tokio::time::advance(super::ORDINARY_DEADLINE).await;
        let error = task
            .await
            .expect("public search task")
            .expect_err("validation deadline");
        assert_eq!(error.code(), ControlErrorCode::DeadlineExceeded);
        assert_eq!(
            broker.call_admission.available_permits(),
            31,
            "timed-out validation worker must retain the public permit"
        );
        assert_eq!(registry.scan_calls.load(Ordering::SeqCst), 0);
        assert_eq!(registry.endpoint_calls.load(Ordering::SeqCst), 0);
        assert!(probe.calls().is_empty(), "probe must not be dispatched");

        gate.release();
    }

    #[tokio::test]
    async fn liveness_admission_never_exceeds_sixteen_concurrent_probes() {
        let descriptors = (0..32)
            .map(|index| descriptor(19100 + index, RUN_A))
            .collect::<Vec<_>>();
        let gate = Arc::new(Semaphore::new(0));
        let probe = FakeProbe::gated(&descriptors, Arc::clone(&gate));
        let broker = broker(FakeRegistry::new(descriptors), Arc::clone(&probe));
        let task = tokio::spawn(async move {
            broker
                .discover(client(), deadline(), CancellationToken::new())
                .await
        });

        wait_for_active(&probe, 16).await;
        assert_eq!(probe.maximum_active(), 16);
        gate.add_permits(32);
        let report = task.await.expect("discovery task").expect("discovery");
        assert_eq!(report.live_instances.len(), 32);
        assert_eq!(probe.maximum_active(), 16);
    }

    #[tokio::test]
    async fn cancellation_interrupts_waiting_for_a_liveness_permit() {
        let descriptors = (0..17)
            .map(|index| descriptor(19200 + index, RUN_A))
            .collect::<Vec<_>>();
        let gate = Arc::new(Semaphore::new(0));
        let probe = FakeProbe::gated(&descriptors, Arc::clone(&gate));
        let registry =
            FakeRegistry::with_scan_candidates(descriptors[..16].to_vec(), descriptors.clone());
        let broker = broker(registry, Arc::clone(&probe));
        let blocker = {
            let broker = broker.clone();
            tokio::spawn(async move {
                broker
                    .discover(client(), deadline(), CancellationToken::new())
                    .await
            })
        };
        wait_for_active(&probe, 16).await;

        let cancelled = CancellationToken::new();
        let targeted = {
            let broker = broker.clone();
            let cancelled = cancelled.clone();
            tokio::spawn(async move {
                broker
                    .resolve(
                        InstanceSelector {
                            proxy_endpoint: Some(endpoint(19216)),
                            run_id: None,
                        },
                        SelectorRequirement::SnapshotRead,
                        deadline(),
                        cancelled,
                    )
                    .await
            })
        };
        tokio::task::yield_now().await;
        cancelled.cancel();
        let error = targeted
            .await
            .expect("targeted task")
            .expect_err("cancelled while queued");
        assert_eq!(error.code(), ControlErrorCode::Cancelled);
        assert!(
            !probe
                .calls()
                .iter()
                .any(|call| call.endpoint == endpoint(19216))
        );

        gate.add_permits(16);
        blocker
            .await
            .expect("blocker task")
            .expect("blocker discovery");
    }

    #[tokio::test(start_paused = true)]
    async fn discovery_uses_one_outer_deadline_across_probe_waves() {
        let descriptors = (0..32)
            .map(|index| descriptor(19300 + index, RUN_A))
            .collect::<Vec<_>>();
        let probe = FakeProbe::delayed(&descriptors, Duration::from_millis(80));
        let broker = broker(FakeRegistry::new(descriptors), Arc::clone(&probe));
        let started = Instant::now();

        let error = broker
            .discover(
                client(),
                started + Duration::from_millis(100),
                CancellationToken::new(),
            )
            .await
            .expect_err("outer deadline");

        assert_eq!(error.code(), ControlErrorCode::DeadlineExceeded);
        assert_eq!(
            Instant::now().duration_since(started),
            Duration::from_millis(100)
        );
        assert_eq!(probe.maximum_active(), 16);
    }

    #[tokio::test]
    async fn list_instances_enriches_descriptors_with_live_describe_status() {
        let descriptor = descriptor(19001, RUN_A);
        let broker = broker(
            FakeRegistry::new(vec![descriptor.clone()]),
            FakeProbe::live(&[descriptor]),
        );

        let result = broker
            .list_instances_impl(client(), deadline(), CancellationToken::new())
            .await
            .expect("list instances");
        let listed = result.instances.first().expect("listed instance");

        assert_eq!(listed.instance.proxy_endpoint, Some(endpoint(19001)));
        assert_eq!(
            listed
                .instance
                .run_id
                .as_ref()
                .expect("listed run ID")
                .as_str(),
            RUN_A
        );
        assert_eq!(listed.local_proxy_url, "http://127.0.0.1:19001");
        assert_eq!(listed.fluxcope_version, "9.8.7-test");
        assert_eq!(listed.rpc_version, RPC_VERSION);
        assert_eq!(listed.config_mode, ConfigMode::Temporary);
        assert_eq!(listed.persistence, PersistenceMode::Ephemeral);
        assert_eq!(listed.config_source, None);
        assert!(listed.recording_enabled);
        assert_eq!(listed.retained_capture_count, 19001);
        assert_eq!(listed.settings_revision, 41);
    }

    #[tokio::test]
    async fn negotiated_client_metadata_is_reused_but_each_tool_opens_a_fresh_private_call() {
        let descriptor = descriptor(19001, RUN_A);
        let probe = FakeProbe::live(std::slice::from_ref(&descriptor));
        let broker = broker(FakeRegistry::new(vec![descriptor]), Arc::clone(&probe));
        let (client_transport, server_transport) = duplex(64 * 1024);
        let server = tokio::spawn(async move {
            let service = broker.serve(server_transport).await.expect("serve broker");
            service.waiting().await.expect("broker shutdown");
        });
        let declared = client();
        let client_info = ClientInfo::new(
            ClientCapabilities::default(),
            Implementation::new(declared.name.clone(), declared.version.clone()),
        )
        .with_protocol_version(ProtocolVersion::V_2024_11_05);
        let client_peer = client_info
            .serve(client_transport)
            .await
            .expect("initialize client");
        let arguments = object!({
            "instance": {
                "proxy_endpoint": "127.0.0.1:19001",
                "run_id": RUN_A
            }
        });

        for _ in 0..2 {
            client_peer
                .call_tool(
                    CallToolRequestParams::new("get_status").with_arguments(arguments.clone()),
                )
                .await
                .expect("get status");
        }
        client_peer
            .call_tool(CallToolRequestParams::new("list_instances"))
            .await
            .expect("list instances");
        client_peer.cancel().await.expect("close client");
        server.await.expect("server task");

        let calls = probe.calls();
        assert_eq!(calls.len(), 5);
        assert!(
            calls
                .iter()
                .all(|call| call.endpoint == endpoint(19001) && call.client == declared)
        );
    }

    #[tokio::test(start_paused = true)]
    async fn resolution_honors_deadline_and_cancellation_bounds() {
        let descriptor = descriptor(19001, RUN_A);
        let gate = Arc::new(Semaphore::new(0));
        let probe = FakeProbe::gated(std::slice::from_ref(&descriptor), gate);
        let broker = broker(FakeRegistry::new(vec![descriptor]), probe);
        let started = Instant::now();
        let error = broker
            .resolve(
                InstanceSelector::default(),
                SelectorRequirement::SnapshotRead,
                started + Duration::from_millis(25),
                CancellationToken::new(),
            )
            .await
            .expect_err("deadline");
        assert_eq!(error.code(), ControlErrorCode::DeadlineExceeded);
        assert_eq!(
            Instant::now().duration_since(started),
            Duration::from_millis(25)
        );

        let cancelled = CancellationToken::new();
        cancelled.cancel();
        let error = broker
            .resolve(
                InstanceSelector::default(),
                SelectorRequirement::SnapshotRead,
                Instant::now() + Duration::from_secs(1),
                cancelled,
            )
            .await
            .expect_err("cancelled");
        assert_eq!(error.code(), ControlErrorCode::Cancelled);
    }

    #[tokio::test]
    async fn generic_instance_unavailable_does_not_authorize_stale_pruning() {
        let stale = descriptor(19500, RUN_STALE);
        let registry = FakeRegistry::new(vec![stale.clone()]);
        let probe = FakeProbe::live(std::slice::from_ref(&stale));
        probe.mark_unavailable(stale.proxy_endpoint());
        let broker = broker(Arc::clone(&registry), probe);

        let report = broker
            .discover(client(), deadline(), CancellationToken::new())
            .await
            .expect("generic probe failure is diagnostic");

        assert_eq!(report.stale_count, 0);
        assert_eq!(registry.prune_calls(), 0);
        assert_eq!(report.rejected.len(), 1);
        assert_eq!(report.rejected[0].code(), "instance_unavailable");
    }

    #[tokio::test]
    async fn run_only_selector_probes_only_matching_registry_candidates() {
        let target = descriptor(19501, RUN_A);
        let unrelated = [descriptor(19502, RUN_B), descriptor(19503, RUN_STALE)];
        let mut descriptors = unrelated.to_vec();
        descriptors.push(target.clone());
        let probe = FakeProbe::live(&descriptors);
        let broker = broker(FakeRegistry::new(descriptors), Arc::clone(&probe));

        let resolved = resolve(
            &broker,
            InstanceSelector {
                proxy_endpoint: None,
                run_id: Some(RunId::from_str(RUN_A).expect("run ID")),
            },
            SelectorRequirement::SnapshotRead,
        )
        .await
        .expect("run-only target");

        assert_eq!(
            resolved.descriptor.proxy_endpoint(),
            target.proxy_endpoint()
        );
        assert_eq!(
            probe.calls(),
            vec![ProbeCall {
                endpoint: target.proxy_endpoint(),
                client: DeclaredClient {
                    name: "fluxcope-internal".to_owned(),
                    version: env!("CARGO_PKG_VERSION").to_owned(),
                },
            }]
        );
    }

    #[tokio::test]
    async fn endpoint_generation_mismatch_is_rejected_before_any_probe() {
        let current = descriptor(19504, RUN_A);
        let probe = FakeProbe::live(std::slice::from_ref(&current));
        let broker = broker(FakeRegistry::new(vec![current.clone()]), Arc::clone(&probe));

        let error = resolve(
            &broker,
            InstanceSelector {
                proxy_endpoint: Some(current.proxy_endpoint()),
                run_id: Some(RunId::from_str(RUN_B).expect("run ID")),
            },
            SelectorRequirement::SnapshotRead,
        )
        .await
        .expect_err("generation conflict");

        assert_eq!(error.code(), ControlErrorCode::InstanceGenerationConflict);
        assert!(probe.calls().is_empty());
    }

    #[tokio::test]
    async fn a_large_discovery_wave_does_not_prequeue_ahead_of_a_targeted_call() {
        let descriptors = (0..40)
            .map(|index| descriptor(19600 + index, RUN_A))
            .collect::<Vec<_>>();
        let target = descriptors.last().expect("target descriptor").clone();
        let target_endpoint = target.proxy_endpoint();
        let gate = Arc::new(Semaphore::new(0));
        let probe = FakeProbe::gated(&descriptors, Arc::clone(&gate));
        let broker = broker(FakeRegistry::new(descriptors), Arc::clone(&probe));
        let bulk_cancelled = CancellationToken::new();
        let targeted_cancelled = CancellationToken::new();
        let bulk = {
            let broker = broker.clone();
            let cancelled = bulk_cancelled.clone();
            tokio::spawn(async move { broker.discover(client(), deadline(), cancelled).await })
        };
        wait_for_active(&probe, 16).await;
        // Finish the blocking registry read before testing probe queue ordering.
        let target_scan = broker
            .scan(
                Some(target_endpoint),
                deadline(),
                targeted_cancelled.clone(),
            )
            .await
            .expect("targeted registry scan");
        let mut targeted = Box::pin({
            let broker = broker.clone();
            let cancelled = targeted_cancelled.clone();
            async move {
                broker
                    .probe_scan(target_scan, client(), deadline(), cancelled)
                    .await
            }
        });
        // Poll through admission so the target is queued before a bulk slot opens.
        assert!(futures::poll!(targeted.as_mut()).is_pending());
        let targeted = tokio::spawn(targeted);

        gate.add_permits(1);
        tokio::time::timeout(Duration::from_millis(100), async {
            while !probe
                .calls()
                .iter()
                .any(|call| call.endpoint == target_endpoint)
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("targeted call receives a probe turn after one bulk completion");

        bulk_cancelled.cancel();
        targeted_cancelled.cancel();
        let _ = bulk.await;
        let _ = targeted.await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancelled_and_expired_blocking_scans_retain_shared_scan_admission() {
        let gate = Arc::new(BlockingGate::default());
        let registry = Arc::new(BlockingScanRegistry {
            gate: Arc::clone(&gate),
            started: AtomicUsize::new(0),
        });
        let broker = Broker::with_dependencies(
            PathBuf::from("/test/.fluxcope/run/instances"),
            Arc::clone(&registry),
            FakeProbe::live(&[]),
        );
        let mut calls = Vec::new();
        for index in 0..32 {
            let broker = broker.clone();
            let cancelled = CancellationToken::new();
            let call_cancelled = cancelled.clone();
            let call_deadline = if index < 16 {
                deadline()
            } else {
                Instant::now() + Duration::from_millis(25)
            };
            let task = tokio::spawn(async move {
                broker
                    .discover(client(), call_deadline, call_cancelled)
                    .await
            });
            calls.push((cancelled, task));
        }
        tokio::time::timeout(Duration::from_secs(1), async {
            while registry.started.load(Ordering::SeqCst) != 32 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("all admitted blocking scans started");
        for (cancelled, _) in calls.iter().take(16) {
            cancelled.cancel();
        }
        for (_, task) in calls {
            let _ = task.await;
        }

        let extra_cancelled = CancellationToken::new();
        let extra = {
            let broker = broker.clone();
            let cancelled = extra_cancelled.clone();
            tokio::spawn(async move { broker.discover(client(), deadline(), cancelled).await })
        };
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(
            registry.started.load(Ordering::SeqCst),
            32,
            "cancelled or expired blocking work must retain admission until it exits"
        );

        extra_cancelled.cancel();
        gate.release();
        let _ = extra.await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancelled_cleanup_cannot_commit_after_waiting_on_a_registry_lock() {
        let stale = descriptor(19700, RUN_STALE);
        let gate = Arc::new(BlockingGate::default());
        let registry = Arc::new(BlockingPruneRegistry {
            descriptor: stale.clone(),
            gate: Arc::clone(&gate),
            prune_started: AtomicBool::new(false),
            prune_committed: AtomicUsize::new(0),
        });
        let probe = FakeProbe::live(std::slice::from_ref(&stale));
        probe.mark_stale_connect(stale.proxy_endpoint());
        let broker = Broker::with_dependencies(
            PathBuf::from("/test/.fluxcope/run/instances"),
            Arc::clone(&registry),
            probe,
        );
        let cancelled = CancellationToken::new();
        let call = {
            let cancelled = cancelled.clone();
            tokio::spawn(async move { broker.discover(client(), deadline(), cancelled).await })
        };
        tokio::time::timeout(Duration::from_secs(1), async {
            while !registry.prune_started.load(Ordering::SeqCst) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("cleanup reached blocked registry mutation");

        cancelled.cancel();
        let _ = call.await;
        gate.release();
        tokio::time::sleep(Duration::from_millis(50)).await;

        assert_eq!(
            registry.prune_committed.load(Ordering::SeqCst),
            0,
            "cleanup must recheck cancellation immediately before mutation"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn contended_stale_cleanup_degrades_after_its_short_mutation_wait_budget() {
        let stale = descriptor(19702, RUN_STALE);
        let gate = Arc::new(BlockingGate::default());
        let registry = Arc::new(BlockingPruneRegistry {
            descriptor: stale.clone(),
            gate: Arc::clone(&gate),
            prune_started: AtomicBool::new(false),
            prune_committed: AtomicUsize::new(0),
        });
        let probe = FakeProbe::live(std::slice::from_ref(&stale));
        probe.mark_stale_connect(stale.proxy_endpoint());
        let broker = Broker::with_dependencies(
            PathBuf::from("/test/.fluxcope/run/instances"),
            Arc::clone(&registry),
            probe,
        );

        let report = tokio::time::timeout(
            Duration::from_secs(1),
            broker.discover(
                client(),
                Instant::now() + Duration::from_secs(5),
                CancellationToken::new(),
            ),
        )
        .await
        .expect("stale cleanup must not consume the ordinary call deadline")
        .expect("cleanup lock contention should degrade to a diagnostic");
        gate.release();

        assert_eq!(report.rejected.len(), 1);
        assert_eq!(report.rejected[0].code(), "stale_cleanup_failed");
        assert!(
            report.rejected[0]
                .message()
                .contains("mutation lock wait limit")
        );
    }
    #[tokio::test]
    async fn duplicate_stale_probe_results_are_pruned_once_per_identity() {
        let stale = descriptor(19701, RUN_STALE);
        let registry = FakeRegistry::new(vec![stale.clone(), stale.clone()]);
        let probe = FakeProbe::live(std::slice::from_ref(&stale));
        probe.mark_stale_connect(stale.proxy_endpoint());
        let broker = broker(Arc::clone(&registry), probe);

        let report = broker
            .discover(client(), deadline(), CancellationToken::new())
            .await
            .expect("stale discovery");

        assert_eq!(report.stale_count, 1);
        assert_eq!(registry.prune_calls(), 1);
    }

    #[tokio::test]
    async fn discovery_does_not_reserialize_results_to_estimate_transport_bytes() {
        let live = descriptor(19702, RUN_A);
        let broker = broker(
            FakeRegistry::new(vec![live.clone()]),
            FakeProbe::live(&[live]),
        );

        broker
            .discover(client(), deadline(), CancellationToken::new())
            .await
            .expect("live discovery");

        assert_eq!(broker.telemetry.snapshot().bytes_relayed, 0);
    }

    #[tokio::test]
    async fn forwarded_call_records_definitive_connection_failure() {
        let instance = descriptor(19704, RUN_A);
        let probe = FakeProbe::live(std::slice::from_ref(&instance));
        probe.mark_stale_connect(instance.proxy_endpoint());
        let broker = broker(FakeRegistry::new(Vec::new()), probe);

        let error = broker
            .call_instance(
                &instance,
                ControlOperation::DescribeInstance,
                client(),
                deadline(),
                CancellationToken::new(),
            )
            .await
            .expect_err("stale connection");

        assert!(error.is_definitive_stale_connect());
        assert_eq!(broker.telemetry.snapshot().connection_failures, 1);
    }

    #[tokio::test]
    async fn forwarded_call_records_client_cancellation() {
        let instance = descriptor(19705, RUN_A);
        let broker = broker(
            FakeRegistry::new(Vec::new()),
            FakeProbe::live(std::slice::from_ref(&instance)),
        );
        let cancelled = CancellationToken::new();
        cancelled.cancel();

        let error = broker
            .call_instance(
                &instance,
                ControlOperation::DescribeInstance,
                client(),
                deadline(),
                cancelled,
            )
            .await
            .expect_err("cancelled call");

        assert_eq!(error.code(), ControlErrorCode::Cancelled);
        assert_eq!(broker.telemetry.snapshot().cancelled_calls, 1);
    }
    #[test]
    fn status_warns_when_private_rpc_responses_were_not_delivered() {
        let warnings = status_warnings(
            Default::default(),
            Default::default(),
            &crate::control::audit::InstanceAuditSnapshot {
                response_delivery_failures: 1,
                ..Default::default()
            },
        );

        assert!(
            warnings
                .iter()
                .any(|warning| warning.code == "response_delivery_failed")
        );
    }

    #[tokio::test]
    async fn probe_contract_borrows_large_descriptors_instead_of_cloning_them() {
        let descriptor = descriptor(19703, RUN_A);
        let probe = FakeProbe::live(std::slice::from_ref(&descriptor));

        probe
            .describe(&descriptor, client(), deadline(), CancellationToken::new())
            .await
            .expect("borrowed descriptor probe");
    }
    mod mcp {
        pub(super) mod capture {
            use super::super::*;
            use crate::{
                capture::{CaptureRecord, CaptureSequence, CaptureSnapshotMode, CapturedExchange},
                control::capture_query::{CaptureQuery, CompiledCaptureQuery, match_capture_page},
                control_rpc::protocol::ControlOperation,
            };
            use hyper::Method;
            use serde_json::{Map, Value, json};

            struct CaptureProbe;

            impl InstanceProbe for CaptureProbe {
                fn describe<'a>(
                    &'a self,
                    descriptor: &'a InstanceDescriptor,
                    _client: DeclaredClient,
                    _deadline: Instant,
                    _cancelled: CancellationToken,
                ) -> Pin<Box<dyn Future<Output = Result<ControlResult, ControlError>> + Send + 'a>>
                {
                    Box::pin(async move { Ok(describe(descriptor, 1)) })
                }

                fn call<'a>(
                    &'a self,
                    descriptor: &'a InstanceDescriptor,
                    operation: ControlOperation,
                    _client: DeclaredClient,
                    _deadline: Instant,
                    cancelled: CancellationToken,
                ) -> Pin<Box<dyn Future<Output = Result<ControlResult, ControlError>> + Send + 'a>>
                {
                    Box::pin(async move {
                        let instance = InstanceScope {
                            proxy_endpoint: descriptor.proxy_endpoint(),
                            run_id: descriptor.run_id().clone(),
                        };
                        match operation {
                            ControlOperation::SearchCaptures {
                                query,
                                cursor,
                                limit,
                            } => {
                                let sequence = if descriptor.proxy_endpoint().port() == 19801 {
                                    11
                                } else {
                                    22
                                };
                                let record = CaptureRecord::from_completed(CapturedExchange {
                                    sequence: CaptureSequence::new(sequence),
                                    method: Method::GET,
                                    uri: format!("https://instance-{sequence}.example/capture"),
                                    mapped_uri: None,
                                    local_path: None,
                                    status: Some(200),
                                    req_headers: vec![],
                                    res_headers: vec![],
                                    req_body: None,
                                    res_body: None,
                                });
                                let snapshots =
                                    vec![record.snapshot(CaptureSnapshotMode::MetadataOnly)];
                                let page = match_capture_page(
                                    &snapshots,
                                    &CompiledCaptureQuery::compile(*query)?,
                                    cursor,
                                    limit.unwrap_or(20),
                                    &cancelled,
                                )?;
                                Ok(ControlResult::SearchCaptures {
                                    instance,
                                    captures: page.captures,
                                    next_cursor: page.next_cursor,
                                })
                            }
                            ControlOperation::SetRecordingEnabled { enabled } => {
                                Ok(ControlResult::SetRecordingEnabled {
                                    instance,
                                    previous: !enabled,
                                    current: enabled,
                                })
                            }
                            other => Err(ControlError::invalid_argument(format!(
                                "unexpected capture probe operation: {other:?}"
                            ))),
                        }
                    })
                }
            }

            fn arguments(value: Value) -> Map<String, Value> {
                value.as_object().expect("tool argument object").clone()
            }

            fn tool_json(result: &rmcp::model::CallToolResult) -> Value {
                result
                    .structured_content
                    .clone()
                    .expect("structured tool result")
            }

            #[tokio::test]
            async fn child_transport_enforces_selection_and_never_cross_routes_capture_rows() {
                let first = descriptor(19801, RUN_A);
                let second = descriptor(19802, RUN_B);
                let registry = FakeRegistry::new(vec![first.clone(), second.clone()]);
                let broker = Broker::with_dependencies(
                    PathBuf::from("/test/.fluxcope/run/instances"),
                    registry,
                    Arc::new(CaptureProbe),
                );
                let (client_transport, server_transport) = duplex(64 * 1024);
                let server = tokio::spawn(async move {
                    let service = broker.serve(server_transport).await.expect("serve broker");
                    service.waiting().await.expect("broker shutdown");
                });
                let client = ClientInfo::new(
                    ClientCapabilities::default(),
                    Implementation::new("capture-routing-test", "1.0"),
                )
                .with_protocol_version(ProtocolVersion::LATEST)
                .serve(client_transport)
                .await
                .expect("initialize client");

                client
                    .call_tool(
                        CallToolRequestParams::new("search_captures")
                            .with_arguments(arguments(json!({"query": {}}))),
                    )
                    .await
                    .expect_err("multiple instances require a selector");

                for (descriptor, expected_sequence) in [(first, 11), (second, 22)] {
                    let search = client
                        .call_tool(
                            CallToolRequestParams::new("search_captures").with_arguments(
                                arguments(json!({
                                    "instance": {
                                        "proxy_endpoint": descriptor.proxy_endpoint(),
                                        "run_id": descriptor.run_id()
                                    },
                                    "query": CaptureQuery::default(),
                                    "limit": 10
                                })),
                            ),
                        )
                        .await
                        .expect("selected search");
                    let value = tool_json(&search);
                    assert_eq!(
                        value["instance"]["proxy_endpoint"],
                        descriptor.proxy_endpoint().to_string()
                    );
                    assert_eq!(value["instance"]["run_id"], descriptor.run_id().to_string());
                    assert_eq!(value["captures"][0]["capture_sequence"], expected_sequence);
                }

                let mutation = client
                    .call_tool(
                        CallToolRequestParams::new("set_recording_enabled").with_arguments(
                            arguments(json!({
                                "instance": {
                                    "proxy_endpoint": "127.0.0.1:19801",
                                    "run_id": RUN_A
                                },
                                "enabled": true
                            })),
                        ),
                    )
                    .await
                    .expect("selected mutation");
                assert_eq!(
                    tool_json(&mutation),
                    json!({
                        "instance": {
                            "proxy_endpoint": "127.0.0.1:19801",
                            "run_id": RUN_A
                        },
                        "previous": false,
                        "current": true
                    })
                );

                client.cancel().await.expect("close client");
                server.await.expect("server task");
            }
        }
    }

    #[tokio::test]
    async fn oversized_public_mapping_is_rejected_before_instance_resolution() {
        let registry = FakeRegistry::new(Vec::new());
        let broker = broker(Arc::clone(&registry), FakeProbe::live(&[]));
        let rules = (0..10_000)
            .map(|index| {
                json!({
                    "from": format!("https://source-{index}.example.com/{}", "x".repeat(40)),
                    "to": format!("https://target-{index}.example.com/{}", "y".repeat(40)),
                    "enabled": true
                })
            })
            .collect::<Vec<_>>();
        let input = serde_json::from_value::<ValidateMappingSettingsInput>(json!({
            "proxy": {
                "enabled": true,
                "active_preset": "large",
                "presets": [{
                    "name": "large",
                    "map_remote": {
                        "enabled": true,
                        "rules": rules
                    },
                    "map_local": {
                        "enabled": true,
                        "rules": []
                    }
                }]
            }
        }))
        .expect("large public mapping input");

        let error = broker
            .validate_mapping_settings_impl(
                input,
                client(),
                Instant::now() + Duration::from_secs(5),
                CancellationToken::new(),
            )
            .await
            .expect_err("oversized public mapping must fail before resolution");

        assert_eq!(error.code(), ControlErrorCode::RpcFrameTooLarge);
        assert_eq!(registry.scan_calls.load(Ordering::SeqCst), 0);
        assert_eq!(registry.endpoint_calls.load(Ordering::SeqCst), 0);
        assert_eq!(
            broker.mapping_conversion_admission.available_permits(),
            super::MAPPING_CONVERSION_LIMIT
        );
    }

    mod body_resources;
    mod capture_wait;
    mod json_inspection;
    mod search_extract;

    #[test]
    fn every_control_error_code_has_a_stable_public_conversion() {
        let codes = [
            ControlErrorCode::InvalidArgument,
            ControlErrorCode::NoInstances,
            ControlErrorCode::InstanceRequired,
            ControlErrorCode::InstanceNotFound,
            ControlErrorCode::InstanceGenerationConflict,
            ControlErrorCode::InstanceUnavailable,
            ControlErrorCode::RpcVersionMismatch,
            ControlErrorCode::RpcFrameTooLarge,
            ControlErrorCode::ResourceLimit,
            ControlErrorCode::CaptureNotFound,
            ControlErrorCode::CaptureRevisionConflict,
            ControlErrorCode::SettingsRevisionConflict,
            ControlErrorCode::MappingValidationFailed,
            ControlErrorCode::TuiDraftConflict,
            ControlErrorCode::ServiceUnavailable,
            ControlErrorCode::UnsupportedBodyEncoding,
            ControlErrorCode::DeadlineExceeded,
            ControlErrorCode::Cancelled,
            ControlErrorCode::UndecodableBody,
            ControlErrorCode::BodyNotTextual,
            ControlErrorCode::NotFound,
            ControlErrorCode::MalformedJson,
            ControlErrorCode::JsonDepthLimit,
            ControlErrorCode::JsonSizeLimit,
            ControlErrorCode::InternalError,
        ];

        for code in codes {
            let converted = to_mcp_error(ControlError::new(
                code,
                "stable message",
                false,
                json!({"field": "value"}),
            ));
            let data = converted.data.expect("structured error data");
            assert_eq!(data["code"], code.as_str(), "{code:?}");
            assert_eq!(data["retryable"], false, "{code:?}");
            assert_eq!(data["details"]["field"], "value", "{code:?}");
        }
    }
}
