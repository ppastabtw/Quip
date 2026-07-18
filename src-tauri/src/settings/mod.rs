//! Workstream 4: persisted app settings — enabled state, window-context
//! toggle, learning pause, active profile, backend mode, and model variant.
//! Stored as JSON in the app data dir; the tray and settings window both
//! read and write through the engine.

use quip_contracts::ModelVariant;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackendMode {
    Fixture,
    Live,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppSettings {
    pub enabled: bool,
    pub window_context: bool,
    pub learning_paused: bool,
    pub active_profile: String,
    pub backend_mode: BackendMode,
    pub model_variant: ModelVariant,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            window_context: true,
            learning_paused: false,
            active_profile: "profile_a".to_string(),
            backend_mode: BackendMode::Fixture,
            model_variant: ModelVariant::GlobalPlusPersonal,
        }
    }
}

impl AppSettings {
    pub fn load(data_dir: &Path) -> Self {
        let path = data_dir.join("settings.json");
        let mut settings: Self = std::fs::read_to_string(&path)
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default();
        settings.apply_environment_overrides();
        settings
    }

    pub fn save(&self, data_dir: &Path) {
        let path = data_dir.join("settings.json");
        if let Err(e) = std::fs::create_dir_all(data_dir)
            .and_then(|_| std::fs::write(&path, serde_json::to_string_pretty(self).unwrap()))
        {
            tracing::warn!(error = %e, path = %path.display(), "failed to persist settings");
        }
    }

    fn apply_environment_overrides(&mut self) {
        match std::env::var("QUIP_BACKEND_MODE").as_deref() {
            Ok("fixture") => self.backend_mode = BackendMode::Fixture,
            Ok("live") => self.backend_mode = BackendMode::Live,
            _ => {}
        }
        match std::env::var("QUIP_MODEL_VARIANT").as_deref() {
            Ok("base") => self.model_variant = ModelVariant::Base,
            Ok("global") => self.model_variant = ModelVariant::Global,
            Ok("global_plus_personal") => self.model_variant = ModelVariant::GlobalPlusPersonal,
            _ => {}
        }
    }
}
