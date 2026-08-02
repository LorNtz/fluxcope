use std::borrow::Cow;

use ratatui::{
    style::{Color, Style},
    text::Line,
};

use crate::app::{
    FieldEditKind, ProxyRuleTable, ProxyWidget, RecordingWidget, SelectTarget, SettingsPaneFocus,
    SettingsPopup, SettingsTopic,
};

use super::content::{SettingsContentItem, SettingsFieldRow, SettingsFullWidthTable};
use super::controls::{
    PEM_FILENAME_INPUT_WIDTH_COLS, PORT_INPUT_WIDTH_COLS, SettingsCheckboxControl,
    SettingsTextInputControl, settings_select_control,
};
use super::prefilter::prefilter_table_widget;
use super::tables::proxy_rule_table_widget;

pub(in crate::ui) fn settings_content_items_with_error<'a>(
    popup: &'a SettingsPopup,
    table_max_height: u16,
) -> Vec<SettingsContentItem<'a>> {
    let mut items = settings_content_items(popup, table_max_height);
    if let Some(error) = popup.error() {
        items.push(SettingsContentItem::Line(Line::from("")));
        items.push(SettingsContentItem::Line(Line::styled(
            format!("! {error}"),
            Style::default().fg(Color::Red),
        )));
    }
    items
}

fn settings_content_items<'a>(
    popup: &'a SettingsPopup,
    table_max_height: u16,
) -> Vec<SettingsContentItem<'a>> {
    match popup.topic {
        SettingsTopic::Server => vec![setting_text_field(
            popup,
            0,
            "Proxy port",
            Cow::Owned(popup.draft().server.port.to_string()),
            FieldEditKind::ServerPort,
            Some(PORT_INPUT_WIDTH_COLS),
        )],
        SettingsTopic::Certificate => {
            let mut items = vec![setting_text_field(
                popup,
                0,
                "CA store dir",
                Cow::Borrowed(popup.draft().certificate.store_dir.as_str()),
                FieldEditKind::CertificateStoreDir,
                None,
            )];
            items.push(setting_text_field(
                popup,
                1,
                "CA PEM filename",
                Cow::Borrowed(popup.draft().certificate.pem_filename.as_str()),
                FieldEditKind::CertificatePemFilename,
                Some(PEM_FILENAME_INPUT_WIDTH_COLS),
            ));
            items
        }
        SettingsTopic::Recording => vec![
            checkbox_row(
                popup,
                RecordingWidget::StartRecordingOnLaunch.row(),
                popup.draft().recording.start_record_on_launch,
                "Start recording on launch",
            ),
            SettingsContentItem::Divider {
                title: "URL Prefilter",
            },
            checkbox_row(
                popup,
                RecordingWidget::PrefilterEnabled.row(),
                popup.draft().recording.prefilter.enable,
                "URL prefilter enabled",
            ),
            SettingsContentItem::FullWidthTable {
                row: RecordingWidget::IncludeUrlPatterns.row(),
                table: SettingsFullWidthTable::Prefilter(prefilter_table_widget(
                    popup,
                    table_max_height,
                )),
            },
        ],
        SettingsTopic::Interface => vec![checkbox_row(
            popup,
            0,
            popup.draft().ui.request_list.auto_expand,
            "Auto-expand request tree",
        )],
        SettingsTopic::Proxy => proxy_items(popup, table_max_height),
    }
}

fn setting_text_field<'a>(
    popup: &'a SettingsPopup,
    row: usize,
    label: &'static str,
    value: Cow<'a, str>,
    kind: FieldEditKind,
    fixed_edit_width_cols: Option<u16>,
) -> SettingsContentItem<'a> {
    let edit = popup.active_field_edit(kind);
    SettingsContentItem::Field {
        row,
        field: SettingsFieldRow::new(
            label,
            popup.focus == SettingsPaneFocus::Content && popup.selected_row == row,
            SettingsTextInputControl {
                value: edit.map_or(value, |edit| Cow::Borrowed(edit.value)),
                cursor: edit.map(|edit| edit.cursor),
                fixed_edit_width_cols,
                hint: popup.field_hint(kind),
            },
        ),
    }
}

fn checkbox_row<'a>(
    popup: &SettingsPopup,
    row: usize,
    checked: bool,
    label: impl Into<Cow<'a, str>>,
) -> SettingsContentItem<'a> {
    let selected = popup.focus == SettingsPaneFocus::Content && popup.selected_row == row;
    SettingsContentItem::Field {
        row,
        field: SettingsFieldRow::new(label, selected, SettingsCheckboxControl { checked }),
    }
}

fn proxy_items(popup: &SettingsPopup, table_max_height: u16) -> Vec<SettingsContentItem<'_>> {
    let preset = popup.active_proxy_preset();
    let widgets = popup.visible_proxy_widgets();
    if widgets.is_empty() {
        return vec![SettingsContentItem::Line(Line::from(
            "No proxy preset configured.",
        ))];
    }

    let mut items = Vec::with_capacity(widgets.len().saturating_add(2));
    for (row, widget) in widgets.iter().enumerate() {
        let item = match widget {
            ProxyWidget::Preset => proxy_preset_select(popup, row),
            ProxyWidget::PresetName => setting_text_field(
                popup,
                row,
                "Preset name",
                Cow::Borrowed(preset.map_or("", |preset| preset.name.as_str())),
                FieldEditKind::ProxyPresetName,
                None,
            ),
            ProxyWidget::MappingEnabled => checkbox_row(
                popup,
                row,
                popup
                    .draft()
                    .proxy
                    .as_ref()
                    .is_some_and(|proxy| proxy.enable),
                "Mapping enabled",
            ),
            ProxyWidget::MapRemoteEnabled => checkbox_row(
                popup,
                row,
                preset.is_some_and(|preset| preset.map_remote.enable),
                "Map remote enabled",
            ),
            ProxyWidget::RemoteRules => SettingsContentItem::FullWidthTable {
                row,
                table: SettingsFullWidthTable::Proxy(proxy_rule_table_widget(
                    popup,
                    ProxyRuleTable::Remote,
                    preset,
                    table_max_height,
                )),
            },
            ProxyWidget::MapLocalEnabled => checkbox_row(
                popup,
                row,
                preset.is_some_and(|preset| preset.map_local.enable),
                "Map local enabled",
            ),
            ProxyWidget::LocalRules => SettingsContentItem::FullWidthTable {
                row,
                table: SettingsFullWidthTable::Proxy(proxy_rule_table_widget(
                    popup,
                    ProxyRuleTable::Local,
                    preset,
                    table_max_height,
                )),
            },
        };
        items.push(item);

        match widget {
            ProxyWidget::MappingEnabled => items.push(SettingsContentItem::Divider {
                title: "Map Remote",
            }),
            ProxyWidget::RemoteRules => {
                items.push(SettingsContentItem::Divider { title: "Map Local" })
            }
            _ => {}
        }
    }

    items
}

fn proxy_preset_select(popup: &SettingsPopup, row: usize) -> SettingsContentItem<'_> {
    settings_select(popup, row, SelectTarget::ProxyPreset, "Preset")
}

fn settings_select<'a>(
    popup: &'a SettingsPopup,
    row: usize,
    target: SelectTarget,
    label: &'static str,
) -> SettingsContentItem<'a> {
    SettingsContentItem::Field {
        row,
        field: SettingsFieldRow::new(
            label,
            popup.select_is_selected(target),
            settings_select_control(popup, target),
        ),
    }
}
