use super::*;
use crate::settings::{ProxyMapLocalRule, ProxyMapLocalSettings, ProxyPresetSettings};

#[test]
fn scoped_tables_preserve_disabled_duplicates_original_order_and_omissions() {
    let duplicate = ProxyMapLocalRule {
        from: "https://api.example/a".into(),
        to: "/missing/mock.json".into(),
        enable: false,
    };
    let proxy = ProxySettings {
        active_preset: Some("selected".into()),
        presets: vec![
            ProxyPresetSettings {
                name: "unrelated".into(),
                ..Default::default()
            },
            ProxyPresetSettings {
                name: "selected".into(),
                map_local: ProxyMapLocalSettings {
                    enable: true,
                    rules: vec![
                        duplicate.clone(),
                        duplicate.clone(),
                        ProxyMapLocalRule {
                            enable: true,
                            ..duplicate
                        },
                    ],
                },
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    let view = MappingSettingsView::scoped(
        &proxy,
        MappingReadScope {
            preset: Some("selected".into()),
            table: Some(ProxyRuleTable::Local),
        },
    )
    .unwrap();
    assert_eq!(view.presets_total, 2);
    assert_eq!(view.presets_omitted, 1);
    assert_eq!(view.active_preset.as_deref(), Some("selected"));
    let preset = &view.presets[0];
    assert_eq!(preset.index, 1);
    assert!(!preset.map_remote.included);
    assert_eq!(preset.map_remote.rules_omitted, 0);
    assert!(preset.map_local.included);
    assert_eq!(
        preset
            .map_local
            .rules
            .iter()
            .map(|rule| (rule.index, rule.enabled))
            .collect::<Vec<_>>(),
        vec![(0, false), (1, false), (2, true)]
    );
    assert_eq!(
        preset.map_local.rules[0].from,
        preset.map_local.rules[1].from
    );
    let full = MappingSettingsView::scoped(&proxy, Default::default()).unwrap();
    assert!(full.presets[1].map_remote.included);
    assert!(full.presets[1].map_remote.rules.is_empty());
    assert_eq!(full.presets_omitted, 0);
    assert!(
        MappingSettingsView::scoped(
            &proxy,
            MappingReadScope {
                preset: Some("missing".into()),
                table: None
            }
        )
        .is_err()
    );
}

#[test]
fn preview_url_admission_enforces_count_and_utf8_byte_boundaries() {
    assert!(validate_preview_urls(&vec!["https://a.example".into(); MAX_PREVIEW_URLS]).is_ok());
    assert!(
        validate_preview_urls(&vec!["https://a.example".into(); MAX_PREVIEW_URLS + 1]).is_err()
    );
    assert!(validate_preview_urls(&[String::new()]).is_err());
    assert!(validate_preview_urls(&["a".repeat(MAX_PREVIEW_URL_BYTES)]).is_ok());
    assert!(validate_preview_urls(&["界".repeat(MAX_PREVIEW_URL_BYTES / 3 + 1)]).is_err());
}

#[test]
fn oversized_selected_rules_are_rejected_but_omitted_rules_do_not_consume_budget() {
    let proxy = ProxySettings {
        presets: vec![ProxyPresetSettings {
            name: "large".into(),
            map_local: ProxyMapLocalSettings {
                enable: true,
                rules: vec![ProxyMapLocalRule {
                    from: "x".repeat(crate::control_rpc::framing::RESPONSE_MAX_BYTES),
                    to: "/mock.json".into(),
                    enable: false,
                }],
            },
            ..Default::default()
        }],
        ..Default::default()
    };
    let omitted = MappingSettingsView::scoped(
        &proxy,
        MappingReadScope {
            preset: Some("large".into()),
            table: Some(ProxyRuleTable::Remote),
        },
    )
    .unwrap();
    assert_eq!(omitted.presets[0].map_local.rules_omitted, 1);
    assert!(!omitted.presets[0].map_local.included);
    let selected = MappingSettingsView::scoped(&proxy, MappingReadScope::default());
    assert!(matches!(selected, Err(error) if error.code() == ControlErrorCode::ResourceLimit));
}
