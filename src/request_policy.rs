use parking_lot::RwLock;
use std::sync::Arc;

use crate::{
    mapping::{
        DiagnosticSeverity as MappingDiagnosticSeverity, MappingDecision, MappingDiagnosticCode,
        MappingEngine, MappingLocation,
    },
    recording::{PrefilterDiagnostic, RecordingPrefilter},
    settings::AppSettings,
};

#[derive(Debug, Default)]
pub(crate) struct RequestPolicy {
    mapping: MappingEngine,
    prefilter: RecordingPrefilter,
}

impl RequestPolicy {
    pub(crate) fn compile(settings: &AppSettings) -> RequestPolicyCompilation {
        let mut mapping = MappingEngine::compile(settings.proxy.as_ref());
        let (prefilter, prefilter_diagnostics) =
            RecordingPrefilter::compile(&settings.recording.prefilter);
        let diagnostics = mapping
            .take_diagnostics()
            .into_iter()
            .map(|diagnostic| RequestPolicyDiagnostic {
                severity: match diagnostic.severity {
                    MappingDiagnosticSeverity::Warning => RequestPolicyDiagnosticSeverity::Warning,
                    MappingDiagnosticSeverity::Error => RequestPolicyDiagnosticSeverity::Error,
                },
                source: RequestPolicyDiagnosticSource::ProxyMapping {
                    code: diagnostic.code,
                    location: diagnostic.location,
                },
                message: diagnostic.message,
            })
            .chain(
                prefilter_diagnostics
                    .into_iter()
                    .map(|PrefilterDiagnostic { message }| RequestPolicyDiagnostic {
                        severity: RequestPolicyDiagnosticSeverity::Error,
                        source: RequestPolicyDiagnosticSource::RecordingPrefilter,
                        message,
                    }),
            )
            .collect();
        RequestPolicyCompilation {
            policy: Self { mapping, prefilter },
            diagnostics,
        }
    }

    pub(crate) fn mapping(&self) -> &MappingEngine {
        &self.mapping
    }

    pub(crate) fn evaluate(
        &self,
        original_uri: &http::Uri,
        recording_enabled: bool,
    ) -> RequestPolicyEvaluation {
        let mapping = self.mapping.map_request(original_uri);
        let urls = if recording_enabled {
            let original_url = original_uri.to_string();
            let mapped_effective_url = mapping.mapped_uri.as_ref().map(ToString::to_string);
            let effective_url = mapped_effective_url
                .as_deref()
                .unwrap_or(original_url.as_str());
            let included = self.prefilter.includes(effective_url);
            RequestUrlState::Recording {
                original_url,
                mapped_effective_url,
                included,
            }
        } else if mapping.mapped_uri.is_some() || mapping.local_path.is_some() {
            RequestUrlState::MappingOnly {
                original_url: original_uri.to_string(),
            }
        } else {
            RequestUrlState::None
        };
        RequestPolicyEvaluation { mapping, urls }
    }
}

pub(crate) struct RequestPolicyCompilation {
    pub policy: RequestPolicy,
    pub diagnostics: Vec<RequestPolicyDiagnostic>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RequestPolicyDiagnosticSeverity {
    Warning,
    Error,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RequestPolicyDiagnosticSource {
    ProxyMapping {
        code: MappingDiagnosticCode,
        location: Option<MappingLocation>,
    },
    RecordingPrefilter,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RequestPolicyDiagnostic {
    pub severity: RequestPolicyDiagnosticSeverity,
    pub source: RequestPolicyDiagnosticSource,
    pub message: String,
}

pub(crate) struct RequestPolicyEvaluation {
    mapping: MappingDecision,
    urls: RequestUrlState,
}

impl RequestPolicyEvaluation {
    pub(crate) fn into_parts(self) -> (MappingDecision, RequestUrlState) {
        (self.mapping, self.urls)
    }
}

pub(crate) enum RequestUrlState {
    None,
    MappingOnly {
        original_url: String,
    },
    Recording {
        original_url: String,
        mapped_effective_url: Option<String>,
        included: bool,
    },
}

impl RequestUrlState {
    pub(crate) fn original_url(&self) -> Option<&str> {
        match self {
            Self::None => None,
            Self::MappingOnly { original_url } | Self::Recording { original_url, .. } => {
                Some(original_url)
            }
        }
    }

    pub(crate) fn recording(&self) -> Option<RecordingPolicyEvaluation<'_>> {
        let Self::Recording {
            original_url,
            mapped_effective_url,
            included,
        } = self
        else {
            return None;
        };
        Some(RecordingPolicyEvaluation {
            original_url,
            effective_url: mapped_effective_url.as_deref().unwrap_or(original_url),
            included: *included,
        })
    }
}

#[derive(Clone, Copy)]
pub(crate) struct RecordingPolicyEvaluation<'a> {
    pub original_url: &'a str,
    pub effective_url: &'a str,
    pub included: bool,
}

#[derive(Clone)]
pub(crate) struct RequestPolicyStore {
    current: Arc<RwLock<Arc<RequestPolicy>>>,
}

impl RequestPolicyStore {
    pub(crate) fn new(policy: RequestPolicy) -> Self {
        Self {
            current: Arc::new(RwLock::new(Arc::new(policy))),
        }
    }

    pub(crate) fn current(&self) -> Arc<RequestPolicy> {
        Arc::clone(&self.current.read())
    }

    pub(crate) fn replace(&self, policy: RequestPolicy) {
        *self.current.write() = Arc::new(policy);
    }
}

impl Default for RequestPolicyStore {
    fn default() -> Self {
        Self::new(RequestPolicy::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{
        ProxyMapRemoteRule, ProxyMapRemoteSettings, ProxyPresetSettings, ProxySettings,
        RecordingPrefilterPatternSettings, RecordingPrefilterSettings,
    };

    #[test]
    fn compilation_reports_mapping_and_prefilter_diagnostics_together() {
        let mut settings = AppSettings::default();
        settings.recording.prefilter = RecordingPrefilterSettings {
            enable: true,
            include_url_patterns: vec![RecordingPrefilterPatternSettings::new("[")],
        };
        settings.proxy = Some(ProxySettings {
            enable: true,
            active_preset: Some("missing".to_string()),
            presets: vec![ProxyPresetSettings {
                name: "available".to_string(),
                ..ProxyPresetSettings::default()
            }],
        });

        let compilation = RequestPolicy::compile(&settings);

        assert!(compilation.policy.mapping.diagnostics().is_empty());
        assert_eq!(compilation.diagnostics.len(), 2);
        assert_eq!(
            compilation.diagnostics[0].severity,
            RequestPolicyDiagnosticSeverity::Warning
        );
        assert_eq!(
            compilation.diagnostics[0].source,
            RequestPolicyDiagnosticSource::ProxyMapping {
                code: MappingDiagnosticCode::ActivePresetNotFound,
                location: None,
            }
        );
        assert!(
            compilation.diagnostics[0]
                .message
                .contains("active preset 'missing' was not found")
        );
        assert_eq!(
            compilation.diagnostics[1].severity,
            RequestPolicyDiagnosticSeverity::Error
        );
        assert_eq!(
            compilation.diagnostics[1].source,
            RequestPolicyDiagnosticSource::RecordingPrefilter
        );
        assert!(
            compilation.diagnostics[1]
                .message
                .contains("include_url_patterns[0]")
        );
    }

    #[test]
    fn replacement_is_visible_to_cloned_stores_as_one_coherent_snapshot() {
        let store = RequestPolicyStore::default();
        let clone = store.clone();
        let mut settings = AppSettings::default();
        settings.recording.prefilter = RecordingPrefilterSettings {
            enable: true,
            include_url_patterns: vec![RecordingPrefilterPatternSettings::new(
                "http://mapped.example.com:80/*",
            )],
        };
        settings.proxy = Some(ProxySettings {
            enable: true,
            active_preset: Some("test".to_string()),
            presets: vec![ProxyPresetSettings {
                name: "test".to_string(),
                map_remote: ProxyMapRemoteSettings {
                    enable: true,
                    rules: vec![ProxyMapRemoteRule {
                        from: "https://api.example.com".to_string(),
                        to: "http://mapped.example.com".to_string(),
                        enable: true,
                    }],
                },
                ..ProxyPresetSettings::default()
            }],
        });

        store.replace(RequestPolicy::compile(&settings).policy);

        let current = clone.current();
        let original_uri = "https://api.example.com/path"
            .parse()
            .expect("test URI should parse");
        let (evaluation, urls) = current.evaluate(&original_uri, true).into_parts();
        assert_eq!(
            evaluation.mapped_uri.as_ref().map(ToString::to_string),
            Some("http://mapped.example.com/path".to_string())
        );
        let recording = urls
            .recording()
            .expect("recording evaluation should be present");
        assert_eq!(recording.original_url, "https://api.example.com/path");
        assert_eq!(recording.effective_url, "http://mapped.example.com/path");
        assert!(recording.included);
    }
}
