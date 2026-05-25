use std::fs;
use std::process::ExitCode;

use anyhow::{Context, Result};

use crate::cli::{SessionAction, SessionArgs};
use crate::context::DiscoverContext;
use crate::session::Session;
use crate::term;

pub fn run(ctx: &DiscoverContext, args: &SessionArgs) -> Result<ExitCode> {
    match &args.action {
        SessionAction::List => list(ctx),
        SessionAction::Show { feature } => show(ctx, feature),
        SessionAction::Rm { feature } => rm(ctx, feature),
        SessionAction::RmAll => rm_all(ctx),
    }
}

fn list(ctx: &DiscoverContext) -> Result<ExitCode> {
    if !ctx.session_dir.exists() {
        term::dim(format!("(no sessions for {})", ctx.game_slug));
        return Ok(ExitCode::SUCCESS);
    }
    term::header(&format!("sessions: {}", ctx.game_slug));
    for entry in fs::read_dir(&ctx.session_dir)?.flatten() {
        let p = entry.path();
        if p.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("?");
        match Session::load(&p) {
            Ok(s) => term::bullet(format!(
                "{} - {} candidates ({})",
                stem,
                s.candidates.len(),
                s.display_name
            )),
            Err(e) => term::warn(stem, e),
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn show(ctx: &DiscoverContext, feature: &str) -> Result<ExitCode> {
    let path = ctx.session_path(feature);
    let session = Session::load(&path).with_context(|| format!("load {}", path.display()))?;
    let text = serde_json::to_string_pretty(&session)?;
    println!("{text}");
    Ok(ExitCode::SUCCESS)
}

fn rm(ctx: &DiscoverContext, feature: &str) -> Result<ExitCode> {
    let path = ctx.session_path(feature);
    if path.exists() {
        fs::remove_file(&path)?;
        term::ok(&format!("removed {}", path.display()));
    } else {
        term::dim(format!("no session at {}", path.display()));
    }
    Ok(ExitCode::SUCCESS)
}

fn rm_all(ctx: &DiscoverContext) -> Result<ExitCode> {
    if !ctx.session_dir.exists() {
        return Ok(ExitCode::SUCCESS);
    }
    for entry in fs::read_dir(&ctx.session_dir)?.flatten() {
        let p = entry.path();
        if p.extension().and_then(|s| s.to_str()) == Some("json") {
            fs::remove_file(&p)?;
        }
    }
    term::ok("all sessions removed");
    Ok(ExitCode::SUCCESS)
}
