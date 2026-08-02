use super::{BodyDisplayPreparation, BodyRenderText, BodyViewerKey, MainDisplayTab};
use crate::app::App;
use crate::capture::{
    BodySide, BodyStreamState, CaptureRecord, DecodeDisplayMode, DecodeKey, DecodeResult,
};
use std::time::Instant;

const BODY_UNAVAILABLE_TEXT: &str = "(Body display unavailable: decoder queue is full)";

impl App {
    pub fn enter_current_body_viewer(&mut self) -> bool {
        if !self.ensure_current_body_viewer_content() {
            return false;
        }

        self.detail_panel.body_viewer.enter();
        true
    }

    pub fn ensure_current_body_viewer_content(&mut self) -> bool {
        let Some(key) = self.current_body_viewer_key() else {
            return false;
        };

        if self.detail_panel.body_viewer.has_content_for(key) {
            return true;
        }

        let Some((key, text)) = self.current_body_text() else {
            return false;
        };
        self.detail_panel.body_viewer.load_content(key, text);
        true
    }

    pub fn current_body_viewer_key(&self) -> Option<BodyViewerKey> {
        let capture = self.selected_request()?;
        self.detail_panel.active_tab.is_body().then(|| {
            BodyViewerKey::with_revision(
                capture.sequence,
                self.detail_panel.active_tab,
                capture.revision,
            )
        })
    }

    pub(crate) fn prepare_current_body_display(&mut self, now: Instant) -> BodyDisplayPreparation {
        if self.log_panel.visible || self.certificate_popup.visible || self.settings_popup.visible {
            self.detail_panel.disable_pending_body_deferral();
            return BodyDisplayPreparation::ReadyToRender;
        }

        let Some(key) = self.current_body_viewer_key() else {
            return BodyDisplayPreparation::ReadyToRender;
        };
        if self.detail_panel.body_viewer.has_content_for(key) {
            return BodyDisplayPreparation::ReadyToRender;
        }

        self.prepare_body_text(key, now);
        self.detail_panel
            .pending_body_started_at(key)
            .map(|started_at| BodyDisplayPreparation::Pending { started_at })
            .unwrap_or(BodyDisplayPreparation::ReadyToRender)
    }

    fn prepare_body_text(&mut self, key: BodyViewerKey, now: Instant) {
        if self.detail_panel.has_body_text_state(key) {
            return;
        }

        let (record, capture) = {
            let Some(record) = self.selected_capture_record() else {
                return;
            };
            let capture = record.summary();

            if BodyViewerKey::with_revision(capture.sequence, key.tab(), capture.revision) != key {
                return;
            }
            (record, capture)
        };
        let (side, status, mode) = match key.tab() {
            MainDisplayTab::RequestBody => (
                BodySide::Request,
                &capture.request_body,
                DecodeDisplayMode::Request,
            ),
            MainDisplayTab::ResponseBody => (
                BodySide::Response,
                &capture.response_body,
                if capture.request.local_path.is_some() {
                    DecodeDisplayMode::MapLocal
                } else {
                    DecodeDisplayMode::Response
                },
            ),
            MainDisplayTab::RequestHeader | MainDisplayTab::ResponseHeader => return,
        };

        if !matches!(
            status.stream,
            BodyStreamState::Complete | BodyStreamState::Failed | BodyStreamState::Cancelled
        ) {
            self.detail_panel.cache_body_text(
                key,
                format!("(Body streaming… {} bytes observed)", status.observed_bytes),
            );
            return;
        }

        if status.observed_bytes == 0 {
            let text = if mode == DecodeDisplayMode::MapLocal {
                String::new()
            } else {
                "(No body)".to_string()
            };
            self.detail_panel.cache_body_text(key, text);
            return;
        }

        if let Some(client) = self.decode_client.as_ref() {
            let headers = match key.tab() {
                MainDisplayTab::RequestBody => capture.request.headers.clone(),
                MainDisplayTab::ResponseBody => capture
                    .response
                    .as_ref()
                    .map(|response| response.headers.clone())
                    .unwrap_or_else(|| record.empty_headers()),
                MainDisplayTab::RequestHeader | MainDisplayTab::ResponseHeader => return,
            };
            let requested = client.request(
                DecodeKey {
                    sequence: key.sequence(),
                    side,
                    revision: key.revision(),
                    mode,
                },
                record.body_preview(side),
                headers,
            );
            if requested {
                self.detail_panel.set_body_text_pending(key, now);
            } else {
                self.detail_panel
                    .set_body_text_unavailable(key, BODY_UNAVAILABLE_TEXT);
            }
            return;
        }

        if let Some(text) = body_text_for_record(&record, key.tab()) {
            self.detail_panel.cache_body_text(key, text);
        }
    }

    pub(crate) fn body_render_text(&self, key: BodyViewerKey) -> Option<BodyRenderText<'_>> {
        self.detail_panel.body_render_text(key)
    }

    fn body_text_is_ready(&self, key: BodyViewerKey) -> bool {
        self.cached_body_text(key).is_some()
    }

    pub(crate) fn refresh_selected_live_body(&mut self) -> bool {
        if self.detail_panel.body_viewer.is_active() {
            return false;
        }
        let Some(key) = self.current_body_viewer_key() else {
            return false;
        };
        let Some(capture) = self.selected_request() else {
            return false;
        };
        let status = match key.tab() {
            MainDisplayTab::RequestBody => &capture.request_body,
            MainDisplayTab::ResponseBody => &capture.response_body,
            MainDisplayTab::RequestHeader | MainDisplayTab::ResponseHeader => return false,
        };
        if matches!(
            status.stream,
            BodyStreamState::Complete | BodyStreamState::Failed | BodyStreamState::Cancelled
        ) {
            return false;
        }
        let text = format!("(Body streaming… {} bytes observed)", status.observed_bytes);
        if self.cached_body_text(key) == Some(text.as_str()) {
            return false;
        }
        self.detail_panel.cache_body_text(key, text);
        true
    }

    pub(crate) fn apply_decode_result(&mut self, result: DecodeResult) -> bool {
        let tab = match result.key.side {
            BodySide::Request => MainDisplayTab::RequestBody,
            BodySide::Response => MainDisplayTab::ResponseBody,
        };
        let key = BodyViewerKey::with_revision(result.key.sequence, tab, result.key.revision);
        if self.current_body_viewer_key() != Some(key)
            || !self.detail_panel.body_text_is_pending(key)
        {
            return false;
        }
        self.detail_panel.cache_body_text(key, result.text);
        true
    }

    pub(crate) fn cached_body_text(&self, key: BodyViewerKey) -> Option<&str> {
        self.detail_panel.cached_body_text(key)
    }

    #[cfg(test)]
    pub(crate) fn cached_body_render_text(&self, key: BodyViewerKey) -> Option<&str> {
        self.detail_panel.cached_body_render_text(key)
    }

    pub(crate) fn body_text_revision(&self) -> u64 {
        self.detail_panel.body_text_revision()
    }

    fn current_body_text(&mut self) -> Option<(BodyViewerKey, String)> {
        let key = self.current_body_viewer_key()?;
        self.prepare_body_text(key, Instant::now());
        if !self.body_text_is_ready(key) {
            return None;
        }

        self.detail_panel
            .take_cached_body_text(key)
            .map(|text| (key, text))
    }
}

fn body_text_for_record(record: &CaptureRecord, tab: MainDisplayTab) -> Option<String> {
    let capture = record.summary();
    let (side, status) = match tab {
        MainDisplayTab::RequestBody => (BodySide::Request, &capture.request_body),
        MainDisplayTab::ResponseBody => (BodySide::Response, &capture.response_body),
        MainDisplayTab::RequestHeader | MainDisplayTab::ResponseHeader => return None,
    };
    if !matches!(
        status.stream,
        BodyStreamState::Complete | BodyStreamState::Failed | BodyStreamState::Cancelled
    ) {
        return Some(format!(
            "(Body streaming… {} bytes observed)",
            status.observed_bytes
        ));
    }

    let preview = record.body_preview(side);
    let body = if preview.is_empty() {
        None
    } else {
        std::str::from_utf8(&preview).ok()
    };
    Some(match side {
        BodySide::Request => format_request_body(body, &capture.request.headers),
        BodySide::Response if capture.request.local_path.is_some() => {
            body.unwrap_or_default().to_string()
        }
        BodySide::Response => match body {
            Some(body) => format_json_body(body),
            None if status.observed_bytes == 0 => "(No body)".to_string(),
            None => binary_body_summary(&preview, status.observed_bytes),
        },
    })
}

fn binary_body_summary(bytes: &[u8], observed_bytes: u64) -> String {
    use std::fmt::Write as _;

    let mut hex = String::new();
    for (index, byte) in bytes.iter().take(32).enumerate() {
        if index > 0 {
            hex.push(' ');
        }
        let _ = write!(hex, "{byte:02x}");
    }
    format!(
        "[Binary body: {observed_bytes} bytes observed, first {} retained bytes in hex: {hex}]",
        bytes.len().min(32)
    )
}

fn format_request_body(body: Option<&str>, headers: &[(String, String)]) -> String {
    match body {
        Some("") => "(Empty body)".to_string(),
        Some(body) if is_form_data(headers) => format_form_body(body),
        Some(body) => format_json_body(body),
        None => "(No body)".to_string(),
    }
}

fn is_form_data(headers: &[(String, String)]) -> bool {
    headers.iter().any(|(key, value)| {
        key.eq_ignore_ascii_case("content-type")
            && value
                .to_ascii_lowercase()
                .contains("application/x-www-form-urlencoded")
    })
}

fn format_form_body(body: &str) -> String {
    body.split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let mut parts = pair.splitn(2, '=');
            let key = parts.next().unwrap_or("");
            let value = parts.next().unwrap_or("");
            format!(
                "{}: {}",
                decode_url_component(key),
                decode_url_component(value)
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn format_json_body(body: &str) -> String {
    serde_json::from_str::<serde_json::Value>(body)
        .map(|value| serde_json::to_string_pretty(&value).unwrap_or_else(|_| body.to_string()))
        .unwrap_or_else(|_| body.to_string())
}

fn decode_url_component(input: &str) -> String {
    let input_bytes = input.as_bytes();
    let mut decoded = Vec::with_capacity(input_bytes.len());
    let mut cursor = 0;

    while cursor < input_bytes.len() {
        match input_bytes[cursor] {
            b'%' if cursor + 2 < input_bytes.len() => {
                let h1 = input_bytes[cursor + 1];
                let h2 = input_bytes[cursor + 2];
                if let (Some(hi), Some(lo)) = (hex_to_nibble(h1), hex_to_nibble(h2)) {
                    decoded.push(hi * 16 + lo);
                    cursor += 3;
                    continue;
                }
                decoded.push(input_bytes[cursor]);
                cursor += 1;
            }
            b'+' => {
                decoded.push(b' ');
                cursor += 1;
            }
            byte => {
                decoded.push(byte);
                cursor += 1;
            }
        }
    }

    String::from_utf8(decoded).unwrap_or_else(|err| String::from_utf8_lossy(err.as_bytes()).into())
}

fn hex_to_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::{CaptureRecord, CaptureSequence, CapturedExchange};
    use http::Method;
    use std::sync::Arc;

    fn completed_record(
        request_body: Option<&str>,
        response_body: Option<&str>,
        request_headers: Vec<(String, String)>,
        local_path: Option<&str>,
    ) -> Arc<CaptureRecord> {
        CaptureRecord::from_completed(CapturedExchange {
            sequence: CaptureSequence::new(0),
            method: Method::POST,
            uri: "https://example.com/api".to_string(),
            mapped_uri: None,
            local_path: local_path.map(str::to_string),
            status: Some(200),
            req_headers: request_headers,
            res_headers: Vec::new(),
            req_body: request_body.map(str::to_string),
            res_body: response_body.map(str::to_string),
        })
    }

    #[test]
    fn map_local_response_body_preserves_raw_text() {
        let raw_body = "{\n  \"z\": 1,\n  \"a\": 2\n}\n";
        let record = completed_record(None, Some(raw_body), Vec::new(), Some("/tmp/api.json"));

        assert_eq!(
            body_text_for_record(&record, MainDisplayTab::ResponseBody).as_deref(),
            Some(raw_body)
        );
    }

    #[test]
    fn non_local_response_body_uses_json_formatting() {
        let raw_body = r#"{"z":1,"a":2}"#;
        let record = completed_record(None, Some(raw_body), Vec::new(), None);

        assert_ne!(
            body_text_for_record(&record, MainDisplayTab::ResponseBody).as_deref(),
            Some(raw_body)
        );
    }

    #[test]
    fn form_request_body_decodes_percent_encoded_utf8_text() {
        let headers = vec![(
            "content-type".to_string(),
            "application/x-www-form-urlencoded".to_string(),
        )];
        let record = completed_record(
            Some("name=%E4%B8%AD%E6%96%87&city=%E5%8C%97%E4%BA%AC"),
            None,
            headers,
            None,
        );

        assert_eq!(
            body_text_for_record(&record, MainDisplayTab::RequestBody).as_deref(),
            Some("name: 中文\ncity: 北京")
        );
    }

    #[test]
    fn request_and_response_json_bodies_use_the_same_formatting() {
        let raw_body = r#"{"z":1,"a":2}"#;
        let record = completed_record(Some(raw_body), Some(raw_body), Vec::new(), None);

        assert_eq!(
            body_text_for_record(&record, MainDisplayTab::RequestBody),
            body_text_for_record(&record, MainDisplayTab::ResponseBody)
        );
    }
}
