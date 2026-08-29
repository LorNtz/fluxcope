use crate::instance::RunId;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, value::RawValue};
use std::{
    net::SocketAddr,
    time::{Duration, Instant},
};

pub(crate) const RPC_VERSION: u16 = 1;
const MAX_IDENTIFIER_BYTES: usize = 128;
const DESCRIBE_INSTANCE_MAX_DEADLINE: Duration = Duration::from_secs(30);

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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ControlOperation {
    DescribeInstance,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DescribeInstanceArguments {}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct InstanceScope {
    pub(crate) proxy_endpoint: SocketAddr,
    pub(crate) run_id: RunId,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum ControlResult {
    DescribeInstance { instance: InstanceScope },
}

impl ControlResult {
    pub(crate) fn instance_scope(&self) -> &InstanceScope {
        match self {
            Self::DescribeInstance { instance } => instance,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ControlErrorCode {
    InvalidArgument,
    InstanceGenerationConflict,
    InstanceUnavailable,
    RpcVersionMismatch,
    RpcFrameTooLarge,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ControlError {
    pub(crate) code: ControlErrorCode,
    pub(crate) message: String,
    pub(crate) retryable: bool,
    pub(crate) details: Value,
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
        }
    }

    pub(crate) fn invalid_argument(message: impl Into<String>) -> Self {
        Self::new(
            ControlErrorCode::InvalidArgument,
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
                RawValue::from_string("{}".to_owned())
                    .map_err(|error| ControlError::invalid_argument(error.to_string()))?,
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

        let (operation, maximum_deadline) = match self.operation {
            ControlOperationKind::DescribeInstance => {
                parse_arguments::<DescribeInstanceArguments>(&self.arguments)?;
                (
                    ControlOperation::DescribeInstance,
                    DESCRIBE_INSTANCE_MAX_DEADLINE,
                )
            }
        };
        let declared = Duration::from_millis(self.deadline_ms);
        let deadline = received_at + declared.min(maximum_deadline);
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
