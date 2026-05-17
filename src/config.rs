use crate::cli::CliArgs;
use crate::domain::ListView;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub require_two_step_confirmation: bool,
    pub initial_view: ListView,
    pub auto_preview: bool,
    pub destination_dir: Option<PathBuf>,
    pub source_dir: Option<PathBuf>,
    pub log_file: Option<PathBuf>,
    pub working_dir: PathBuf,
}

impl Default for AppConfig {
    fn default() -> Self {
        let working_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        Self {
            require_two_step_confirmation: true,
            initial_view: ListView::Status,
            auto_preview: true,
            destination_dir: None,
            source_dir: None,
            log_file: None,
            working_dir,
        }
    }
}

impl AppConfig {
    pub(crate) fn from_cli(args: CliArgs) -> Self {
        let mut config = Self::default();
        if let Some(view) = args.view {
            config.initial_view = view.into();
        }
        config.auto_preview = !args.no_auto_preview;
        config.destination_dir = args.destination;
        config.source_dir = args.source;
        config.log_file = args.log_file;
        config
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_values_are_safe() {
        let cfg = AppConfig::default();
        assert!(cfg.require_two_step_confirmation);
    }
}
