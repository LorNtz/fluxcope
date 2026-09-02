pub(crate) mod mapping_ops;

use anyhow::Context;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::{
    env, fs, io,
    io::Read,
    path::{Path, PathBuf},
    sync::Arc,
};

use crate::{cli::ConfigSelection, instance::DefaultConfigLease};

const DEFAULT_PROXY_PORT: u16 = 8989;
const DEFAULT_CERTIFICATE_STORE_DIR: &str = "~/.fluxcope/certificate/";
const DEFAULT_CERTIFICATE_PEM_FILENAME: &str = "fluxcope-ca.pem";

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub struct AppSettings {
    pub server: ServerSettings,
    #[serde(default, skip_serializing_if = "McpSettings::is_default")]
    pub mcp: McpSettings,
    pub certificate: CertificateSettings,
    pub recording: RecordingSettings,
    pub ui: UiSettings,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy: Option<ProxySettings>,
}

// Keep the top-level schema default explicit so new settings fields require
// an intentional default choice.
#[allow(clippy::derivable_impls)]
impl Default for AppSettings {
    fn default() -> Self {
        Self {
            server: ServerSettings::default(),
            mcp: McpSettings::default(),
            certificate: CertificateSettings::default(),
            recording: RecordingSettings::default(),
            ui: UiSettings::default(),
            proxy: None,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub struct ServerSettings {
    pub port: u16,
}

impl Default for ServerSettings {
    fn default() -> Self {
        Self {
            port: DEFAULT_PROXY_PORT,
        }
    }
}
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub struct McpSettings {
    #[serde(default, skip_serializing_if = "is_false")]
    pub enable: bool,
}

impl McpSettings {
    fn is_default(&self) -> bool {
        !self.enable
    }
}

fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub struct CertificateSettings {
    pub store_dir: String,
    pub pem_filename: String,
}

impl Default for CertificateSettings {
    fn default() -> Self {
        Self {
            store_dir: DEFAULT_CERTIFICATE_STORE_DIR.to_string(),
            pem_filename: DEFAULT_CERTIFICATE_PEM_FILENAME.to_string(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub struct RecordingSettings {
    pub start_record_on_launch: bool,
    #[serde(
        default,
        skip_serializing_if = "RecordingPrefilterSettings::is_default"
    )]
    pub prefilter: RecordingPrefilterSettings,
}

impl Default for RecordingSettings {
    fn default() -> Self {
        Self {
            start_record_on_launch: true,
            prefilter: RecordingPrefilterSettings::default(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub struct RecordingPrefilterSettings {
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    pub enable: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub include_url_patterns: Vec<RecordingPrefilterPatternSettings>,
}

impl RecordingPrefilterSettings {
    fn is_default(settings: &Self) -> bool {
        settings == &Self::default()
    }

    fn disabled() -> Self {
        Self {
            enable: false,
            include_url_patterns: Vec::new(),
        }
    }
}

impl Default for RecordingPrefilterSettings {
    fn default() -> Self {
        Self {
            enable: true,
            include_url_patterns: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RecordingPrefilterPatternSettings {
    pub pattern: String,
    pub enable: bool,
}

impl RecordingPrefilterPatternSettings {
    pub(crate) fn new(pattern: impl Into<String>) -> Self {
        Self {
            pattern: pattern.into(),
            enable: true,
        }
    }
}

impl Default for RecordingPrefilterPatternSettings {
    fn default() -> Self {
        Self::new(String::new())
    }
}

impl<'de> Deserialize<'de> for RecordingPrefilterPatternSettings {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum PatternRepresentation {
            Legacy(String),
            Detailed {
                pattern: String,
                #[serde(default = "default_true")]
                enable: bool,
            },
        }

        match PatternRepresentation::deserialize(deserializer)? {
            PatternRepresentation::Legacy(pattern) => Ok(Self::new(pattern)),
            PatternRepresentation::Detailed { pattern, enable } => Ok(Self { pattern, enable }),
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub struct UiSettings {
    pub request_list: RequestListSettings,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub struct RequestListSettings {
    pub auto_expand: bool,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(default)]
pub struct ProxySettings {
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    pub enable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_preset: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub presets: Vec<ProxyPresetSettings>,
}

impl Default for ProxySettings {
    fn default() -> Self {
        Self {
            enable: true,
            active_preset: None,
            presets: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(default)]
pub struct ProxyPresetSettings {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    #[serde(default, skip_serializing_if = "ProxyMapRemoteSettings::is_default")]
    pub map_remote: ProxyMapRemoteSettings,
    #[serde(default, skip_serializing_if = "ProxyMapLocalSettings::is_default")]
    pub map_local: ProxyMapLocalSettings,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(default)]
pub struct ProxyMapRemoteSettings {
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    pub enable: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<ProxyMapRemoteRule>,
}

impl ProxyMapRemoteSettings {
    fn is_default(settings: &Self) -> bool {
        settings == &Self::default()
    }
}

impl Default for ProxyMapRemoteSettings {
    fn default() -> Self {
        Self {
            enable: true,
            rules: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(default)]
pub struct ProxyMapLocalSettings {
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    pub enable: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<ProxyMapLocalRule>,
}

impl ProxyMapLocalSettings {
    fn is_default(settings: &Self) -> bool {
        settings == &Self::default()
    }
}

impl Default for ProxyMapLocalSettings {
    fn default() -> Self {
        Self {
            enable: true,
            rules: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(default)]
pub struct ProxyMapRemoteRule {
    pub from: String,
    pub to: String,
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    pub enable: bool,
}

impl Default for ProxyMapRemoteRule {
    fn default() -> Self {
        Self {
            from: String::new(),
            to: String::new(),
            enable: true,
        }
    }
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Serialize)]
#[serde(default)]
pub struct ProxyMapLocalRule {
    pub from: String,
    pub to: String,
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    pub enable: bool,
}

impl Default for ProxyMapLocalRule {
    fn default() -> Self {
        Self {
            from: String::new(),
            to: String::new(),
            enable: true,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ConfigMode {
    DefaultOwned,
    ReadOnlyFile,
    Temporary,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, JsonSchema, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PersistenceMode {
    Persistent,
    Ephemeral,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SettingsUiContext {
    pub(crate) config_mode: ConfigMode,
    pub(crate) persistence: PersistenceMode,
}

impl Default for SettingsUiContext {
    fn default() -> Self {
        Self {
            config_mode: ConfigMode::DefaultOwned,
            persistence: PersistenceMode::Persistent,
        }
    }
}

pub(crate) struct SettingsSession {
    settings: Arc<AppSettings>,
    mode: ConfigMode,
    persistence: PersistenceMode,
    source_path: Option<PathBuf>,
    default_lease: Option<DefaultConfigLease>,
    load_diagnostics: Vec<SettingsLoadDiagnostic>,
}

impl SettingsSession {
    pub(crate) fn load(selection: &ConfigSelection) -> anyhow::Result<Self> {
        match selection {
            ConfigSelection::DefaultOwned => {
                let path = default_config_path()?;
                let lease = DefaultConfigLease::acquire(&default_config_lock_path()?)
                    .context("failed to acquire default configuration ownership")?;
                reject_default_config_symlink(&path)?;
                let mut manager = SettingsManager::load_from_path(&path)
                    .context("failed to load default configuration")?;
                let source_path = fs::canonicalize(&path)
                    .context("failed to resolve default configuration path")?;
                Ok(Self {
                    settings: Arc::new(manager.settings().clone()),
                    mode: ConfigMode::DefaultOwned,
                    persistence: PersistenceMode::Persistent,
                    source_path: Some(source_path),
                    default_lease: Some(lease),
                    load_diagnostics: manager.take_load_diagnostics(),
                })
            }
            ConfigSelection::ReadOnlyFile(path) => {
                let source_path = fs::canonicalize(path).with_context(|| {
                    format!("failed to resolve configuration {}", path.display())
                })?;
                let content = fs::read_to_string(&source_path).with_context(|| {
                    format!("failed to read configuration {}", source_path.display())
                })?;
                let (settings, load_diagnostics) = if content.trim().is_empty() {
                    (AppSettings::default(), Vec::new())
                } else {
                    deserialize_settings_tolerantly(&content)?
                };
                Ok(Self {
                    settings: Arc::new(settings),
                    mode: ConfigMode::ReadOnlyFile,
                    persistence: PersistenceMode::Ephemeral,
                    source_path: Some(source_path),
                    default_lease: None,
                    load_diagnostics,
                })
            }
            ConfigSelection::Temporary { port, .. } => {
                let mut settings = AppSettings::default();
                settings.server.port = *port;
                Ok(Self::temporary(settings))
            }
        }
    }

    pub(crate) fn temporary(settings: AppSettings) -> Self {
        Self {
            settings: Arc::new(settings),
            mode: ConfigMode::Temporary,
            persistence: PersistenceMode::Ephemeral,
            source_path: None,
            default_lease: None,
            load_diagnostics: Vec::new(),
        }
    }

    pub(crate) fn snapshot(&self) -> Arc<AppSettings> {
        Arc::clone(&self.settings)
    }

    #[cfg(test)]
    pub(crate) fn commit(&mut self, candidate: AppSettings) -> io::Result<PersistenceMode> {
        if self.persistence == PersistenceMode::Persistent {
            let path = self.source_path.as_deref().ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "persistent settings session has no source path",
                )
            })?;
            write_settings_atomically(path, &candidate)?;
        }
        self.settings = Arc::new(candidate);
        Ok(self.persistence)
    }

    pub(crate) fn source_path(&self) -> Option<&Path> {
        self.source_path.as_deref()
    }

    pub(crate) fn ui_context(&self) -> SettingsUiContext {
        debug_assert_eq!(
            self.default_lease.is_some(),
            self.mode == ConfigMode::DefaultOwned
        );
        SettingsUiContext {
            config_mode: self.mode,
            persistence: self.persistence,
        }
    }

    pub(crate) fn into_runtime_parts(
        self,
    ) -> (
        Arc<AppSettings>,
        SettingsUiContext,
        SettingsCommitter,
        Option<DefaultConfigLease>,
    ) {
        let context = SettingsUiContext {
            config_mode: self.mode,
            persistence: self.persistence,
        };
        let committer = SettingsCommitter {
            persistence: self.persistence,
            source_path: self.source_path,
            #[cfg(test)]
            observer: None,
        };
        (self.settings, context, committer, self.default_lease)
    }

    pub(crate) fn take_load_diagnostics(&mut self) -> Vec<SettingsLoadDiagnostic> {
        std::mem::take(&mut self.load_diagnostics)
    }

    pub(crate) fn certificate_store_dir(&self) -> io::Result<PathBuf> {
        expand_home_path(&self.settings.certificate.store_dir)
    }

    pub(crate) fn certificate_pem_filename(&self) -> &str {
        &self.settings.certificate.pem_filename
    }

    pub(crate) fn recording_settings(&self) -> &RecordingSettings {
        &self.settings.recording
    }
}

#[derive(Clone)]
pub(crate) struct SettingsCommitter {
    persistence: PersistenceMode,
    source_path: Option<PathBuf>,
    #[cfg(test)]
    observer: Option<Arc<dyn SettingsCommitObserver>>,
}

#[cfg(test)]
pub(crate) trait SettingsCommitObserver: Send + Sync {
    fn prepare(&self) -> io::Result<()>;
    fn prepared(&self);
    fn rename(&self) -> io::Result<()>;
    fn committed(&self);
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SettingsCommitPhase {
    PreCommit,
    Committing,
    Committed,
}

pub(crate) struct PreparedSettingsCommit {
    phase: SettingsCommitPhase,
    persistence: PersistenceMode,
    settings: Arc<AppSettings>,
    source_path: Option<PathBuf>,
    temporary: Option<NamedTempFile>,
    #[cfg(test)]
    observer: Option<Arc<dyn SettingsCommitObserver>>,
}

pub(crate) struct CommittedSettings {
    pub(crate) settings: Arc<AppSettings>,
    pub(crate) persistence: PersistenceMode,
    #[cfg(test)]
    pub(crate) persisted_path: Option<PathBuf>,
}

impl SettingsCommitter {
    #[cfg(test)]
    pub(crate) fn persistence(&self) -> PersistenceMode {
        self.persistence
    }

    #[cfg(test)]
    pub(crate) fn for_test(
        persistence: PersistenceMode,
        source_path: Option<PathBuf>,
        observer: Arc<dyn SettingsCommitObserver>,
    ) -> Self {
        Self {
            persistence,
            source_path,
            observer: Some(observer),
        }
    }

    pub(crate) fn prepare(&self, settings: Arc<AppSettings>) -> io::Result<PreparedSettingsCommit> {
        #[cfg(test)]
        if let Some(observer) = &self.observer {
            observer.prepare()?;
        }
        let temporary = if self.persistence == PersistenceMode::Persistent {
            let path = self.source_path.as_deref().ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "persistent settings committer has no source path",
                )
            })?;
            Some(prepare_settings_temporary(path, &settings)?)
        } else {
            None
        };
        #[cfg(test)]
        if let Some(observer) = &self.observer {
            observer.prepared();
        }
        Ok(PreparedSettingsCommit {
            phase: SettingsCommitPhase::PreCommit,
            persistence: self.persistence,
            settings,
            source_path: self.source_path.clone(),
            temporary,
            #[cfg(test)]
            observer: self.observer.clone(),
        })
    }
}

impl PreparedSettingsCommit {
    #[cfg(test)]
    pub(crate) fn phase(&self) -> SettingsCommitPhase {
        self.phase
    }

    #[cfg(test)]
    pub(crate) fn persistence(&self) -> PersistenceMode {
        self.persistence
    }

    pub(crate) fn begin_commit(&mut self) -> io::Result<()> {
        if self.phase != SettingsCommitPhase::PreCommit {
            return Err(io::Error::other("settings commit has already started"));
        }
        self.phase = SettingsCommitPhase::Committing;
        Ok(())
    }

    pub(crate) fn commit_to_completion(&mut self) -> io::Result<CommittedSettings> {
        if self.phase != SettingsCommitPhase::Committing {
            return Err(io::Error::other("settings commit has not started"));
        }
        #[cfg(test)]
        if let Some(observer) = &self.observer {
            observer.rename()?;
        }
        if let Some(temporary) = self.temporary.take() {
            let path = self.source_path.as_deref().ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "prepared persistent commit has no destination",
                )
            })?;
            temporary.persist(path).map_err(|error| error.error)?;
        }
        self.phase = SettingsCommitPhase::Committed;
        #[cfg(test)]
        if let Some(observer) = &self.observer {
            observer.committed();
        }
        Ok(CommittedSettings {
            settings: Arc::clone(&self.settings),
            persistence: self.persistence,
            #[cfg(test)]
            persisted_path: self.source_path.clone(),
        })
    }

    #[cfg(test)]
    pub(crate) fn commit(&mut self) -> io::Result<CommittedSettings> {
        self.begin_commit()?;
        self.commit_to_completion()
    }
}

fn prepare_settings_temporary(path: &Path, settings: &AppSettings) -> io::Result<NamedTempFile> {
    let directory = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(directory)?;
    let mut value = serde_yaml::to_value(settings).map_err(yaml_error)?;
    if let Some(mut previous) = read_yaml_value(path)? {
        let _ = remove_malformed_prefilter_pattern_entries(&mut previous);
        preserve_semantic_noop_entries(&mut value, &previous, settings)?;
    }
    let content = serde_yaml::to_string(&value).map_err(yaml_error)?;
    let mut temporary = NamedTempFile::new_in(directory)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        temporary
            .as_file()
            .set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    temporary.write_all(content.as_bytes())?;
    temporary.as_file().sync_all()?;
    Ok(temporary)
}

pub struct SettingsManager {
    path: PathBuf,
    settings: AppSettings,
    load_diagnostics: Vec<SettingsLoadDiagnostic>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SettingsLoadDiagnostic {
    pub message: String,
}

impl SettingsManager {
    pub fn load_from_path(path: impl AsRef<Path>) -> io::Result<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            crate::private_fs::ensure_directory(parent)?;
        }

        if !path.exists() {
            let manager = Self {
                path,
                settings: AppSettings::default(),
                load_diagnostics: Vec::new(),
            };
            manager.save()?;
            return Ok(manager);
        }

        let mut content = String::new();
        crate::private_fs::open_file(&path, false)?.read_to_string(&mut content)?;
        let (settings, load_diagnostics) = if content.trim().is_empty() {
            (AppSettings::default(), Vec::new())
        } else {
            deserialize_settings_tolerantly(&content)?
        };
        let manager = Self {
            path,
            settings,
            load_diagnostics,
        };
        if manager.load_diagnostics.is_empty() {
            manager.save()?;
        }

        Ok(manager)
    }

    #[cfg(test)]
    pub fn server_port(&self) -> u16 {
        self.settings.server.port
    }

    #[cfg(test)]
    pub fn certificate_store_dir(&self) -> io::Result<PathBuf> {
        expand_home_path(&self.settings.certificate.store_dir)
    }

    #[cfg(test)]
    pub fn certificate_pem_filename(&self) -> &str {
        &self.settings.certificate.pem_filename
    }

    #[cfg(test)]
    pub fn recording_settings(&self) -> &RecordingSettings {
        &self.settings.recording
    }

    #[cfg(test)]
    pub fn ui_settings(&self) -> &UiSettings {
        &self.settings.ui
    }

    pub fn settings(&self) -> &AppSettings {
        &self.settings
    }

    pub(crate) fn take_load_diagnostics(&mut self) -> Vec<SettingsLoadDiagnostic> {
        std::mem::take(&mut self.load_diagnostics)
    }

    #[cfg(test)]
    pub fn proxy_settings(&self) -> Option<&ProxySettings> {
        self.settings.proxy.as_ref()
    }

    #[cfg(test)]
    pub fn update<F>(&mut self, change: F) -> io::Result<()>
    where
        F: FnOnce(&mut AppSettings),
    {
        let mut next = self.settings.clone();
        change(&mut next);
        self.write_settings(&next)?;
        self.settings = next;
        Ok(())
    }

    #[cfg(test)]
    pub fn set_server_port(&mut self, port: u16) -> io::Result<()> {
        self.update(|settings| settings.server.port = port)
    }

    fn save(&self) -> io::Result<()> {
        self.write_settings(&self.settings)
    }

    fn write_settings(&self, settings: &AppSettings) -> io::Result<()> {
        write_settings_atomically(&self.path, settings)
    }
}
fn write_settings_atomically(path: &Path, settings: &AppSettings) -> io::Result<()> {
    let directory = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    crate::private_fs::ensure_directory(directory)?;

    let mut value = serde_yaml::to_value(settings).map_err(yaml_error)?;
    if let Some(mut previous) = read_yaml_value(path)? {
        let _ = remove_malformed_prefilter_pattern_entries(&mut previous);
        preserve_semantic_noop_entries(&mut value, &previous, settings)?;
    }
    let content = serde_yaml::to_string(&value).map_err(yaml_error)?;

    crate::private_fs::write_file(path, content.as_bytes())
}

#[cfg(feature = "benchmark")]
pub(crate) fn benchmark_atomic_settings_write(
    path: &Path,
    settings: &AppSettings,
) -> io::Result<()> {
    write_settings_atomically(path, settings)
}

fn default_config_path() -> io::Result<PathBuf> {
    Ok(home_dir()?.join(".fluxcope/config.yml"))
}
fn default_config_lock_path() -> io::Result<PathBuf> {
    Ok(home_dir()?.join(".wirelens/run/default-config.lock"))
}

fn reject_default_config_symlink(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "default configuration path must not be a symlink",
        )),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn expand_home_path(path: &str) -> io::Result<PathBuf> {
    if path == "~" {
        return home_dir();
    }

    if let Some(rest) = path.strip_prefix("~/") {
        return Ok(home_dir()?.join(rest));
    }

    Ok(PathBuf::from(path))
}

fn home_dir() -> io::Result<PathBuf> {
    let home = env::var_os("HOME").ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "HOME environment variable is not set",
        )
    })?;

    Ok(PathBuf::from(home))
}

fn yaml_error(error: serde_yaml::Error) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error)
}

fn deserialize_settings_tolerantly(
    content: &str,
) -> io::Result<(AppSettings, Vec<SettingsLoadDiagnostic>)> {
    let mut sanitized: serde_yaml::Value = serde_yaml::from_str(content).map_err(yaml_error)?;
    let mut diagnostics = remove_malformed_prefilter_pattern_entries(&mut sanitized);
    match serde_yaml::from_value(sanitized.clone()) {
        Ok(settings) => Ok((settings, diagnostics)),
        Err(original_error) => {
            let Some(error) = replace_malformed_prefilter(&mut sanitized)? else {
                return Err(yaml_error(original_error));
            };
            let settings = serde_yaml::from_value(sanitized).map_err(yaml_error)?;
            diagnostics.push(SettingsLoadDiagnostic {
                message: format!("recording.prefilter is malformed and has been disabled: {error}"),
            });
            Ok((settings, diagnostics))
        }
    }
}

fn remove_malformed_prefilter_pattern_entries(
    value: &mut serde_yaml::Value,
) -> Vec<SettingsLoadDiagnostic> {
    let key = |name: &str| serde_yaml::Value::String(name.to_string());
    let Some(patterns) = value
        .as_mapping_mut()
        .and_then(|root| root.get_mut(key("recording")))
        .and_then(serde_yaml::Value::as_mapping_mut)
        .and_then(|recording| recording.get_mut(key("prefilter")))
        .and_then(serde_yaml::Value::as_mapping_mut)
        .and_then(|prefilter| prefilter.get_mut(key("include_url_patterns")))
        .and_then(serde_yaml::Value::as_sequence_mut)
    else {
        return Vec::new();
    };

    let mut diagnostics = Vec::new();
    let mut valid_patterns = Vec::with_capacity(patterns.len());
    for (index, value) in std::mem::take(patterns).into_iter().enumerate() {
        match serde_yaml::from_value::<RecordingPrefilterPatternSettings>(value.clone()) {
            Ok(_) => valid_patterns.push(value),
            Err(error) => diagnostics.push(SettingsLoadDiagnostic {
                message: format!(
                    "recording.prefilter.include_url_patterns[{index}] ignored malformed pattern entry: {error}"
                ),
            }),
        }
    }
    *patterns = valid_patterns;
    diagnostics
}

fn replace_malformed_prefilter(value: &mut serde_yaml::Value) -> io::Result<Option<String>> {
    let key = |name: &str| serde_yaml::Value::String(name.to_string());
    let Some(prefilter) = value
        .as_mapping_mut()
        .and_then(|root| root.get_mut(key("recording")))
        .and_then(serde_yaml::Value::as_mapping_mut)
        .and_then(|recording| recording.get_mut(key("prefilter")))
    else {
        return Ok(None);
    };
    let Err(error) = serde_yaml::from_value::<RecordingPrefilterSettings>(prefilter.clone()) else {
        return Ok(None);
    };
    *prefilter =
        serde_yaml::to_value(RecordingPrefilterSettings::disabled()).map_err(yaml_error)?;
    Ok(Some(error.to_string()))
}

fn default_true() -> bool {
    true
}

fn is_true(value: &bool) -> bool {
    *value
}

fn read_yaml_value(path: &Path) -> io::Result<Option<serde_yaml::Value>> {
    match crate::private_fs::open_file(path, false) {
        Ok(mut file) => {
            let mut content = String::new();
            file.read_to_string(&mut content)?;
            if content.trim().is_empty() {
                Ok(None)
            } else {
                serde_yaml::from_str(&content).map(Some).map_err(yaml_error)
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn preserve_semantic_noop_entries(
    value: &mut serde_yaml::Value,
    previous: &serde_yaml::Value,
    settings: &AppSettings,
) -> io::Result<()> {
    // Preserve explicit default entries without teaching each setting type how to track presence.
    let mut missing = Vec::new();
    collect_missing_entries(Some(value), previous, &mut Vec::new(), &mut missing);

    let mut batch = value.clone();
    let inserted_all = missing
        .iter()
        .all(|entry| insert_config_entry(&mut batch, &entry.path, entry.value.clone()));
    if inserted_all
        && let Ok(batch_settings) = serde_yaml::from_value::<AppSettings>(batch.clone())
        && batch_settings == *settings
    {
        *value = batch;
        reorder_mappings_like_previous(value, previous);
        return Ok(());
    }

    for entry in missing {
        let mut candidate = value.clone();
        if !insert_config_entry(&mut candidate, &entry.path, entry.value) {
            continue;
        }

        let Ok(candidate_settings) = serde_yaml::from_value::<AppSettings>(candidate.clone())
        else {
            continue;
        };
        if candidate_settings == *settings {
            *value = candidate;
        }
    }
    reorder_mappings_like_previous(value, previous);

    Ok(())
}

pub(crate) fn benchmark_yaml_semantic_preservation(rule_count: usize) -> usize {
    let rules = (0..rule_count)
        .map(|index| ProxyMapRemoteRule {
            from: format!("https://source-{index}.example"),
            to: format!("https://target-{index}.example"),
            enable: true,
        })
        .collect();
    let settings = AppSettings {
        proxy: Some(ProxySettings {
            enable: true,
            active_preset: Some("benchmark".to_string()),
            presets: vec![ProxyPresetSettings {
                name: "benchmark".to_string(),
                map_remote: ProxyMapRemoteSettings {
                    enable: true,
                    rules,
                },
                map_local: ProxyMapLocalSettings::default(),
            }],
        }),
        ..AppSettings::default()
    };
    let mut value = serde_yaml::to_value(&settings).expect("benchmark settings should serialize");
    let mut previous = value.clone();
    add_explicit_default_enable_fields(&mut previous);
    preserve_semantic_noop_entries(&mut value, &previous, &settings)
        .expect("benchmark preservation should remain semantically valid");
    serde_yaml::to_string(&value)
        .expect("benchmark settings should serialize to text")
        .len()
}

fn add_explicit_default_enable_fields(value: &mut serde_yaml::Value) {
    let key = |name: &str| serde_yaml::Value::String(name.to_string());
    let Some(proxy) = value
        .as_mapping_mut()
        .and_then(|root| root.get_mut(key("proxy")))
        .and_then(serde_yaml::Value::as_mapping_mut)
    else {
        return;
    };
    proxy.insert(key("enable"), serde_yaml::Value::Bool(true));
    let Some(presets) = proxy
        .get_mut(key("presets"))
        .and_then(serde_yaml::Value::as_sequence_mut)
    else {
        return;
    };
    for preset in presets {
        let Some(remote) = preset
            .as_mapping_mut()
            .and_then(|preset| preset.get_mut(key("map_remote")))
            .and_then(serde_yaml::Value::as_mapping_mut)
        else {
            continue;
        };
        remote.insert(key("enable"), serde_yaml::Value::Bool(true));
        if let Some(rules) = remote
            .get_mut(key("rules"))
            .and_then(serde_yaml::Value::as_sequence_mut)
        {
            for rule in rules {
                if let Some(rule) = rule.as_mapping_mut() {
                    rule.insert(key("enable"), serde_yaml::Value::Bool(true));
                }
            }
        }
    }
}

fn collect_missing_entries(
    current: Option<&serde_yaml::Value>,
    previous: &serde_yaml::Value,
    path: &mut Vec<ConfigPathSegment>,
    missing: &mut Vec<MissingConfigEntry>,
) {
    let Some(current) = current else {
        missing.push(MissingConfigEntry {
            path: path.clone(),
            value: previous.clone(),
        });
        return;
    };

    match (current, previous) {
        (serde_yaml::Value::Mapping(current), serde_yaml::Value::Mapping(previous)) => {
            for (key, previous_child) in previous {
                path.push(ConfigPathSegment::Key(key.clone()));
                collect_missing_entries(current.get(key), previous_child, path, missing);
                path.pop();
            }
        }
        (serde_yaml::Value::Sequence(current), serde_yaml::Value::Sequence(previous)) => {
            for (index, previous_child) in previous.iter().enumerate().take(current.len()) {
                path.push(ConfigPathSegment::Index(index));
                collect_missing_entries(Some(&current[index]), previous_child, path, missing);
                path.pop();
            }
        }
        _ => {}
    }
}

fn insert_config_entry(
    value: &mut serde_yaml::Value,
    path: &[ConfigPathSegment],
    entry: serde_yaml::Value,
) -> bool {
    if path.is_empty() {
        *value = entry;
        return true;
    }

    let mut current = value;
    for segment in &path[..path.len() - 1] {
        match segment {
            ConfigPathSegment::Key(key) => {
                let serde_yaml::Value::Mapping(mapping) = current else {
                    return false;
                };
                let Some(next) = mapping.get_mut(key) else {
                    return false;
                };
                current = next;
            }
            ConfigPathSegment::Index(index) => {
                let serde_yaml::Value::Sequence(sequence) = current else {
                    return false;
                };
                let Some(next) = sequence.get_mut(*index) else {
                    return false;
                };
                current = next;
            }
        }
    }

    match path.last().expect("path is not empty") {
        ConfigPathSegment::Key(key) => {
            let serde_yaml::Value::Mapping(mapping) = current else {
                return false;
            };
            mapping.insert(key.clone(), entry);
            true
        }
        ConfigPathSegment::Index(index) => {
            let serde_yaml::Value::Sequence(sequence) = current else {
                return false;
            };
            if *index > sequence.len() {
                return false;
            }
            sequence.insert(*index, entry);
            true
        }
    }
}

fn reorder_mappings_like_previous(value: &mut serde_yaml::Value, previous: &serde_yaml::Value) {
    match (value, previous) {
        (serde_yaml::Value::Mapping(current), serde_yaml::Value::Mapping(previous)) => {
            let mut ordered = serde_yaml::Mapping::new();

            for (key, previous_child) in previous {
                let Some(mut current_child) = current.remove(key) else {
                    continue;
                };
                reorder_mappings_like_previous(&mut current_child, previous_child);
                ordered.insert(key.clone(), current_child);
            }

            for (key, child) in std::mem::take(current) {
                ordered.insert(key, child);
            }
            *current = ordered;
        }
        (serde_yaml::Value::Sequence(current), serde_yaml::Value::Sequence(previous)) => {
            for (current_child, previous_child) in current.iter_mut().zip(previous) {
                reorder_mappings_like_previous(current_child, previous_child);
            }
        }
        _ => {}
    }
}

#[derive(Clone)]
struct MissingConfigEntry {
    path: Vec<ConfigPathSegment>,
    value: serde_yaml::Value,
}

#[derive(Clone)]
enum ConfigPathSegment {
    Key(serde_yaml::Value),
    Index(usize),
}

#[cfg(test)]
mod tests;
