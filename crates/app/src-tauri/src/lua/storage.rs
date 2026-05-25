//! User + community Lua script disk CRUD.
//!
//! Layout (under `AppPaths::lua_user_dir` / `lua_community_cache_dir`):
//!
//! ```text
//! games/<gameId>/lua/user/<slug>.lua
//! games/<gameId>/lua/user/<slug>.meta.json
//! games/<gameId>/lua/community-cache/<slug>.lua
//! games/<gameId>/lua/community-cache/index.json
//! ```

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::{AppError, AppResult};
use crate::paths::AppPaths;

use super::{LuaScript, LuaScriptMeta, LuaSource};

const SLUG_MAX_LEN: usize = 64;

/// Reject anything but `[a-z0-9_-]+`. This is enforced because slugs are
/// embedded directly into filesystem paths — any path-traversal sneak
/// (`..`, `/`, `\`, NUL) would be a serious bug. Length-cap protects
/// against pathological NTFS issues.
pub fn validate_slug(slug: &str) -> AppResult<()> {
    if slug.is_empty() || slug.len() > SLUG_MAX_LEN {
        return Err(AppError::Other(format!(
            "invalid script slug `{slug}`: must be 1..={SLUG_MAX_LEN} chars"
        )));
    }
    let ok = slug
        .chars()
        .all(|c| matches!(c, 'a'..='z' | '0'..='9' | '_' | '-'));
    if !ok {
        return Err(AppError::Other(format!(
            "invalid script slug `{slug}`: allowed chars are a-z 0-9 _ -"
        )));
    }
    Ok(())
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn script_path(dir: &Path, slug: &str) -> PathBuf {
    dir.join(format!("{slug}.lua"))
}

fn meta_path(dir: &Path, slug: &str) -> PathBuf {
    dir.join(format!("{slug}.meta.json"))
}

fn read_user_meta(dir: &Path, slug: &str) -> LuaScriptMeta {
    let path = meta_path(dir, slug);
    match fs::read_to_string(&path) {
        Ok(s) => serde_json::from_str(&s).unwrap_or_else(|_| LuaScriptMeta {
            name: slug.to_string(),
            ..Default::default()
        }),
        Err(_) => LuaScriptMeta {
            name: slug.to_string(),
            ..Default::default()
        },
    }
}

fn write_user_meta(dir: &Path, slug: &str, meta: &LuaScriptMeta) -> AppResult<()> {
    let path = meta_path(dir, slug);
    let json = serde_json::to_vec_pretty(meta)?;
    fs::write(&path, json)?;
    Ok(())
}

/// Enumerate user scripts in `<lua_user_dir>/<gameId>/`. Returns an empty
/// list if the directory doesn't exist yet — never an error in that case.
pub fn list_user_scripts(paths: &AppPaths, game_id: &str) -> AppResult<Vec<LuaScript>> {
    let dir = paths.lua_user_dir(game_id);
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for entry in fs::read_dir(&dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("lua") {
            continue;
        }
        let Some(slug) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if validate_slug(slug).is_err() {
            continue;
        }
        let meta = read_user_meta(&dir, slug);
        let modified = entry
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64);
        out.push(LuaScript {
            slug: slug.to_string(),
            name: if meta.name.is_empty() {
                slug.to_string()
            } else {
                meta.name
            },
            source: LuaSource::User,
            description: meta.description,
            author: None,
            modified_unix_secs: modified,
            installed: true,
        });
    }
    out.sort_by_key(|a| a.name.to_lowercase());
    Ok(out)
}

pub fn read_user_script(paths: &AppPaths, game_id: &str, slug: &str) -> AppResult<String> {
    validate_slug(slug)?;
    let dir = paths.lua_user_dir(game_id);
    let path = script_path(&dir, slug);
    Ok(fs::read_to_string(&path)?)
}

/// Create or overwrite a user script. Always writes BOTH the `.lua` body and
/// the `.meta.json` sidecar (so the listing's `name` stays in sync). Sets
/// `modifiedUnixSecs` on every save; `createdUnixSecs` is preserved if the
/// sidecar already exists.
pub fn save_user_script(
    paths: &AppPaths,
    game_id: &str,
    slug: &str,
    name: &str,
    code: &str,
) -> AppResult<LuaScript> {
    validate_slug(slug)?;
    let dir = paths.lua_user_dir(game_id);
    fs::create_dir_all(&dir)?;

    let existing = read_user_meta(&dir, slug);
    let now = now_unix();
    let created = if existing.created_unix_secs == 0 {
        now
    } else {
        existing.created_unix_secs
    };

    let display_name = if name.trim().is_empty() {
        slug.to_string()
    } else {
        name.trim().to_string()
    };

    let meta = LuaScriptMeta {
        name: display_name.clone(),
        description: existing.description,
        created_unix_secs: created,
        modified_unix_secs: now,
    };

    fs::write(script_path(&dir, slug), code)?;
    write_user_meta(&dir, slug, &meta)?;

    Ok(LuaScript {
        slug: slug.to_string(),
        name: display_name,
        source: LuaSource::User,
        description: meta.description,
        author: None,
        modified_unix_secs: Some(now),
        installed: true,
    })
}

pub fn delete_user_script(paths: &AppPaths, game_id: &str, slug: &str) -> AppResult<()> {
    validate_slug(slug)?;
    let dir = paths.lua_user_dir(game_id);
    let s = script_path(&dir, slug);
    let m = meta_path(&dir, slug);
    if s.exists() {
        fs::remove_file(&s)?;
    }
    if m.exists() {
        fs::remove_file(&m)?;
    }
    Ok(())
}

/// Read a community script body. Returns the cached copy (must have been
/// installed first via `install_community_script`).
pub fn read_community_script(paths: &AppPaths, game_id: &str, slug: &str) -> AppResult<String> {
    validate_slug(slug)?;
    let dir = paths.lua_community_cache_dir(game_id);
    let path = script_path(&dir, slug);
    Ok(fs::read_to_string(&path)?)
}

pub fn write_community_script(
    paths: &AppPaths,
    game_id: &str,
    slug: &str,
    code: &str,
) -> AppResult<()> {
    validate_slug(slug)?;
    let dir = paths.lua_community_cache_dir(game_id);
    fs::create_dir_all(&dir)?;
    fs::write(script_path(&dir, slug), code)?;
    Ok(())
}

pub fn is_community_installed(paths: &AppPaths, game_id: &str, slug: &str) -> bool {
    if validate_slug(slug).is_err() {
        return false;
    }
    let dir = paths.lua_community_cache_dir(game_id);
    script_path(&dir, slug).exists()
}
