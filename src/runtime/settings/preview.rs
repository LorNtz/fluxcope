use super::*;
use crate::control::settings::{
    MappingPreviewReply,
    mapping::{MappingMutationPreview, validate_preview_urls},
};
use crate::settings::mapping_ops::explain_compiled_mapping;

impl SettingsTransactionClient {
    pub(crate) async fn preview_mapping_mutation(
        &self,
        mutation: MappingMutation,
        expected_revision: SettingsRevision,
        urls: Vec<String>,
        cancelled: CancellationToken,
    ) -> Result<MappingPreviewReply, ControlError> {
        validate_preview_urls(&urls)?;
        let permit = acquire_worker_permit(Arc::clone(&self.mapping_workers), &cancelled).await?;
        let snapshot = match self
            .runtime
            .request(
                RuntimeRequest::PreviewMappingSnapshot { expected_revision },
                cancelled.clone(),
            )
            .await?
        {
            RuntimeReply::MappingSettings(snapshot) => snapshot,
            _ => {
                return Err(ControlError::internal(
                    "runtime returned an unexpected mapping preview snapshot",
                ));
            }
        };
        let settings = snapshot.settings;
        let preview = run_worker_with_permit(permit, cancelled.clone(), move || {
            let (candidate, change) =
                apply_mapping_mutation_owned(settings.as_ref().clone(), mutation).map_err(
                    |error| {
                        ControlError::new(
                            ControlErrorCode::InvalidArgument,
                            error.to_string(),
                            false,
                            serde_json::json!({"stage": "validation", "location": error.location}),
                        )
                    },
                )?;
            // Compile the complete resulting policy exactly as commit does; no mapped file is opened.
            let policy = compile_candidate(&candidate)?;
            let validation = validate_mapping_candidate(candidate.proxy.as_ref());
            let mut explanations = Vec::with_capacity(urls.len());
            for url in urls {
                if cancelled.is_cancelled() {
                    return Err(ControlError::cancelled("mapping preview was cancelled"));
                }
                explanations.push(explain_compiled_mapping(
                    candidate.proxy.as_ref(),
                    &url,
                    validation.clone(),
                    policy.mapping(),
                ));
            }
            Ok::<_, ControlError>(MappingMutationPreview {
                affected: change.affected,
                effect: change.effect,
                validation,
                explanations,
            })
        })
        .await??;
        Ok(MappingPreviewReply {
            revision: snapshot.revision,
            config_mode: snapshot.config_mode,
            persistence: snapshot.persistence,
            preview,
        })
    }
}

pub(super) fn compile_candidate(settings: &AppSettings) -> Result<RequestPolicy, ControlError> {
    let compilation = RequestPolicy::compile(settings);
    if compilation
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity == RequestPolicyDiagnosticSeverity::Error)
    {
        let diagnostics_total = compilation.diagnostics.len();
        let diagnostics = compilation
            .diagnostics
            .iter()
            .take(MAX_MAPPING_DIAGNOSTICS)
            .map(|diagnostic| diagnostic.message.clone())
            .collect::<Vec<_>>();
        return Err(ControlError::new(
            ControlErrorCode::MappingValidationFailed,
            "mapping settings failed compilation",
            false,
            serde_json::json!({
                "stage": "compilation", "diagnostics": diagnostics,
                "diagnostics_total": diagnostics_total,
                "diagnostics_omitted": diagnostics_total.saturating_sub(MAX_MAPPING_DIAGNOSTICS),
            }),
        ));
    }
    Ok(compilation.policy)
}

#[cfg(all(test, unix))]
#[path = "preview_tests.rs"]
mod tests;
