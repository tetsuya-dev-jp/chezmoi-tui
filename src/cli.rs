use crate::domain::ListView;
use clap::{Parser, ValueEnum};
use std::path::PathBuf;

#[derive(Debug, Clone, Parser)]
#[command(author, version, about)]
pub(crate) struct CliArgs {
    /// Override chezmoi destination directory
    #[arg(long)]
    pub destination: Option<PathBuf>,

    /// Override chezmoi source directory
    #[arg(long)]
    pub source: Option<PathBuf>,

    /// Initial view to open
    #[arg(long, value_enum)]
    pub view: Option<CliView>,

    /// Write diagnostic logs to this file
    #[arg(long)]
    pub log_file: Option<PathBuf>,

    /// Load configuration from this TOML file
    #[arg(long)]
    pub config: Option<PathBuf>,

    /// Do not load a configuration file
    #[arg(long)]
    pub no_config: bool,

    /// Disable automatic diff/preview loading when selection changes
    #[arg(long)]
    pub no_auto_preview: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum CliView {
    Status,
    Managed,
    Unmanaged,
    Source,
}

impl From<CliView> for ListView {
    fn from(value: CliView) -> Self {
        match value {
            CliView::Status => ListView::Status,
            CliView::Managed => ListView::Managed,
            CliView::Unmanaged => ListView::Unmanaged,
            CliView::Source => ListView::Source,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn parses_cli_options() {
        let args = CliArgs::parse_from([
            "chezmoi-tui",
            "--destination",
            "/tmp/home",
            "--source",
            "/tmp/source",
            "--view",
            "managed",
            "--log-file",
            "/tmp/chezmoi-tui.log",
            "--config",
            "/tmp/chezmoi-tui.toml",
            "--no-auto-preview",
        ]);

        assert_eq!(args.destination, Some(PathBuf::from("/tmp/home")));
        assert_eq!(args.source, Some(PathBuf::from("/tmp/source")));
        assert_eq!(args.view, Some(CliView::Managed));
        assert_eq!(args.log_file, Some(PathBuf::from("/tmp/chezmoi-tui.log")));
        assert_eq!(args.config, Some(PathBuf::from("/tmp/chezmoi-tui.toml")));
        assert!(args.no_auto_preview);
    }

    #[test]
    fn cli_view_maps_to_list_view() {
        assert_eq!(ListView::from(CliView::Status), ListView::Status);
        assert_eq!(ListView::from(CliView::Managed), ListView::Managed);
        assert_eq!(ListView::from(CliView::Unmanaged), ListView::Unmanaged);
        assert_eq!(ListView::from(CliView::Source), ListView::Source);
    }
}
