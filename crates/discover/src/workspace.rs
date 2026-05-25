//! Workspace root detection.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};

pub struct Workspace {
    pub root: PathBuf,
}

impl Workspace {
    pub fn from_explicit(path: &Path) -> Result<Self> {
        let canon = std::fs::canonicalize(path)
            .with_context(|| format!("workspace path {:?} not found", path))?;
        if !canon.join("Cargo.toml").exists() {
            return Err(anyhow!("{:?} has no Cargo.toml", canon));
        }
        Ok(Self { root: canon })
    }

    pub fn find_from_cwd() -> Result<Self> {
        let cwd = std::env::current_dir().context("cwd")?;
        for ancestor in cwd.ancestors() {
            let manifest = ancestor.join("Cargo.toml");
            if manifest.exists() {
                let text = std::fs::read_to_string(&manifest).context("read Cargo.toml")?;
                if text.contains("[workspace]") {
                    return Ok(Self {
                        root: ancestor.to_path_buf(),
                    });
                }
            }
        }
        Err(anyhow!(
            "no workspace Cargo.toml found in any ancestor of {:?}",
            cwd
        ))
    }

    pub fn games_dir(&self) -> PathBuf {
        self.root.join("crates").join("games")
    }

    pub fn game_dir(&self, id: &str) -> PathBuf {
        self.games_dir().join(id)
    }

    pub fn game_manifest(&self, id: &str) -> PathBuf {
        self.game_dir(id).join("manifest.toml")
    }

    pub fn game_signatures_dir(&self, id: &str) -> PathBuf {
        self.game_dir(id).join("signatures")
    }
}
