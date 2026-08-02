use super::super::terminal_text::{hard_wrap_text, pad_to_width, text_width, wrap_cell_text};
use super::detail_content_area;
use crate::app::{App, MainDisplayTab};
use ratatui::{
    layout::{Position, Rect},
    style::{Color, Style},
    text::{Line, Span},
};
use std::borrow::Cow;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct TableRow {
    key: String,
    value: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct HeaderTableColumns {
    key_width: u16,
    value_width: u16,
    gap: u16,
    total_width: u16,
}

pub(super) struct HeaderTableRender {
    pub(super) lines: Vec<Line<'static>>,
}

pub(super) fn borrowed_lines<'a>(lines: &'a [Line<'static>]) -> Vec<Line<'a>> {
    lines
        .iter()
        .map(|line| Line {
            style: line.style,
            alignment: line.alignment,
            spans: line
                .spans
                .iter()
                .map(|span| Span::styled(span.content.as_ref(), span.style))
                .collect(),
        })
        .collect()
}

trait HeaderTableRowText {
    fn key(&self) -> &str;
    fn value(&self) -> &str;
}

impl HeaderTableRowText for TableRow {
    fn key(&self) -> &str {
        &self.key
    }

    fn value(&self) -> &str {
        &self.value
    }
}

struct HeaderTableRowRef<'a> {
    key: &'a str,
    value: Cow<'a, str>,
}

impl HeaderTableRowText for HeaderTableRowRef<'_> {
    fn key(&self) -> &str {
        self.key
    }

    fn value(&self) -> &str {
        self.value.as_ref()
    }
}

pub(super) fn request_header_rows(req: &crate::capture::CaptureSummary) -> Vec<TableRow> {
    let mut lines = vec![
        header_table_row("Method", req.request.method.to_string()),
        header_table_row("URI", req.request.original_uri.clone()),
    ];
    if let Some(mapped_uri) = req.request.mapped_uri() {
        lines.push(header_table_row("Mapped URI", mapped_uri));
    }
    if let Some(local_path) = &req.request.local_path {
        lines.push(header_table_row("Map Local File", local_path.clone()));
    }

    lines.extend(
        req.request
            .headers
            .iter()
            .map(|(key, value)| header_table_row(key.clone(), value.clone())),
    );
    lines.push(header_table_row(
        "Capture Body",
        capture_body_status(&req.request_body),
    ));
    if req.metadata_truncation.request_target || req.metadata_truncation.request_headers {
        lines.push(header_table_row(
            "Capture Metadata",
            metadata_truncation_text(
                req.metadata_truncation.request_target,
                req.metadata_truncation.request_headers,
            ),
        ));
    }

    lines
}

pub(super) fn response_header_rows(req: &crate::capture::CaptureSummary) -> Vec<TableRow> {
    let mut lines = vec![header_table_row(
        "Status",
        req.response
            .as_ref()
            .map(|response| response.status)
            .map_or("N/A".to_string(), |status| status.to_string()),
    )];
    if let Some(response) = &req.response {
        lines.extend(
            response
                .headers
                .iter()
                .map(|(key, value)| header_table_row(key.clone(), value.clone())),
        );
    }
    lines.push(header_table_row(
        "Capture Body",
        capture_body_status(&req.response_body),
    ));
    if req.metadata_truncation.response_headers {
        lines.push(header_table_row(
            "Capture Metadata",
            "response headers truncated".to_string(),
        ));
    }

    lines
}

fn header_table_row(key: impl Into<String>, value: impl Into<String>) -> TableRow {
    TableRow {
        key: key.into(),
        value: value.into(),
    }
}

pub(super) fn header_table_render(
    rows: &[TableRow],
    width: u16,
    selected_row: Option<usize>,
) -> HeaderTableRender {
    let columns = header_table_columns(rows, width);
    let mut lines = Vec::new();

    for (row_index, row) in rows.iter().enumerate() {
        lines.extend(header_table_row_lines(
            row,
            columns,
            selected_row == Some(row_index),
        ));
    }

    if lines.is_empty() {
        lines.push(Line::from("(No headers)"));
    }

    HeaderTableRender { lines }
}

fn header_table_columns<T: HeaderTableRowText>(rows: &[T], width: u16) -> HeaderTableColumns {
    let total_width = width.max(1);
    let gap = if total_width >= 4 {
        2
    } else if total_width >= 3 {
        1
    } else {
        0
    };
    let available_width = total_width.saturating_sub(gap);
    let minimum_value_width = if available_width >= 2 {
        total_width / 2
    } else {
        0
    };
    let maximum_key_width = available_width
        .saturating_sub(minimum_value_width)
        .max(u16::from(available_width > 0));
    let natural_key_width = rows
        .iter()
        .map(|row| text_width(row.key()))
        .max()
        .unwrap_or(1)
        .max(1);
    let key_width = natural_key_width.min(maximum_key_width);
    let value_width = available_width.saturating_sub(key_width);

    HeaderTableColumns {
        key_width,
        value_width,
        gap,
        total_width,
    }
}

fn header_table_row_lines(
    row: &impl HeaderTableRowText,
    columns: HeaderTableColumns,
    selected: bool,
) -> Vec<Line<'static>> {
    let key_lines = wrap_cell_text(row.key(), columns.key_width);
    let value_lines = wrap_cell_text(row.value(), columns.value_width);
    let row_height = key_lines.len().max(value_lines.len()).max(1);
    let key_top_padding = (row_height - key_lines.len()) / 2;
    let value_top_padding = (row_height - value_lines.len()) / 2;

    (0..row_height)
        .map(|line_index| {
            let key = centered_line_segment(&key_lines, line_index, key_top_padding);
            let value = centered_line_segment(&value_lines, line_index, value_top_padding);
            let line = format_header_table_line(key, value, columns);
            let span = if selected {
                Span::styled(line, Style::default().bg(Color::White).fg(Color::DarkGray))
            } else {
                Span::raw(line)
            };
            Line::from(span)
        })
        .collect()
}

fn header_table_row_height(row: &impl HeaderTableRowText, columns: HeaderTableColumns) -> usize {
    wrapped_line_count(row.key(), columns.key_width)
        .max(wrapped_line_count(row.value(), columns.value_width))
        .max(1)
}

fn wrapped_line_count(text: &str, max_width: u16) -> usize {
    let max_width = max_width as usize;
    if max_width == 0 {
        return 1;
    }

    text.split('\n')
        .map(|source_line| hard_wrap_text(source_line, max_width).len().max(1))
        .sum::<usize>()
        .max(1)
}

fn centered_line_segment(lines: &[String], line_index: usize, top_padding: usize) -> &str {
    line_index
        .checked_sub(top_padding)
        .and_then(|index| lines.get(index))
        .map(String::as_str)
        .unwrap_or("")
}

fn format_header_table_line(key: &str, value: &str, columns: HeaderTableColumns) -> String {
    let mut line = pad_to_width(key, columns.key_width);
    if columns.value_width > 0 {
        line.push_str(&" ".repeat(columns.gap as usize));
        line.push_str(value);
    }

    pad_to_width(&line, columns.total_width)
}

pub(super) fn header_table_row_at_position(
    app: &App,
    area: Rect,
    position: Position,
) -> Option<usize> {
    let content_area = detail_content_area(area);
    if !content_area.contains(position) {
        return None;
    }

    let req = app.selected_request()?;
    let rows = match app.detail_panel.active_tab {
        MainDisplayTab::RequestHeader => request_header_row_refs(&req),
        MainDisplayTab::ResponseHeader => response_header_row_refs(&req),
        MainDisplayTab::RequestBody | MainDisplayTab::ResponseBody => return None,
    };
    let line_index = app.detail_panel.scroll.offset as usize
        + position.y.saturating_sub(content_area.y) as usize;
    let columns = header_table_columns(&rows, content_area.width);
    let mut row_start = 0;

    rows.iter().enumerate().find_map(|(index, row)| {
        let row_end = row_start + header_table_row_height(row, columns);
        let matches = (row_start..row_end).contains(&line_index);
        row_start = row_end;
        matches.then_some(index)
    })
}

fn request_header_row_refs(req: &crate::capture::CaptureSummary) -> Vec<HeaderTableRowRef<'_>> {
    let extra_rows = usize::from(req.request.mapped_uri().is_some())
        + usize::from(req.request.local_path.is_some());
    let mut rows = Vec::with_capacity(2 + extra_rows + req.request.headers.len());
    rows.push(header_table_row_ref(
        "Method",
        req.request.method.to_string(),
    ));
    rows.push(header_table_row_ref(
        "URI",
        req.request.original_uri.as_str(),
    ));
    if let Some(mapped_uri) = req.request.mapped_uri() {
        rows.push(header_table_row_ref("Mapped URI", mapped_uri));
    }
    if let Some(local_path) = &req.request.local_path {
        rows.push(header_table_row_ref("Map Local File", local_path.as_str()));
    }
    rows.extend(
        req.request
            .headers
            .iter()
            .map(|(key, value)| header_table_row_ref(key.as_str(), value.as_str())),
    );
    rows.push(header_table_row_ref(
        "Capture Body",
        capture_body_status(&req.request_body),
    ));
    if req.metadata_truncation.request_target || req.metadata_truncation.request_headers {
        rows.push(header_table_row_ref(
            "Capture Metadata",
            metadata_truncation_text(
                req.metadata_truncation.request_target,
                req.metadata_truncation.request_headers,
            ),
        ));
    }

    rows
}

fn response_header_row_refs(req: &crate::capture::CaptureSummary) -> Vec<HeaderTableRowRef<'_>> {
    let mut rows = Vec::with_capacity(
        1 + req
            .response
            .as_ref()
            .map_or(0, |response| response.headers.len()),
    );
    rows.push(header_table_row_ref(
        "Status",
        req.response
            .as_ref()
            .map(|response| response.status)
            .map_or_else(
                || Cow::Borrowed("N/A"),
                |status| Cow::Owned(status.to_string()),
            ),
    ));
    if let Some(response) = &req.response {
        rows.extend(
            response
                .headers
                .iter()
                .map(|(key, value)| header_table_row_ref(key.as_str(), value.as_str())),
        );
    }
    rows.push(header_table_row_ref(
        "Capture Body",
        capture_body_status(&req.response_body),
    ));
    if req.metadata_truncation.response_headers {
        rows.push(header_table_row_ref(
            "Capture Metadata",
            "response headers truncated",
        ));
    }

    rows
}

fn capture_body_status(status: &crate::capture::BodyStatus) -> String {
    let stream = match status.stream {
        crate::capture::BodyStreamState::Pending => "pending",
        crate::capture::BodyStreamState::Streaming => "streaming",
        crate::capture::BodyStreamState::Complete => "complete",
        crate::capture::BodyStreamState::Failed => "failed",
        crate::capture::BodyStreamState::Cancelled => "cancelled",
    };
    let mut text = format!(
        "{stream}; {} bytes observed; {} bytes retained",
        status.observed_bytes, status.retained_bytes
    );
    if let Some(limit) = status.preview_limit {
        let limit = match limit {
            crate::capture::BodyPreviewLimit::PerBodyLimit => "per-body preview limit",
            crate::capture::BodyPreviewLimit::TotalMemoryLimit => "total capture memory limit",
        };
        text.push_str(&format!("; truncated by {limit}"));
    }
    if let Some(error) = &status.error {
        text.push_str(&format!("; {error}"));
    }
    text
}

fn metadata_truncation_text(target: bool, headers: bool) -> String {
    match (target, headers) {
        (true, true) => "request target and headers truncated",
        (true, false) => "request target truncated",
        (false, true) => "request headers truncated",
        (false, false) => "not truncated",
    }
    .to_string()
}

fn header_table_row_ref<'a>(key: &'a str, value: impl Into<Cow<'a, str>>) -> HeaderTableRowRef<'a> {
    HeaderTableRowRef {
        key,
        value: value.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_table_keeps_value_column_at_least_half_width_when_key_wraps() {
        let rows = vec![header_table_row(
            "x-very-long-header-name",
            "value-that-also-needs-wrapping",
        )];
        let columns = header_table_columns(&rows, 12);
        let table = header_table_render(&rows, 12, None);

        assert!(columns.value_width >= 6);
        assert!(columns.key_width < text_width(&rows[0].key));
        assert!(table.lines.len() > 1);
    }
}
