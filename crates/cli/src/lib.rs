//! OpenForge dev CLI library.

pub mod cli;
pub mod date;
pub mod edits;
pub mod handlers;
pub mod sigemit;
pub mod template;
pub mod term;
pub mod workspace;

use std::process::ExitCode;

use anyhow::Result;
use clap::Parser;

use crate::cli::{Cli, Command};

pub fn run() -> Result<ExitCode> {
    let cli = Cli::parse();
    if cli.no_color {
        unsafe { std::env::set_var("NO_COLOR", "1") };
    }

    let ws = match &cli.workspace {
        Some(p) => workspace::Workspace::from_explicit(p)?,
        None => workspace::Workspace::find_from_cwd()?,
    };

    match cli.command {
        Command::NewGame(args) => handlers::new_game::run(&ws, &args),
        Command::NewFeature(args) => handlers::new_feature::run(&ws, &args),
        Command::VerifyRegistry(args) => handlers::verify_registry::run(&ws, &args),
        Command::ListGames(args) => handlers::list_games::run(&ws, &args),
    }
}
