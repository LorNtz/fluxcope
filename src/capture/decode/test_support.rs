use crate::capture::{CapturedBodyPreview, CapturedHeaders};
use flate2::{Compression, write::GzEncoder};
use hyper::body::Bytes;
use std::io::Write;

pub(super) fn preview(bytes: Vec<u8>) -> CapturedBodyPreview {
    CapturedBodyPreview::unbudgeted(Bytes::from(bytes))
}

pub(super) fn headers(encoding: Option<&str>, content_type: Option<&str>) -> CapturedHeaders {
    let mut values = Vec::new();
    if let Some(encoding) = encoding {
        values.push(("Content-Encoding".to_owned(), encoding.to_owned()));
    }
    if let Some(content_type) = content_type {
        values.push(("Content-Type".to_owned(), content_type.to_owned()));
    }
    CapturedHeaders::unbudgeted(values.into())
}

pub(super) fn gzip(input: &[u8]) -> Vec<u8> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(input).expect("gzip input");
    encoder.finish().expect("gzip finish")
}
