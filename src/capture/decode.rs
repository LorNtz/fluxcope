use std::{
    collections::HashSet,
    fmt,
    io::{self, Read, Write},
    sync::{
        Arc,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
};

use anyhow::{Context, Result};
#[cfg(test)]
use hyper::body::Bytes;
use parking_lot::Mutex;
use tokio::{
    sync::mpsc,
    task::{JoinHandle, JoinSet},
};
use tokio_util::sync::CancellationToken;

use super::{BodySide, CaptureSequence, CapturedBodyPreview, CapturedHeaders};

const DISPLAY_LIMIT_SUFFIX: &str = "\n[Display truncated at configured limit]";

#[derive(Clone, Debug)]
pub(crate) struct DecodePolicy {
    pub queue_capacity: usize,
    pub max_active: usize,
    pub max_queued_input_bytes: usize,
    pub max_output_bytes: usize,
    pub max_json_input_bytes: usize,
}

impl Default for DecodePolicy {
    fn default() -> Self {
        Self {
            queue_capacity: 8,
            max_active: 2,
            max_queued_input_bytes: 32 * 1024 * 1024,
            max_output_bytes: 16 * 1024 * 1024,
            max_json_input_bytes: 2 * 1024 * 1024,
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
}

struct DecodeClientState {
    pending: Mutex<HashSet<DecodeKey>>,
    desired: Mutex<Option<DecodeKey>>,
    queued_input_bytes: AtomicUsize,
}

impl DecodeClient {
    pub fn request(
        &self,
        key: DecodeKey,
        input: CapturedBodyPreview,
        headers: CapturedHeaders,
    ) -> bool {
        *self.state.desired.lock() = Some(key);
        {
            let mut pending = self.state.pending.lock();
            if !pending.insert(key) {
                return true;
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
        let job = DecodeJob {
            key,
            input,
            headers,
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
        self.metrics.rejected.fetch_add(1, Ordering::Relaxed);
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
}

struct DecodeCompletion {
    key: DecodeKey,
    result: DecodeResult,
}

pub(crate) fn start_decode_service(
    mut policy: DecodePolicy,
    shutdown: CancellationToken,
) -> DecodeService {
    policy.queue_capacity = policy.queue_capacity.max(1);
    policy.max_active = policy.max_active.max(1);
    let policy = Arc::new(policy);
    let (tx, rx) = mpsc::channel(policy.queue_capacity);
    let (result_tx, results) = mpsc::channel(policy.queue_capacity);
    let state = Arc::new(DecodeClientState {
        pending: Mutex::new(HashSet::new()),
        desired: Mutex::new(None),
        queued_input_bytes: AtomicUsize::new(0),
    });
    let metrics = Arc::new(DecodeMetrics::default());
    let client = DecodeClient {
        tx,
        state: Arc::clone(&state),
        policy: Arc::clone(&policy),
        metrics: Arc::clone(&metrics),
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
                active.spawn(async move {
                    let result = tokio::task::spawn_blocking(move || decode_job(job, &worker_policy))
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

fn decode_job(job: DecodeJob, policy: &DecodePolicy) -> DecodeResult {
    let input = job.input.flatten();
    let decoded = decode_content_encoded(input, &job.headers, policy.max_output_bytes);
    let (bytes, mut limited, error) = match decoded {
        Ok(stage) => (stage.bytes, stage.limited, None),
        Err(error) => {
            let mut fallback = job.input.flatten();
            let limited = fallback.len() > policy.max_output_bytes;
            fallback.truncate(policy.max_output_bytes);
            (fallback, limited, Some(error.to_string()))
        }
    };
    let formatted = format_body(&bytes, &job.headers, job.key.mode, policy);
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

struct DecodedStage {
    bytes: Vec<u8>,
    limited: bool,
}

fn decode_content_encoded(
    mut current: Vec<u8>,
    headers: &[(String, String)],
    limit: usize,
) -> io::Result<DecodedStage> {
    let encodings = content_encodings(headers);
    for encoding in encodings.iter().rev() {
        let stage = match encoding.as_str() {
            "gzip" | "x-gzip" => {
                read_limited(flate2::read::GzDecoder::new(current.as_slice()), limit)
            }
            "deflate" => decode_deflate_limited(&current, limit),
            "br" => read_limited(brotli::Decompressor::new(current.as_slice(), 4096), limit),
            "zstd" => zstd::stream::read::Decoder::new(current.as_slice())
                .and_then(|decoder| read_limited(decoder, limit)),
            "identity" => {
                return Ok(DecodedStage {
                    bytes: current,
                    limited: false,
                });
            }
            unsupported => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unsupported content encoding {unsupported}"),
            )),
        };
        let stage = stage?;
        if stage.limited {
            return Ok(stage);
        }
        current = stage.bytes;
    }
    let limited = current.len() > limit;
    current.truncate(limit);
    Ok(DecodedStage {
        bytes: current,
        limited,
    })
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

fn decode_deflate_limited(input: &[u8], limit: usize) -> io::Result<DecodedStage> {
    read_limited(flate2::read::ZlibDecoder::new(input), limit)
        .or_else(|_| read_limited(flate2::read::DeflateDecoder::new(input), limit))
}

fn read_limited(mut reader: impl Read, limit: usize) -> io::Result<DecodedStage> {
    let mut bytes = Vec::with_capacity(limit.min(64 * 1024));
    reader
        .by_ref()
        .take(limit.saturating_add(1) as u64)
        .read_to_end(&mut bytes)?;
    let limited = bytes.len() > limit;
    bytes.truncate(limit);
    Ok(DecodedStage { bytes, limited })
}

struct FormattedBody {
    text: String,
    limited: bool,
}

fn format_body(
    bytes: &[u8],
    headers: &[(String, String)],
    mode: DecodeDisplayMode,
    policy: &DecodePolicy,
) -> FormattedBody {
    if bytes.is_empty() {
        return capped_text(
            if mode == DecodeDisplayMode::MapLocal {
                ""
            } else {
                "(No body)"
            },
            policy.max_output_bytes,
        );
    }
    let Ok(text) = std::str::from_utf8(bytes) else {
        return capped_text(&binary_body_summary(bytes), policy.max_output_bytes);
    };
    if mode == DecodeDisplayMode::MapLocal {
        return capped_text(text, policy.max_output_bytes);
    }
    if mode == DecodeDisplayMode::Request && is_form_data(headers) {
        return format_form(text, policy.max_output_bytes);
    }
    if bytes.len() <= policy.max_json_input_bytes
        && let Ok(value) = serde_json::from_slice::<serde_json::Value>(bytes)
    {
        let mut writer = LimitedWriter::new(policy.max_output_bytes);
        let result = serde_json::to_writer_pretty(&mut writer, &value);
        if result.is_ok() || writer.limited {
            let decoded = String::from_utf8_lossy(&writer.bytes);
            let capped = capped_text(&decoded, policy.max_output_bytes);
            return FormattedBody {
                text: capped.text,
                limited: writer.limited || capped.limited,
            };
        }
    }
    capped_text(text, policy.max_output_bytes)
}

fn is_form_data(headers: &[(String, String)]) -> bool {
    headers.iter().any(|(name, value)| {
        name.eq_ignore_ascii_case("content-type")
            && value
                .to_ascii_lowercase()
                .contains("application/x-www-form-urlencoded")
    })
}

fn format_form(input: &str, limit: usize) -> FormattedBody {
    let mut output = LimitedString::new(limit);
    for (index, (key, value)) in url::form_urlencoded::parse(input.as_bytes()).enumerate() {
        if index > 0 && fmt::Write::write_char(&mut output, '\n').is_err() {
            break;
        }
        if fmt::Write::write_fmt(&mut output, format_args!("{key}: {value}")).is_err() {
            break;
        }
    }
    FormattedBody {
        text: output.text,
        limited: output.limited,
    }
}

fn capped_text(input: &str, limit: usize) -> FormattedBody {
    if input.len() <= limit {
        return FormattedBody {
            text: input.to_string(),
            limited: false,
        };
    }
    let mut end = limit;
    while !input.is_char_boundary(end) {
        end -= 1;
    }
    FormattedBody {
        text: input[..end].to_string(),
        limited: true,
    }
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

struct LimitedWriter {
    bytes: Vec<u8>,
    limit: usize,
    limited: bool,
}

impl LimitedWriter {
    fn new(limit: usize) -> Self {
        Self {
            bytes: Vec::with_capacity(limit.min(64 * 1024)),
            limit,
            limited: false,
        }
    }
}

impl Write for LimitedWriter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        let remaining = self.limit.saturating_sub(self.bytes.len());
        if buffer.len() > remaining {
            self.bytes.extend_from_slice(&buffer[..remaining]);
            self.limited = true;
            return Err(io::Error::other("display output limit reached"));
        }
        self.bytes.extend_from_slice(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

struct LimitedString {
    text: String,
    limit: usize,
    limited: bool,
}

impl LimitedString {
    fn new(limit: usize) -> Self {
        Self {
            text: String::with_capacity(limit.min(64 * 1024)),
            limit,
            limited: false,
        }
    }
}

impl fmt::Write for LimitedString {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        let remaining = self.limit.saturating_sub(self.text.len());
        if text.len() > remaining {
            let mut end = remaining;
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            self.text.push_str(&text[..end]);
            self.limited = true;
            return Err(fmt::Error);
        }
        self.text.push_str(text);
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
