mod body;
mod decode;
mod model;
mod publisher;
mod store;

use hyper::Method;

pub(crate) use body::{BodyTaskTracker, drain_body, tee_body};
pub(crate) use decode::{
    DecodeClient, DecodeDisplayMode, DecodeKey, DecodeMetrics, DecodeMetricsSnapshot, DecodePolicy,
    DecodeResult, start_decode_service,
};
pub use model::{
    BodyPreviewLimit, BodySide, BodyStatus, BodyStreamState, CaptureRecord, CaptureSummary,
    CapturedBodyPreview, CapturedHeaders, MetadataTruncation, RequestMetadata, ResponseMetadata,
};
pub(crate) use publisher::{
    CaptureDirtySignal, CaptureHandle, CaptureMetrics, CaptureMetricsSnapshot, CapturePolicy,
    CapturePublisher, RequestCaptureInput, ResponseCaptureInput,
};
pub(crate) use store::{CaptureRetentionPolicy, CaptureStore};

pub(crate) fn benchmark_ordered_store(captures: Vec<CapturedExchange>) -> usize {
    let mut store = CaptureStore::new(CaptureRetentionPolicy {
        max_records: usize::MAX,
        max_bytes: usize::MAX,
    });
    for capture in captures {
        store.insert(CaptureRecord::from_completed(capture));
    }
    store.len()
}

/// Monotonic identity assigned when a request is admitted for capture.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct CaptureSequence(u64);

impl CaptureSequence {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn value(self) -> u64 {
        self.0
    }
}

/// Immutable, completed capture representation used while live streaming is introduced.
#[derive(Clone, Debug)]
pub struct CapturedExchange {
    pub sequence: CaptureSequence,
    pub method: Method,
    pub uri: String,
    pub mapped_uri: Option<String>,
    pub local_path: Option<String>,
    pub status: Option<u16>,
    pub req_headers: Vec<(String, String)>,
    pub res_headers: Vec<(String, String)>,
    pub req_body: Option<String>,
    pub res_body: Option<String>,
}

impl CapturedExchange {
    pub fn display_uri(&self) -> &str {
        self.mapped_uri.as_deref().unwrap_or(&self.uri)
    }

    pub(crate) fn retained_bytes(&self) -> usize {
        const RECORD_OVERHEAD: usize = 256;

        RECORD_OVERHEAD
            .saturating_add(self.uri.len())
            .saturating_add(self.mapped_uri.as_ref().map_or(0, String::len))
            .saturating_add(self.local_path.as_ref().map_or(0, String::len))
            .saturating_add(headers_bytes(&self.req_headers))
            .saturating_add(headers_bytes(&self.res_headers))
            .saturating_add(self.req_body.as_ref().map_or(0, String::len))
            .saturating_add(self.res_body.as_ref().map_or(0, String::len))
    }
}

fn headers_bytes(headers: &[(String, String)]) -> usize {
    headers.iter().fold(0_usize, |total, (name, value)| {
        total.saturating_add(name.len()).saturating_add(value.len())
    })
}
