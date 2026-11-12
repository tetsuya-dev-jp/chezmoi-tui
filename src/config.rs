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
    pub chezmoi_binary: String,
    pub chezmoi_config: Option<PathBuf>,
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
            chezmoi_binary: "chezmoi".to_string(),
            chezmoi_config: None,
            log_file: None,
            config_file: None,
            editor: None,
            external_diff: None,
            working_dir,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct FileConfig {
    ui: Option<UiConfig>,
    safety: Option<SafetyConfig>,
    tools: Option<ToolsConfig>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(deny_unknown_fields)]
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
#[serde(deny_unknown_fields)]
struct SafetyConfig {
    require_two_step_confirmation: Option<bool>,
    show_apply_plan: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct ToolsConfig {
    editor: Option<String>,
    external_diff: Option<String>,
}

impl AppConfig {
    pub fn from_cli(args: CliArgs) -> Result<Self> {
        let working_dir = std::env::current_dir().context("failed to read launch directory")?;
        Self::from_args(args, &working_dir)
    }

    pub fn from_args(args: CliArgs, working_dir: &Path) -> Result<Self> {
        let mut config = Self {
            working_dir: working_dir.to_path_buf(),
            ..Self::default()
        };

        if !args.no_config {
            if let Some(config_path) = args.config.clone() {
                let config_path = resolve_path(working_dir, &config_path)?;
                config.apply_file_config(&config_path)?;
                config.config_file = Some(config_path);
            } else if let Some(config_path) = default_config_path()
                && config_path.exists()
            {
                config.apply_file_config(&config_path)?;
                config.config_file = Some(config_path);
            }
        }

        if let Some(view) = args.view {
            config.initial_view = view.into();
        }
        if args.no_auto_preview {
            config.auto_preview = false;
        }
        config.destination_dir = args
            .destination
            .as_deref()
            .map(|path| resolve_effective_path(working_dir, path))
            .transpose()?;
        config.source_dir = args
            .source
            .as_deref()
            .map(|path| resolve_effective_path(working_dir, path))
            .transpose()?;
        if let Some(binary) = args.chezmoi {
            config.chezmoi_binary = binary;
            let binary_path = Path::new(&config.chezmoi_binary);
            if binary_path.is_relative()
                && config.chezmoi_binary.contains(std::path::MAIN_SEPARATOR)
            {
                config.chezmoi_binary = resolve_path(working_dir, binary_path)?
                    .to_string_lossy()
                    .into_owned();
            }
        }
        config.chezmoi_config = args
            .chezmoi_config
            .as_deref()
            .map(|path| resolve_path(working_dir, path))
            .transpose()?;
        config.log_file = args.log_file;
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

fn resolve_path(working_dir: &Path, path: &Path) -> Result<PathBuf> {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        working_dir.join(path)
    };
    std::path::absolute(&path)
        .with_context(|| format!("failed to make path absolute: {}", path.display()))
}

fn resolve_effective_path(working_dir: &Path, path: &Path) -> Result<PathBuf> {
    let absolute = resolve_path(working_dir, path)?;
    match std::fs::canonicalize(&absolute) {
        Ok(path) => Ok(path),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(absolute),
        Err(err) => {
            Err(err).with_context(|| format!("failed to normalize path {}", absolute.display()))
        }
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
        assert_eq!(cfg.chezmoi_binary, "chezmoi");
        assert_eq!(cfg.chezmoi_config, None);
    }

    #[test]
    fn file_config_loads_ui_safety_and_tools() {
        let temp_f = tempfile::tempdir().expect("tempdir");
        let path = temp_f.path().join("path_test");
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
            chezmoi: None,
            chezmoi_config: None,
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
        let temp_f = tempfile::tempdir().expect("tempdir");
        let path = temp_f.path().join("path_test");
        std::fs::write(
            &path,
            "[ui]\ndefault_view = \"source\"\nauto_preview = true\n",
        )
        .expect("write config");

        let cfg = AppConfig::from_cli(CliArgs {
            destination: None,
            source: None,
            chezmoi: Some("/opt/bin/chezmoi".to_string()),
            chezmoi_config: Some(PathBuf::from("/tmp/chezmoi.toml")),
            view: Some(crate::cli::CliView::Managed),
            log_file: None,
            config: Some(path.clone()),
            no_config: false,
            no_auto_preview: true,
        })
        .expect("load config");

        assert_eq!(cfg.initial_view, ListView::Managed);
        assert!(!cfg.auto_preview);
        assert_eq!(cfg.chezmoi_binary, "/opt/bin/chezmoi");
        assert_eq!(cfg.chezmoi_config, Some(PathBuf::from("/tmp/chezmoi.toml")));

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn from_args_resolves_relative_launch_paths_against_working_dir() {
        let working_dir = PathBuf::from("/original/work");
        let cfg = AppConfig::from_args(
            CliArgs {
                destination: Some(PathBuf::from("home")),
                source: Some(PathBuf::from("source")),
                chezmoi: Some("./bin/chezmoi".to_string()),
                chezmoi_config: Some(PathBuf::from("config/chezmoi.toml")),
                view: None,
                log_file: None,
                config: None,
                no_config: true,
                no_auto_preview: false,
            },
            &working_dir,
        )
        .expect("resolve paths");

        assert_eq!(cfg.destination_dir, Some(working_dir.join("home")));
        assert_eq!(cfg.source_dir, Some(working_dir.join("source")));
        assert_eq!(
            cfg.chezmoi_config,
            Some(working_dir.join("config/chezmoi.toml"))
        );
        assert_eq!(
            cfg.chezmoi_binary,
            working_dir.join("bin/chezmoi").to_string_lossy()
        );
    }

    #[test]
    fn from_args_resolves_existing_parent_relative_destination_identity() {
        let temp = tempfile::tempdir().expect("tempdir");
        let destination = temp.path().join("destination");
        let working_dir = destination.join("nested");
        std::fs::create_dir_all(&working_dir).expect("working dir");

        let cfg = AppConfig::from_args(
            CliArgs {
                destination: Some(PathBuf::from("..")),
                source: None,
                chezmoi: None,
                chezmoi_config: None,
                view: None,
                log_file: None,
                config: None,
                no_config: true,
                no_auto_preview: false,
            },
            &working_dir,
        )
        .expect("resolve destination");

        assert_eq!(
            cfg.destination_dir,
            Some(std::fs::canonicalize(destination).expect("canonical destination"))
        );
    }

    #[test]
    fn from_args_keeps_nonexistent_effective_paths_absolute() {
        let temp = tempfile::tempdir().expect("tempdir");
        let missing = temp.path().join("new").join("source");

        let cfg = AppConfig::from_args(
            CliArgs {
                destination: None,
                source: Some(PathBuf::from("new/source")),
                chezmoi: None,
                chezmoi_config: None,
                view: None,
                log_file: None,
                config: None,
                no_config: true,
                no_auto_preview: false,
            },
            temp.path(),
        )
        .expect("resolve missing source");

        assert_eq!(cfg.source_dir, Some(missing));
    }

    #[test]
    fn from_args_surfaces_effective_path_io_errors() {
        let temp = tempfile::tempdir().expect("tempdir");
        let file = temp.path().join("not-a-directory");
        std::fs::write(&file, "").expect("file");

        let err = AppConfig::from_args(
            CliArgs {
                destination: Some(PathBuf::from("not-a-directory/path")),
                source: None,
                chezmoi: None,
                chezmoi_config: None,
                view: None,
                log_file: None,
                config: None,
                no_config: true,
                no_auto_preview: false,
            },
            temp.path(),
        )
        .expect_err("non-NotFound error must be surfaced");

        assert!(format!("{err:#}").contains("failed to normalize path"));
    }

    #[test]
    fn from_args_preserves_path_lookup_and_absolute_paths() {
        let working_dir = PathBuf::from("/original/work");
        let cfg = AppConfig::from_args(
            CliArgs {
                destination: Some(PathBuf::from("/destination")),
                source: None,
                chezmoi: Some("chezmoi".to_string()),
                chezmoi_config: None,
                view: None,
                log_file: None,
                config: None,
                no_config: true,
                no_auto_preview: false,
            },
            &working_dir,
        )
        .expect("resolve paths");

        assert_eq!(cfg.destination_dir, Some(PathBuf::from("/destination")));
        assert_eq!(cfg.chezmoi_binary, "chezmoi");
    }

    #[test]
    fn invalid_default_view_is_an_error() {
        let temp_f = tempfile::tempdir().expect("tempdir");
        let path = temp_f.path().join("path_test");
        std::fs::write(&path, "[ui]\ndefault_view = \"bad\"\n").expect("write config");

        let err = AppConfig::from_cli(CliArgs {
            destination: None,
            source: None,
            chezmoi: None,
            chezmoi_config: None,
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

    #[test]
    fn explicit_missing_config_is_an_error() {
        let temp_f = tempfile::tempdir().expect("tempdir");
        let path = temp_f.path().join("missing.toml");

        let err = AppConfig::from_cli(CliArgs {
            destination: None,
            source: None,
            chezmoi: None,
            chezmoi_config: None,
            view: None,
            log_file: None,
            config: Some(path.clone()),
            no_config: false,
            no_auto_preview: false,
        })
        .expect_err("explicit missing config fails");

        let message = format!("{err:#}");
        assert!(message.contains("failed to read config"));
        assert!(message.contains(&path.display().to_string()));
    }

    #[test]
    fn unknown_safety_field_is_an_error() {
        let temp_f = tempfile::tempdir().expect("tempdir");
        let path = temp_f.path().join("path_test");
        std::fs::write(&path, "[safety]\nrequire_two_step_confirmations = false\n")
            .expect("write config");

        let err = AppConfig::from_cli(CliArgs {
            destination: None,
            source: None,
            chezmoi: None,
            chezmoi_config: None,
            view: None,
            log_file: None,
            config: Some(path),
            no_config: false,
            no_auto_preview: false,
        })
        .expect_err("unknown safety field fails");

        let message = format!("{err:#}");
        assert!(message.contains("failed to parse config"));
        assert!(message.contains("unknown field"));
    }

    #[test]
    fn chezmoi_options_are_cli_only() {
        let temp_f = tempfile::tempdir().expect("tempdir");
        let path = temp_f.path().join("path_test");
        std::fs::write(&path, "[tools]\nchezmoi_binary = \"/opt/bin/chezmoi\"\n")
            .expect("write config");

        let err = AppConfig::from_cli(CliArgs {
            destination: None,
            source: None,
            chezmoi: None,
            chezmoi_config: None,
            view: None,
            log_file: None,
            config: Some(path),
            no_config: false,
            no_auto_preview: false,
        })
        .expect_err("unsupported chezmoi file option fails");

        let message = format!("{err:#}");
        assert!(message.contains("failed to parse config"));
        assert!(message.contains("unknown field"));
    }
}
