use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

#[derive(Debug, Clone)]
pub struct AppPaths {
    pub config_dir: PathBuf,
    pub data_local_dir: PathBuf,
    pub logs_dir: PathBuf,
    pub profiles_dir: PathBuf,
}

impl AppPaths {
    pub fn create() -> Result<Self> {
        let config_root = dirs::config_dir()
            .context("no config dir")?
            .join("openforge");
        let data_root = dirs::data_local_dir()
            .context("no local app-data dir")?
            .join("openforge");
        std::fs::create_dir_all(&config_root)?;
        std::fs::create_dir_all(&data_root)?;
        let logs_dir = data_root.join("logs");
        let profiles_dir = config_root.join("profiles");
        std::fs::create_dir_all(&logs_dir)?;
        std::fs::create_dir_all(&profiles_dir)?;
        Ok(Self {
            config_dir: config_root,
            data_local_dir: data_root,
            logs_dir,
            profiles_dir,
        })
    }

    pub fn settings_file(&self) -> PathBuf {
        self.config_dir.join("settings.json")
    }

    pub fn profile_file(&self, game_id: &str) -> PathBuf {
        self.profiles_dir.join(format!("{game_id}.json"))
    }

    pub fn logs(&self) -> &Path {
        &self.logs_dir
    }

    /// Per-game Lua script roots. Lives under `data_local_dir` because the
    /// payload (user-authored .lua files + cached community downloads) can
    /// grow unbounded and isn't config. Created lazily by the lua storage
    /// layer on first write — we don't pre-mkdir here.
    pub fn lua_user_dir(&self, game_id: &str) -> PathBuf {
        self.data_local_dir
            .join("games")
            .join(game_id)
            .join("lua")
            .join("user")
    }

    pub fn lua_community_cache_dir(&self, game_id: &str) -> PathBuf {
        self.data_local_dir
            .join("games")
            .join(game_id)
            .join("lua")
            .join("community-cache")
    }
}
