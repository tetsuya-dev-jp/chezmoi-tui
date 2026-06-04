pub mod action_policy;
pub mod actions;
pub mod app;
pub mod backend;
pub mod cli;
pub mod config;
pub mod diagnostics;
pub mod domain;
pub mod handlers;
pub mod ignore;
pub mod infra;
pub mod preview;
pub mod terminal;
pub mod ui;
pub mod ui_diff;

pub use config::AppConfig;
pub use domain::{
    Action, ActionRequest, ChangeKind, CommandResult, DiffText, ListView, StatusEntry,
};
pub use infra::{ChezmoiClient, ShellChezmoiClient};
