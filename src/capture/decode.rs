use std::{
    borrow::Cow,
    collections::{HashMap, HashSet},
    fmt,
    io::{self, BufReader, Read, Write},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
};

use anyhow::{Context, Result};
use hyper::body::Bytes;
use parking_lot::Mutex;
use tokio::{
    sync::mpsc,
    task::{JoinHandle, JoinSet},
    time::Instant,
};
use tokio_util::sync::CancellationToken;

use super::{
    BodySide, BodyWorkAdmission, CaptureSequence, CapturedBodyPreview, CapturedHeaders,
    body_work::QueuedBodyWorkLease,
};
use crate::{
    control::body::{MAX_CONTENT_ENCODING_LAYERS, MAX_DECODED_CONTENT_BYTES},
    control_rpc::protocol::ControlErrorCode,
};

const DISPLAY_LIMIT_SUFFIX: &str = "\n[Display truncated at configured limit]";

#[derive(Clone, Debug)]
pub(crate) struct DecodePolicy {
    pub queue_capacity: usize,
    pub max_active: usize,
    pub max_queued_input_bytes: usize,
    pub max_output_bytes: usize,
    pub max_json_input_bytes: usize,
    #[cfg(test)]
    pub(crate) format_progress_probe: Option<Arc<DecodeProgressProbe>>,
}

impl Default for DecodePolicy {
    fn default() -> Self {
        Self {
            queue_capacity: 8,
            max_active: 2,
            max_queued_input_bytes: 32 * 1024 * 1024,
            max_output_bytes: 16 * 1024 * 1024,
            max_json_input_bytes: 2 * 1024 * 1024,
            #[cfg(test)]
            format_progress_probe: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub(crate) enum DecodeDisplayMode {
    Request,
    Response,
    MapLocal,
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub(crate) struct DecodeKey {
    pub sequence: CaptureSequence,
    pub side: BodySide,
    pub revision: u64,
    pub mode: DecodeDisplayMode,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct DecodeMetricsSnapshot {
    pub rejected: u64,
    pub superseded: u64,
    pub output_limited: u64,
    pub failed: u64,
}

#[derive(Default)]
pub(crate) struct DecodeMetrics {
    rejected: AtomicU64,
    superseded: AtomicU64,
    output_limited: AtomicU64,
    failed: AtomicU64,
}

impl DecodeMetrics {
    pub fn snapshot(&self) -> DecodeMetricsSnapshot {
        DecodeMetricsSnapshot {
            rejected: self.rejected.load(Ordering::Relaxed),
            superseded: self.superseded.load(Ordering::Relaxed),
            output_limited: self.output_limited.load(Ordering::Relaxed),
            failed: self.failed.load(Ordering::Relaxed),
        }
    }
}

#[derive(Clone)]
pub(crate) struct DecodeClient {
    tx: mpsc::Sender<DecodeJob>,
    state: Arc<DecodeClientState>,
    policy: Arc<DecodePolicy>,
    metrics: Arc<DecodeMetrics>,
    body_work: Arc<BodyWorkAdmission>,
}

struct DecodeCancellation {
    flag: AtomicBool,
    token: CancellationToken,
}

impl DecodeCancellation {
    fn new() -> Self {
        Self {
            flag: AtomicBool::new(false),
            token: CancellationToken::new(),
        }
    }

    fn cancel(&self) {
        self.flag.store(true, Ordering::Release);
        self.token.cancel();
    }
}

struct DecodeClientState {
    pending: Mutex<HashSet<DecodeKey>>,
    desired: Mutex<Option<DecodeKey>>,
    active_cancellations: Mutex<HashMap<DecodeKey, Arc<DecodeCancellation>>>,
    queued_input_bytes: AtomicUsize,
}

impl DecodeClient {
    pub fn request(
        &self,
        key: DecodeKey,
        input: CapturedBodyPreview,
        headers: CapturedHeaders,
    ) -> bool {
        {
            let mut pending = self.state.pending.lock();
            if !pending.insert(key) {
                *self.state.desired.lock() = Some(key);
                for (active_key, active_cancelled) in self.state.active_cancellations.lock().iter()
                {
                    if *active_key != key {
                        active_cancelled.cancel();
                    }
                }
                return true;
            }
        }
        *self.state.desired.lock() = Some(key);
        for (active_key, active_cancelled) in self.state.active_cancellations.lock().iter() {
            if *active_key != key {
                active_cancelled.cancel();
            }
        }

        if !reserve_queued_bytes(
            &self.state.queued_input_bytes,
            input.len(),
            self.policy.max_queued_input_bytes,
        ) {
            self.reject(key);
            return false;
        }
        let Some(body_work) = self.body_work.try_admit_tui(input.len()) else {
            self.state
                .queued_input_bytes
                .fetch_sub(input.len(), Ordering::AcqRel);
            self.reject(key);
            return false;
        };
        let job = DecodeJob {
            key,
            input,
            headers,
            body_work: Some(body_work),
            cancellation: Arc::new(DecodeCancellation::new()),
        };
        if let Err(error) = self.tx.try_send(job) {
            let job = error.into_inner();
            self.state
                .queued_input_bytes
                .fetch_sub(job.input.len(), Ordering::AcqRel);
            self.reject(key);
            return false;
        }
        true
    }

    fn reject(&self, key: DecodeKey) {
        self.state.pending.lock().remove(&key);
        self.state.active_cancellations.lock().remove(&key);
        self.metrics.rejected.fetch_add(1, Ordering::Relaxed);
    }
    #[cfg(test)]
    pub(crate) fn test_body_work_admission(&self) -> &Arc<BodyWorkAdmission> {
        &self.body_work
    }
}
pub(crate) struct DecodeResult {
    pub key: DecodeKey,
    pub text: String,
    pub limited: bool,
    pub error: Option<String>,
}

pub(crate) struct DecodeService {
    pub client: DecodeClient,
    pub results: mpsc::Receiver<DecodeResult>,
    pub metrics: Arc<DecodeMetrics>,
    pub task: JoinHandle<Result<()>>,
}

struct DecodeJob {
    key: DecodeKey,
    input: CapturedBodyPreview,
    headers: CapturedHeaders,
    body_work: Option<QueuedBodyWorkLease>,
    cancellation: Arc<DecodeCancellation>,
}

struct DecodeCompletion {
    key: DecodeKey,
    result: DecodeResult,
}

#[cfg(test)]
pub(crate) fn start_decode_service(
    policy: DecodePolicy,
    shutdown: CancellationToken,
) -> DecodeService {
    start_decode_service_with_admission(policy, shutdown, Arc::new(BodyWorkAdmission::new()))
}

pub(crate) fn start_decode_service_with_admission(
    mut policy: DecodePolicy,
    shutdown: CancellationToken,
    body_work: Arc<BodyWorkAdmission>,
) -> DecodeService {
    policy.queue_capacity = policy.queue_capacity.max(1);
    policy.max_active = policy.max_active.max(1);
    let policy = Arc::new(policy);
    let (tx, rx) = mpsc::channel(policy.queue_capacity);
    let (result_tx, results) = mpsc::channel(policy.queue_capacity);
    let state = Arc::new(DecodeClientState {
        pending: Mutex::new(HashSet::new()),
        desired: Mutex::new(None),
        active_cancellations: Mutex::new(HashMap::new()),
        queued_input_bytes: AtomicUsize::new(0),
    });
    let metrics = Arc::new(DecodeMetrics::default());
    let client = DecodeClient {
        tx,
        state: Arc::clone(&state),
        policy: Arc::clone(&policy),
        metrics: Arc::clone(&metrics),
        body_work,
    };
    let task = tokio::spawn(run_decode_service(
        rx,
        result_tx,
        Arc::clone(&state),
        Arc::clone(&policy),
        Arc::clone(&metrics),
        shutdown,
    ));
    DecodeService {
        client,
        results,
        metrics,
        task,
    }
}

async fn run_decode_service(
    mut jobs: mpsc::Receiver<DecodeJob>,
    results: mpsc::Sender<DecodeResult>,
    state: Arc<DecodeClientState>,
    policy: Arc<DecodePolicy>,
    metrics: Arc<DecodeMetrics>,
    shutdown: CancellationToken,
) -> Result<()> {
    let mut active: JoinSet<DecodeCompletion> = JoinSet::new();
    let mut jobs_open = true;
    loop {
        if !jobs_open && active.is_empty() {
            break;
        }
        tokio::select! {
            biased;
            _ = shutdown.cancelled() => break,
            completion = active.join_next(), if !active.is_empty() => {
                let completion = completion
                    .context("active decode set ended unexpectedly")?
                    .context("decode worker task failed to join")?;
                state.pending.lock().remove(&completion.key);
                state.active_cancellations.lock().remove(&completion.key);
                if state.desired.lock().as_ref() != Some(&completion.key) {
                    metrics.superseded.fetch_add(1, Ordering::Relaxed);
                    continue;
                }
                if completion.result.limited {
                    metrics.output_limited.fetch_add(1, Ordering::Relaxed);
                }
                if completion.result.error.is_some() {
                    metrics.failed.fetch_add(1, Ordering::Relaxed);
                }
                tokio::select! {
                    _ = shutdown.cancelled() => break,
                    sent = results.send(completion.result) => {
                        if sent.is_err() {
                            break;
                        }
                    }
                }
            }
            job = jobs.recv(), if jobs_open && active.len() < policy.max_active => {
                let Some(job) = job else {
                    jobs_open = false;
                    continue;
                };
                state.queued_input_bytes.fetch_sub(job.input.len(), Ordering::AcqRel);
                if state.desired.lock().as_ref() != Some(&job.key) {
                    state.pending.lock().remove(&job.key);
                    metrics.superseded.fetch_add(1, Ordering::Relaxed);
                    continue;
                }
                let worker_policy = Arc::clone(&policy);
                let key = job.key;
                state
                    .active_cancellations
                    .lock()
                    .insert(key, Arc::clone(&job.cancellation));
                if state.desired.lock().as_ref() != Some(&key) {
                    job.cancellation.cancel();
                }
                active.spawn(async move {
                    let mut job = job;
                    let admission = job
                        .body_work
                        .take()
                        .expect("decode job must retain body-work admission");
                    let active_lease = match admission
                        .acquire_active(
                            Instant::now() + std::time::Duration::from_secs(30),
                            job.cancellation.token.clone(),
                        )
                        .await
                    {
                        Ok(lease) => lease,
                        Err(error) => {
                            return DecodeCompletion {
                                key,
                                result: DecodeResult {
                                    key,
                                    text: "(Body decode was not admitted)".to_owned(),
                                    limited: false,
                                    error: Some(error.message),
                                },
                            };
                        }
                    };
                    let worker_flag = Arc::clone(&job.cancellation);
                    let result = tokio::task::spawn_blocking(move || {
                        let _active_lease = active_lease;
                        decode_job_cancellable(job, &worker_policy, &worker_flag.flag)
                    })
                    .await
                    .unwrap_or_else(|error| DecodeResult {
                        key,
                        text: "(Body decode worker failed)".to_string(),
                        limited: false,
                        error: Some(error.to_string()),
                    });
                    DecodeCompletion { key, result }
                });
            }
        }
    }
    for cancellation in state.active_cancellations.lock().values() {
        cancellation.cancel();
    }
    while active.join_next().await.is_some() {}
    Ok(())
}

fn reserve_queued_bytes(counter: &AtomicUsize, requested: usize, limit: usize) -> bool {
    let mut current = counter.load(Ordering::Acquire);
    loop {
        let Some(updated) = current.checked_add(requested) else {
            return false;
        };
        if updated > limit {
            return false;
        }
        match counter.compare_exchange_weak(current, updated, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => return true,
            Err(observed) => current = observed,
        }
    }
}

#[cfg(test)]
fn decode_job(job: DecodeJob, policy: &DecodePolicy) -> DecodeResult {
    decode_job_cancellable(job, policy, &AtomicBool::new(false))
}

fn decode_job_cancellable(
    job: DecodeJob,
    policy: &DecodePolicy,
    cancelled: &AtomicBool,
) -> DecodeResult {
    let content_policy = ContentDecodePolicy {
        max_output_bytes: policy.max_output_bytes,
        #[cfg(test)]
        progress_probe: None,
    };
    let decoded = decode_content_bytes(&job.input, &job.headers, &content_policy, cancelled);
    let (bytes, known_utf8, mut limited, error): (Cow<'_, [u8]>, _, _, _) = match decoded.as_ref() {
        Ok(decoded) => (
            Cow::Borrowed(decoded.bytes.as_ref()),
            Some(decoded.is_utf8()),
            decoded.output_limited,
            None,
        ),
        Err(error) => {
            let message = error.message();
            let fallback = if *error == DecodeContentError::Cancelled {
                Vec::new()
            } else {
                flatten_content_input(&job.input, &content_policy, cancelled).unwrap_or_default()
            };
            let mut fallback = fallback;
            let limited = fallback.len() > policy.max_output_bytes;
            fallback.truncate(policy.max_output_bytes);
            (Cow::Owned(fallback), None, limited, Some(message))
        }
    };
    let formatted = match format_body(
        bytes.as_ref(),
        known_utf8,
        &job.headers,
        job.key.mode,
        policy,
        cancelled,
    ) {
        Ok(formatted) => formatted,
        Err(error) => {
            return DecodeResult {
                key: job.key,
                text: String::new(),
                limited: false,
                error: Some(error.message()),
            };
        }
    };
    limited |= formatted.limited;
    let mut text = formatted.text;
    if limited {
        text = append_limit_suffix(text, policy.max_output_bytes);
    }
    if let Some(error) = error.as_ref() {
        text = append_error_suffix(text, error, policy.max_output_bytes);
    }
    DecodeResult {
        key: job.key,
        text,
        limited,
        error,
    }
}

fn content_encodings(headers: &[(String, String)]) -> Vec<String> {
    headers
        .iter()
        .filter(|(name, _)| name.eq_ignore_ascii_case("content-encoding"))
        .flat_map(|(_, value)| value.split(','))
        .map(|encoding| encoding.trim().to_ascii_lowercase())
        .filter(|encoding| !encoding.is_empty() && encoding != "identity")
        .collect()
}

const DECODE_CANCELLATION_CHECK_BYTES: usize = 32 * 1_024;

#[derive(Clone)]
pub(crate) struct ContentDecodePolicy {
    max_output_bytes: usize,
    #[cfg(test)]
    progress_probe: Option<Arc<DecodeProgressProbe>>,
}

impl Default for ContentDecodePolicy {
    fn default() -> Self {
        Self {
            max_output_bytes: MAX_DECODED_CONTENT_BYTES,
            #[cfg(test)]
            progress_probe: None,
        }
    }
}

#[cfg(test)]
impl ContentDecodePolicy {
    pub(crate) fn with_test_progress_probe(mut self, probe: Arc<DecodeProgressProbe>) -> Self {
        self.progress_probe = Some(probe);
        self
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DecodedBytes {
    pub(crate) bytes: Bytes,
    pub(crate) encoding_chain: Vec<String>,
    pub(crate) output_limited: bool,
    utf8_valid: bool,
}

impl DecodedBytes {
    #[cfg(test)]
    pub(crate) fn new(bytes: Bytes, encoding_chain: Vec<String>, output_limited: bool) -> Self {
        let utf8_valid = std::str::from_utf8(&bytes).is_ok();
        Self {
            bytes,
            encoding_chain,
            output_limited,
            utf8_valid,
        }
    }
    fn with_utf8_validity(
        bytes: Bytes,
        encoding_chain: Vec<String>,
        output_limited: bool,
        utf8_valid: bool,
    ) -> Self {
        Self {
            bytes,
            encoding_chain,
            output_limited,
            utf8_valid,
        }
    }

    pub(crate) fn is_utf8(&self) -> bool {
        self.utf8_valid
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum DecodeContentError {
    Cancelled,
    Unsupported {
        encoding: String,
        reason: &'static str,
    },
    TooManyLayers {
        layers: usize,
        maximum_layers: usize,
    },
    OutputLimit {
        maximum_output_bytes: usize,
    },
}

impl DecodeContentError {
    pub(crate) fn code(&self) -> ControlErrorCode {
        match self {
            Self::Cancelled => ControlErrorCode::Cancelled,
            Self::Unsupported { .. } | Self::TooManyLayers { .. } => {
                ControlErrorCode::UnsupportedBodyEncoding
            }
            Self::OutputLimit { .. } => ControlErrorCode::ResourceLimit,
        }
    }

    pub(crate) fn details(&self) -> serde_json::Value {
        match self {
            Self::Cancelled => serde_json::json!({}),
            Self::Unsupported { encoding, reason } => {
                serde_json::json!({"encoding": encoding, "reason": reason})
            }
            Self::TooManyLayers {
                layers,
                maximum_layers,
            } => serde_json::json!({
                "layers": layers,
                "maximum_layers": maximum_layers,
            }),
            Self::OutputLimit {
                maximum_output_bytes,
            } => serde_json::json!({
                "maximum_output_bytes": maximum_output_bytes,
            }),
        }
    }

    fn message(&self) -> String {
        match self {
            Self::Cancelled => "body decode was cancelled".to_owned(),
            Self::Unsupported { encoding, reason } => {
                format!("{reason} content encoding {encoding}")
            }
            Self::TooManyLayers {
                layers,
                maximum_layers,
            } => format!("content encoding has {layers} layers; maximum is {maximum_layers}"),
            Self::OutputLimit {
                maximum_output_bytes,
            } => format!("intermediate decoded content exceeds {maximum_output_bytes} bytes"),
        }
    }
}

pub(crate) fn decode_content_bytes(
    input: &CapturedBodyPreview,
    headers: &CapturedHeaders,
    policy: &ContentDecodePolicy,
    cancelled: &AtomicBool,
) -> Result<DecodedBytes, DecodeContentError> {
    check_content_cancelled(cancelled)?;
    let encodings = content_encodings(headers);
    if encodings.len() > MAX_CONTENT_ENCODING_LAYERS {
        return Err(DecodeContentError::TooManyLayers {
            layers: encodings.len(),
            maximum_layers: MAX_CONTENT_ENCODING_LAYERS,
        });
    }
    let mut current = flatten_content_input(input, policy, cancelled)?;
    let mut limited = false;
    for (layer, encoding) in encodings.iter().rev().enumerate() {
        check_content_cancelled(cancelled)?;
        let decoded = match encoding.as_str() {
            "gzip" | "x-gzip" => read_content_limited(
                flate2::read::GzDecoder::new(CancellationReader::new(
                    current.as_slice(),
                    cancelled,
                )),
                policy,
                cancelled,
            ),
            "deflate" => decode_content_deflate(&current, policy, cancelled),
            "br" => read_content_limited(
                brotli::Decompressor::new(
                    CancellationReader::new(current.as_slice(), cancelled),
                    4_096,
                ),
                policy,
                cancelled,
            ),
            "zstd" => zstd::stream::read::Decoder::new(CancellationReader::new(
                current.as_slice(),
                cancelled,
            ))
            .map_err(|_| unsupported_encoding(encoding, "malformed"))
            .and_then(|decoder| read_content_limited(decoder, policy, cancelled)),
            unsupported => {
                return Err(unsupported_encoding(unsupported, "unsupported"));
            }
        }
        .map_err(|error| match error {
            DecodeContentError::Cancelled => error,
            _ => unsupported_encoding(normalized_encoding(encoding), "malformed"),
        })?;
        if decoded.limited && layer + 1 < encodings.len() {
            return Err(DecodeContentError::OutputLimit {
                maximum_output_bytes: policy.max_output_bytes,
            });
        }
        current = decoded.bytes;
        limited |= decoded.limited;
        if decoded.limited {
            break;
        }
    }
    if encodings.is_empty() && current.len() > policy.max_output_bytes {
        current.truncate(policy.max_output_bytes);
        limited = true;
    }
    let bytes = Bytes::from(current);
    let utf8_valid = validate_utf8_cancellable(&bytes, cancelled)?;
    Ok(DecodedBytes::with_utf8_validity(
        bytes,
        encodings
            .into_iter()
            .map(|encoding| normalized_encoding(&encoding).to_owned())
            .collect(),
        limited,
        utf8_valid,
    ))
}

fn flatten_content_input(
    input: &CapturedBodyPreview,
    policy: &ContentDecodePolicy,
    cancelled: &AtomicBool,
) -> Result<Vec<u8>, DecodeContentError> {
    let mut bytes = Vec::with_capacity(input.len());
    for chunk in input.chunks() {
        for part in chunk.chunks(DECODE_CANCELLATION_CHECK_BYTES) {
            check_content_cancelled(cancelled)?;
            bytes.extend_from_slice(part);
            if part.len() == DECODE_CANCELLATION_CHECK_BYTES {
                content_progress_checkpoint(policy, part.len());
            }
        }
    }
    check_content_cancelled(cancelled)?;
    Ok(bytes)
}

fn normalized_encoding(encoding: &str) -> &str {
    if encoding == "x-gzip" {
        "gzip"
    } else {
        encoding
    }
}

fn unsupported_encoding(encoding: &str, reason: &'static str) -> DecodeContentError {
    DecodeContentError::Unsupported {
        encoding: normalized_encoding(encoding).to_owned(),
        reason,
    }
}

struct ContentDecodedStage {
    bytes: Vec<u8>,
    limited: bool,
}

fn decode_content_deflate(
    input: &[u8],
    policy: &ContentDecodePolicy,
    cancelled: &AtomicBool,
) -> Result<ContentDecodedStage, DecodeContentError> {
    read_content_limited(
        flate2::read::ZlibDecoder::new(CancellationReader::new(input, cancelled)),
        policy,
        cancelled,
    )
    .or_else(|error| {
        if error == DecodeContentError::Cancelled {
            Err(error)
        } else {
            read_content_limited(
                flate2::read::DeflateDecoder::new(CancellationReader::new(input, cancelled)),
                policy,
                cancelled,
            )
        }
    })
}

struct CancellationReader<'a, R> {
    inner: R,
    cancelled: &'a AtomicBool,
}

impl<'a, R> CancellationReader<'a, R> {
    fn new(inner: R, cancelled: &'a AtomicBool) -> Self {
        Self { inner, cancelled }
    }
}

impl<R: Read> Read for CancellationReader<'_, R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if self.cancelled.load(Ordering::Acquire) {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "content decode cancelled",
            ));
        }
        let bounded = buffer.len().min(DECODE_CANCELLATION_CHECK_BYTES);
        self.inner.read(&mut buffer[..bounded])
    }
}

fn read_content_limited(
    mut reader: impl Read,
    policy: &ContentDecodePolicy,
    cancelled: &AtomicBool,
) -> Result<ContentDecodedStage, DecodeContentError> {
    let mut bytes = Vec::with_capacity(policy.max_output_bytes.min(64 * 1_024));
    let mut buffer = [0_u8; DECODE_CANCELLATION_CHECK_BYTES];
    loop {
        check_content_cancelled(cancelled)?;
        let read = match reader.read(&mut buffer) {
            Ok(read) => read,
            Err(_) if cancelled.load(Ordering::Acquire) => {
                return Err(DecodeContentError::Cancelled);
            }
            Err(_) => {
                return Err(DecodeContentError::Unsupported {
                    encoding: String::new(),
                    reason: "malformed",
                });
            }
        };
        check_content_cancelled(cancelled)?;
        if read == 0 {
            return Ok(ContentDecodedStage {
                bytes,
                limited: false,
            });
        }
        let remaining = policy.max_output_bytes.saturating_sub(bytes.len());
        let retained = read.min(remaining);
        bytes.extend_from_slice(&buffer[..retained]);
        content_progress_checkpoint(policy, read);
        if read > remaining {
            return Ok(ContentDecodedStage {
                bytes,
                limited: true,
            });
        }
    }
}

fn check_content_cancelled(cancelled: &AtomicBool) -> Result<(), DecodeContentError> {
    if cancelled.load(Ordering::Acquire) {
        Err(DecodeContentError::Cancelled)
    } else {
        Ok(())
    }
}

fn validate_utf8_cancellable(
    bytes: &[u8],
    cancelled: &AtomicBool,
) -> Result<bool, DecodeContentError> {
    let mut start = 0;
    while start < bytes.len() {
        check_content_cancelled(cancelled)?;
        let mut end = start
            .saturating_add(DECODE_CANCELLATION_CHECK_BYTES)
            .min(bytes.len());
        if end < bytes.len() {
            for _ in 0..3 {
                if end == start || bytes[end] & 0b1100_0000 != 0b1000_0000 {
                    break;
                }
                end -= 1;
            }
        }
        if end == start || std::str::from_utf8(&bytes[start..end]).is_err() {
            return Ok(false);
        }
        start = end;
    }
    check_content_cancelled(cancelled)?;
    Ok(true)
}

#[cfg(not(test))]
fn content_progress_checkpoint(_policy: &ContentDecodePolicy, _bytes: usize) {}

#[cfg(test)]
fn content_progress_checkpoint(policy: &ContentDecodePolicy, bytes: usize) {
    if let Some(probe) = policy.progress_probe.as_ref() {
        probe.checkpoint(bytes);
    }
}

#[cfg(not(test))]
fn format_progress_checkpoint(_policy: &DecodePolicy, _bytes: usize) {}

#[cfg(test)]
fn format_progress_checkpoint(policy: &DecodePolicy, bytes: usize) {
    if let Some(probe) = policy.format_progress_probe.as_ref() {
        probe.checkpoint(bytes);
    }
}

#[cfg(test)]
#[derive(Debug)]
pub(crate) struct DecodeProgressProbe {
    checkpoint: tokio::sync::Notify,
    release: std::sync::Condvar,
    state: std::sync::Mutex<DecodeProgressProbeState>,
}

#[cfg(test)]
#[derive(Debug, Default)]
struct DecodeProgressProbeState {
    bytes: usize,
    released: bool,
}

#[cfg(test)]
impl DecodeProgressProbe {
    pub(crate) fn new() -> Self {
        Self {
            checkpoint: tokio::sync::Notify::new(),
            release: std::sync::Condvar::new(),
            state: std::sync::Mutex::new(DecodeProgressProbeState::default()),
        }
    }

    fn checkpoint(&self, bytes: usize) {
        let mut state = self
            .state
            .lock()
            .expect("decode progress probe lock poisoned");
        state.bytes = bytes;
        self.checkpoint.notify_one();
        while !state.released {
            state = self
                .release
                .wait(state)
                .expect("decode progress probe wait poisoned");
        }
    }

    pub(crate) async fn wait_for_checkpoint(&self) {
        self.checkpoint.notified().await;
    }

    pub(crate) fn release(&self) {
        let mut state = self
            .state
            .lock()
            .expect("decode progress probe lock poisoned");
        state.released = true;
        self.release.notify_all();
    }

    pub(crate) fn bytes_since_previous_checkpoint(&self) -> usize {
        self.state
            .lock()
            .expect("decode progress probe lock poisoned")
            .bytes
    }
}

struct FormattedBody {
    text: String,
    limited: bool,
}

fn format_body(
    bytes: &[u8],
    known_utf8: Option<bool>,
    headers: &[(String, String)],
    mode: DecodeDisplayMode,
    policy: &DecodePolicy,
    cancelled: &AtomicBool,
) -> Result<FormattedBody, DecodeContentError> {
    check_content_cancelled(cancelled)?;
    if bytes.is_empty() {
        return capped_text(
            if mode == DecodeDisplayMode::MapLocal {
                ""
            } else {
                "(No body)"
            },
            policy,
            cancelled,
        );
    }
    let utf8_valid = match known_utf8 {
        Some(valid) => valid,
        None => validate_utf8_cancellable(bytes, cancelled)?,
    };
    if !utf8_valid {
        return capped_text(&binary_body_summary(bytes), policy, cancelled);
    }
    // SAFETY: either `DecodedBytes` cached successful UTF-8 validation or the
    // cancellation-aware validator immediately above accepted these exact bytes.
    let text = unsafe { std::str::from_utf8_unchecked(bytes) };
    if mode == DecodeDisplayMode::MapLocal {
        return capped_text(text, policy, cancelled);
    }
    if mode == DecodeDisplayMode::Request && is_form_data(headers) {
        return format_form(text, policy, cancelled);
    }
    if bytes.len() <= policy.max_json_input_bytes {
        let reader = BufReader::with_capacity(
            DECODE_CANCELLATION_CHECK_BYTES,
            CancellationReader::new(bytes, cancelled),
        );
        let parsed = serde_json::from_reader::<_, serde_json::Value>(reader);
        if cancelled.load(Ordering::Acquire) {
            return Err(DecodeContentError::Cancelled);
        }
        if let Ok(value) = parsed {
            let mut writer = LimitedWriter::new(policy, cancelled);
            let result = serde_json::to_writer_pretty(&mut writer, &value);
            check_content_cancelled(cancelled)?;
            if result.is_ok() || writer.limited {
                let limited = writer.limited;
                let text = match String::from_utf8(writer.bytes) {
                    Ok(text) => text,
                    Err(error) if error.utf8_error().error_len().is_none() => {
                        let valid_up_to = error.utf8_error().valid_up_to();
                        let mut bytes = error.into_bytes();
                        bytes.truncate(valid_up_to);
                        String::from_utf8(bytes).expect("truncated JSON prefix is valid UTF-8")
                    }
                    Err(error) => String::from_utf8_lossy(error.as_bytes()).into_owned(),
                };
                return Ok(FormattedBody { text, limited });
            }
        }
    }
    capped_text(text, policy, cancelled)
}

fn is_form_data(headers: &[(String, String)]) -> bool {
    headers.iter().any(|(name, value)| {
        name.eq_ignore_ascii_case("content-type")
            && value
                .to_ascii_lowercase()
                .contains("application/x-www-form-urlencoded")
    })
}

fn format_form(
    input: &str,
    policy: &DecodePolicy,
    cancelled: &AtomicBool,
) -> Result<FormattedBody, DecodeContentError> {
    let mut output = LimitedString::new(policy, cancelled);
    let input = input.as_bytes();
    let mut index = 0;
    let mut checkpoint = 0;
    let mut pair_count = 0;
    let mut decoder = FormComponentDecoder::default();

    'pairs: while index < input.len() {
        if input[index] == b'&' {
            index += 1;
            checkpoint_form_scan(index, &mut checkpoint, policy, cancelled)?;
            continue;
        }
        if pair_count > 0 && fmt::Write::write_char(&mut output, '\n').is_err() {
            break;
        }
        pair_count += 1;
        let mut value = false;
        while index < input.len() && input[index] != b'&' {
            if !value && input[index] == b'=' {
                if decoder.finish(&mut output).is_err()
                    || fmt::Write::write_str(&mut output, ": ").is_err()
                {
                    break 'pairs;
                }
                value = true;
                index += 1;
            } else {
                let (decoded, consumed) = decode_form_byte(&input[index..]);
                if decoder.push(decoded, &mut output).is_err() {
                    break 'pairs;
                }
                index += consumed;
            }
            checkpoint_form_scan(index, &mut checkpoint, policy, cancelled)?;
        }
        if decoder.finish(&mut output).is_err() {
            break;
        }
        if !value && fmt::Write::write_str(&mut output, ": ").is_err() {
            break;
        }
        if index < input.len() {
            index += 1;
            checkpoint_form_scan(index, &mut checkpoint, policy, cancelled)?;
        }
    }
    check_content_cancelled(cancelled)?;
    Ok(FormattedBody {
        text: output.text,
        limited: output.limited,
    })
}

fn checkpoint_form_scan(
    index: usize,
    checkpoint: &mut usize,
    policy: &DecodePolicy,
    cancelled: &AtomicBool,
) -> Result<(), DecodeContentError> {
    if index.saturating_sub(*checkpoint) >= DECODE_CANCELLATION_CHECK_BYTES {
        format_progress_checkpoint(policy, index - *checkpoint);
        check_content_cancelled(cancelled)?;
        *checkpoint = index;
    }
    Ok(())
}

fn decode_form_byte(input: &[u8]) -> (u8, usize) {
    match input {
        [b'+', ..] => (b' ', 1),
        [b'%', high, low, ..] => match (hex_value(*high), hex_value(*low)) {
            (Some(high), Some(low)) => ((high << 4) | low, 3),
            _ => (b'%', 1),
        },
        [byte, ..] => (*byte, 1),
        [] => unreachable!("form decoder requires one input byte"),
    }
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[derive(Default)]
struct FormComponentDecoder {
    bytes: Vec<u8>,
}

impl FormComponentDecoder {
    fn push(&mut self, byte: u8, output: &mut LimitedString<'_>) -> fmt::Result {
        self.bytes.push(byte);
        if self.bytes.len() >= DECODE_CANCELLATION_CHECK_BYTES {
            self.flush(false, output)?;
        }
        Ok(())
    }

    fn finish(&mut self, output: &mut LimitedString<'_>) -> fmt::Result {
        self.flush(true, output)
    }

    fn flush(&mut self, final_chunk: bool, output: &mut LimitedString<'_>) -> fmt::Result {
        let mut consumed = 0;
        while consumed < self.bytes.len() {
            match std::str::from_utf8(&self.bytes[consumed..]) {
                Ok(text) => {
                    fmt::Write::write_str(output, text)?;
                    consumed = self.bytes.len();
                }
                Err(error) => {
                    let valid_end = consumed + error.valid_up_to();
                    if valid_end > consumed {
                        // SAFETY: `valid_up_to` identifies this exact prefix as UTF-8.
                        let text = unsafe {
                            std::str::from_utf8_unchecked(&self.bytes[consumed..valid_end])
                        };
                        fmt::Write::write_str(output, text)?;
                    }
                    match error.error_len() {
                        Some(length) => {
                            fmt::Write::write_char(output, '\u{fffd}')?;
                            consumed = valid_end + length;
                        }
                        None if final_chunk => {
                            fmt::Write::write_char(output, '\u{fffd}')?;
                            consumed = self.bytes.len();
                        }
                        None => {
                            consumed = valid_end;
                            break;
                        }
                    }
                }
            }
        }
        if consumed > 0 {
            self.bytes.drain(..consumed);
        }
        Ok(())
    }
}

fn capped_text(
    input: &str,
    policy: &DecodePolicy,
    cancelled: &AtomicBool,
) -> Result<FormattedBody, DecodeContentError> {
    let mut output = LimitedString::new(policy, cancelled);
    let _ = fmt::Write::write_str(&mut output, input);
    check_content_cancelled(cancelled)?;
    Ok(FormattedBody {
        text: output.text,
        limited: output.limited,
    })
}

fn binary_body_summary(bytes: &[u8]) -> String {
    let first = bytes
        .iter()
        .take(32)
        .map(|byte| format!("{byte:02x}"))
        .collect::<Vec<_>>()
        .join(" ");
    format!(
        "[Binary body: {} retained bytes, first {} bytes in hex: {first}]",
        bytes.len(),
        bytes.len().min(32)
    )
}

fn append_limit_suffix(text: String, limit: usize) -> String {
    append_suffix(text, DISPLAY_LIMIT_SUFFIX, limit)
}

fn append_error_suffix(text: String, error: &str, limit: usize) -> String {
    append_suffix(text, &format!("\n[Display decode failed: {error}]"), limit)
}

fn append_suffix(mut text: String, suffix: &str, limit: usize) -> String {
    if suffix.len() >= limit {
        let mut end = limit.min(suffix.len());
        while !suffix.is_char_boundary(end) {
            end -= 1;
        }
        return suffix[..end].to_string();
    }
    let max_text = limit - suffix.len();
    if text.len() > max_text {
        let mut end = max_text;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
    }
    text.push_str(suffix);
    text
}

struct LimitedWriter<'a> {
    bytes: Vec<u8>,
    limit: usize,
    limited: bool,
    policy: &'a DecodePolicy,
    cancelled: &'a AtomicBool,
}

impl<'a> LimitedWriter<'a> {
    fn new(policy: &'a DecodePolicy, cancelled: &'a AtomicBool) -> Self {
        Self {
            bytes: Vec::with_capacity(policy.max_output_bytes.min(64 * 1024)),
            limit: policy.max_output_bytes,
            limited: false,
            policy,
            cancelled,
        }
    }
}

impl Write for LimitedWriter<'_> {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        if self.cancelled.load(Ordering::Acquire) {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "body formatting cancelled",
            ));
        }
        if buffer.is_empty() {
            return Ok(0);
        }
        let remaining = self.limit.saturating_sub(self.bytes.len());
        if remaining == 0 {
            self.limited = true;
            return Err(io::Error::other("display output limit reached"));
        }
        let length = buffer
            .len()
            .min(remaining)
            .min(DECODE_CANCELLATION_CHECK_BYTES);
        self.bytes.extend_from_slice(&buffer[..length]);
        format_progress_checkpoint(self.policy, length);
        Ok(length)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

struct LimitedString<'a> {
    text: String,
    limit: usize,
    limited: bool,
    policy: &'a DecodePolicy,
    cancelled: &'a AtomicBool,
}

impl<'a> LimitedString<'a> {
    fn new(policy: &'a DecodePolicy, cancelled: &'a AtomicBool) -> Self {
        Self {
            text: String::with_capacity(policy.max_output_bytes.min(64 * 1024)),
            limit: policy.max_output_bytes,
            limited: false,
            policy,
            cancelled,
        }
    }
}

impl fmt::Write for LimitedString<'_> {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        let remaining = self.limit.saturating_sub(self.text.len());
        let mut end = text.len().min(remaining);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        let mut start = 0;
        while start < end {
            if self.cancelled.load(Ordering::Acquire) {
                return Err(fmt::Error);
            }
            let mut chunk_end = start
                .saturating_add(DECODE_CANCELLATION_CHECK_BYTES)
                .min(end);
            while !text.is_char_boundary(chunk_end) {
                chunk_end -= 1;
            }
            self.text.push_str(&text[start..chunk_end]);
            format_progress_checkpoint(self.policy, chunk_end - start);
            start = chunk_end;
        }
        if end < text.len() {
            self.limited = true;
            return Err(fmt::Error);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::{
        Compression,
        write::{DeflateEncoder, GzEncoder, ZlibEncoder},
    };

    fn job(input: Vec<u8>, headers: Vec<(String, String)>) -> DecodeJob {
        job_with_preview(CapturedBodyPreview::unbudgeted(Bytes::from(input)), headers)
    }

    fn job_with_preview(input: CapturedBodyPreview, headers: Vec<(String, String)>) -> DecodeJob {
        DecodeJob {
            key: DecodeKey {
                sequence: CaptureSequence::new(0),
                side: BodySide::Response,
                revision: 1,
                mode: DecodeDisplayMode::Response,
            },
            input,
            headers: CapturedHeaders::unbudgeted(headers.into()),
            body_work: None,
            cancellation: Arc::new(DecodeCancellation::new()),
        }
    }

    #[test]
    fn gzip_decode_and_json_format_are_lazy_and_bounded() {
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder
            .write_all(br#"{"value":[1,2]}"#)
            .expect("gzip write");
        let compressed = encoder.finish().expect("gzip finish");
        let result = decode_job(
            job(compressed, vec![("content-encoding".into(), "gzip".into())]),
            &DecodePolicy::default(),
        );

        assert!(result.text.contains("\"value\": ["));
        assert!(!result.limited);
        assert!(result.error.is_none());
    }

    #[test]
    fn high_compression_output_stops_at_limit() {
        let mut encoder = GzEncoder::new(Vec::new(), Compression::best());
        encoder
            .write_all(&vec![b'a'; 64 * 1024])
            .expect("gzip write");
        let compressed = encoder.finish().expect("gzip finish");
        let policy = DecodePolicy {
            max_output_bytes: 1_024,
            ..DecodePolicy::default()
        };
        let result = decode_job(
            job(compressed, vec![("content-encoding".into(), "gzip".into())]),
            &policy,
        );

        assert!(result.limited);
        assert!(result.text.len() <= policy.max_output_bytes);
        assert!(result.text.contains("Display truncated"));
    }

    #[test]
    fn map_local_text_is_exact_below_limit() {
        let mut exact = job(b"{ compact: true }\n".to_vec(), Vec::new());
        exact.key.mode = DecodeDisplayMode::MapLocal;
        let result = decode_job(exact, &DecodePolicy::default());

        assert_eq!(result.text, "{ compact: true }\n");
    }

    #[test]
    fn form_output_is_capped_after_percent_decoding() {
        let mut form = job(
            b"message=%E4%BD%A0%E5%A5%BD&long=abcdefghijklmnopqrstuvwxyz".to_vec(),
            vec![(
                "content-type".into(),
                "application/x-www-form-urlencoded".into(),
            )],
        );
        form.key.mode = DecodeDisplayMode::Request;
        let policy = DecodePolicy {
            max_output_bytes: 32,
            ..DecodePolicy::default()
        };
        let result = decode_job(form, &policy);

        assert!(result.limited);
        assert!(result.text.len() <= 32);
    }

    #[test]
    fn form_output_preserves_url_decoding_and_empty_field_semantics() {
        let mut form = job(
            b"a+b=c%2Bd&empty=&invalid=%GG&utf8=%E4%BD%A0%E5%A5%BD&bad=%FF&novalue&&".to_vec(),
            vec![(
                "content-type".into(),
                "application/x-www-form-urlencoded".into(),
            )],
        );
        form.key.mode = DecodeDisplayMode::Request;

        let result = decode_job(form, &DecodePolicy::default());

        assert_eq!(
            result.text,
            "a b: c+d\nempty: \ninvalid: %GG\nutf8: 你好\nbad: �\nnovalue: "
        );
        assert!(result.error.is_none());
    }

    #[test]
    fn zlib_and_raw_deflate_are_supported() {
        for (encoding, compressed) in [
            (
                "zlib",
                compress_with(ZlibEncoder::new(Vec::new(), Compression::default())),
            ),
            (
                "raw",
                compress_with(DeflateEncoder::new(Vec::new(), Compression::default())),
            ),
        ] {
            let result = decode_job(
                job(
                    compressed,
                    vec![("content-encoding".into(), "deflate".into())],
                ),
                &DecodePolicy::default(),
            );
            assert_eq!(result.text, "deflate text", "{encoding}");
            assert!(result.error.is_none(), "{encoding}");
        }
    }

    #[test]
    fn brotli_zstd_and_stacked_encodings_are_supported() {
        let input = b"stacked text";
        let mut brotli_bytes = Vec::new();
        {
            let mut writer = brotli::CompressorWriter::new(&mut brotli_bytes, 4096, 5, 22);
            writer.write_all(input).expect("brotli write");
        }
        let brotli_result = decode_job(
            job(brotli_bytes, vec![("content-encoding".into(), "br".into())]),
            &DecodePolicy::default(),
        );
        assert_eq!(brotli_result.text, "stacked text");

        let zstd_bytes = zstd::stream::encode_all(input.as_slice(), 1).expect("zstd encode");
        let zstd_result = decode_job(
            job(zstd_bytes, vec![("content-encoding".into(), "zstd".into())]),
            &DecodePolicy::default(),
        );
        assert_eq!(zstd_result.text, "stacked text");

        let gzip = compress_bytes(input);
        let mut stacked = Vec::new();
        {
            let mut writer = brotli::CompressorWriter::new(&mut stacked, 4096, 5, 22);
            writer.write_all(&gzip).expect("stacked brotli write");
        }
        let stacked_result = decode_job(
            job(
                stacked,
                vec![("content-encoding".into(), "gzip, br".into())],
            ),
            &DecodePolicy::default(),
        );
        assert_eq!(stacked_result.text, "stacked text");
    }

    #[test]
    fn malformed_encoding_falls_back_to_capped_raw_and_non_utf8_is_summarized() {
        let malformed = decode_job(
            job(
                b"not gzip".to_vec(),
                vec![("content-encoding".into(), "gzip".into())],
            ),
            &DecodePolicy::default(),
        );
        assert!(malformed.error.is_some());
        assert!(malformed.text.contains("Display decode failed"));

        let binary = decode_job(
            job(vec![0xff, 0x00, 0x80], Vec::new()),
            &DecodePolicy::default(),
        );
        assert!(binary.text.contains("Binary body"));
        assert!(binary.text.contains("ff 00 80"));
    }

    #[tokio::test]
    async fn duplicate_jobs_coalesce_and_stale_result_is_superseded() {
        let shutdown = CancellationToken::new();
        let mut service = start_decode_service(DecodePolicy::default(), shutdown.clone());
        let key = DecodeKey {
            sequence: CaptureSequence::new(1),
            side: BodySide::Response,
            revision: 1,
            mode: DecodeDisplayMode::Response,
        };
        assert!(service.client.request(
            key,
            CapturedBodyPreview::unbudgeted(Bytes::from_static(b"one")),
            CapturedHeaders::unbudgeted(Arc::from([])),
        ));
        assert!(service.client.request(
            key,
            CapturedBodyPreview::unbudgeted(Bytes::from_static(b"one")),
            CapturedHeaders::unbudgeted(Arc::from([])),
        ));
        let newer = DecodeKey { revision: 2, ..key };
        assert!(service.client.request(
            newer,
            CapturedBodyPreview::unbudgeted(Bytes::from_static(b"two")),
            CapturedHeaders::unbudgeted(Arc::from([])),
        ));

        let result =
            tokio::time::timeout(std::time::Duration::from_secs(1), service.results.recv())
                .await
                .expect("decode should finish")
                .expect("decode result should arrive");
        assert_eq!(result.key, newer);
        shutdown.cancel();
        service
            .task
            .await
            .expect("service join")
            .expect("service stop");
        assert!(service.metrics.snapshot().superseded >= 1);
    }

    #[tokio::test]
    async fn chunked_preview_decodes_to_the_same_display_as_contiguous_input() {
        use crate::capture::{
            CapturePolicy, CapturePublisher, CaptureSnapshotMode, RequestCaptureInput,
        };
        use hyper::{HeaderMap, Method};
        use tokio::sync::mpsc;

        let payload = format!(
            r#"{{"message":"{}","count":3}}"#,
            "fragmented-preview".repeat(12_000)
        )
        .into_bytes();
        let headers = vec![("content-type".into(), "application/json".into())];
        let contiguous = decode_job(
            job(payload.clone(), headers.clone()),
            &DecodePolicy::default(),
        );
        let (tx, mut rx) = mpsc::channel(1);
        let publisher = CapturePublisher::new(tx, CapturePolicy::default());
        let request_headers = HeaderMap::new();
        let handle = publisher
            .try_start(RequestCaptureInput {
                method: Method::GET,
                original_uri: "https://example.com/chunked",
                effective_uri: "https://example.com/chunked",
                local_path: None,
                headers: &request_headers,
            })
            .expect("capture admitted");
        let record = rx.recv().await.expect("capture published");
        for fragment in payload.chunks(37) {
            handle.append(BodySide::Response, fragment);
        }
        let chunked_preview = record
            .snapshot(CaptureSnapshotMode::WithBodyPreviews)
            .response_body
            .preview;

        let chunked = decode_job(
            job_with_preview(chunked_preview, headers),
            &DecodePolicy::default(),
        );

        assert_eq!(chunked.text, contiguous.text);
        assert_eq!(chunked.limited, contiguous.limited);
        assert_eq!(chunked.error, contiguous.error);
    }

    fn compress_bytes(input: &[u8]) -> Vec<u8> {
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(input).expect("gzip write");
        encoder.finish().expect("gzip finish")
    }

    fn compress_with<W>(mut encoder: W) -> Vec<u8>
    where
        W: Write + FinishEncoder,
    {
        encoder.write_all(b"deflate text").expect("deflate write");
        encoder.finish_bytes()
    }

    trait FinishEncoder {
        fn finish_bytes(self) -> Vec<u8>;
    }

    impl FinishEncoder for ZlibEncoder<Vec<u8>> {
        fn finish_bytes(self) -> Vec<u8> {
            self.finish().expect("zlib finish")
        }
    }

    impl FinishEncoder for DeflateEncoder<Vec<u8>> {
        fn finish_bytes(self) -> Vec<u8> {
            self.finish().expect("deflate finish")
        }
    }
}

#[cfg(test)]
mod content_decode_tests;

#[cfg(test)]
mod display_decode_tests;

#[cfg(test)]
mod test_support;
