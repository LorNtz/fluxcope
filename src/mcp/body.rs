use std::{fmt, net::SocketAddr, str::FromStr};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use rmcp::model::{MetaObject, ResourceContents};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, json};
use url::Url;

use super::{capture::RequiredInstanceSelector, schema::InstanceSelector};
use crate::{
    capture::{BodySide, CaptureSequence},
    control::{
        body::{
            BodyContentRequest, BodyPage, BodyRepresentation, DEFAULT_BODY_PAGE_LENGTH,
            DEFAULT_BODY_SEARCH_CONTEXT_BYTES, DEFAULT_BODY_SEARCH_LIMIT,
            ExtractCaptureBodyRequest, ExtractCaptureBodyResult, ExtractSelector,
            MAX_BODY_PAGE_LENGTH, SearchCaptureBodyRequest, SearchCaptureBodyResult, SelectionPage,
            SelectionResourceRequest, parse_selection_resource_uri,
        },
        json_walk::{
            FieldMatchMode, FindJsonPointersRequest, FindJsonPointersResult, JsonPointerPattern,
            MAX_JSON_EXAMPLES, ProbeJsonPointerPatternRequest, ProbeJsonPointerPatternResult,
        },
    },
    control_rpc::protocol::{ControlError, ControlErrorCode},
    instance::RunId,
};

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FindJsonPointersInput {
    pub(crate) instance: RequiredInstanceSelector,
    pub(crate) capture_id: CaptureSequence,
    pub(crate) capture_revision: u64,
    #[schemars(with = "String")]
    pub(crate) side: BodySide,
    pub(crate) field_name: String,
    pub(crate) match_mode: FieldMatchMode,
    #[schemars(range(min = 1, max = 20))]
    pub(crate) limit: Option<usize>,
}

impl FindJsonPointersInput {
    pub(crate) fn selector(&self) -> InstanceSelector {
        self.instance.selector()
    }

    pub(crate) fn operation(
        &self,
    ) -> Result<crate::control_rpc::protocol::ControlOperation, ControlError> {
        let request = FindJsonPointersRequest {
            capture_id: self.capture_id,
            capture_revision: self.capture_revision,
            side: self.side,
            field_name: self.field_name.clone(),
            match_mode: self.match_mode,
            limit: self.limit.unwrap_or(MAX_JSON_EXAMPLES),
        };
        request.validate()?;
        Ok(crate::control_rpc::protocol::ControlOperation::FindJsonPointers(Box::new(request)))
    }
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProbeJsonPointerPatternInput {
    pub(crate) instance: RequiredInstanceSelector,
    pub(crate) capture_id: CaptureSequence,
    pub(crate) capture_revision: u64,
    #[schemars(with = "String")]
    pub(crate) side: BodySide,
    pub(crate) pattern: JsonPointerPattern,
}

impl ProbeJsonPointerPatternInput {
    pub(crate) fn selector(&self) -> InstanceSelector {
        self.instance.selector()
    }

    pub(crate) fn operation(
        &self,
    ) -> Result<crate::control_rpc::protocol::ControlOperation, ControlError> {
        let request = ProbeJsonPointerPatternRequest {
            capture_id: self.capture_id,
            capture_revision: self.capture_revision,
            side: self.side,
            pattern: self.pattern.clone(),
        };
        request.validate()?;
        Ok(
            crate::control_rpc::protocol::ControlOperation::ProbeJsonPointerPattern(Box::new(
                request,
            )),
        )
    }
}

#[derive(Clone, Debug, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FindJsonPointersOutput {
    pub(crate) instance: InstanceSelector,
    #[serde(flatten)]
    pub(crate) result: FindJsonPointersResult,
}

#[derive(Clone, Debug, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProbeJsonPointerPatternOutput {
    pub(crate) instance: InstanceSelector,
    #[serde(flatten)]
    pub(crate) result: ProbeJsonPointerPatternResult,
}
#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SearchCaptureBodyInput {
    pub(crate) instance: RequiredInstanceSelector,
    pub(crate) capture_id: CaptureSequence,
    pub(crate) capture_revision: u64,
    #[schemars(with = "String")]
    pub(crate) side: BodySide,
    #[schemars(length(min = 1, max = 8192))]
    pub(crate) query: String,
    #[schemars(range(min = 1, max = 50))]
    pub(crate) limit: Option<usize>,
    #[schemars(range(min = 0, max = 1024))]
    pub(crate) context_bytes: Option<usize>,
}

impl SearchCaptureBodyInput {
    pub(crate) fn selector(&self) -> InstanceSelector {
        self.instance.selector()
    }

    pub(crate) fn operation(
        &self,
    ) -> Result<crate::control_rpc::protocol::ControlOperation, ControlError> {
        let request = SearchCaptureBodyRequest {
            capture_id: self.capture_id,
            capture_revision: self.capture_revision,
            side: self.side,
            query: self.query.clone(),
            limit: self.limit.unwrap_or(DEFAULT_BODY_SEARCH_LIMIT),
            context_bytes: self
                .context_bytes
                .unwrap_or(DEFAULT_BODY_SEARCH_CONTEXT_BYTES),
        };
        request.validate()?;
        Ok(crate::control_rpc::protocol::ControlOperation::SearchCaptureBody(Box::new(request)))
    }
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ExtractCaptureBodyInput {
    pub(crate) instance: RequiredInstanceSelector,
    pub(crate) capture_id: CaptureSequence,
    pub(crate) capture_revision: u64,
    #[schemars(with = "String")]
    pub(crate) side: BodySide,
    pub(crate) selector: ExtractSelector,
}

impl ExtractCaptureBodyInput {
    pub(crate) fn selector(&self) -> InstanceSelector {
        self.instance.selector()
    }

    pub(crate) fn operation(
        &self,
    ) -> Result<crate::control_rpc::protocol::ControlOperation, ControlError> {
        let request = ExtractCaptureBodyRequest {
            capture_id: self.capture_id,
            capture_revision: self.capture_revision,
            side: self.side,
            selector: self.selector.clone(),
        };
        request.validate()?;
        Ok(crate::control_rpc::protocol::ControlOperation::ExtractCaptureBody(Box::new(request)))
    }
}

#[derive(Clone, Debug, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SearchCaptureBodyOutput {
    pub(crate) instance: InstanceSelector,
    #[serde(flatten)]
    pub(crate) result: SearchCaptureBodyResult,
}

#[derive(Clone, Debug, JsonSchema, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ExtractCaptureBodyOutput {
    pub(crate) instance: InstanceSelector,
    #[serde(flatten)]
    pub(crate) result: ExtractCaptureBodyResult,
}

pub(super) const CONTENT_RESOURCE_TEMPLATE: &str = "fluxcope://{+proxy_endpoint}/runs/{run_id}/captures/{capture_id}/revisions/{capture_revision}/bodies/{side}/content/{representation}{?offset,length}";
pub(super) const JSON_POINTER_RESOURCE_TEMPLATE: &str = "fluxcope://{+proxy_endpoint}/runs/{run_id}/captures/{capture_id}/revisions/{capture_revision}/bodies/{side}/extract/json-pointer{?pointer,offset,length}";
pub(super) const FORM_FIELD_RESOURCE_TEMPLATE: &str = "fluxcope://{+proxy_endpoint}/runs/{run_id}/captures/{capture_id}/revisions/{capture_revision}/bodies/{side}/extract/form-field{?key,offset,length}";

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct SelectionResourceUri(SelectionResourceRequest);

impl SelectionResourceUri {
    pub(super) fn parse(raw: &str) -> Result<Self, ControlError> {
        parse_selection_resource_uri(raw).map(Self)
    }

    pub(super) fn proxy_endpoint(&self) -> SocketAddr {
        self.0.proxy_endpoint
    }

    pub(super) fn run_id(&self) -> &RunId {
        &self.0.run_id
    }

    pub(super) fn request(&self) -> crate::control::body::SelectionContentRequest {
        self.0.content_request()
    }

    pub(super) fn as_request(&self) -> &SelectionResourceRequest {
        &self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct BodyResourceUri {
    uri: String,
    proxy_endpoint: SocketAddr,
    run_id: RunId,
    capture_id: CaptureSequence,
    capture_revision: u64,
    side: BodySide,
    representation: BodyRepresentation,
    offset: usize,
    length: usize,
}

impl BodyResourceUri {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn content(
        proxy_endpoint: SocketAddr,
        run_id: RunId,
        capture_id: CaptureSequence,
        capture_revision: u64,
        side: BodySide,
        representation: BodyRepresentation,
        offset: usize,
        length: usize,
    ) -> Self {
        let side_segment = side_segment(side);
        let representation_segment = representation_segment(representation);
        let capture_id_value = capture_id.value();
        let uri = format!(
            "fluxcope://{proxy_endpoint}/runs/{run_id}/captures/{capture_id_value}/revisions/{capture_revision}/bodies/{side_segment}/content/{representation_segment}?offset={offset}&length={length}"
        );
        Self {
            uri,
            proxy_endpoint,
            run_id,
            capture_id,
            capture_revision,
            side,
            representation,
            offset,
            length,
        }
    }

    pub(super) fn parse(raw: &str) -> Result<Self, ControlError> {
        let remainder = raw
            .strip_prefix("fluxcope://")
            .ok_or_else(|| invalid_uri("body resource URI is malformed"))?;
        let (authority, path_and_query) = remainder
            .split_once('/')
            .ok_or_else(|| invalid_uri("body resource URI is malformed"))?;
        let raw_path = path_and_query
            .split_once('?')
            .map_or(path_and_query, |(path, _)| path);
        if raw_path.contains('%')
            || raw_path
                .split('/')
                .any(|segment| segment == "." || segment == "..")
        {
            return Err(invalid_uri("body resource path is not canonical"));
        }
        let url = Url::parse(raw).map_err(|_| invalid_uri("body resource URI is malformed"))?;
        if url.scheme() != "fluxcope"
            || !url.username().is_empty()
            || url.password().is_some()
            || url.fragment().is_some()
        {
            return Err(invalid_uri("body resource URI is not canonical"));
        }
        let proxy_endpoint = authority
            .parse::<SocketAddr>()
            .map_err(|_| invalid_uri("body resource authority must be an IP endpoint"))?;
        if authority != proxy_endpoint.to_string() {
            return Err(invalid_uri("body resource authority is not canonical"));
        }

        let segments = url
            .path_segments()
            .ok_or_else(|| invalid_uri("body resource path is malformed"))?
            .collect::<Vec<_>>();
        if segments.len() != 10
            || segments[0] != "runs"
            || segments[2] != "captures"
            || segments[4] != "revisions"
            || segments[6] != "bodies"
            || segments[8] != "content"
        {
            return Err(invalid_uri("body resource path is not canonical"));
        }
        let run_id = RunId::from_str(segments[1])
            .map_err(|_| invalid_uri("body resource run ID is invalid"))?;
        let capture_id = CaptureSequence::new(parse_decimal_u64(segments[3], "capture ID")?);
        let capture_revision = parse_decimal_u64(segments[5], "capture revision")?;
        let side = match segments[7] {
            "request" => BodySide::Request,
            "response" => BodySide::Response,
            _ => return Err(invalid_uri("body resource side is invalid")),
        };
        let representation = match segments[9] {
            "raw" => BodyRepresentation::Raw,
            "decoded" => BodyRepresentation::Decoded,
            _ => return Err(invalid_uri("body resource representation is invalid")),
        };

        let mut offset = None;
        let mut length = None;
        if let Some(query) = url.query() {
            if query.is_empty() {
                return Err(invalid_uri("body resource query is empty"));
            }
            for pair in query.split('&') {
                let (name, value) = pair
                    .split_once('=')
                    .ok_or_else(|| invalid_uri("body resource query is malformed"))?;
                match name {
                    "offset" if offset.is_none() => {
                        offset = Some(parse_decimal_usize(value, "offset")?);
                    }
                    "length" if length.is_none() => {
                        length = Some(parse_decimal_usize(value, "length")?);
                    }
                    "offset" | "length" => {
                        return Err(invalid_uri("body resource query key is duplicated"));
                    }
                    _ => return Err(invalid_uri("body resource query key is unknown")),
                }
            }
        }
        let offset = offset.unwrap_or(0);
        let length = length.unwrap_or(DEFAULT_BODY_PAGE_LENGTH);
        if length == 0 || length > MAX_BODY_PAGE_LENGTH {
            return Err(invalid_uri("body resource page length is out of range"));
        }
        Ok(Self::content(
            proxy_endpoint,
            run_id,
            capture_id,
            capture_revision,
            side,
            representation,
            offset,
            length,
        ))
    }

    pub(super) fn as_str(&self) -> &str {
        &self.uri
    }

    pub(super) fn proxy_endpoint(&self) -> SocketAddr {
        self.proxy_endpoint
    }

    pub(super) fn run_id(&self) -> &RunId {
        &self.run_id
    }

    pub(super) fn capture_id(&self) -> CaptureSequence {
        self.capture_id
    }

    pub(super) fn capture_revision(&self) -> u64 {
        self.capture_revision
    }

    pub(super) fn side(&self) -> BodySide {
        self.side
    }

    pub(super) fn representation(&self) -> BodyRepresentation {
        self.representation
    }

    #[cfg(test)]
    pub(super) fn offset(&self) -> usize {
        self.offset
    }

    #[cfg(test)]
    pub(super) fn length(&self) -> usize {
        self.length
    }

    pub(super) fn next_page(&self, offset: usize) -> Self {
        Self::content(
            self.proxy_endpoint,
            self.run_id.clone(),
            self.capture_id,
            self.capture_revision,
            self.side,
            self.representation,
            offset,
            self.length,
        )
    }

    pub(super) fn request(&self) -> BodyContentRequest {
        BodyContentRequest {
            capture_id: self.capture_id,
            capture_revision: self.capture_revision,
            side: self.side,
            representation: self.representation,
            offset: self.offset,
            length: self.length,
        }
    }
}

impl fmt::Display for BodyResourceUri {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

pub(super) fn body_page_resource_contents(
    requested: &BodyResourceUri,
    page: BodyPage,
) -> Result<ResourceContents, ControlError> {
    let next_uri = page
        .next_offset
        .map(|offset| requested.next_page(offset).to_string());
    let fluxcope = json!({
        "requested_range": page.requested_range,
        "actual_range": page.actual_range,
        "total_bytes": page.total_bytes,
        "next_uri": next_uri,
        "proxy_endpoint": requested.proxy_endpoint().to_string(),
        "run_id": requested.run_id().to_string(),
        "capture_id": requested.capture_id().value(),
        "capture_revision": requested.capture_revision(),
        "side": requested.side(),
        "representation": requested.representation(),
        "stream": page.source.stream,
        "observed_bytes": page.source.observed_bytes,
        "retained_bytes": page.source.retained_bytes,
        "source_truncated": page.source.truncated,
        "source_truncation_reason": page.source.truncation_reason,
        "decoded_encoding_chain": page.source.decoded_encoding_chain,
        "decoded_output_limited": page.source.decoded_output_limited,
    });
    let mut metadata = Map::new();
    metadata.insert("fluxcope".to_owned(), fluxcope);
    let uri = requested.to_string();
    let mut contents = if requested.representation() == BodyRepresentation::Decoded {
        match String::from_utf8(page.content.to_vec()) {
            Ok(text) => ResourceContents::text(text, uri),
            Err(error) => ResourceContents::blob(BASE64_STANDARD.encode(error.into_bytes()), uri),
        }
    } else {
        ResourceContents::blob(BASE64_STANDARD.encode(&page.content), uri)
    };
    if let Some(media_type) = page.media_type {
        contents = contents.with_mime_type(media_type);
    }
    Ok(contents.with_meta(MetaObject(metadata)))
}

pub(super) fn selection_page_resource_contents(
    requested: &SelectionResourceUri,
    page: SelectionPage,
) -> Result<ResourceContents, ControlError> {
    let next_uri = page.next_uri.clone();
    let request = requested.as_request();
    let fluxcope = json!({
        "requested_range": page.requested_range,
        "actual_range": page.actual_range,
        "selected_bytes": page.selected_bytes,
        "next_offset": page.next_offset,
        "next_uri": next_uri,
        "proxy_endpoint": request.proxy_endpoint.to_string(),
        "run_id": request.run_id.to_string(),
        "capture_id": request.capture_id.value(),
        "capture_revision": request.capture_revision,
        "side": request.side,
        "selector_kind": request.selector.kind(),
        "stream": page.source.stream,
        "observed_bytes": page.source.observed_bytes,
        "retained_bytes": page.source.retained_bytes,
        "source_truncated": page.source.truncated,
        "source_truncation_reason": page.source.truncation_reason,
        "decoded_encoding_chain": page.source.decoded_encoding_chain,
        "decoded_output_limited": page.source.decoded_output_limited,
        "media_type": page.media_type.clone(),
    });
    let mut metadata = Map::new();
    metadata.insert("fluxcope".to_owned(), fluxcope);
    let text = String::from_utf8(page.content.to_vec())
        .map_err(|_| ControlError::internal("selected resource page was not valid UTF-8"))?;
    Ok(ResourceContents::text(text, request.to_uri()?)
        .with_mime_type(page.media_type)
        .with_meta(MetaObject(metadata)))
}

fn side_segment(side: BodySide) -> &'static str {
    match side {
        BodySide::Request => "request",
        BodySide::Response => "response",
    }
}

fn representation_segment(representation: BodyRepresentation) -> &'static str {
    match representation {
        BodyRepresentation::Raw => "raw",
        BodyRepresentation::Decoded => "decoded",
    }
}

fn parse_decimal_u64(raw: &str, field: &str) -> Result<u64, ControlError> {
    if raw.is_empty()
        || (raw.len() > 1 && raw.starts_with('0'))
        || !raw.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(invalid_uri(format!(
            "body resource {field} is not canonical"
        )));
    }
    raw.parse()
        .map_err(|_| invalid_uri(format!("body resource {field} is out of range")))
}

fn parse_decimal_usize(raw: &str, field: &str) -> Result<usize, ControlError> {
    let value = parse_decimal_u64(raw, field)?;
    usize::try_from(value)
        .map_err(|_| invalid_uri(format!("body resource {field} is out of range")))
}

fn invalid_uri(message: impl Into<String>) -> ControlError {
    ControlError::new(ControlErrorCode::InvalidArgument, message, false, json!({}))
}

#[cfg(test)]
mod content_resource_tests;
#[cfg(test)]
mod json_inspection_tests;
#[cfg(test)]
mod search_extract_tests;
