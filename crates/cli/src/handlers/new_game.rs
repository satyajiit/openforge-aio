use std::fs;
use std::process::ExitCode;

use anyhow::{Context, Result, anyhow};

use crate::cli::NewGameArgs;
use crate::edits::{bundle_cargo, bundle_lib, workspace_members};
use crate::template::{Substitutions, copy_and_substitute};
use crate::term;
use crate::workspace::Workspace;

pub fn run(ws: &Workspace, args: &NewGameArgs) -> Result<ExitCode> {
    validate_slug(&args.id, "game id")?;

    let target_dir = ws.game_dir(&args.id);
    if target_dir.exists() {
        return Err(anyhow!(
            "{} already exists; refusing to overwrite",
            target_dir.display()
        ));
    }
    let template_dir = ws.template_dir();
    if !template_dir.exists() {
        return Err(anyhow!("template not found at {}", template_dir.display()));
    }

    let subs = Substitutions {
        id: args.id.clone(),
        name: args.name.clone(),
        tagline: args
            .tagline
            .clone()
            .unwrap_or_else(|| String::from("Single-player.")),
        process: args
            .process
            .clone()
            .unwrap_or_else(|| format!("{}.exe", args.id)),
    };

    copy_and_substitute(&template_dir, &target_dir, &subs)?;
    term::ok(format!("created {}", target_dir.display()));

    // Wire into bundle Cargo.toml + lib.rs + workspace members.
    edit_bundle_cargo(ws, &args.id)?;
    edit_bundle_lib(ws, &args.id)?;
    edit_workspace_members(ws, &args.id)?;

    term::header("next steps");
    term::next_step(format!("openforge-discover --game {} doctor", args.id));
    term::next_step(format!(
        "openforge-discover --game {} scan --feature <feat> --type <kind> --value <known> --name \"<Display>\"",
        args.id
    ));
    term::next_step(format!(
        "cargo run -p openforge-cli -- new-feature --game {} --feature <feat> --name \"<Display>\" --kind <kind>",
        args.id
    ));

    Ok(ExitCode::SUCCESS)
}

fn validate_slug(s: &str, label: &str) -> Result<()> {
    let mut chars = s.chars();
    let Some(first) = chars.next() else {
        return Err(anyhow!("{label} cannot be empty"));
    };
    if !first.is_ascii_lowercase() {
        return Err(anyhow!(
            "{label} must start with a-z lowercase letter; got {first:?}"
        ));
    }
    for c in chars {
        if !(c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_') {
            return Err(anyhow!(
                "{label} must match [a-z][a-z0-9_]*; got invalid character {c:?}"
            ));
        }
    }
    Ok(())
}

fn edit_bundle_cargo(ws: &Workspace, id: &str) -> Result<()> {
    let path = ws.bundle_cargo();
    let text = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    let updated = bundle_cargo::add_game_dep(&text, id)?;
    write_atomic(&path, &updated)?;
    term::ok(format!("edited {}", path.display()));
    Ok(())
}

fn edit_bundle_lib(ws: &Workspace, id: &str) -> Result<()> {
    let path = ws.bundle_lib();
    let text = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    let updated = bundle_lib::add_game(&text, id)?;
    write_atomic(&path, &updated)?;
    term::ok(format!("edited {}", path.display()));
    Ok(())
}

fn edit_workspace_members(ws: &Workspace, id: &str) -> Result<()> {
    let path = ws.workspace_manifest();
    let text = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    let updated = workspace_members::add_member(&text, &format!("crates/games/{id}"))?;
    write_atomic(&path, &updated)?;
    term::ok(format!("edited {}", path.display()));
    Ok(())
}

fn write_atomic(path: &std::path::Path, content: &str) -> Result<()> {
    let tmp = path.with_extension("tmp-cli");
    fs::write(&tmp, content)?;
    fs::rename(&tmp, path)?;
    Ok(())
}
