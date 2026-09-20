#![cfg(unix)]

use super::{
    body_test_support::{headers, request, status},
    page_decoded_body, page_raw_body,
};
use crate::{
    capture::{
        BodyStreamState, CaptureRecord, CaptureSequence, CaptureSnapshotMode, CapturedBodyPreview,
        CapturedExchange, CapturedHeaders, ContentDecodePolicy, DecodedBytes, decode_content_bytes,
    },
    control::body::BodyRepresentation,
    control_rpc::protocol::ControlErrorCode,
};
use flate2::{
    Compression,
    write::{DeflateEncoder, GzEncoder, ZlibEncoder},
};
use hyper::{Method, body::Bytes};
use serde_json::json;
use std::{io::Write, sync::atomic::AtomicBool};

#[test]
fn raw_page_slices_across_preview_chunks_without_flattening_before_transport() {
    let retained = format!("{}abcdef{}", "x".repeat(64 * 1_024 - 3), "y".repeat(65_000));
    let snapshot = CaptureRecord::from_completed(CapturedExchange {
        sequence: CaptureSequence::new(7),
        method: Method::GET,
        uri: "https://example.test/chunked".to_owned(),
        mapped_uri: None,
        local_path: None,
        status: Some(200),
        req_headers: vec![],
        res_headers: vec![(
            "Content-Type".to_owned(),
            "  application/octet-stream ; charset=binary  ".to_owned(),
        )],
        req_body: None,
        res_body: Some(retained.clone()),
    })
    .snapshot(CaptureSnapshotMode::WithBodyPreviews);
    let preview = snapshot.response_body.preview;
    assert!(
        preview.chunks().count() > 1,
        "fixture must cross chunk nodes"
    );
    assert_eq!(preview.test_flatten_calls(), 0);

    let page = page_raw_body(
        &preview,
        &request(BodyRepresentation::Raw, 64 * 1_024 - 4, 8),
        &snapshot.response_body.status,
        &snapshot.response.as_ref().expect("response").headers,
    )
    .expect("raw page");
    assert_eq!(page.content.as_ref(), b"xabcdefy");
    assert_eq!(page.actual_range.offset, 64 * 1_024 - 4);
    assert_eq!(page.actual_range.length, 8);
    assert_eq!(page.next_offset, Some(64 * 1_024 + 4));
    assert_eq!(
        page.media_type.as_deref(),
        Some("application/octet-stream ; charset=binary")
    );
    assert_eq!(preview.test_flatten_calls(), 0);
}

#[test]
fn raw_page_is_byte_exact_and_clamps_offsets_beyond_the_retained_body() {
    let preview = CapturedBodyPreview::unbudgeted(Bytes::from_static(&[0xff, 0x00, 0x80, b'a']));
    let body_status = status(BodyStreamState::Complete, 4);
    let raw_headers = headers(" application/octet-stream ");
    let page = page_raw_body(
        &preview,
        &request(BodyRepresentation::Raw, 1, 2),
        &body_status,
        &raw_headers,
    )
    .expect("binary range");
    assert_eq!(page.content.as_ref(), &[0x00, 0x80]);
    assert_eq!(page.requested_range.offset, 1);
    assert_eq!(page.requested_range.length, 2);
    assert_eq!(page.actual_range.offset, 1);
    assert_eq!(page.actual_range.length, 2);
    assert_eq!(page.total_bytes, 4);
    assert_eq!(page.next_offset, Some(3));

    let beyond = page_raw_body(
        &preview,
        &request(BodyRepresentation::Raw, usize::MAX, 2),
        &body_status,
        &raw_headers,
    )
    .expect("offset beyond body");
    assert!(beyond.content.is_empty());
    assert_eq!(beyond.actual_range.offset, 4);
    assert_eq!(beyond.actual_range.length, 0);
    assert_eq!(beyond.total_bytes, 4);
    assert_eq!(beyond.next_offset, None);
}

#[test]
fn decoded_utf8_rejects_only_a_misaligned_start_and_reports_nearest_boundaries() {
    let decoded = DecodedBytes::new(
        Bytes::from_static("中a".as_bytes()),
        vec!["gzip".to_owned()],
        false,
    );
    let error = page_decoded_body(
        &decoded,
        &request(BodyRepresentation::Decoded, 1, 2),
        &status(BodyStreamState::Complete, 12),
        &headers("  text/plain; charset=utf-8  "),
    )
    .expect_err("misaligned UTF-8 start");
    assert_eq!(error.code, ControlErrorCode::InvalidArgument);
    assert_eq!(
        error.details,
        json!({"offset": 1, "nearest_start": 0, "nearest_end": 3})
    );
}

#[test]
fn decoded_utf8_treats_length_as_a_maximum_and_aligns_actual_end_down() {
    let decoded = DecodedBytes::new(Bytes::from_static("a中".as_bytes()), Vec::new(), false);
    let page = page_decoded_body(
        &decoded,
        &request(BodyRepresentation::Decoded, 0, 2),
        &status(BodyStreamState::Complete, 4),
        &headers(" text/plain; charset=utf-8 "),
    )
    .expect("aligned UTF-8 start");
    assert_eq!(page.content.as_ref(), b"a");
    assert_eq!(page.requested_range.length, 2);
    assert_eq!(page.actual_range.length, 1);
    assert_eq!(page.next_offset, Some(1));
    assert_eq!(
        page.media_type.as_deref(),
        Some("text/plain; charset=utf-8")
    );

    let final_page = page_decoded_body(
        &decoded,
        &request(BodyRepresentation::Decoded, 1, 64),
        &status(BodyStreamState::Complete, 4),
        &headers("text/plain"),
    )
    .expect("final aligned page");
    assert_eq!(final_page.content.as_ref(), "中".as_bytes());
    assert_eq!(final_page.actual_range.length, 3);
    assert_eq!(final_page.next_offset, None);
}

#[test]
fn decoded_utf8_short_page_advances_to_the_next_character_boundary() {
    let decoded = DecodedBytes::new(Bytes::from_static("中a".as_bytes()), Vec::new(), false);
    let page = page_decoded_body(
        &decoded,
        &request(BodyRepresentation::Decoded, 0, 1),
        &status(BodyStreamState::Complete, 4),
        &headers("text/plain"),
    )
    .expect("short decoded page");

    assert_eq!(page.content.as_ref(), "中".as_bytes());
    assert_eq!(page.requested_range.length, 1);
    assert_eq!(page.actual_range.length, 3);
    assert_eq!(page.next_offset, Some(3));
}

fn encode_with<W>(mut encoder: W, plain: &[u8]) -> Vec<u8>
where
    W: Write,
    W: RuntimeFinishEncoder,
{
    encoder.write_all(plain).expect("encoded body input");
    encoder.finish_bytes()
}

trait RuntimeFinishEncoder {
    fn finish_bytes(self) -> Vec<u8>;
}

impl RuntimeFinishEncoder for GzEncoder<Vec<u8>> {
    fn finish_bytes(self) -> Vec<u8> {
        self.finish().expect("gzip finish")
    }
}

impl RuntimeFinishEncoder for ZlibEncoder<Vec<u8>> {
    fn finish_bytes(self) -> Vec<u8> {
        self.finish().expect("zlib finish")
    }
}

impl RuntimeFinishEncoder for DeflateEncoder<Vec<u8>> {
    fn finish_bytes(self) -> Vec<u8> {
        self.finish().expect("deflate finish")
    }
}

#[test]
fn decoded_page_offsets_apply_after_every_content_encoding_codec() {
    let plain = b"prefix-middle-suffix";
    let gzip = encode_with(GzEncoder::new(Vec::new(), Compression::default()), plain);
    let zlib = encode_with(ZlibEncoder::new(Vec::new(), Compression::default()), plain);
    let raw_deflate = encode_with(
        DeflateEncoder::new(Vec::new(), Compression::default()),
        plain,
    );
    let mut br = Vec::new();
    {
        let mut encoder = brotli::CompressorWriter::new(&mut br, 4_096, 5, 22);
        encoder.write_all(plain).expect("brotli body input");
    }
    let zstd = zstd::stream::encode_all(plain.as_slice(), 1).expect("zstd body input");

    for (label, encoded, encoding) in [
        ("identity", plain.to_vec(), "identity"),
        ("gzip", gzip, "gzip"),
        ("zlib", zlib, "deflate"),
        ("raw deflate", raw_deflate, "deflate"),
        ("brotli", br, "br"),
        ("zstd", zstd, "zstd"),
    ] {
        let decoded = decode_content_bytes(
            &CapturedBodyPreview::unbudgeted(Bytes::from(encoded)),
            &CapturedHeaders::unbudgeted(
                vec![("Content-Encoding".to_owned(), encoding.to_owned())].into(),
            ),
            &ContentDecodePolicy::default(),
            &AtomicBool::new(false),
        )
        .unwrap_or_else(|_| panic!("{label} decode"));
        let page = page_decoded_body(
            &decoded,
            &request(BodyRepresentation::Decoded, 7, 6),
            &status(BodyStreamState::Complete, plain.len()),
            &headers("text/plain"),
        )
        .unwrap_or_else(|_| panic!("{label} decoded page"));
        assert_eq!(page.content.as_ref(), b"middle", "{label}");
        assert_eq!(page.actual_range.offset, 7, "{label}");
        assert_eq!(page.actual_range.length, 6, "{label}");
    }
}

#[test]
fn decoded_output_limit_does_not_report_source_capture_truncation() {
    let decoded = DecodedBytes::new(
        Bytes::from_static(b"bounded decoded content"),
        vec!["gzip".to_owned()],
        true,
    );
    let page = page_decoded_body(
        &decoded,
        &request(BodyRepresentation::Decoded, 0, 8),
        &status(BodyStreamState::Complete, 23),
        &headers("text/plain"),
    )
    .expect("decoded page");

    assert!(!page.source.truncated);
    assert_eq!(page.source.truncation_reason, None);
    assert!(page.source.decoded_output_limited);
}

#[test]
fn decoded_valid_utf8_aligns_ranges_even_without_a_textual_media_type() {
    let decoded = DecodedBytes::new(Bytes::from_static("中".as_bytes()), Vec::new(), false);
    let error = page_decoded_body(
        &decoded,
        &request(BodyRepresentation::Decoded, 1, 1),
        &status(BodyStreamState::Complete, 3),
        &headers("application/octet-stream"),
    )
    .expect_err("valid UTF-8 must use character boundaries");

    assert_eq!(error.code, ControlErrorCode::InvalidArgument);
    assert_eq!(
        error.details,
        json!({"offset": 1, "nearest_start": 0, "nearest_end": 3})
    );
}

#[test]
fn decoded_binary_ranges_remain_byte_exact_without_utf8_alignment() {
    let decoded = DecodedBytes::new(
        Bytes::from_static(&[0xff, 0x00, 0x80, b'a']),
        vec!["gzip".to_owned()],
        false,
    );
    let page = page_decoded_body(
        &decoded,
        &request(BodyRepresentation::Decoded, 1, 2),
        &status(BodyStreamState::Complete, 4),
        &headers("application/octet-stream"),
    )
    .expect("decoded binary range");
    assert_eq!(page.content.as_ref(), &[0x00, 0x80]);
    assert_eq!(page.actual_range.offset, 1);
    assert_eq!(page.actual_range.length, 2);
}

#[test]
fn decoded_page_owns_only_its_bounded_window() {
    let source = Bytes::from(vec![b'x'; 1024 * 1024]);
    let source_start = source.as_ptr() as usize;
    let source_end = source_start + source.len();
    let decoded = DecodedBytes::new(source, Vec::new(), false);

    let page = page_decoded_body(
        &decoded,
        &request(BodyRepresentation::Decoded, 128, 64),
        &status(BodyStreamState::Complete, decoded.bytes.len()),
        &headers("text/plain"),
    )
    .expect("decoded page");
    let page_start = page.content.as_ptr() as usize;

    assert_eq!(page.content.len(), 64);
    assert!(
        page_start < source_start || page_start >= source_end,
        "page bytes must not retain the full decoded allocation"
    );
}
