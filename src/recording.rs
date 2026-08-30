use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use globset::{GlobBuilder, GlobSet, GlobSetBuilder};

use crate::settings::RecordingPrefilterSettings;

#[derive(Clone, Debug)]
pub struct RecordingState {
    enabled: Arc<AtomicBool>,
}

impl RecordingState {
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled: Arc::new(AtomicBool::new(enabled)),
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }
    #[cfg(unix)]
    pub(crate) fn set_enabled(&self, enabled: bool) -> bool {
        self.enabled.swap(enabled, Ordering::Relaxed)
    }

    pub fn toggle(&self) -> bool {
        self.enabled.fetch_xor(true, Ordering::Relaxed) ^ true
    }
}

impl Default for RecordingState {
    fn default() -> Self {
        Self::new(true)
    }
}

#[derive(Debug)]
pub(crate) struct RecordingPrefilter {
    enabled: bool,
    matcher: RecordingPrefilterMatcher,
}

#[derive(Debug)]
enum RecordingPrefilterMatcher {
    IncludeAll,
    IncludeNone,
    Patterns(GlobSet),
}

impl RecordingPrefilter {
    pub(crate) fn compile(
        settings: &RecordingPrefilterSettings,
    ) -> (Self, Vec<PrefilterDiagnostic>) {
        let mut diagnostics = Vec::new();
        let mut builder = GlobSetBuilder::new();
        let mut valid_patterns = 0_usize;

        for (index, pattern) in settings.include_url_patterns.iter().enumerate() {
            if !pattern.enable {
                continue;
            }

            match GlobBuilder::new(&pattern.pattern)
                .case_insensitive(false)
                .literal_separator(false)
                .build()
            {
                Ok(glob) => {
                    builder.add(glob);
                    valid_patterns = valid_patterns.saturating_add(1);
                }
                Err(error) => diagnostics.push(PrefilterDiagnostic::pattern(
                    index,
                    &pattern.pattern,
                    error.to_string(),
                )),
            }
        }

        let matcher = if settings.include_url_patterns.is_empty() {
            RecordingPrefilterMatcher::IncludeAll
        } else if valid_patterns == 0 {
            RecordingPrefilterMatcher::IncludeNone
        } else {
            match builder.build() {
                Ok(matcher) => RecordingPrefilterMatcher::Patterns(matcher),
                Err(error) => {
                    diagnostics.push(PrefilterDiagnostic::matcher(error.to_string()));
                    RecordingPrefilterMatcher::IncludeNone
                }
            }
        };

        (
            Self {
                enabled: settings.enable,
                matcher,
            },
            diagnostics,
        )
    }

    pub(crate) fn includes(&self, effective_url: &str) -> bool {
        if !self.enabled {
            return true;
        }
        match &self.matcher {
            RecordingPrefilterMatcher::IncludeAll => true,
            RecordingPrefilterMatcher::IncludeNone => false,
            RecordingPrefilterMatcher::Patterns(matcher) => {
                matcher.is_match(effective_url)
                    || default_port_alternate_url(effective_url)
                        .is_some_and(|alternate_url| matcher.is_match(alternate_url))
            }
        }
    }
}

fn default_port_alternate_url(effective_url: &str) -> Option<String> {
    let scheme_end = effective_url.find("://")?;
    let (default_port, default_port_text) =
        if effective_url[..scheme_end].eq_ignore_ascii_case("https") {
            (443, ":443")
        } else if effective_url[..scheme_end].eq_ignore_ascii_case("http") {
            (80, ":80")
        } else {
            return None;
        };
    let authority_start = scheme_end.saturating_add(3);
    let authority_end = effective_url[authority_start..]
        .find(['/', '?', '#'])
        .map_or(effective_url.len(), |offset| authority_start + offset);
    let authority = &effective_url[authority_start..authority_end];
    let parsed_authority = authority.parse::<http::uri::Authority>().ok()?;

    let (capacity, first, second) = match parsed_authority.port_u16() {
        Some(port) if port == default_port => {
            let port_start = authority_start.saturating_add(authority.rfind(':')?);
            (
                effective_url
                    .len()
                    .saturating_sub(authority_end - port_start),
                &effective_url[..port_start],
                &effective_url[authority_end..],
            )
        }
        Some(_) => return None,
        None => (
            effective_url.len().saturating_add(default_port_text.len()),
            &effective_url[..authority_end],
            &effective_url[authority_end..],
        ),
    };

    let mut alternate_url = String::with_capacity(capacity);
    alternate_url.push_str(first);
    if parsed_authority.port().is_none() {
        alternate_url.push_str(default_port_text);
    }
    alternate_url.push_str(second);
    Some(alternate_url)
}

impl Default for RecordingPrefilter {
    fn default() -> Self {
        Self {
            enabled: true,
            matcher: RecordingPrefilterMatcher::IncludeAll,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PrefilterDiagnostic {
    pub message: String,
}

impl PrefilterDiagnostic {
    fn pattern(index: usize, pattern: &str, error: String) -> Self {
        Self {
            message: format!(
                "recording.prefilter.include_url_patterns[{index}] ignored invalid glob {pattern:?}: {error}"
            ),
        }
    }

    fn matcher(error: String) -> Self {
        Self {
            message: format!(
                "recording prefilter matcher could not be built; no URLs will be recorded: {error}"
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::RecordingPrefilterPatternSettings;

    fn pattern(value: &str) -> RecordingPrefilterPatternSettings {
        RecordingPrefilterPatternSettings::new(value)
    }

    fn disabled_pattern(value: &str) -> RecordingPrefilterPatternSettings {
        RecordingPrefilterPatternSettings {
            pattern: value.to_string(),
            enable: false,
        }
    }

    #[test]
    fn disabled_or_empty_prefilter_includes_every_url() {
        let (disabled, diagnostics) = RecordingPrefilter::compile(&RecordingPrefilterSettings {
            enable: false,
            include_url_patterns: vec![pattern("https://api.example.com/*")],
        });
        assert!(diagnostics.is_empty());
        assert!(disabled.includes("https://other.example.com/path?x=1"));

        let (empty, diagnostics) =
            RecordingPrefilter::compile(&RecordingPrefilterSettings::default());
        assert!(diagnostics.is_empty());
        assert!(empty.includes("https://other.example.com/path?x=1"));
    }

    #[test]
    fn glob_matches_complete_effective_url_including_query() {
        let (prefilter, diagnostics) = RecordingPrefilter::compile(&RecordingPrefilterSettings {
            enable: true,
            include_url_patterns: vec![pattern("https://{api,cdn}.example.com/v1/*?client=?")],
        });
        assert!(diagnostics.is_empty());
        assert!(prefilter.includes("https://api.example.com/v1/orders?client=a"));
        assert!(prefilter.includes("https://cdn.example.com/v1/assets/icon?client=b"));
        assert!(!prefilter.includes("https://api.example.com/v1/orders?client=ab"));
        assert!(!prefilter.includes("https://API.example.com/v1/orders?client=a"));
    }

    #[test]
    fn default_ports_are_optional_for_matching_and_queries_are_preserved() {
        let (prefilter, diagnostics) = RecordingPrefilter::compile(&RecordingPrefilterSettings {
            enable: true,
            include_url_patterns: vec![
                pattern("https://api.example.com/v1/*?client=?"),
                pattern("http://plain.example.com/*"),
            ],
        });
        assert!(diagnostics.is_empty());

        assert!(prefilter.includes("https://api.example.com:443/v1/orders?client=a"));
        assert!(!prefilter.includes("https://api.example.com:443/v1/orders?client=ab"));
        assert!(prefilter.includes("http://plain.example.com:80/path"));
    }

    #[test]
    fn explicit_default_port_patterns_match_urls_without_ports() {
        let (prefilter, diagnostics) = RecordingPrefilter::compile(&RecordingPrefilterSettings {
            enable: true,
            include_url_patterns: vec![
                pattern("https://api.example.com:443/*"),
                pattern("http://plain.example.com:80/*"),
            ],
        });
        assert!(diagnostics.is_empty());

        assert!(prefilter.includes("https://api.example.com/path"));
        assert!(prefilter.includes("http://plain.example.com/path"));
    }

    #[test]
    fn raw_default_port_patterns_still_match_and_non_default_ports_remain_significant() {
        let (explicit_port, diagnostics) =
            RecordingPrefilter::compile(&RecordingPrefilterSettings {
                enable: true,
                include_url_patterns: vec![pattern("https://api.example.com:443/*")],
            });
        assert!(diagnostics.is_empty());
        assert!(explicit_port.includes("https://api.example.com:443/path"));

        let (implicit_port, diagnostics) =
            RecordingPrefilter::compile(&RecordingPrefilterSettings {
                enable: true,
                include_url_patterns: vec![pattern("https://api.example.com/*")],
            });
        assert!(diagnostics.is_empty());
        assert!(!implicit_port.includes("https://api.example.com:8443/path"));
    }

    #[test]
    fn invalid_patterns_are_ignored_while_valid_patterns_still_match() {
        let (prefilter, diagnostics) = RecordingPrefilter::compile(&RecordingPrefilterSettings {
            enable: true,
            include_url_patterns: vec![pattern("["), pattern("https://api.example.com/*")],
        });

        assert_eq!(diagnostics.len(), 1);
        assert!(diagnostics[0].message.contains("[0]"));
        assert!(prefilter.includes("https://api.example.com/path?x=1"));
        assert!(!prefilter.includes("https://other.example.com/path?x=1"));
    }

    #[test]
    fn all_invalid_enabled_patterns_include_no_urls() {
        let (prefilter, diagnostics) = RecordingPrefilter::compile(&RecordingPrefilterSettings {
            enable: true,
            include_url_patterns: vec![pattern("["), pattern("{")],
        });

        assert_eq!(diagnostics.len(), 2);
        assert!(!prefilter.includes("https://other.example.com/path?x=1"));
    }

    #[test]
    fn all_disabled_patterns_include_no_urls_without_diagnostics() {
        let (prefilter, diagnostics) = RecordingPrefilter::compile(&RecordingPrefilterSettings {
            enable: true,
            include_url_patterns: vec![
                disabled_pattern("["),
                disabled_pattern("https://api.example.com/*"),
            ],
        });

        assert!(diagnostics.is_empty());
        assert!(!prefilter.includes("https://api.example.com/path"));
        assert!(!prefilter.includes("https://other.example.com/path"));
    }

    #[test]
    fn disabled_invalid_pattern_is_skipped_while_enabled_pattern_matches() {
        let (prefilter, diagnostics) = RecordingPrefilter::compile(&RecordingPrefilterSettings {
            enable: true,
            include_url_patterns: vec![disabled_pattern("["), pattern("https://api.example.com/*")],
        });

        assert!(diagnostics.is_empty());
        assert!(prefilter.includes("https://api.example.com/path"));
        assert!(!prefilter.includes("https://other.example.com/path"));
    }
}
