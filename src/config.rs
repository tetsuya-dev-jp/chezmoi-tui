use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub config_version: u32,
    pub theme: String,
    pub keymap: String,
    pub show_only_changed: bool,
    pub require_two_step_confirmation: bool,
    pub unmanaged_exclude_paths: Vec<String>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            config_version: 1,
            theme: "default".to_string(),
            keymap: "vim".to_string(),
            show_only_changed: false,
            require_two_step_confirmation: true,
            unmanaged_exclude_paths: Vec::new(),
        }
    }
}

impl AppConfig {
    pub fn load_or_default() -> Result<Self> {
        let path = config_path()?;
        if !path.exists() {
            return Ok(Self::default());
        }

        let raw = fs::read_to_string(&path)
            .with_context(|| format!("failed to read config: {}", path.display()))?;

        let parsed = toml::from_str::<AppConfig>(&raw)
            .with_context(|| format!("failed to parse config: {}", path.display()))?;

        Ok(parsed)
    }

    pub fn save(&self) -> Result<PathBuf> {
        let path = config_path()?;
        ensure_parent_dir(&path)?;

        let body = toml::to_string_pretty(self).context("failed to serialize config")?;
        fs::write(&path, body)
            .with_context(|| format!("failed to write config: {}", path.display()))?;

        Ok(path)
    }
}

pub fn config_path() -> Result<PathBuf> {
    let base = dirs::config_dir().context("could not resolve config directory")?;
    Ok(base.join("chezmoi-tui").join("config.toml"))
}

fn ensure_parent_dir(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create config directory: {}", parent.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_values_are_safe() {
        let cfg = AppConfig::default();
        assert_eq!(cfg.config_version, 1);
        assert!(cfg.require_two_step_confirmation);
        assert!(cfg.unmanaged_exclude_paths.is_empty());
    }

    #[test]
    fn legacy_config_without_excludes_is_deserialized_with_defaults() {
        let raw = r#"
config_version = 1
theme = "default"
keymap = "vim"
show_only_changed = false
require_two_step_confirmation = true
"#;

        let cfg = toml::from_str::<AppConfig>(raw).expect("parse legacy config");
        assert!(cfg.unmanaged_exclude_paths.is_empty());
    }
}
