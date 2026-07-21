use hyper::Method;

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

/// Immutable, completed capture representation used while the live capture store is introduced.
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
}
