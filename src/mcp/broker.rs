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
    model::{ErrorCode, Implementation, ProtocolVersion, ServerCapabilities, ServerInfo},
    service::RequestContext,
    tool, tool_handler, tool_router,
};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use crate::{
    control_rpc::{
        client::ControlRpcClient,
        protocol::{
            ControlError, ControlErrorCode, ControlOperation, ControlResult, DeclaredClient,
            RPC_VERSION,
        },
    },
    instance_registry::{DiscoveryDiagnostic, InstanceDescriptor, RegistryScan, RegistryScanner},
};

use super::{
    capture::{
        SearchCapturesInput, SearchCapturesResult, SetRecordingEnabledInput,
        SetRecordingEnabledResult, recording_result, search_result,
    },
    schema::{
        BrokerLimits, BrokerStatusResult, BrokerVersions, DiscoverySummary, GetStatusInput,
        GetStatusResult, InstanceSelector, InstanceSummary, ListInstancesResult, RegistryStatus,
        RejectedDescriptor, TransportStatus,
    },
    telemetry::{ActivityGuard, BrokerTelemetry},
};

const PUBLIC_CALL_LIMIT: usize = 32;
const BLOCKING_SCAN_LIMIT: usize = 32;
const LIVENESS_PROBE_LIMIT: usize = 16;
const AMBIGUOUS_INSTANCE_LIMIT: usize = 16;
const ORDINARY_DEADLINE: Duration = Duration::from_secs(30);
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
                            instance,
                            config_mode,
                            persistence,
                            recording_enabled,
                            retained_capture_count,
                            settings_revision,
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
    cancelled: CancellationToken,
) -> Result<(SearchCapturesInput, PublicCallPermit), ControlError> {
    run_public_search_validation(permit, cancelled, move || {
        input.validate()?;
        Ok(input)
    })
    .await
}

async fn run_public_search_validation<F>(
    permit: PublicCallPermit,
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
        _ = cancelled.cancelled() => {
            Err(ControlError::cancelled("capture search validation cancelled"))
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
    telemetry: Arc<BrokerTelemetry>,
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
    pub(crate) fn new(wirelens_home: &Path) -> io::Result<Self> {
        let registry = Arc::new(RegistryScanner::initialize(wirelens_home)?);
        Ok(Self::with_dependencies(
            wirelens_home.join("run").join("instances"),
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
                name: "wirelens-internal".to_owned(),
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
        if !stale.is_empty() {
            self.prune_stale_batch(stale, deadline, cancelled.clone())
                .await?;
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
        let task = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            registry.prune_batch_if_current(&descriptors, deadline, &cleanup_cancelled)
        });
        tokio::select! {
            biased;
            () = cancelled.cancelled() => {
                self.telemetry.record_cancelled_call();
                Err(ControlError::cancelled("stale cleanup cancelled"))
            }
            () = tokio::time::sleep_until(deadline) => {
                Err(ControlError::deadline_exceeded("stale cleanup deadline elapsed"))
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
                client,
                deadline,
                cancelled,
            )
            .await?;
        status_result(resolved.description)
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
            .probe
            .call(
                &resolved.descriptor,
                input.operation(),
                client,
                deadline,
                cancelled,
            )
            .await?;
        recording_result(result)
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
            .probe
            .call(
                &resolved.descriptor,
                input.operation(),
                client,
                deadline,
                cancelled,
            )
            .await?;
        search_result(result)
    }

    async fn get_broker_status_impl(
        &self,
        client: DeclaredClient,
        deadline: Instant,
        cancelled: CancellationToken,
    ) -> Result<BrokerStatusResult, McpDomainError> {
        let report = self.discover(client, deadline, cancelled).await?;
        let telemetry = self.telemetry.snapshot();
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
        })
    }

    #[tool(
        name = "list_instances",
        description = "List live MCP-enabled Wirelens proxy instances",
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
        description = "Get bounded Wirelens broker discovery and transport status",
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
        description = "Get identity, configuration, recording, and capture status for one Wirelens instance",
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
        description = "Explicitly enable or disable live recording for one Wirelens instance",
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
        let call = self.try_admit_public_call().map_err(to_mcp_error)?;
        let cancelled = context.ct.clone();
        let (input, _call) = validate_public_search_input(input, call, cancelled.clone())
            .await
            .map_err(to_mcp_error)?;
        let client = declared_client(&context).map_err(to_mcp_error)?;
        self.search_captures_impl(
            input,
            client,
            Instant::now() + ORDINARY_DEADLINE,
            context.ct.clone(),
        )
        .await
        .map(Json)
        .map_err(to_mcp_error)
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for Broker {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new(
                "wirelens",
                env!("CARGO_PKG_VERSION"),
            ))
            .with_protocol_version(ProtocolVersion::LATEST)
            .with_instructions(
                "Use list_instances first and select an explicit instance when more than one is live.",
            )
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
        wirelens_version: descriptor.binary_version().to_owned(),
        rpc_version: RPC_VERSION,
        config_mode,
        persistence,
        config_source: descriptor.config_source().map(Path::to_path_buf),
        recording_enabled,
        retained_capture_count,
        settings_revision,
    })
}

fn status_result(result: ControlResult) -> Result<GetStatusResult, ControlError> {
    let (
        instance,
        config_mode,
        persistence,
        recording_enabled,
        retained_capture_count,
        settings_revision,
    ) = match result {
        ControlResult::GetStatus {
            instance,
            config_mode,
            persistence,
            recording_enabled,
            retained_capture_count,
            settings_revision,
        }
        | ControlResult::DescribeInstance {
            instance,
            config_mode,
            persistence,
            recording_enabled,
            retained_capture_count,
            settings_revision,
        } => (
            instance,
            config_mode,
            persistence,
            recording_enabled,
            retained_capture_count,
            settings_revision,
        ),
        _ => {
            return Err(ControlError::internal(
                "private RPC returned an unexpected status result",
            ));
        }
    };
    Ok(GetStatusResult {
        instance: InstanceSelector {
            proxy_endpoint: Some(instance.proxy_endpoint),
            run_id: Some(instance.run_id),
        },
        config_mode,
        persistence,
        recording_enabled,
        retained_capture_count,
        settings_revision,
    })
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
    if error.code() == ControlErrorCode::InvalidArgument {
        ErrorData::invalid_params(error.message().to_owned(), Some(data))
    } else {
        ErrorData::new(ErrorCode(-32000), error.message().to_owned(), Some(data))
    }
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
    use tokio::time::Instant;
    use tokio::{
        io::duplex,
        sync::{Notify, Semaphore, mpsc},
    };
    use tokio_util::sync::CancellationToken;

    use super::{
        Broker, InstanceProbe, McpDomainError, RegistryAccess, SelectorRequirement,
        run_public_search_validation,
    };
    use crate::{
        control_rpc::{
            client::connect_failure,
            protocol::{
                ControlError, ControlErrorCode, ControlResult, DeclaredClient, InstanceScope,
            },
        },
        instance::RunId,
        instance_registry::{DiscoveryDiagnostic, InstanceDescriptor, RegistryScan},
        mcp::{capture::SearchCapturesInput, schema::InstanceSelector},
        settings::{ConfigMode, PersistenceMode},
    };

    const RUN_A: &str = "AAAAAAAAAAAAAAAAAAAAAA";
    const RUN_B: &str = "AQEBAQEBAQEBAQEBAQEBAQ";
    const RUN_STALE: &str = "AgICAgICAgICAgICAgICAg";

    #[derive(Clone)]
    enum ProbePlan {
        Live(ControlResult),
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
                        ProbePlan::Live(describe(
                            descriptor,
                            descriptor.proxy_endpoint().port() as usize,
                        )),
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
                        Some(ProbePlan::Live(result)) => Ok(result),
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
            Ok(descriptors.len())
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
            "rpc_version": 1,
            "binary_version": "9.8.7-test",
            "pid": u32::from(port),
            "proxy_endpoint": endpoint,
            "local_proxy_url": format!("http://{endpoint}"),
            "run_id": run_id,
            "started_at": "2026-08-24T00:00:00Z",
            "socket_path": format!("/tmp/wirelens-mcp-{port}.sock"),
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
            PathBuf::from("/test/.wirelens/run/instances"),
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
            run_public_search_validation(permit, task_cancelled, move || {
                started_tx.send(()).expect("validation started");
                worker_gate.wait();
                Ok(SearchCapturesInput::default())
            })
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
        assert_eq!(listed.wirelens_version, "9.8.7-test");
        assert_eq!(listed.rpc_version, 1);
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

        assert_eq!(
            probe.calls(),
            vec![
                ProbeCall {
                    endpoint: endpoint(19001),
                    client: declared.clone()
                },
                ProbeCall {
                    endpoint: endpoint(19001),
                    client: declared.clone()
                },
                ProbeCall {
                    endpoint: endpoint(19001),
                    client: declared
                },
            ]
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
                    name: "wirelens-internal".to_owned(),
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
        let target_run_id = target.run_id().clone();
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
        let targeted = {
            let broker = broker.clone();
            let cancelled = targeted_cancelled.clone();
            tokio::spawn(async move {
                broker
                    .resolve(
                        InstanceSelector {
                            proxy_endpoint: Some(target_endpoint),
                            run_id: Some(target_run_id),
                        },
                        SelectorRequirement::SnapshotRead,
                        deadline(),
                        cancelled,
                    )
                    .await
            })
        };
        tokio::task::yield_now().await;

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
            PathBuf::from("/test/.wirelens/run/instances"),
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
            PathBuf::from("/test/.wirelens/run/instances"),
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
                    PathBuf::from("/test/.wirelens/run/instances"),
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
}
