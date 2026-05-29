use serde::{Deserialize, Serialize};
use std::{
    env, fs, io,
    path::{Path, PathBuf},
};

const DEFAULT_PROXY_PORT: u16 = 8989;
const DEFAULT_CERTIFICATE_STORE_DIR: &str = "~/.wirelens/certificate/";
const DEFAULT_CERTIFICATE_PEM_FILENAME: &str = "wirelens-ca.pem";

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct AppSettings {
    pub server: ServerSettings,
    pub certificate: CertificateSettings,
    pub ui: UiSettings,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            server: ServerSettings::default(),
            certificate: CertificateSettings::default(),
            ui: UiSettings::default(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
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

#[derive(Clone, Debug, Deserialize, Serialize)]
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

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct UiSettings {
    pub request_list: RequestListSettings,
}

impl Default for UiSettings {
    fn default() -> Self {
        Self {
            request_list: RequestListSettings::default(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct RequestListSettings {
    pub auto_expand: bool,
}

impl Default for RequestListSettings {
    fn default() -> Self {
        Self { auto_expand: false }
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

    pub fn ui_settings(&self) -> &UiSettings {
        &self.settings.ui
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

        let content = serde_yaml::to_string(settings).map_err(yaml_error)?;
        fs::write(&self.path, content)
    }
}

fn default_config_path() -> io::Result<PathBuf> {
    Ok(home_dir()?.join(".wirelens/config.yml"))
}

fn expand_home_path(path: &str) -> io::Result<PathBuf> {
    if path == "~" {
        return Ok(home_dir()?);
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

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
        assert!(!saved.ui.request_list.auto_expand);

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
        assert!(!manager.ui_settings().request_list.auto_expand);

        let saved: AppSettings =
            serde_yaml::from_str(&fs::read_to_string(&path)?).map_err(yaml_error)?;
        assert_eq!(9010, saved.server.port);
        assert_eq!(DEFAULT_CERTIFICATE_STORE_DIR, saved.certificate.store_dir);
        assert_eq!(
            DEFAULT_CERTIFICATE_PEM_FILENAME,
            saved.certificate.pem_filename
        );
        assert!(!saved.ui.request_list.auto_expand);

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
    fn default_config_path_uses_wirelens_home_dir() -> io::Result<()> {
        assert_eq!(
            home_dir()?.join(".wirelens/config.yml"),
            default_config_path()?
        );
        Ok(())
    }

    fn temp_config_path() -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();

        env::temp_dir().join(format!("wirelens-settings-{nanos}/config.yml"))
    }
}
