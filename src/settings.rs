use serde::{Deserialize, Serialize};
use std::{
    env, fs, io,
    path::{Path, PathBuf},
};

const DEFAULT_PROXY_PORT: u16 = 8989;
const DEFAULT_CERTIFICATE_STORE_DIR: &str = "~/.wirelens/certificate/";
const DEFAULT_CERTIFICATE_PEM_FILENAME: &str = "wirelens-ca.pem";

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub struct AppSettings {
    pub server: ServerSettings,
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
}

impl Default for RecordingSettings {
    fn default() -> Self {
        Self {
            start_record_on_launch: true,
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

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
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

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub struct ProxyPresetSettings {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    #[serde(default, skip_serializing_if = "ProxyMapRemoteSettings::is_default")]
    pub map_remote: ProxyMapRemoteSettings,
    #[serde(default, skip_serializing_if = "ProxyMapLocalSettings::is_default")]
    pub map_local: ProxyMapLocalSettings,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
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

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
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

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
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

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
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

pub struct SettingsManager {
    path: PathBuf,
    settings: AppSettings,
}

impl SettingsManager {
    pub fn load() -> io::Result<Self> {
        Self::load_from_path(default_config_path()?)
    }

    pub fn load_from_path(path: impl AsRef<Path>) -> io::Result<Self> {
        let path = path.as_ref().to_path_buf();

        if !path.exists() {
            let manager = Self {
                path,
                settings: AppSettings::default(),
            };
            manager.save()?;
            return Ok(manager);
        }

        let content = fs::read_to_string(&path)?;
        let settings = if content.trim().is_empty() {
            AppSettings::default()
        } else {
            serde_yaml::from_str(&content).map_err(yaml_error)?
        };
        let manager = Self { path, settings };
        manager.save()?;

        Ok(manager)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn server_port(&self) -> u16 {
        self.settings.server.port
    }

    pub fn certificate_store_dir(&self) -> io::Result<PathBuf> {
        expand_home_path(&self.settings.certificate.store_dir)
    }

    pub fn certificate_pem_filename(&self) -> &str {
        &self.settings.certificate.pem_filename
    }

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

    pub fn proxy_settings(&self) -> Option<&ProxySettings> {
        self.settings.proxy.as_ref()
    }

    #[allow(dead_code)]
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

    #[allow(dead_code)]
    pub fn set_server_port(&mut self, port: u16) -> io::Result<()> {
        self.update(|settings| {
            settings.server.port = port;
        })
    }

    fn save(&self) -> io::Result<()> {
        self.write_settings(&self.settings)
    }

    fn write_settings(&self, settings: &AppSettings) -> io::Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }

        let mut value = serde_yaml::to_value(settings).map_err(yaml_error)?;
        if let Some(previous) = read_yaml_value(&self.path)? {
            preserve_semantic_noop_entries(&mut value, &previous, settings)?;
        }

        let content = serde_yaml::to_string(&value).map_err(yaml_error)?;
        fs::write(&self.path, content)
    }
}

fn default_config_path() -> io::Result<PathBuf> {
    Ok(home_dir()?.join(".wirelens/config.yml"))
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

fn default_true() -> bool {
    true
}

fn is_true(value: &bool) -> bool {
    *value
}

fn read_yaml_value(path: &Path) -> io::Result<Option<serde_yaml::Value>> {
    match fs::read_to_string(path) {
        Ok(content) if content.trim().is_empty() => Ok(None),
        Ok(content) => serde_yaml::from_str(&content).map(Some).map_err(yaml_error),
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

    for entry in missing {
        let mut candidate = value.clone();
        if !insert_config_entry(&mut candidate, &entry.path, entry.value) {
            continue;
        }

        let candidate_settings: AppSettings =
            serde_yaml::from_value(candidate.clone()).map_err(yaml_error)?;
        if candidate_settings == *settings {
            *value = candidate;
        }
    }
    reorder_mappings_like_previous(value, previous);

    Ok(())
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
mod tests {
    use super::*;

    #[test]
    fn creates_default_config_when_missing() -> io::Result<()> {
        let path = temp_config_path();
        let _ = fs::remove_file(&path);

        let manager = SettingsManager::load_from_path(&path)?;

        assert_eq!(DEFAULT_PROXY_PORT, manager.server_port());
        assert!(path.exists());

        let saved: AppSettings =
            serde_yaml::from_str(&fs::read_to_string(&path)?).map_err(yaml_error)?;
        assert_eq!(DEFAULT_PROXY_PORT, saved.server.port);
        assert_eq!(DEFAULT_CERTIFICATE_STORE_DIR, saved.certificate.store_dir);
        assert_eq!(
            DEFAULT_CERTIFICATE_PEM_FILENAME,
            saved.certificate.pem_filename
        );
        assert!(manager.recording_settings().start_record_on_launch);
        assert!(saved.recording.start_record_on_launch);
        assert!(!saved.ui.request_list.auto_expand);
        assert!(saved.proxy.is_none());
        assert!(!fs::read_to_string(&path)?.contains("proxy:"));

        let _ = fs::remove_file(path);
        Ok(())
    }

    #[test]
    fn reads_existing_config_and_updates_file() -> io::Result<()> {
        let path = temp_config_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&path, "server:\n  port: 9000\n")?;

        let mut manager = SettingsManager::load_from_path(&path)?;
        assert_eq!(9000, manager.server_port());

        manager.set_server_port(9010)?;

        assert_eq!(9010, manager.server_port());
        assert_eq!(
            home_dir()?.join(".wirelens/certificate/"),
            manager.certificate_store_dir()?
        );
        assert_eq!(
            DEFAULT_CERTIFICATE_PEM_FILENAME,
            manager.certificate_pem_filename()
        );
        assert!(manager.recording_settings().start_record_on_launch);
        assert!(!manager.ui_settings().request_list.auto_expand);

        let saved: AppSettings =
            serde_yaml::from_str(&fs::read_to_string(&path)?).map_err(yaml_error)?;
        assert_eq!(9010, saved.server.port);
        assert_eq!(DEFAULT_CERTIFICATE_STORE_DIR, saved.certificate.store_dir);
        assert_eq!(
            DEFAULT_CERTIFICATE_PEM_FILENAME,
            saved.certificate.pem_filename
        );
        assert!(saved.recording.start_record_on_launch);
        assert!(!saved.ui.request_list.auto_expand);
        assert!(saved.proxy.is_none());
        assert!(!fs::read_to_string(&path)?.contains("proxy:"));

        let _ = fs::remove_file(path);
        Ok(())
    }

    #[test]
    fn exposes_loaded_settings_snapshot() -> io::Result<()> {
        let path = temp_config_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&path, "server:\n  port: 9012\n")?;

        let manager = SettingsManager::load_from_path(&path)?;

        assert_eq!(9012, manager.settings().server.port);

        let _ = fs::remove_file(path);
        Ok(())
    }

    #[test]
    fn reads_request_list_auto_expand_setting() -> io::Result<()> {
        let path = temp_config_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&path, "ui:\n  request_list:\n    auto_expand: true\n")?;

        let manager = SettingsManager::load_from_path(&path)?;

        assert!(manager.ui_settings().request_list.auto_expand);

        let saved: AppSettings =
            serde_yaml::from_str(&fs::read_to_string(&path)?).map_err(yaml_error)?;
        assert!(saved.ui.request_list.auto_expand);

        let _ = fs::remove_file(path);
        Ok(())
    }

    #[test]
    fn reads_start_record_on_launch_setting() -> io::Result<()> {
        let path = temp_config_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&path, "recording:\n  start_record_on_launch: false\n")?;

        let manager = SettingsManager::load_from_path(&path)?;

        assert!(!manager.recording_settings().start_record_on_launch);

        let saved: AppSettings =
            serde_yaml::from_str(&fs::read_to_string(&path)?).map_err(yaml_error)?;
        assert!(!saved.recording.start_record_on_launch);

        let _ = fs::remove_file(path);
        Ok(())
    }

    #[test]
    fn reads_optional_proxy_settings_without_filling_empty_sections() -> io::Result<()> {
        let path = temp_config_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(
            &path,
            r#"
proxy:
  active_preset: dev
  presets:
    - name: dev
      map_remote:
        rules:
          - from: "https://a.com"
            to: "http://b.test.com"
"#,
        )?;

        let manager = SettingsManager::load_from_path(&path)?;
        let proxy = manager
            .proxy_settings()
            .expect("proxy settings should deserialize");

        assert!(proxy.enable);
        assert_eq!(proxy.active_preset.as_deref(), Some("dev"));
        assert_eq!(proxy.presets.len(), 1);
        assert!(proxy.presets[0].map_remote.enable);
        assert_eq!(proxy.presets[0].map_remote.rules.len(), 1);
        assert!(proxy.presets[0].map_remote.rules[0].enable);
        assert!(proxy.presets[0].map_local.rules.is_empty());

        let saved = yaml_from_path(&path)?;
        let proxy = mapping_entry(&saved, "proxy").expect("proxy should be serialized");
        assert!(mapping_entry(proxy, "enable").is_none());
        let presets = sequence_entry(proxy, "presets").expect("presets should be serialized");
        let preset = presets.first().expect("preset should be serialized");
        let map_remote =
            mapping_entry(preset, "map_remote").expect("map_remote should be serialized");
        assert!(mapping_entry(map_remote, "enable").is_none());
        let rules = sequence_entry(map_remote, "rules").expect("rules should be serialized");
        assert!(mapping_entry(&rules[0], "enable").is_none());
        assert!(mapping_entry(preset, "map_local").is_none());

        let _ = fs::remove_file(path);
        Ok(())
    }

    #[test]
    fn preserves_explicit_default_true_proxy_enable_fields() -> io::Result<()> {
        let path = temp_config_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(
            &path,
            r#"
proxy:
  enable: true
  active_preset: dev
  presets:
    - name: dev
      map_remote:
        enable: true
        rules:
          - from: "https://a.com"
            to: "http://b.test.com"
            enable: true
      map_local:
        enable: true
        rules:
          - from: "http://b.test.com"
            to: "~/api.json"
            enable: true
"#,
        )?;

        SettingsManager::load_from_path(&path)?;

        let saved = yaml_from_path(&path)?;
        let proxy = mapping_entry(&saved, "proxy").expect("proxy should be serialized");
        assert_eq!(bool_entry(proxy, "enable"), Some(true));
        let presets = sequence_entry(proxy, "presets").expect("presets should be serialized");
        let preset = presets.first().expect("preset should be serialized");
        let map_remote =
            mapping_entry(preset, "map_remote").expect("map_remote should be serialized");
        assert_eq!(bool_entry(map_remote, "enable"), Some(true));
        let remote_rules =
            sequence_entry(map_remote, "rules").expect("remote rules should be serialized");
        assert_eq!(bool_entry(&remote_rules[0], "enable"), Some(true));
        let map_local = mapping_entry(preset, "map_local").expect("map_local should be serialized");
        assert_eq!(mapping_keys(map_local), vec!["enable", "rules"]);
        assert_eq!(bool_entry(map_local, "enable"), Some(true));
        let local_rules =
            sequence_entry(map_local, "rules").expect("local rules should be serialized");
        assert_eq!(bool_entry(&local_rules[0], "enable"), Some(true));

        let _ = fs::remove_file(path);
        Ok(())
    }

    #[test]
    fn preserves_existing_mapping_order_at_all_config_levels() -> io::Result<()> {
        let path = temp_config_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(
            &path,
            r#"
proxy:
  presets:
    - map_local:
        enable: true
        rules:
          - to: "~/api.json"
            enable: true
            from: "http://b.test.com"
      map_remote:
        rules:
          - enable: true
            to: "http://b.test.com"
            from: "https://a.com"
        enable: true
      name: dev
  active_preset: dev
  enable: true
server:
  port: 9000
"#,
        )?;

        SettingsManager::load_from_path(&path)?;

        let saved = yaml_from_path(&path)?;
        let root_keys = mapping_keys(&saved);
        assert!(root_keys.starts_with(&["proxy", "server"]));
        assert!(root_keys.contains(&"certificate"));
        assert!(root_keys.contains(&"recording"));
        assert!(root_keys.contains(&"ui"));
        let proxy = mapping_entry(&saved, "proxy").expect("proxy should be serialized");
        assert_eq!(
            mapping_keys(proxy),
            vec!["presets", "active_preset", "enable"]
        );
        let presets = sequence_entry(proxy, "presets").expect("presets should be serialized");
        let preset = presets.first().expect("preset should be serialized");
        assert_eq!(
            mapping_keys(preset),
            vec!["map_local", "map_remote", "name"]
        );

        let map_local = mapping_entry(preset, "map_local").expect("map_local should be serialized");
        assert_eq!(mapping_keys(map_local), vec!["enable", "rules"]);
        let local_rules = sequence_entry(map_local, "rules").expect("local rule should exist");
        assert_eq!(mapping_keys(&local_rules[0]), vec!["to", "enable", "from"]);

        let map_remote =
            mapping_entry(preset, "map_remote").expect("map_remote should be serialized");
        assert_eq!(mapping_keys(map_remote), vec!["rules", "enable"]);
        let remote_rules = sequence_entry(map_remote, "rules").expect("remote rule should exist");
        assert_eq!(mapping_keys(&remote_rules[0]), vec!["enable", "to", "from"]);

        let _ = fs::remove_file(path);
        Ok(())
    }

    #[test]
    fn preserves_present_default_proxy_fields_across_value_types() -> io::Result<()> {
        let path = temp_config_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(
            &path,
            r#"
proxy:
  enable: true
  active_preset: null
  presets:
    - name: ""
      map_remote:
        enable: true
        rules: []
      map_local: {}
"#,
        )?;

        SettingsManager::load_from_path(&path)?;

        let saved = yaml_from_path(&path)?;
        let proxy = mapping_entry(&saved, "proxy").expect("proxy should be serialized");
        assert_eq!(bool_entry(proxy, "enable"), Some(true));
        assert!(matches!(
            mapping_entry(proxy, "active_preset"),
            Some(serde_yaml::Value::Null)
        ));
        let presets = sequence_entry(proxy, "presets").expect("presets should be serialized");
        let preset = presets.first().expect("preset should be serialized");
        assert_eq!(string_entry(preset, "name"), Some(""));
        let map_remote =
            mapping_entry(preset, "map_remote").expect("map_remote should be serialized");
        assert_eq!(bool_entry(map_remote, "enable"), Some(true));
        assert_eq!(sequence_entry(map_remote, "rules").map(Vec::len), Some(0));
        let map_local = mapping_entry(preset, "map_local").expect("map_local should be preserved");
        assert!(matches!(map_local, serde_yaml::Value::Mapping(mapping) if mapping.is_empty()));

        let _ = fs::remove_file(path);
        Ok(())
    }

    #[test]
    fn does_not_restore_previous_config_that_changes_settings() -> io::Result<()> {
        let path = temp_config_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(
            &path,
            r#"
proxy:
  active_preset: dev
  presets:
    - name: dev
"#,
        )?;

        let mut manager = SettingsManager::load_from_path(&path)?;
        manager.update(|settings| {
            settings.proxy = None;
        })?;

        let saved = yaml_from_path(&path)?;
        assert!(mapping_entry(&saved, "proxy").is_none());

        let _ = fs::remove_file(path);
        Ok(())
    }

    #[test]
    fn default_config_path_uses_wirelens_home_dir() -> io::Result<()> {
        assert_eq!(
            home_dir()?.join(".wirelens/config.yml"),
            default_config_path()?
        );
        Ok(())
    }

    fn temp_config_path() -> PathBuf {
        env::temp_dir().join(format!(
            "wirelens-settings-{}/config.yml",
            uuid::Uuid::new_v4()
        ))
    }

    fn yaml_from_path(path: &Path) -> io::Result<serde_yaml::Value> {
        serde_yaml::from_str(&fs::read_to_string(path)?).map_err(yaml_error)
    }

    fn mapping_entry<'a>(value: &'a serde_yaml::Value, key: &str) -> Option<&'a serde_yaml::Value> {
        let serde_yaml::Value::Mapping(mapping) = value else {
            return None;
        };
        mapping.get(key)
    }

    fn sequence_entry<'a>(
        value: &'a serde_yaml::Value,
        key: &str,
    ) -> Option<&'a Vec<serde_yaml::Value>> {
        let serde_yaml::Value::Sequence(sequence) = mapping_entry(value, key)? else {
            return None;
        };
        Some(sequence)
    }

    fn bool_entry(value: &serde_yaml::Value, key: &str) -> Option<bool> {
        let serde_yaml::Value::Bool(field) = mapping_entry(value, key)? else {
            return None;
        };
        Some(*field)
    }

    fn string_entry<'a>(value: &'a serde_yaml::Value, key: &str) -> Option<&'a str> {
        let serde_yaml::Value::String(field) = mapping_entry(value, key)? else {
            return None;
        };
        Some(field)
    }

    fn mapping_keys(value: &serde_yaml::Value) -> Vec<&str> {
        let serde_yaml::Value::Mapping(mapping) = value else {
            return Vec::new();
        };
        mapping
            .keys()
            .filter_map(|key| match key {
                serde_yaml::Value::String(key) => Some(key.as_str()),
                _ => None,
            })
            .collect()
    }
}
