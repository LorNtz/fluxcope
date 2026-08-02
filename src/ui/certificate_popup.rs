use qrcode::{EcLevel, QrCode, render::unicode};
use ratatui::{
    Frame,
    layout::Alignment,
    style::{Color, Style},
    text::Line,
    widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap},
};

use super::chrome::centered_rect;
use super::terminal_text::{text_width, wrap_text};
use super::{App, PopupFocus};

pub(super) fn render_certificate_popup(frame: &mut Frame, app: &App) {
    let download_url = app.certificate_popup.download_url.as_deref();
    let qr_lines = download_url.map(build_qr_lines).unwrap_or_default();
    let description_lines = [
        "Connect your mobile phone to the same LAN as this device",
        "then scan to download the proxy CA certificate and install it.",
        "Don't forget to manually trust the CA if you're on iOS 10 or later.",
    ];
    let qr_width = qr_lines
        .iter()
        .map(|line| text_width(line))
        .max()
        .unwrap_or(0);
    let prose_width = description_lines
        .iter()
        .copied()
        .chain([
            "Certificate download URL is unavailable.",
            "Press Esc to close",
        ])
        .chain(download_url)
        .map(text_width)
        .max()
        .unwrap_or(0);
    let available_width = frame.area().width.saturating_sub(4).max(1);
    let width = qr_width
        .max(prose_width)
        .saturating_add(4)
        .max(56)
        .min(available_width);
    let wrap_width = width.saturating_sub(4).max(1);
    let available_height = frame.area().height.saturating_sub(2).max(1);
    let content_height = available_height.saturating_sub(2) as usize;

    let mut lines = PopupLines::default();
    lines.push_blank();
    for description in description_lines {
        lines.push_centered_wrapped(description, wrap_width);
    }
    lines.push_blank();
    if qr_lines.is_empty() {
        lines.push_centered_wrapped("Certificate download URL is unavailable.", wrap_width);
    } else {
        lines.extend_centered(qr_lines);
        if let Some(download_url) = download_url {
            let url_lines = wrap_text(download_url, wrap_width);
            if lines.len() + url_lines.len() < content_height {
                lines.push_blank();
                lines.extend_centered(url_lines);
                lines.push_blank();
            }
        }
    }

    let height = u16::try_from(lines.len())
        .unwrap_or(u16::MAX)
        .saturating_add(2)
        .min(available_height);
    let area = centered_rect(width, height, frame.area());
    let content = Paragraph::new(lines.into_lines())
        .block(
            Block::default()
                .title("Install Certificate")
                .title_alignment(Alignment::Center)
                .title_bottom(Line::from("Press Esc to close").alignment(Alignment::Center))
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(if app.is_popup_focused(PopupFocus::Certificate) {
                    Style::default().fg(Color::Green)
                } else {
                    Style::default()
                }),
        )
        .alignment(Alignment::Center)
        .wrap(Wrap { trim: false });

    frame.render_widget(Clear, area);
    frame.render_widget(content, area);
}

#[derive(Default)]
struct PopupLines {
    lines: Vec<Line<'static>>,
}

impl PopupLines {
    fn len(&self) -> usize {
        self.lines.len()
    }

    fn push_blank(&mut self) {
        self.lines.push(Line::from(""));
    }

    fn push_centered_wrapped(&mut self, text: &str, max_width: u16) {
        self.extend_centered(wrap_text(text, max_width));
    }

    fn extend_centered(&mut self, lines: impl IntoIterator<Item = String>) {
        self.lines.extend(
            lines
                .into_iter()
                .map(|line| Line::from(line).alignment(Alignment::Center)),
        );
    }

    fn into_lines(self) -> Vec<Line<'static>> {
        self.lines
    }
}

fn build_qr_lines(download_url: &str) -> Vec<String> {
    match QrCode::with_error_correction_level(download_url.as_bytes(), EcLevel::L) {
        Ok(code) => code
            .render::<unicode::Dense1x2>()
            .quiet_zone(false)
            .build()
            .lines()
            .map(str::to_string)
            .collect(),
        Err(_) => vec!["Unable to generate QR code".to_string()],
    }
}
