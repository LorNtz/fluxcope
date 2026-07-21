use crate::settings::{ProxyMapLocalRule, ProxyMapRemoteRule, ProxyPresetSettings, ProxySettings};
use http::Uri;
use parking_lot::RwLock;
use std::{collections::HashMap, env, path::PathBuf, sync::Arc};
use url::Url;

#[derive(Clone)]
pub struct MappingStore {
    current: Arc<RwLock<Arc<MappingEngine>>>,
}

impl MappingStore {
    pub fn new(engine: MappingEngine) -> Self {
        Self {
            current: Arc::new(RwLock::new(Arc::new(engine))),
        }
    }

    pub fn current(&self) -> Arc<MappingEngine> {
        Arc::clone(&self.current.read())
    }

    pub fn replace(&self, engine: MappingEngine) {
        *self.current.write() = Arc::new(engine);
    }
}

impl Default for MappingStore {
    fn default() -> Self {
        Self::new(MappingEngine::default())
    }
}

#[derive(Clone, Debug, Default)]
pub struct MappingEngine {
    remote_rules: CompiledRules<RemoteTarget>,
    local_rules: CompiledRules<PathBuf>,
    diagnostics: Vec<MappingDiagnostic>,
}

impl MappingEngine {
    pub fn compile(proxy: Option<&ProxySettings>) -> Self {
        let mut engine = Self::default();
        let Some(proxy) = proxy else {
            return engine;
        };
        if !proxy.enable {
            return engine;
        }

        let Some((preset_index, preset)) = active_preset(proxy, &mut engine.diagnostics) else {
            return engine;
        };

        if preset.map_remote.enable {
            compile_remote_rules(preset_index, &preset.map_remote.rules, &mut engine);
        }
        if preset.map_local.enable {
            compile_local_rules(preset_index, &preset.map_local.rules, &mut engine);
        }

        engine
    }

    pub fn map_request(&self, uri: &Uri) -> MappingDecision {
        if self.remote_rules.is_empty() && self.local_rules.is_empty() {
            return MappingDecision::default();
        }
        let Some(original) = RequestUrl::from_uri(uri) else {
            return MappingDecision::default();
        };

        let remote_url = self
            .remote_rules
            .match_target(&original)
            .map(|target| target.apply_to(&original.url));
        let effective_url = remote_url.as_ref().unwrap_or(&original.url);
        if self.local_rules.is_empty() {
            return MappingDecision {
                mapped_uri: remote_url.and_then(|url| url.as_str().parse().ok()),
                local_path: None,
            };
        }
        let effective_request = RequestUrl::from_url(effective_url.clone());
        let local_path = effective_request
            .as_ref()
            .and_then(|request| self.local_rules.match_target(request))
            .cloned();

        MappingDecision {
            mapped_uri: remote_url.and_then(|url| url.as_str().parse().ok()),
            local_path,
        }
    }

    pub fn diagnostics(&self) -> &[MappingDiagnostic] {
        &self.diagnostics
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MappingDecision {
    pub mapped_uri: Option<Uri>,
    pub local_path: Option<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MappingDiagnostic {
    pub severity: DiagnosticSeverity,
    pub code: MappingDiagnosticCode,
    pub location: Option<MappingLocation>,
    pub message: String,
}

impl MappingDiagnostic {
    fn warning(code: MappingDiagnosticCode, message: impl Into<String>) -> Self {
        Self {
            severity: DiagnosticSeverity::Warning,
            code,
            location: None,
            message: message.into(),
        }
    }

    fn rule(
        severity: DiagnosticSeverity,
        code: MappingDiagnosticCode,
        location: MappingLocation,
        message: impl Into<String>,
    ) -> Self {
        Self {
            severity,
            code,
            location: Some(location),
            message: message.into(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiagnosticSeverity {
    Warning,
    Error,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MappingDiagnosticCode {
    MissingActivePreset,
    ActivePresetNotFound,
    InvalidRuleSource,
    InvalidRuleTarget,
    EmptyPresetName,
    DuplicatePresetName,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MappingTable {
    Remote,
    Local,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MappingField {
    From,
    To,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MappingLocation {
    pub preset_index: usize,
    pub table: MappingTable,
    pub rule_index: usize,
    pub field: MappingField,
}

#[derive(Clone, Debug)]
struct CompiledRules<T> {
    by_origin: HashMap<OriginKey, OriginRules<T>>,
}

impl<T> CompiledRules<T> {
    fn insert(&mut self, pattern: RulePattern, target: T) {
        let rules = self.by_origin.entry(pattern.origin).or_default();
        if let Some(path) = pattern.path {
            rules.path_rules.insert(path, target);
        } else {
            rules.host_rule = Some(target);
        }
    }

    fn is_empty(&self) -> bool {
        self.by_origin.is_empty()
    }

    fn match_target<'a>(&'a self, request: &RequestUrl) -> Option<&'a T> {
        let rules = self.by_origin.get(&request.origin)?;
        rules
            .path_rules
            .get(&request.path) // rules matching the whole path will have higher priority
            .or(rules.host_rule.as_ref())
    }
}

impl<T> Default for CompiledRules<T> {
    fn default() -> Self {
        Self {
            by_origin: HashMap::new(),
        }
    }
}

#[derive(Clone, Debug)]
struct OriginRules<T> {
    host_rule: Option<T>,
    path_rules: HashMap<String, T>,
}

impl<T> Default for OriginRules<T> {
    fn default() -> Self {
        Self {
            host_rule: None,
            path_rules: HashMap::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct OriginKey {
    scheme: String,
    host: String,
    port: u16,
}

impl OriginKey {
    fn from_url(url: &Url) -> Result<Self, String> {
        let Some(host) = url.host_str() else {
            return Err("missing host".to_string());
        };
        let Some(port) = url.port_or_known_default() else {
            return Err("missing known port".to_string());
        };

        Ok(Self {
            scheme: url.scheme().to_ascii_lowercase(),
            host: host.to_ascii_lowercase(),
            port,
        })
    }
}

#[derive(Clone, Debug)]
struct RulePattern {
    origin: OriginKey,
    path: Option<String>,
}

#[derive(Clone, Debug)]
struct RemoteTarget {
    scheme: String,
    host: String,
    port: Option<u16>,
    path: Option<String>,
}

impl RemoteTarget {
    fn apply_to(&self, original: &Url) -> Url {
        let mut mapped = original.clone();
        let _ = mapped.set_scheme(&self.scheme);
        let _ = mapped.set_host(Some(&self.host));
        let _ = mapped.set_port(self.port);
        if let Some(path) = &self.path {
            mapped.set_path(path);
        }
        mapped.set_fragment(None);
        mapped
    }
}

#[derive(Clone, Debug)]
struct RequestUrl {
    url: Url,
    origin: OriginKey,
    path: String,
}

impl RequestUrl {
    fn from_uri(uri: &Uri) -> Option<Self> {
        Url::parse(&uri.to_string()).ok().and_then(Self::from_url)
    }

    fn from_url(url: Url) -> Option<Self> {
        if !matches!(url.scheme(), "http" | "https") || !url.has_host() {
            return None;
        }

        let origin = OriginKey::from_url(&url).ok()?;
        let path = url.path().to_string();

        Some(Self { url, origin, path })
    }
}

fn active_preset<'a>(
    proxy: &'a ProxySettings,
    diagnostics: &mut Vec<MappingDiagnostic>,
) -> Option<(usize, &'a ProxyPresetSettings)> {
    let active_name = proxy.active_preset.as_deref().unwrap_or("").trim();
    if let Some(diagnostic) = active_preset_diagnostic(proxy) {
        diagnostics.push(diagnostic);
        return None;
    }
    if active_name.is_empty() {
        return None;
    }
    proxy
        .presets
        .iter()
        .enumerate()
        .find(|(_, preset)| preset.name == active_name)
}

fn active_preset_diagnostic(proxy: &ProxySettings) -> Option<MappingDiagnostic> {
    let active_name = proxy.active_preset.as_deref().unwrap_or("").trim();
    if active_name.is_empty() {
        return (!proxy.presets.is_empty()).then(|| {
            MappingDiagnostic::warning(
                MappingDiagnosticCode::MissingActivePreset,
                "proxy mapping has presets but no active_preset",
            )
        });
    }
    (!proxy
        .presets
        .iter()
        .any(|preset| preset.name == active_name))
    .then(|| {
        MappingDiagnostic::warning(
            MappingDiagnosticCode::ActivePresetNotFound,
            format!("proxy mapping active preset '{active_name}' was not found"),
        )
    })
}

fn compile_remote_rules(
    preset_index: usize,
    rules: &[ProxyMapRemoteRule],
    engine: &mut MappingEngine,
) {
    for (index, rule) in rules.iter().enumerate() {
        if !rule.enable {
            continue;
        }

        let pattern = match parse_rule_url(&rule.from) {
            Ok(url) => url,
            Err(error) => {
                engine.diagnostics.push(rule_diagnostic(
                    DiagnosticSeverity::Warning,
                    MappingDiagnosticCode::InvalidRuleSource,
                    preset_index,
                    MappingTable::Remote,
                    index,
                    MappingField::From,
                    format!("map_remote rule {index} skipped: invalid from URL: {error}"),
                ));
                continue;
            }
        };
        let target = match parse_remote_target(&rule.to) {
            Ok(target) => target,
            Err(error) => {
                engine.diagnostics.push(rule_diagnostic(
                    DiagnosticSeverity::Warning,
                    MappingDiagnosticCode::InvalidRuleTarget,
                    preset_index,
                    MappingTable::Remote,
                    index,
                    MappingField::To,
                    format!("map_remote rule {index} skipped: invalid to URL: {error}"),
                ));
                continue;
            }
        };

        engine.remote_rules.insert(pattern, target);
    }
}

fn compile_local_rules(
    preset_index: usize,
    rules: &[ProxyMapLocalRule],
    engine: &mut MappingEngine,
) {
    for (index, rule) in rules.iter().enumerate() {
        if !rule.enable {
            continue;
        }

        let pattern = match parse_rule_url(&rule.from) {
            Ok(url) => url,
            Err(error) => {
                engine.diagnostics.push(rule_diagnostic(
                    DiagnosticSeverity::Warning,
                    MappingDiagnosticCode::InvalidRuleSource,
                    preset_index,
                    MappingTable::Local,
                    index,
                    MappingField::From,
                    format!("map_local rule {index} skipped: invalid from URL: {error}"),
                ));
                continue;
            }
        };
        let target = match expand_home_path(rule.to.trim()) {
            Ok(path) => path,
            Err(error) => {
                engine.diagnostics.push(rule_diagnostic(
                    DiagnosticSeverity::Warning,
                    MappingDiagnosticCode::InvalidRuleTarget,
                    preset_index,
                    MappingTable::Local,
                    index,
                    MappingField::To,
                    format!("map_local rule {index} skipped: invalid to path: {error}"),
                ));
                continue;
            }
        };

        engine.local_rules.insert(pattern, target);
    }
}

fn rule_diagnostic(
    severity: DiagnosticSeverity,
    code: MappingDiagnosticCode,
    preset_index: usize,
    table: MappingTable,
    rule_index: usize,
    field: MappingField,
    message: impl Into<String>,
) -> MappingDiagnostic {
    MappingDiagnostic::rule(
        severity,
        code,
        MappingLocation {
            preset_index,
            table,
            rule_index,
            field,
        },
        message,
    )
}

pub(crate) fn validate_proxy_settings(proxy: &ProxySettings) -> Vec<MappingDiagnostic> {
    let mut diagnostics = Vec::new();
    let mut names = std::collections::HashSet::new();
    for (preset_index, preset) in proxy.presets.iter().enumerate() {
        if preset.name.trim().is_empty() {
            diagnostics.push(MappingDiagnostic::warning(
                MappingDiagnosticCode::EmptyPresetName,
                "proxy preset names cannot be empty",
            ));
        } else if !names.insert(preset.name.as_str()) {
            diagnostics.push(MappingDiagnostic::warning(
                MappingDiagnosticCode::DuplicatePresetName,
                format!("duplicate proxy preset name: {}", preset.name),
            ));
        }
        validate_rules(
            preset_index,
            MappingTable::Remote,
            preset
                .map_remote
                .rules
                .iter()
                .map(|rule| (&rule.from, &rule.to)),
            &mut diagnostics,
        );
        for (rule_index, rule) in preset.map_local.rules.iter().enumerate() {
            if let Err(error) = parse_rule_url(&rule.from) {
                diagnostics.push(rule_diagnostic(
                    DiagnosticSeverity::Error,
                    MappingDiagnosticCode::InvalidRuleSource,
                    preset_index,
                    MappingTable::Local,
                    rule_index,
                    MappingField::From,
                    format!("map_local.from {error}"),
                ));
            }
            if let Err(error) = expand_home_path(rule.to.trim()) {
                diagnostics.push(rule_diagnostic(
                    DiagnosticSeverity::Error,
                    MappingDiagnosticCode::InvalidRuleTarget,
                    preset_index,
                    MappingTable::Local,
                    rule_index,
                    MappingField::To,
                    format!("map_local.to {error}"),
                ));
            }
        }
    }
    if let Some(diagnostic) = active_preset_diagnostic(proxy) {
        diagnostics.push(diagnostic);
    }
    for diagnostic in &mut diagnostics {
        diagnostic.severity = DiagnosticSeverity::Error;
    }
    diagnostics
}

fn validate_rules<'a>(
    preset_index: usize,
    table: MappingTable,
    rules: impl Iterator<Item = (&'a String, &'a String)>,
    diagnostics: &mut Vec<MappingDiagnostic>,
) {
    for (rule_index, (from, to)) in rules.enumerate() {
        if let Err(error) = parse_rule_url(from) {
            diagnostics.push(rule_diagnostic(
                DiagnosticSeverity::Error,
                MappingDiagnosticCode::InvalidRuleSource,
                preset_index,
                table,
                rule_index,
                MappingField::From,
                format!("map_remote.from {error}"),
            ));
        }
        if let Err(error) = parse_remote_target(to) {
            diagnostics.push(rule_diagnostic(
                DiagnosticSeverity::Error,
                MappingDiagnosticCode::InvalidRuleTarget,
                preset_index,
                table,
                rule_index,
                MappingField::To,
                format!("map_remote.to {error}"),
            ));
        }
    }
}

fn parse_rule_url(raw: &str) -> Result<RulePattern, String> {
    let raw = raw.trim();
    let url = parse_http_url(raw)?;

    Ok(RulePattern {
        origin: OriginKey::from_url(&url)?,
        path: explicit_path(raw).map(|_| url.path().to_string()),
    })
}

fn parse_remote_target(raw: &str) -> Result<RemoteTarget, String> {
    let raw = raw.trim();
    let url = parse_http_url(raw)?;
    let host = url
        .host_str()
        .ok_or_else(|| "missing host".to_string())?
        .to_string();

    Ok(RemoteTarget {
        scheme: url.scheme().to_string(),
        host,
        port: url.port(),
        path: explicit_path(raw).map(|_| url.path().to_string()),
    })
}

fn parse_http_url(raw: &str) -> Result<Url, String> {
    if raw.is_empty() {
        return Err("value is empty".to_string());
    }

    let url = Url::parse(raw).map_err(|error| error.to_string())?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err("scheme must be http or https".to_string());
    }
    if !url.has_host() {
        return Err("missing host".to_string());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("userinfo is not supported".to_string());
    }
    if url.query().is_some() {
        return Err("query is not supported in mapping rules".to_string());
    }
    if url.fragment().is_some() {
        return Err("fragment is not supported in mapping rules".to_string());
    }

    Ok(url)
}

fn explicit_path(raw: &str) -> Option<()> {
    let (_, after_scheme) = raw.split_once("://")?;
    after_scheme.find('/').map(|_| ())
}

fn expand_home_path(path: &str) -> Result<PathBuf, String> {
    if path.is_empty() {
        return Err("path is empty".to_string());
    }

    if path == "~" {
        return home_dir();
    }

    if let Some(rest) = path.strip_prefix("~/") {
        return home_dir().map(|home| home.join(rest));
    }

    Ok(PathBuf::from(path))
}

fn home_dir() -> Result<PathBuf, String> {
    env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| "HOME environment variable is not set".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{ProxyMapLocalSettings, ProxyMapRemoteSettings, ProxyPresetSettings};

    fn remote_rule(from: &str, to: &str) -> ProxyMapRemoteRule {
        ProxyMapRemoteRule {
            from: from.to_string(),
            to: to.to_string(),
            enable: true,
        }
    }

    fn local_rule(from: &str, to: &str) -> ProxyMapLocalRule {
        ProxyMapLocalRule {
            from: from.to_string(),
            to: to.to_string(),
            enable: true,
        }
    }

    fn proxy_with_preset(preset: ProxyPresetSettings) -> ProxySettings {
        ProxySettings {
            enable: true,
            active_preset: Some(preset.name.clone()),
            presets: vec![preset],
        }
    }

    fn decision(engine: &MappingEngine, uri: &str) -> MappingDecision {
        engine.map_request(&uri.parse().expect("test URI should parse"))
    }

    #[test]
    fn missing_proxy_compiles_to_noop() {
        let engine = MappingEngine::compile(None);

        assert_eq!(
            MappingDecision::default(),
            decision(&engine, "https://a.com/some/path?x=1")
        );
        assert!(engine.diagnostics().is_empty());
    }

    #[test]
    fn runtime_and_save_validation_agree_on_missing_active_preset() {
        let proxy = ProxySettings {
            enable: true,
            active_preset: None,
            presets: vec![ProxyPresetSettings {
                name: "dev".to_string(),
                ..ProxyPresetSettings::default()
            }],
        };

        assert_eq!(
            MappingEngine::compile(Some(&proxy)).diagnostics()[0].code,
            MappingDiagnosticCode::MissingActivePreset
        );
        assert_eq!(
            validate_proxy_settings(&proxy)[0].code,
            MappingDiagnosticCode::MissingActivePreset
        );
    }

    #[test]
    fn remote_host_rule_rewrites_origin_and_preserves_path_and_query() {
        let proxy = proxy_with_preset(ProxyPresetSettings {
            name: "dev".to_string(),
            map_remote: ProxyMapRemoteSettings {
                enable: true,
                rules: vec![remote_rule("https://a.com", "http://a.test.com")],
            },
            map_local: ProxyMapLocalSettings::default(),
        });

        let mapped = decision(
            &MappingEngine::compile(Some(&proxy)),
            "https://a.com/api?v=1",
        )
        .mapped_uri;

        assert_eq!(
            mapped.map(|uri| uri.to_string()),
            Some("http://a.test.com/api?v=1".to_string())
        );
    }

    #[test]
    fn remote_path_rule_beats_later_host_rule() {
        let proxy = proxy_with_preset(ProxyPresetSettings {
            name: "dev".to_string(),
            map_remote: ProxyMapRemoteSettings {
                enable: true,
                rules: vec![
                    remote_rule("https://a.com", "http://a.test.com"),
                    remote_rule(
                        "https://a.com/some/api1",
                        "http://b.test.com/some/test/api1",
                    ),
                    remote_rule("https://a.com", "http://c.test.com"),
                ],
            },
            map_local: ProxyMapLocalSettings::default(),
        });
        let engine = MappingEngine::compile(Some(&proxy));

        assert_eq!(
            decision(&engine, "https://a.com/some/api1?q=1")
                .mapped_uri
                .map(|uri| uri.to_string()),
            Some("http://b.test.com/some/test/api1?q=1".to_string())
        );
        assert_eq!(
            decision(&engine, "https://a.com/other?q=1")
                .mapped_uri
                .map(|uri| uri.to_string()),
            Some("http://c.test.com/other?q=1".to_string())
        );
    }

    #[test]
    fn remote_to_blank_path_preserves_original_path() {
        let proxy = proxy_with_preset(ProxyPresetSettings {
            name: "dev".to_string(),
            map_remote: ProxyMapRemoteSettings {
                enable: true,
                rules: vec![remote_rule("https://b.com/some/api2", "http://b.test.com")],
            },
            map_local: ProxyMapLocalSettings::default(),
        });

        assert_eq!(
            decision(
                &MappingEngine::compile(Some(&proxy)),
                "https://b.com/some/api2?x=1"
            )
            .mapped_uri
            .map(|uri| uri.to_string()),
            Some("http://b.test.com/some/api2?x=1".to_string())
        );
    }

    #[test]
    fn remote_from_blank_path_to_specific_path_maps_host_to_endpoint() {
        let proxy = proxy_with_preset(ProxyPresetSettings {
            name: "dev".to_string(),
            map_remote: ProxyMapRemoteSettings {
                enable: true,
                rules: vec![remote_rule("https://c.com", "http://c.test.com/some/path")],
            },
            map_local: ProxyMapLocalSettings::default(),
        });

        assert_eq!(
            decision(
                &MappingEngine::compile(Some(&proxy)),
                "https://c.com/anything?x=1"
            )
            .mapped_uri
            .map(|uri| uri.to_string()),
            Some("http://c.test.com/some/path?x=1".to_string())
        );
    }

    #[test]
    fn disabled_rules_are_ignored() {
        let mut disabled = remote_rule("https://a.com", "http://disabled.test.com");
        disabled.enable = false;
        let proxy = proxy_with_preset(ProxyPresetSettings {
            name: "dev".to_string(),
            map_remote: ProxyMapRemoteSettings {
                enable: true,
                rules: vec![disabled],
            },
            map_local: ProxyMapLocalSettings::default(),
        });

        assert_eq!(
            MappingDecision::default(),
            decision(&MappingEngine::compile(Some(&proxy)), "https://a.com/api")
        );
    }

    #[test]
    fn local_mapping_matches_after_remote_mapping() {
        let proxy = proxy_with_preset(ProxyPresetSettings {
            name: "dev".to_string(),
            map_remote: ProxyMapRemoteSettings {
                enable: true,
                rules: vec![remote_rule(
                    "https://a.com/some/api1",
                    "http://b.test.com/some/test/api1",
                )],
            },
            map_local: ProxyMapLocalSettings {
                enable: true,
                rules: vec![local_rule(
                    "http://b.test.com/some/test/api1",
                    "/tmp/api1.json",
                )],
            },
        });

        let result = decision(
            &MappingEngine::compile(Some(&proxy)),
            "https://a.com/some/api1?q=1",
        );

        assert_eq!(
            result.mapped_uri.map(|uri| uri.to_string()),
            Some("http://b.test.com/some/test/api1?q=1".to_string())
        );
        assert_eq!(result.local_path, Some(PathBuf::from("/tmp/api1.json")));
    }

    #[test]
    fn invalid_rule_url_is_reported_and_skipped() {
        let proxy = proxy_with_preset(ProxyPresetSettings {
            name: "dev".to_string(),
            map_remote: ProxyMapRemoteSettings {
                enable: true,
                rules: vec![remote_rule("https://a.com/api?q=1", "http://b.test.com")],
            },
            map_local: ProxyMapLocalSettings::default(),
        });

        let engine = MappingEngine::compile(Some(&proxy));

        assert_eq!(
            MappingDecision::default(),
            decision(&engine, "https://a.com/api?q=1")
        );
        assert_eq!(engine.diagnostics().len(), 1);
    }

    #[test]
    fn mapping_store_replaces_engine_for_future_requests() {
        let store = MappingStore::default();
        let uri = "https://a.com/api".parse().expect("test URI should parse");
        assert_eq!(
            MappingDecision::default(),
            store.current().map_request(&uri)
        );

        let proxy = proxy_with_preset(ProxyPresetSettings {
            name: "dev".to_string(),
            map_remote: ProxyMapRemoteSettings {
                enable: true,
                rules: vec![remote_rule("https://a.com", "http://b.test.com")],
            },
            map_local: ProxyMapLocalSettings::default(),
        });

        store.replace(MappingEngine::compile(Some(&proxy)));

        assert_eq!(
            store
                .current()
                .map_request(&uri)
                .mapped_uri
                .map(|uri| uri.to_string()),
            Some("http://b.test.com/api".to_string())
        );
    }
}
