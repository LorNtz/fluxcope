use crate::{
    capture::CaptureSequence,
    control::capture_query::{CaptureDetail, CaptureQuery, CaptureSearchCursor, CompactCapture},
    instance::RunId,
    settings::{ConfigMode, PersistenceMode},
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, value::RawValue};
use std::{
    net::SocketAddr,
    time::{Duration, Instant},
};

pub(crate) const RPC_VERSION: u16 = 1;
const MAX_IDENTIFIER_BYTES: usize = 128;
const ORDINARY_MAX_DEADLINE: Duration = Duration::from_secs(30);

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DeclaredClient {
    pub(crate) name: String,
    pub(crate) version: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ControlOperationKind {
    DescribeInstance,
    GetStatus,
    SetRecordingEnabled,
    SearchCaptures,
    GetCapture,
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ControlRequest {
    pub(crate) request_id: String,
    pub(crate) run_id: RunId,
    pub(crate) deadline: Instant,
    pub(crate) client: DeclaredClient,
    pub(crate) operation: ControlOperation,
}

#[derive(Clone, Debug, Eq, PartialEq)]
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

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct InstanceScope {
    pub(crate) proxy_endpoint: SocketAddr,
    pub(crate) run_id: RunId,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
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
        config_mode: ConfigMode,
        persistence: PersistenceMode,
        recording_enabled: bool,
        retained_capture_count: usize,
        settings_revision: u64,
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
}

impl ControlResult {
    pub(crate) fn instance_scope(&self) -> &InstanceScope {
        match self {
            Self::DescribeInstance { instance, .. }
            | Self::GetStatus { instance, .. }
            | Self::SetRecordingEnabled { instance, .. }
            | Self::SearchCaptures { instance, .. }
            | Self::GetCapture { instance, .. } => instance,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
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
    ServiceUnavailable,
    DeadlineExceeded,
    Cancelled,
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
            Self::DeadlineExceeded => "deadline_exceeded",
            Self::Cancelled => "cancelled",
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
            "no live Wirelens instances are available",
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
    pub(crate) fn new(
        request_id: String,
        run_id: RunId,
        deadline_ms: u64,
        client: DeclaredClient,
        operation: ControlOperation,
    ) -> Result<Self, ControlError> {
        let (operation, arguments) = match operation {
            ControlOperation::DescribeInstance => (
                ControlOperationKind::DescribeInstance,
                serialize_arguments(&DescribeInstanceArguments {})?,
            ),
            ControlOperation::GetStatus => (
                ControlOperationKind::GetStatus,
                serialize_arguments(&GetStatusArguments {})?,
            ),
            ControlOperation::SetRecordingEnabled { enabled } => (
                ControlOperationKind::SetRecordingEnabled,
                serialize_arguments(&SetRecordingEnabledArguments { enabled })?,
            ),
            ControlOperation::SearchCaptures {
                query,
                cursor,
                limit,
            } => (
                ControlOperationKind::SearchCaptures,
                serialize_arguments(&SearchCapturesArguments {
                    query: *query,
                    cursor,
                    limit,
                })?,
            ),
            ControlOperation::GetCapture {
                capture_id,
                expected_revision,
            } => (
                ControlOperationKind::GetCapture,
                serialize_arguments(&GetCaptureArguments {
                    capture_id,
                    expected_revision,
                })?,
            ),
        };
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
            ControlOperationKind::DescribeInstance
            | ControlOperationKind::GetStatus
            | ControlOperationKind::SetRecordingEnabled
            | ControlOperationKind::SearchCaptures
            | ControlOperationKind::GetCapture => ORDINARY_MAX_DEADLINE,
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

pub(crate) fn decode_request_payload(
    payload: &[u8],
    received_at: Instant,
) -> Result<ControlRequest, ControlError> {
    strict_from_slice::<RequestEnvelope>(payload)?.validate(received_at)
}

pub(crate) fn decode_response_payload(
    payload: &[u8],
    expected_request_id: &str,
    expected_scope: &InstanceScope,
) -> Result<ControlResult, ControlError> {
    strict_from_slice::<ResponseEnvelope>(payload)?.validate(expected_request_id, expected_scope)
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
