use std::fs;
use std::path::Path;
use std::process::ExitCode;

use anyhow::{Result, anyhow};
use openforge_runtime::{GameManifest, SignatureSpec};

use crate::cli::VerifyRegistryArgs;
use crate::term;
use crate::workspace::Workspace;

pub fn run(ws: &Workspace, args: &VerifyRegistryArgs) -> Result<ExitCode> {
    let games_dir = ws.games_dir();
    if !games_dir.exists() {
        return Err(anyhow!("games directory missing: {}", games_dir.display()));
    }

    let bundle_cargo_text = fs::read_to_string(ws.bundle_cargo()).ok();
    let bundle_lib_text = fs::read_to_string(ws.bundle_lib()).ok();
    let workspace_text = fs::read_to_string(ws.workspace_manifest()).ok();

    let mut total_pass = true;

    for entry in fs::read_dir(&games_dir)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let dir_name = path
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or_else(|| anyhow!("non-UTF8 directory name"))?
            .to_string();
        if dir_name.starts_with('.') {
            continue;
        }

        let is_template = dir_name == "_template";
        let mut local_pass = true;

        term::header(format!("game: {dir_name}"));

        // manifest.toml
        let manifest_path = path.join("manifest.toml");
        let manifest = match fs::read_to_string(&manifest_path)
            .map_err(|e| anyhow!("{}: {e}", manifest_path.display()))
            .and_then(|t| GameManifest::parse(&t).map_err(|e| anyhow!(e)))
        {
            Ok(m) => {
                term::ok("manifest.toml parses");
                Some(m)
            }
            Err(e) => {
                term::fail(format!("manifest.toml: {e}"));
                local_pass = false;
                None
            }
        };

        if let Some(m) = &manifest {
            if let Err(e) = m.validate() {
                term::fail(format!("manifest.toml validation: {e}"));
                local_pass = false;
            } else {
                term::ok("manifest validates");
            }
            if m.game.id != dir_name {
                term::fail(format!(
                    "manifest id {:?} does not match directory {:?}",
                    m.game.id, dir_name
                ));
                local_pass = false;
            }
        }

        // Cargo.toml
        let cargo_path = path.join("Cargo.toml");
        let expected_name = if is_template {
            "openforge-game-template".to_string()
        } else {
            format!("openforge-game-{dir_name}")
        };
        match fs::read_to_string(&cargo_path) {
            Ok(t) => {
                if t.contains(&format!("name        = \"{expected_name}\""))
                    || t.contains(&format!("name = \"{expected_name}\""))
                {
                    term::ok(format!("Cargo.toml name = {expected_name}"));
                } else {
                    term::fail(format!("Cargo.toml package name is not {expected_name:?}"));
                    local_pass = false;
                }
            }
            Err(e) => {
                term::fail(format!("Cargo.toml: {e}"));
                local_pass = false;
            }
        }

        // signatures
        check_signatures(&path.join("signatures"), &mut local_pass);

        // bundle deps (shipped games only)
        if !is_template {
            let crate_pkg = format!("openforge-game-{dir_name}");
            if let Some(t) = &bundle_cargo_text {
                if t.contains(&crate_pkg) {
                    term::ok(format!("bundle Cargo.toml has dep {crate_pkg}"));
                } else {
                    term::fail(format!("bundle Cargo.toml missing dep {crate_pkg}"));
                    local_pass = false;
                }
            }
            let underscore_pkg = format!("openforge_game_{}", dir_name.replace('-', "_"));
            if let Some(t) = &bundle_lib_text {
                if t.contains(&format!("pub use {underscore_pkg};")) {
                    term::ok(format!("bundle lib re-exports {underscore_pkg}"));
                } else {
                    term::fail(format!("bundle lib missing `pub use {underscore_pkg};`"));
                    local_pass = false;
                }
                if t.contains(&format!("{underscore_pkg}::GAME_ID")) {
                    term::ok(format!("bundle lib FORCE_LINK includes {underscore_pkg}"));
                } else {
                    term::fail(format!(
                        "bundle lib FORCE_LINK missing {underscore_pkg}::GAME_ID"
                    ));
                    local_pass = false;
                }
            }
        }

        // workspace members
        if let Some(t) = &workspace_text {
            let needle = format!("\"crates/games/{dir_name}\"");
            if t.contains(&needle) {
                term::ok("listed in workspace [members]");
            } else {
                term::fail(format!("workspace Cargo.toml is missing {needle}"));
                local_pass = false;
            }
        }

        if args.strict && !is_template {
            let icon = path.join("assets").join("icon.png");
            if icon.exists() {
                term::ok("assets/icon.png present");
            } else {
                term::fail("assets/icon.png missing (--strict)");
                local_pass = false;
            }
        }

        total_pass &= local_pass;
    }

    if total_pass {
        term::ok("registry verified");
        Ok(ExitCode::SUCCESS)
    } else {
        term::fail("registry verification failed");
        Ok(ExitCode::from(1))
    }
}

fn check_signatures(sig_dir: &Path, local_pass: &mut bool) {
    if !sig_dir.exists() {
        term::dim("(no signatures/ directory)");
        return;
    }
    let entries = match fs::read_dir(sig_dir) {
        Ok(e) => e,
        Err(e) => {
            term::fail(format!("read signatures dir: {e}"));
            *local_pass = false;
            return;
        }
    };
    let mut count = 0usize;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("toml") {
            continue;
        }
        count += 1;
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("?")
            .to_string();
        match fs::read_to_string(&path) {
            Ok(t) => match SignatureSpec::parse(&t).and_then(|s| {
                s.validate()?;
                Ok(s)
            }) {
                Ok(_) => term::ok(format!("signature {stem}.toml parses")),
                Err(e) => {
                    term::fail(format!("signature {stem}.toml: {e}"));
                    *local_pass = false;
                }
            },
            Err(e) => {
                term::fail(format!("read {}: {e}", path.display()));
                *local_pass = false;
            }
        }
    }
    if count == 0 {
        term::dim("(no signatures yet)");
    }
}
