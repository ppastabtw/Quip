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
    /// The burst window in words. The playground derives its chunk boundary
    /// and character backstop from this value.
    pub window_words: usize,
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
            window_words: 10,
        }
    }
}

impl AppSettings {
    /// Context can be needed either for the current model request or for a
    /// later confirmed learning label. The composition engine still enforces
    /// `window_context` before any captured snippet reaches inference.
    pub fn should_capture_context(&self) -> bool {
        self.window_context || !self.learning_paused
    }

    pub fn load(data_dir: &Path) -> Self {
        let path = data_dir.join("settings.json");
        let mut settings: Self = std::fs::read_to_string(&path)
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default();
        if settings.window_words == 5 {
            settings.window_words = 10;
        }
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

#[cfg(test)]
mod tests {
    use super::AppSettings;

    #[test]
    fn context_capture_policy_covers_model_and_learning_uses() {
        let mut settings = AppSettings::default();
        settings.window_context = true;
        settings.learning_paused = true;
        assert!(settings.should_capture_context());

        settings.window_context = false;
        settings.learning_paused = false;
        assert!(settings.should_capture_context());

        settings.window_context = false;
        settings.learning_paused = true;
        assert!(!settings.should_capture_context());
    }
}
