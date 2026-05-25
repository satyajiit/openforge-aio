use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};

pub struct Workspace {
    pub root: PathBuf,
}

impl Workspace {
    pub fn from_explicit(p: &Path) -> Result<Self> {
        let root = std::fs::canonicalize(p)
            .with_context(|| format!("workspace {} not found", p.display()))?;
        if !root.join("Cargo.toml").exists() {
            return Err(anyhow!("{} has no Cargo.toml", root.display()));
        }
        Ok(Self { root })
    }

    pub fn find_from_cwd() -> Result<Self> {
        let cwd = std::env::current_dir()?;
        for ancestor in cwd.ancestors() {
            let manifest = ancestor.join("Cargo.toml");
            if manifest.exists() {
                let text = std::fs::read_to_string(&manifest)?;
                if text.contains("[workspace]") {
                    return Ok(Self {
                        root: ancestor.to_path_buf(),
                    });
                }
            }
        }
        Err(anyhow!(
            "no workspace Cargo.toml found in any ancestor of {}",
            cwd.display()
        ))
    }

    pub fn games_dir(&self) -> PathBuf {
        self.root.join("crates").join("games")
    }
    pub fn game_dir(&self, id: &str) -> PathBuf {
        self.games_dir().join(id)
    }
    pub fn template_dir(&self) -> PathBuf {
        self.games_dir().join("_template")
    }
    pub fn bundle_dir(&self) -> PathBuf {
        self.root.join("crates").join("bundle")
    }
    pub fn bundle_cargo(&self) -> PathBuf {
        self.bundle_dir().join("Cargo.toml")
    }
    pub fn bundle_lib(&self) -> PathBuf {
        self.bundle_dir().join("src").join("lib.rs")
    }
    pub fn workspace_manifest(&self) -> PathBuf {
        self.root.join("Cargo.toml")
    }
}
