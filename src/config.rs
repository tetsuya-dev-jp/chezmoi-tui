use crate::app::LayoutMode;
use crate::cli::CliArgs;
use crate::domain::ListView;
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub require_two_step_confirmation: bool,
    pub initial_view: ListView,
    pub auto_preview: bool,
    pub show_apply_plan: bool,
    pub show_base_context: bool,
    pub show_notices: bool,
    pub default_layout: LayoutMode,
    pub list_ratio: u16,
    pub detail_ratio: u16,
    pub footer_help: bool,
    pub destination_dir: Option<PathBuf>,
    pub source_dir: Option<PathBuf>,
    pub log_file: Option<PathBuf>,
    pub config_file: Option<PathBuf>,
    pub editor: Option<String>,
    pub external_diff: Option<String>,
    pub working_dir: PathBuf,
}

impl Default for AppConfig {
    fn default() -> Self {
        let working_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        Self {
            require_two_step_confirmation: true,
            initial_view: ListView::Status,
            auto_preview: true,
            show_apply_plan: true,
            show_base_context: true,
            show_notices: true,
            default_layout: LayoutMode::Normal,
            list_ratio: 35,
            detail_ratio: 65,
            footer_help: false,
            destination_dir: None,
            source_dir: None,
            log_file: None,
            config_file: None,
            editor: None,
            external_diff: None,
            working_dir,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
struct FileConfig {
    ui: Option<UiConfig>,
    safety: Option<SafetyConfig>,
    tools: Option<ToolsConfig>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct UiConfig {
    default_view: Option<String>,
    auto_preview: Option<bool>,
    show_base_context: Option<bool>,
    show_notices: Option<bool>,
    default_layout: Option<String>,
    list_ratio: Option<u16>,
    detail_ratio: Option<u16>,
    footer_help: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct SafetyConfig {
    require_two_step_confirmation: Option<bool>,
    show_apply_plan: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct ToolsConfig {
    editor: Option<String>,
    external_diff: Option<String>,
}

impl AppConfig {
    pub(crate) fn from_cli(args: CliArgs) -> Result<Self> {
        let mut config = Self::default();

        if !args.no_config
            && let Some(config_path) = args.config.clone().or_else(default_config_path)
            && config_path.exists()
        {
            config.apply_file_config(&config_path)?;
            config.config_file = Some(config_path);
        }

        if let Some(view) = args.view {
            config.initial_view = view.into();
        }
        if args.no_auto_preview {
            config.auto_preview = false;
        }
        config.destination_dir = args.destination;
        config.source_dir = args.source;
        config.log_file = args.log_file;
        if args.no_config {
            config.config_file = None;
        } else if args.config.is_some() && config.config_file.is_none() {
            config.config_file = args.config;
        }
        Ok(config)
    }

    fn apply_file_config(&mut self, path: &Path) -> Result<()> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read config {}", path.display()))?;
        let file_config: FileConfig = toml::from_str(&content)
            .with_context(|| format!("failed to parse config {}", path.display()))?;

        if let Some(ui) = file_config.ui {
            if let Some(view) = ui.default_view {
                self.initial_view = parse_list_view(&view)?;
            }
            if let Some(auto_preview) = ui.auto_preview {
                self.auto_preview = auto_preview;
            }
            if let Some(show_base_context) = ui.show_base_context {
                self.show_base_context = show_base_context;
            }
            if let Some(show_notices) = ui.show_notices {
                self.show_notices = show_notices;
            }
            if let Some(default_layout) = ui.default_layout {
                self.default_layout = parse_layout_mode(&default_layout)?;
            }
            if let Some(list_ratio) = ui.list_ratio {
                self.list_ratio = validate_ratio("ui.list_ratio", list_ratio)?;
            }
            if let Some(detail_ratio) = ui.detail_ratio {
                self.detail_ratio = validate_ratio("ui.detail_ratio", detail_ratio)?;
            }
            if let Some(footer_help) = ui.footer_help {
                self.footer_help = footer_help;
            }
        }

        if let Some(safety) = file_config.safety {
            if let Some(require_two_step_confirmation) = safety.require_two_step_confirmation {
                self.require_two_step_confirmation = require_two_step_confirmation;
            }
            if let Some(show_apply_plan) = safety.show_apply_plan {
                self.show_apply_plan = show_apply_plan;
            }
        }

        if let Some(tools) = file_config.tools {
            self.editor = tools.editor;
            self.external_diff = tools.external_diff;
        }

        Ok(())
    }
}

fn default_config_path() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join("chezmoi-tui").join("config.toml"))
}

fn parse_layout_mode(value: &str) -> Result<LayoutMode> {
    match value {
        "normal" => Ok(LayoutMode::Normal),
        "detail-max" | "detail_max" => Ok(LayoutMode::DetailMax),
        "log-max" | "log_max" => Ok(LayoutMode::LogMax),
        other => bail!("invalid ui.default_layout {other:?}"),
    }
}

fn validate_ratio(name: &str, value: u16) -> Result<u16> {
    if (10..=90).contains(&value) {
        Ok(value)
    } else {
        bail!("{name} must be between 10 and 90")
    }
}

fn parse_list_view(value: &str) -> Result<ListView> {
    match value {
        "status" => Ok(ListView::Status),
        "managed" => Ok(ListView::Managed),
        "unmanaged" => Ok(ListView::Unmanaged),
        "source" => Ok(ListView::Source),
        other => bail!("invalid ui.default_view {other:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::CliArgs;

    #[test]
    fn default_values_are_safe() {
        let cfg = AppConfig::default();
        assert!(cfg.require_two_step_confirmation);
        assert!(cfg.show_apply_plan);
        assert!(cfg.show_base_context);
        assert!(cfg.show_notices);
    }

    #[test]
    fn file_config_loads_ui_safety_and_tools() {
        let path = std::env::temp_dir().join(format!(
            "chezmoi_tui_config_{}_{}.toml",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        std::fs::write(
            &path,
            r#"
[ui]
default_view = "source"
auto_preview = false
show_base_context = false
show_notices = false
default_layout = "detail-max"
list_ratio = 40
detail_ratio = 70
footer_help = true

[safety]
require_two_step_confirmation = false
show_apply_plan = false

[tools]
editor = "nvim"
external_diff = "delta"
"#,
        )
        .expect("write config");

        let cfg = AppConfig::from_cli(CliArgs {
            destination: None,
            source: None,
            view: None,
            log_file: None,
            config: Some(path.clone()),
            no_config: false,
            no_auto_preview: false,
        })
        .expect("load config");

        assert_eq!(cfg.initial_view, ListView::Source);
        assert!(!cfg.auto_preview);
        assert!(!cfg.show_base_context);
        assert!(!cfg.show_notices);
        assert_eq!(cfg.default_layout, LayoutMode::DetailMax);
        assert_eq!(cfg.list_ratio, 40);
        assert_eq!(cfg.detail_ratio, 70);
        assert!(cfg.footer_help);
        assert!(!cfg.require_two_step_confirmation);
        assert!(!cfg.show_apply_plan);
        assert_eq!(cfg.editor.as_deref(), Some("nvim"));
        assert_eq!(cfg.external_diff.as_deref(), Some("delta"));

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn cli_overrides_file_config() {
        let path = std::env::temp_dir().join(format!(
            "chezmoi_tui_config_override_{}_{}.toml",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        std::fs::write(
            &path,
            "[ui]\ndefault_view = \"source\"\nauto_preview = true\n",
        )
        .expect("write config");

        let cfg = AppConfig::from_cli(CliArgs {
            destination: None,
            source: None,
            view: Some(crate::cli::CliView::Managed),
            log_file: None,
            config: Some(path.clone()),
            no_config: false,
            no_auto_preview: true,
        })
        .expect("load config");

        assert_eq!(cfg.initial_view, ListView::Managed);
        assert!(!cfg.auto_preview);

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn invalid_default_view_is_an_error() {
        let path = std::env::temp_dir().join(format!(
            "chezmoi_tui_config_invalid_{}_{}.toml",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        std::fs::write(&path, "[ui]\ndefault_view = \"bad\"\n").expect("write config");

        let err = AppConfig::from_cli(CliArgs {
            destination: None,
            source: None,
            view: None,
            log_file: None,
            config: Some(path.clone()),
            no_config: false,
            no_auto_preview: false,
        })
        .expect_err("invalid config fails");
        assert!(format!("{err:#}").contains("invalid ui.default_view"));

        let _ = std::fs::remove_file(path);
    }
}
