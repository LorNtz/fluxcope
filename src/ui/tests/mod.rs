use super::*;
use crate::{
    app::SettingsUiContext,
    capture::{
        BodySide, CaptureRetentionPolicy, CaptureSequence, CapturedExchange, DecodeDisplayMode,
        DecodeKey, DecodePolicy, DecodeResult, start_decode_service,
    },
    logging::LogRetentionPolicy,
    recording::RecordingState,
    settings::{
        AppSettings, ConfigMode, PersistenceMode, ProxyPresetSettings, ProxySettings,
        RecordingPrefilterPatternSettings, RequestListSettings, UiSettings,
    },
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton};
use http::Method;
use ratatui::{Terminal, backend::TestBackend, buffer::Buffer};
use tokio_util::sync::CancellationToken;

fn ui_settings(auto_expand: bool) -> UiSettings {
    UiSettings {
        request_list: RequestListSettings { auto_expand },
    }
}
fn app_with_settings_context(context: SettingsUiContext) -> App {
    App::with_runtime_policies(
        AppSettings::default(),
        RecordingState::default(),
        LogRetentionPolicy::default(),
        CaptureRetentionPolicy::default(),
        context,
    )
}

fn proxy_settings(active: &str, presets: &[&str]) -> ProxySettings {
    ProxySettings {
        enable: true,
        active_preset: Some(active.to_string()),
        presets: presets
            .iter()
            .map(|preset| ProxyPresetSettings {
                name: (*preset).to_string(),
                ..ProxyPresetSettings::default()
            })
            .collect(),
    }
}

fn proxy_settings_with_rule_counts(remote_count: usize, local_count: usize) -> ProxySettings {
    ProxySettings {
        enable: true,
        active_preset: Some("dev".to_string()),
        presets: vec![ProxyPresetSettings {
            name: "dev".to_string(),
            map_remote: crate::settings::ProxyMapRemoteSettings {
                enable: true,
                rules: (0..remote_count)
                    .map(|index| crate::settings::ProxyMapRemoteRule {
                        from: format!("https://api.example.com/v{index}"),
                        to: format!("http://localhost:30{index:02}"),
                        enable: true,
                    })
                    .collect(),
            },
            map_local: crate::settings::ProxyMapLocalSettings {
                enable: true,
                rules: (0..local_count)
                    .map(|index| crate::settings::ProxyMapLocalRule {
                        from: format!("https://static.example.com/app{index}.js"),
                        to: format!("~/fixtures/app{index}.js"),
                        enable: true,
                    })
                    .collect(),
            },
        }],
    }
}

fn captured(sequence: u64, uri: &str) -> CapturedExchange {
    CapturedExchange {
        sequence: CaptureSequence::new(sequence),
        method: Method::GET,
        uri: uri.to_string(),
        mapped_uri: None,
        local_path: None,
        status: None,
        req_headers: vec![],
        res_headers: vec![],
        req_body: None,
        res_body: None,
    }
}

fn mouse(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
    MouseEvent {
        kind,
        column,
        row,
        modifiers: KeyModifiers::empty(),
    }
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::empty())
}

fn focus_settings_content(app: &mut App) {
    app.handle_key_event(key(KeyCode::Right));
    assert_eq!(app.settings_popup.focus, SettingsPaneFocus::Content);
}

fn laid_out_ui(app: &App) -> RootView {
    let mut ui = RootView::new();
    View::layout(&mut ui, Rect::new(0, 0, 100, 12), app);

    ui
}

fn render_to_buffer(app: &mut App) -> (RootView, Buffer) {
    render_to_buffer_with_size(app, 100, 12)
}

fn render_to_buffer_with_size(app: &mut App, width: u16, height: u16) -> (RootView, Buffer) {
    let _ = app.prepare_current_body_display(std::time::Instant::now());
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("test backend should initialize");
    let mut ui = RootView::new();

    terminal
        .draw(|frame| RootView::render(&mut ui, frame, app))
        .expect("UI should render in tests");

    (ui, terminal.backend_mut().buffer().clone())
}

fn settings_content_test_area(area: Rect) -> Rect {
    settings_popup_layout(area).content_viewport
}

fn settings_content_items_for_test<'a>(
    popup: &'a SettingsPopup,
    root_area: Rect,
) -> Vec<SettingsContentItem<'a>> {
    let table_max_height = settings_table_max_height(settings_content_test_area(root_area).height);
    settings_content_items_with_error(popup, table_max_height)
}

fn action_dialog_test_area(area: Rect, app: &App) -> Rect {
    let dialog = app
        .settings_popup
        .unsaved_dialog()
        .expect("unsaved settings dialog should be active");
    action_dialog_area(&dialog, settings_popup_area(area))
}

fn rule_editor_test_area(area: Rect) -> Rect {
    rule_editor_area(settings_popup_area(area))
}

fn settings_popup_footer_row(buffer: &Buffer) -> String {
    let area = settings_popup_area(buffer.area);
    buffer_row(buffer, area.bottom().saturating_sub(1), area.x, area.width)
}

fn rule_editor_footer_row(buffer: &Buffer) -> String {
    let area = rule_editor_test_area(buffer.area);
    buffer_row(buffer, area.bottom().saturating_sub(1), area.x, area.width)
}

fn render_detail_top_row(app: &mut App) -> (RootView, String) {
    let (ui, buffer) = render_to_buffer(app);
    let detail_area = ui.detail.area();
    let detail_top_row = buffer_row(&buffer, detail_area.y, detail_area.x, detail_area.width);

    (ui, detail_top_row)
}

fn buffer_row(buffer: &Buffer, y: u16, x: u16, width: u16) -> String {
    let mut row = String::new();
    for column in x..x.saturating_add(width) {
        row.push_str(buffer[(column, y)].symbol());
    }

    row
}

fn find_buffer_text(buffer: &Buffer, area: Rect, text: &str) -> Option<Position> {
    let symbols = text
        .chars()
        .map(|symbol| symbol.to_string())
        .collect::<Vec<_>>();
    let symbol_count = u16::try_from(symbols.len()).ok()?;
    if symbol_count == 0 || area.width < symbol_count {
        return None;
    }

    for y in area.y..area.bottom() {
        for x in area.x..=area.right().saturating_sub(symbol_count) {
            if symbols.iter().enumerate().all(|(offset, symbol)| {
                buffer[(x + u16::try_from(offset).unwrap_or(u16::MAX), y)].symbol() == symbol
            }) {
                return Some(Position::new(x, y));
            }
        }
    }

    None
}

fn table_checkbox_on_value_row(buffer: &Buffer, area: Rect, value: Position) -> Position {
    for x in area.x..value.x {
        let mark = buffer_row(buffer, value.y, x, 3);
        if mark == "[✓]" || mark == "[ ]" {
            return Position::new(x, value.y);
        }
    }

    panic!("table checkbox should render on the value row");
}

fn assert_centered_divider_with_padding(
    buffer: &Buffer,
    area: Rect,
    divider: Position,
    title: &str,
) {
    let row = buffer_row(buffer, divider.y, area.x, area.width);
    let title_byte = row.find(title).expect("divider title should render");
    let before_title = &row[..title_byte];
    let after_title = &row[title_byte + title.len()..];
    let left_dividers = before_title.chars().filter(|ch| *ch == '─').count();
    let right_dividers = after_title.chars().filter(|ch| *ch == '─').count();

    assert!(left_dividers > 0, "{row:?}");
    assert!(right_dividers > 0, "{row:?}");
    assert!(
        left_dividers.abs_diff(right_dividers) <= 1,
        "{row:?}: left={left_dividers}, right={right_dividers}"
    );

    let padding_width = area.width.saturating_sub(1).max(1);
    let upper_padding = buffer_row(buffer, divider.y.saturating_sub(1), area.x, padding_width);
    let lower_padding = buffer_row(buffer, divider.y.saturating_add(1), area.x, padding_width);

    assert!(upper_padding.trim().is_empty(), "{upper_padding:?}");
    assert!(lower_padding.trim().is_empty(), "{lower_padding:?}");
}

fn rule_table_scrollbar_column(buffer: &Buffer, area: Rect, title: &str) -> Option<u16> {
    let title = find_buffer_text(buffer, area, title)?;
    let table_right = (title.x..area.right())
        .find(|x| buffer[(*x, title.y)].symbol() == symbols::border::ROUNDED.top_right)?;

    table_right.checked_sub(1)
}

fn field_box_top_left(buffer: &Buffer, area: Rect, label_text: &str) -> Option<Position> {
    let symbols = label_text
        .chars()
        .map(|symbol| symbol.to_string())
        .collect::<Vec<_>>();
    let symbol_count = u16::try_from(symbols.len()).ok()?;
    if symbol_count == 0 || area.width < symbol_count {
        return None;
    }

    for y in area.y..area.bottom() {
        for label_x in area.x..=area.right().saturating_sub(symbol_count) {
            if !symbols.iter().enumerate().all(|(offset, symbol)| {
                buffer[(label_x + u16::try_from(offset).unwrap_or(u16::MAX), y)].symbol() == symbol
            }) {
                continue;
            }

            let border_y = y.saturating_sub(1);
            let search_start = label_x.saturating_add(symbol_count).min(area.right());
            for x in search_start..area.right() {
                if buffer[(x, border_y)].symbol() == symbols::border::ROUNDED.top_left {
                    return Some(Position::new(x, border_y));
                }
            }
        }
    }

    None
}

fn field_box_top_right(buffer: &Buffer, area: Rect, label_text: &str) -> Option<Position> {
    field_box_bounds(buffer, area, label_text).map(|(_, top_right)| top_right)
}

fn field_box_width(buffer: &Buffer, area: Rect, label_text: &str) -> Option<u16> {
    let (top_left, top_right) = field_box_bounds(buffer, area, label_text)?;

    Some(top_right.x.saturating_sub(top_left.x).saturating_add(1))
}

fn field_box_bounds(buffer: &Buffer, area: Rect, label_text: &str) -> Option<(Position, Position)> {
    let top_left = field_box_top_left(buffer, area, label_text)?;
    for x in top_left.x.saturating_add(1)..area.right() {
        if buffer[(x, top_left.y)].symbol() == symbols::border::ROUNDED.top_right {
            return Some((top_left, Position::new(x, top_left.y)));
        }
    }

    None
}

fn tab_click_position(area: Rect, target: MainDisplayTab) -> Position {
    let mut column = area.x.saturating_add(1);
    for (index, tab) in MainDisplayTab::all().iter().copied().enumerate() {
        if index > 0 {
            column = column.saturating_add(1);
        }
        if tab == target {
            return Position::new(column.saturating_add(1), area.y);
        }
        column = column.saturating_add(text_width(tab.title()).saturating_add(2));
    }

    panic!("target tab should exist")
}

fn mouse_inside(kind: MouseEventKind, area: Rect) -> MouseEvent {
    mouse(kind, area.x.saturating_add(1), area.y.saturating_add(1))
}

fn mouse_down_inside(area: Rect) -> MouseEvent {
    mouse_inside(MouseEventKind::Down(MouseButton::Left), area)
}

mod detail;
mod request_list;
mod root_layout;
mod settings_dialogs;
mod settings_fields;
mod settings_proxy;
mod settings_recording;
