#![cfg(unix)]

use super::BodyCacheKey;
use crate::{
    capture::{BodySide, BodyStatus, BodyStreamState, CaptureSequence, CapturedHeaders},
    control::body::{BodyContentRequest, BodyRepresentation},
    control_rpc::protocol::InstanceScope,
    instance::InstanceIdentity,
};

pub(super) fn scope(port: u16) -> InstanceScope {
    let identity = InstanceIdentity::new(format!("127.0.0.1:{port}").parse().unwrap())
        .expect("instance identity");
    InstanceScope {
        proxy_endpoint: identity.proxy_endpoint(),
        run_id: identity.run_id().clone(),
    }
}

pub(super) fn status(stream: BodyStreamState, retained_bytes: usize) -> BodyStatus {
    BodyStatus {
        stream,
        observed_bytes: retained_bytes as u64,
        retained_bytes,
        preview_limit: None,
        error: None,
    }
}

pub(super) fn headers(content_type: &str) -> CapturedHeaders {
    CapturedHeaders::unbudgeted(vec![("Content-Type".to_owned(), content_type.to_owned())].into())
}

pub(super) fn request(
    representation: BodyRepresentation,
    offset: usize,
    length: usize,
) -> BodyContentRequest {
    BodyContentRequest {
        capture_id: CaptureSequence::new(7),
        capture_revision: 3,
        side: BodySide::Response,
        representation,
        offset,
        length,
    }
}

pub(super) fn key(instance: InstanceScope, revision: u64, side: BodySide) -> BodyCacheKey {
    BodyCacheKey {
        instance,
        capture_id: CaptureSequence::new(7),
        capture_revision: revision,
        side,
        representation: BodyRepresentation::Decoded,
    }
}
