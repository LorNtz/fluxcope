use std::{fmt, net::SocketAddr, str::FromStr};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use rmcp::model::{MetaObject, ResourceContents};
use serde_json::{Map, json};
use url::Url;

use crate::{
    capture::{BodySide, CaptureSequence},
    control::body::{
        BodyContentRequest, BodyPage, BodyRepresentation, DEFAULT_BODY_PAGE_LENGTH,
        MAX_BODY_PAGE_LENGTH,
    },
    control_rpc::protocol::{ControlError, ControlErrorCode},
    instance::RunId,
};

pub(super) const CONTENT_RESOURCE_TEMPLATE: &str = "wirelens://{+proxy_endpoint}/runs/{run_id}/captures/{capture_id}/revisions/{capture_revision}/bodies/{side}/content/{representation}{?offset,length}";

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
            "wirelens://{proxy_endpoint}/runs/{run_id}/captures/{capture_id_value}/revisions/{capture_revision}/bodies/{side_segment}/content/{representation_segment}?offset={offset}&length={length}"
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
            .strip_prefix("wirelens://")
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
        if url.scheme() != "wirelens"
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
    let wirelens = json!({
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
    metadata.insert("wirelens".to_owned(), wirelens);
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
mod task10_tests;
