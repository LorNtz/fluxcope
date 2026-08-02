use super::super::terminal_text::{pad_to_width, text_width, wrap_cell_text};
use super::detail_content_area;
use crate::app::MainDisplayTab;
use ratatui::{
    layout::{Position, Rect},
    style::{Color, Style},
    text::{Line, Span},
};

#[derive(Clone, Debug, PartialEq, Eq)]
struct TableRow {
    key: String,
    value: String,
}

pub(super) struct HeaderTableModel {
    source_rows: Vec<TableRow>,
    rows: Vec<HeaderTableRowModel>,
    columns: HeaderTableColumns,
    line_count: usize,
}

struct HeaderTableRowModel {
    lines: Vec<String>,
}

impl HeaderTableModel {
    pub(super) fn for_tab(
        req: &crate::capture::CaptureSummary,
        tab: MainDisplayTab,
        width: u16,
    ) -> Option<Self> {
        let rows = match tab {
            MainDisplayTab::RequestHeader => request_header_rows(req),
            MainDisplayTab::ResponseHeader => response_header_rows(req),
            MainDisplayTab::RequestBody | MainDisplayTab::ResponseBody => return None,
        };
        Some(Self::new(rows, width))
    }

    fn new(rows: Vec<TableRow>, width: u16) -> Self {
        let columns = header_table_columns(&rows, width);
        let wrapped_rows = rows
            .iter()
            .map(|row| HeaderTableRowModel {
                lines: header_table_row_lines(row, columns),
            })
            .collect::<Vec<_>>();
        let line_count = wrapped_rows.iter().map(|row| row.lines.len()).sum();
        Self {
            source_rows: rows,
            rows: wrapped_rows,
            columns,
            line_count,
        }
    }

    pub(super) fn set_width(&mut self, width: u16) {
        if self.columns.total_width == width.max(1) {
            return;
        }

        self.columns = header_table_columns(&self.source_rows, width);
        self.rows = self
            .source_rows
            .iter()
            .map(|row| HeaderTableRowModel {
                lines: header_table_row_lines(row, self.columns),
            })
            .collect();
        self.line_count = self.rows.iter().map(|row| row.lines.len()).sum();
    }

    pub(super) fn render_lines(&self, selected_row: Option<usize>) -> Vec<Line<'_>> {
        let mut lines = Vec::with_capacity(self.line_count.max(1));

        for (row_index, row) in self.rows.iter().enumerate() {
            let selected = selected_row == Some(row_index);
            lines.extend(row.lines.iter().map(|line| {
                let span = if selected {
                    Span::styled(
                        line.as_str(),
                        Style::default().bg(Color::White).fg(Color::DarkGray),
                    )
                } else {
                    Span::raw(line.as_str())
                };
                Line::from(span)
            }));
        }

        if lines.is_empty() {
            lines.push(Line::from("(No headers)"));
        }

        lines
    }

    pub(super) fn row_at_line(&self, line_index: usize) -> Option<usize> {
        let mut row_start = 0;

        self.rows.iter().enumerate().find_map(|(index, row)| {
            let row_end = row_start + row.lines.len();
            let matches = (row_start..row_end).contains(&line_index);
            row_start = row_end;
            matches.then_some(index)
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct HeaderTableColumns {
    key_width: u16,
    value_width: u16,
    gap: u16,
    total_width: u16,
}

fn request_header_rows(req: &crate::capture::CaptureSummary) -> Vec<TableRow> {
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

fn response_header_rows(req: &crate::capture::CaptureSummary) -> Vec<TableRow> {
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

fn header_table_columns(rows: &[TableRow], width: u16) -> HeaderTableColumns {
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
        .map(|row| text_width(&row.key))
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

fn header_table_row_lines(row: &TableRow, columns: HeaderTableColumns) -> Vec<String> {
    let key_lines = wrap_cell_text(&row.key, columns.key_width);
    let value_lines = wrap_cell_text(&row.value, columns.value_width);
    let row_height = key_lines.len().max(value_lines.len()).max(1);
    let key_top_padding = (row_height - key_lines.len()) / 2;
    let value_top_padding = (row_height - value_lines.len()) / 2;

    (0..row_height)
        .map(|line_index| {
            let key = centered_line_segment(&key_lines, line_index, key_top_padding);
            let value = centered_line_segment(&value_lines, line_index, value_top_padding);
            format_header_table_line(key, value, columns)
        })
        .collect()
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
    model: &HeaderTableModel,
    area: Rect,
    position: Position,
    scroll_offset: u16,
) -> Option<usize> {
    let content_area = detail_content_area(area);
    if !content_area.contains(position) {
        return None;
    }

    let line_index = scroll_offset as usize + position.y.saturating_sub(content_area.y) as usize;
    model.row_at_line(line_index)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_table_keeps_value_column_at_least_half_width_when_key_wraps() {
        let rows = vec![header_table_row(
            "x-very-long-header-name",
            "value-that-also-needs-wrapping",
        )];
        let model = HeaderTableModel::new(rows, 12);
        let lines = model.render_lines(None);

        assert!(model.columns.value_width >= 6);
        assert!(model.columns.key_width < text_width("x-very-long-header-name"));
        assert!(lines.len() > 1);
        assert!((0..lines.len()).all(|line| model.row_at_line(line) == Some(0)));
    }

    #[test]
    fn header_table_render_and_hit_testing_share_wrapped_row_boundaries() {
        let model = HeaderTableModel::new(
            vec![
                header_table_row("short", "value"),
                header_table_row("wrapped-header-name", "wrapped-value"),
                header_table_row("last", "row"),
            ],
            12,
        );
        let rendered = model.render_lines(None);
        let row_indexes = (0..rendered.len())
            .map(|line| model.row_at_line(line))
            .collect::<Vec<_>>();

        assert_eq!(row_indexes.first(), Some(&Some(0)));
        assert!(row_indexes.iter().filter(|row| **row == Some(1)).count() > 1);
        assert_eq!(row_indexes.last(), Some(&Some(2)));
        assert_eq!(model.row_at_line(rendered.len()), None);
    }
}
