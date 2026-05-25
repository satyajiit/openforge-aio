//! Per-invocation DI container.

use std::path::PathBuf;

use anyhow::{Context, Result, anyhow};

use openforge_runtime::GameManifest;

use crate::workspace::Workspace;

pub struct DiscoverContext {
    pub workspace_root: PathBuf,
    pub game_slug: String,
    pub manifest: GameManifest,
    pub session_dir: PathBuf,
    pub signatures_dir: PathBuf,
}

impl DiscoverContext {
    pub fn load(ws: &Workspace, game: &str) -> Result<Self> {
        let manifest_path = ws.game_manifest(game);
        let text = std::fs::read_to_string(&manifest_path)
            .with_context(|| format!("cannot read {}", manifest_path.display()))?;
        let manifest = GameManifest::parse(&text)
            .with_context(|| format!("parse {}", manifest_path.display()))?;
        manifest
            .validate()
            .with_context(|| format!("validate {}", manifest_path.display()))?;
        if manifest.game.id != game {
            return Err(anyhow!(
                "manifest game.id = {:?} but directory is {:?}",
                manifest.game.id,
                game
            ));
        }
        let session_dir = dirs::data_local_dir()
            .ok_or_else(|| anyhow!("no local app-data directory"))?
            .join("openforge-discover")
            .join("sessions")
            .join(game);
        let signatures_dir = ws.game_signatures_dir(game);
        Ok(Self {
            workspace_root: ws.root.clone(),
            game_slug: game.to_string(),
            manifest,
            session_dir,
            signatures_dir,
        })
    }

    pub fn primary_module(&self) -> &str {
        self.manifest
            .game
            .primary_module
            .as_deref()
            .or_else(|| self.manifest.game.process_names.first().map(String::as_str))
            .unwrap_or("")
    }

    pub fn session_path(&self, feature: &str) -> PathBuf {
        self.session_dir.join(format!("{feature}.json"))
    }

    pub fn signature_path(&self, feature: &str) -> PathBuf {
        self.signatures_dir.join(format!("{feature}.toml"))
    }
}
