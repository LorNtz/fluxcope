use std::ops::{Deref, DerefMut};

use crate::settings::AppSettings;

/// Owns the persisted baseline and the one editable settings copy.
///
/// Navigation and proxy-rule cursors live in `SettingsPopup`; they borrow this
/// draft for every operation and never retain a second settings collection.
pub(super) struct SettingsDraft {
    original: AppSettings,
    current: AppSettings,
    error: Option<String>,
}

impl SettingsDraft {
    pub fn new(settings: AppSettings) -> Self {
        Self {
            original: settings.clone(),
            current: settings,
            error: None,
        }
    }

    pub fn replace(&mut self, settings: AppSettings) {
        *self = Self::new(settings);
    }

    pub fn reset(&mut self) {
        self.replace(AppSettings::default());
    }

    pub fn is_dirty(&self) -> bool {
        self.current != self.original
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub fn set_error(&mut self, error: impl Into<String>) {
        self.error = Some(error.into());
    }

    pub fn clear_error(&mut self) {
        self.error = None;
    }

    pub fn snapshot(&self) -> AppSettings {
        self.current.clone()
    }
}

impl Deref for SettingsDraft {
    type Target = AppSettings;

    fn deref(&self) -> &Self::Target {
        &self.current
    }
}

impl DerefMut for SettingsDraft {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.current
    }
}
