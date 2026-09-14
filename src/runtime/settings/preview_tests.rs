use super::*;
use crate::{
    runtime::settings::test_support::SettingsRuntimeFixture,
    settings::{
        ProxyMapLocalRule, ProxyMapRemoteRule,
        mapping_ops::{MutationEffect, ProxyRuleTable},
    },
};

#[tokio::test]
async fn preview_is_isolated_and_commit_rejects_stale_preview() {
    let fixture = SettingsRuntimeFixture::temporary_with_remote_presets().await;
    let before = fixture.snapshot();
    let mutation = MappingMutation::AppendLocalRule {
        preset: "first".into(),
        rule: ProxyMapLocalRule {
            from: "https://api.example/a".into(),
            to: "/does/not/exist/mock.json".into(),
            enable: true,
        },
    };
    let preview = fixture
        .client()
        .preview_mapping_mutation(
            mutation.clone(),
            before.revision,
            vec!["https://api.example/a".into()],
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(preview.revision, before.revision);
    assert_eq!(preview.preview.effect, MutationEffect::Changed);
    assert_eq!(
        preview.preview.explanations[0].local_path.as_deref(),
        Some(std::path::Path::new("/does/not/exist/mock.json"))
    );
    assert_eq!(fixture.snapshot(), before);
    assert_eq!(fixture.external_write_count(), 0);
    assert_eq!(fixture.policy_swap_count(), 0);
    let unrelated = fixture.preset("second");
    let committed = fixture
        .mutate(
            mutation.clone(),
            preview.revision,
            SettingsTransactionOrigin::Mcp,
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(fixture.preset("second"), unrelated);
    assert_eq!(committed.revision, preview.revision.next());
    let after = fixture.snapshot();
    let stale_preview = fixture
        .client()
        .preview_mapping_mutation(
            mutation.clone(),
            preview.revision,
            vec![],
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert_eq!(
        stale_preview.code(),
        ControlErrorCode::SettingsRevisionConflict
    );
    let stale_commit = fixture
        .mutate(
            mutation,
            preview.revision,
            SettingsTransactionOrigin::Mcp,
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert_eq!(
        stale_commit.code(),
        ControlErrorCode::SettingsRevisionConflict
    );
    assert_eq!(fixture.snapshot(), after);
}

#[tokio::test]
async fn preview_noop_and_dirty_draft_never_advance_or_lock_settings() {
    let fixture = SettingsRuntimeFixture::temporary_with_remote_presets().await;
    fixture.open_clean_popup();
    let before = fixture.snapshot();
    let mutation = MappingMutation::SetActivePreset {
        name: Some("first".into()),
    };
    let preview = fixture
        .client()
        .preview_mapping_mutation(
            mutation.clone(),
            before.revision,
            vec![],
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(preview.preview.effect, MutationEffect::Unchanged);
    assert_eq!(fixture.snapshot(), before);
    fixture.open_dirty_popup();
    let draft = fixture.snapshot();
    let error = fixture
        .client()
        .preview_mapping_mutation(mutation, before.revision, vec![], CancellationToken::new())
        .await
        .unwrap_err();
    assert_eq!(error.code(), ControlErrorCode::TuiDraftConflict);
    assert_eq!(fixture.snapshot(), draft);
    assert_eq!(fixture.external_write_count(), 0);
    assert_eq!(fixture.policy_swap_count(), 0);
}

#[tokio::test]
async fn preview_explains_remote_then_local_using_original_rule_indexes() {
    let fixture = SettingsRuntimeFixture::temporary_with_remote_presets().await;
    let mut revision = fixture.snapshot().revision;
    for mutation in [
        MappingMutation::AppendRemoteRule {
            preset: "first".into(),
            rule: ProxyMapRemoteRule {
                from: "https://api.example".into(),
                to: "http://mock.example".into(),
                enable: true,
            },
        },
        MappingMutation::AppendLocalRule {
            preset: "first".into(),
            rule: ProxyMapLocalRule {
                from: "http://mock.example/a".into(),
                to: "/missing/disabled.json".into(),
                enable: false,
            },
        },
    ] {
        revision = fixture
            .mutate(
                mutation,
                revision,
                SettingsTransactionOrigin::Mcp,
                CancellationToken::new(),
            )
            .await
            .unwrap()
            .revision;
    }
    let before = fixture.snapshot();
    let preview = fixture
        .client()
        .preview_mapping_mutation(
            MappingMutation::AppendLocalRule {
                preset: "first".into(),
                rule: ProxyMapLocalRule {
                    from: "http://mock.example/a".into(),
                    to: "/missing/enabled.json".into(),
                    enable: true,
                },
            },
            revision,
            vec!["https://api.example/a".into()],
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let explanation = &preview.preview.explanations[0];
    assert_eq!(
        explanation.effective_url.as_ref().unwrap().to_string(),
        "http://mock.example/a"
    );
    assert_eq!(
        explanation
            .matches
            .iter()
            .map(|rule| (rule.table, rule.index))
            .collect::<Vec<_>>(),
        vec![(ProxyRuleTable::Remote, 0), (ProxyRuleTable::Local, 1)]
    );
    assert_eq!(fixture.snapshot(), before);
}
