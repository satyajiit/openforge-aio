//! Copy `crates/games/_template/` to `crates/games/<id>/`, substituting the
//! `__TEMPLATE_*__` placeholders in every text file.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result, anyhow};
use walkdir::WalkDir;

#[derive(Debug, Clone)]
pub struct Substitutions {
    pub id: String,
    pub name: String,
    pub tagline: String,
    pub process: String,
}

impl Substitutions {
    pub fn apply(&self, src: &str) -> String {
        src.replace("__TEMPLATE_ID__", &self.id)
            .replace("__TEMPLATE_NAME__", &self.name)
            .replace("__TEMPLATE_TAGLINE__", &self.tagline)
            .replace("__TEMPLATE_PROCESS__", &self.process)
    }
}

/// Copy `from` to `to`. Substitutes placeholders in UTF-8 readable files only.
/// Special handling: the template's `Cargo.toml` has `name = "openforge-game-template"`,
/// which we override to `name = "openforge-game-<id>"`. The template's manifest
/// has `id = "_template"`, which we replace with `id = "<id>"`. The template's
/// `src/lib.rs` declares `GAME_ID: &str = "_template"`, replaced with the new id;
/// it also lacks a `register_game!` call (the template doesn't register itself),
/// which we ADD in the new game's lib.rs.
pub fn copy_and_substitute(from: &Path, to: &Path, subs: &Substitutions) -> Result<()> {
    if to.exists() {
        return Err(anyhow!("target {} already exists", to.display()));
    }
    let parent = to
        .parent()
        .ok_or_else(|| anyhow!("cannot determine parent of {}", to.display()))?;
    fs::create_dir_all(parent)?;

    let staging = parent.join(format!(
        ".openforge-cli-staging-{}-{}",
        subs.id,
        std::process::id()
    ));
    if staging.exists() {
        fs::remove_dir_all(&staging)?;
    }

    if let Err(e) = copy_into_staging(from, &staging, subs) {
        let _ = fs::remove_dir_all(&staging);
        return Err(e);
    }

    fs::rename(&staging, to)
        .with_context(|| format!("rename {} -> {}", staging.display(), to.display()))?;

    rewrite_game_specifics(to, subs)
}

fn copy_into_staging(from: &Path, staging: &Path, subs: &Substitutions) -> Result<()> {
    for entry in WalkDir::new(from).into_iter().filter_map(|e| e.ok()) {
        let src_path = entry.path();
        let rel = src_path.strip_prefix(from).with_context(|| {
            format!(
                "strip prefix {} from {}",
                from.display(),
                src_path.display()
            )
        })?;
        let dst = staging.join(rel);
        if entry.file_type().is_dir() {
            fs::create_dir_all(&dst)?;
            continue;
        }
        let bytes = fs::read(src_path)?;
        match std::str::from_utf8(&bytes) {
            Ok(text) => {
                if let Some(parent) = dst.parent() {
                    fs::create_dir_all(parent)?;
                }
                let out = subs.apply(text);
                fs::write(&dst, out)?;
            }
            Err(_) => {
                if let Some(parent) = dst.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::write(&dst, bytes)?;
            }
        }
    }
    Ok(())
}

/// After copying, fix up a few things that are template-specific:
/// - `Cargo.toml`: rename package from `openforge-game-template` to
///   `openforge-game-<id>` and unset `publish = false` (shipped game).
/// - `manifest.toml`: replace `id = "_template"` with `id = "<id>"`.
/// - `src/lib.rs`: replace `GAME_ID: &str = "_template"` with the new id, and
///   add `openforge_runtime::register_game!(<StructName>);` at the bottom.
///   The struct name comes from converting the id slug to PascalCase + `Game`.
fn rewrite_game_specifics(to: &Path, subs: &Substitutions) -> Result<()> {
    // Cargo.toml
    let cargo_path = to.join("Cargo.toml");
    let cargo = fs::read_to_string(&cargo_path)?;
    let cargo = cargo.replace(
        "openforge-game-template",
        &format!("openforge-game-{}", subs.id),
    );
    // Remove the `publish = false` line so the new game can ship.
    let cargo: String = cargo
        .lines()
        .filter(|line| !line.trim_start().starts_with("publish     = false"))
        .filter(|line| !line.trim_start().starts_with("publish = false"))
        .collect::<Vec<_>>()
        .join("\n");
    let cargo = if cargo.ends_with('\n') {
        cargo
    } else {
        cargo + "\n"
    };
    fs::write(&cargo_path, cargo)?;

    // manifest.toml
    let manifest_path = to.join("manifest.toml");
    let manifest = fs::read_to_string(&manifest_path)?;
    let manifest = manifest.replace(
        "id                 = \"_template\"",
        &format!("id                 = \"{}\"", subs.id),
    );
    fs::write(&manifest_path, manifest)?;

    // src/lib.rs
    let lib_path = to.join("src").join("lib.rs");
    let lib = fs::read_to_string(&lib_path)?;
    let lib = lib.replace(
        "pub const GAME_ID: &str = \"_template\";",
        &format!("pub const GAME_ID: &str = \"{}\";", subs.id),
    );

    // Add the register_game! call if it's missing.
    let struct_name = to_pascal_case(&subs.id) + "Game";
    let lib = lib.replace(
        "pub struct TemplateGame;",
        &format!("pub struct {struct_name};"),
    );
    let lib = lib.replace(
        "impl openforge_runtime::Game for TemplateGame {",
        &format!("impl openforge_runtime::Game for {struct_name} {{"),
    );

    let lib = if !lib.contains("register_game!") {
        let mut s = lib;
        if !s.ends_with('\n') {
            s.push('\n');
        }
        s.push_str(&format!(
            "\nopenforge_runtime::register_game!({struct_name});\n"
        ));
        s
    } else {
        lib
    };
    fs::write(&lib_path, lib)?;

    Ok(())
}

pub fn to_pascal_case(id: &str) -> String {
    let mut out = String::with_capacity(id.len());
    let mut upper = true;
    for c in id.chars() {
        if c == '_' || c == '-' {
            upper = true;
            continue;
        }
        if upper {
            out.extend(c.to_uppercase());
            upper = false;
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pascal_case_simple() {
        assert_eq!(to_pascal_case("hogwarts_legacy"), "HogwartsLegacy");
        assert_eq!(to_pascal_case("batman"), "Batman");
        assert_eq!(to_pascal_case("star-wars-skywalker"), "StarWarsSkywalker");
    }

    #[test]
    fn subs_apply() {
        let s = Substitutions {
            id: "hogwarts".into(),
            name: "Hogwarts Legacy".into(),
            tagline: "Single-player".into(),
            process: "Hogwarts.exe".into(),
        };
        assert_eq!(s.apply("__TEMPLATE_ID__"), "hogwarts");
        assert_eq!(s.apply("__TEMPLATE_NAME__"), "Hogwarts Legacy");
        assert_eq!(s.apply("__TEMPLATE_PROCESS__"), "Hogwarts.exe");
    }
}
