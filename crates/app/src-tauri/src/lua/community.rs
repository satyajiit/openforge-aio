//! Community Lua script index fetcher.
//!
//! Sources its data from the `community-lua-scripts/` directory inside the
//! OpenForge repo itself — no separate repo to maintain. For each game we
//! fetch:
//!
//! ```text
//! https://raw.githubusercontent.com/satyajiit/openforge-aio/main/community-lua-scripts/<gameId>/index.json
//! ```
//!
//! The index is a JSON `{ "scripts": [ { slug, name, description, author,
//! url } ] }`. Script bodies are pulled lazily by `install_community_script`
//! using the `url` field (defaults to `community-lua-scripts/<gameId>/<slug>.lua`
//! in the same repo). If a game's directory doesn't exist yet the fetcher
//! returns an empty list rather than erroring, so the UI always renders the
//! contribute-CTA cleanly.

use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};
use crate::paths::AppPaths;

use super::{LuaScript, LuaSource, storage};

const INDEX_URL_TEMPLATE: &str = "https://raw.githubusercontent.com/satyajiit/openforge-aio/main/community-lua-scripts/{game}/index.json";

const HTTP_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CommunityIndex {
    #[serde(default)]
    scripts: Vec<CommunityEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CommunityEntry {
    slug: String,
    name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    author: Option<String>,
    /// Raw URL to the `.lua` body. If omitted, defaults to
    /// `<repo>/games/<gameId>/<slug>.lua`.
    #[serde(default)]
    url: Option<String>,
}

fn cached_index_path(paths: &AppPaths, game_id: &str) -> PathBuf {
    paths.lua_community_cache_dir(game_id).join("index.json")
}

fn index_url(game_id: &str) -> String {
    INDEX_URL_TEMPLATE.replace("{game}", game_id)
}

fn default_script_url(game_id: &str, slug: &str) -> String {
    format!(
        "https://raw.githubusercontent.com/satyajiit/openforge-aio/main/community-lua-scripts/{game_id}/{slug}.lua"
    )
}

/// Fetch the community index for `game_id`. On HTTP success, persist the
/// raw JSON under `community-cache/index.json`. On any failure (network
/// down, repo doesn't exist yet, 404), fall back to the previously cached
/// index if present, else return an empty list — never propagate the error
/// to the UI as "broken".
pub fn refresh_index(paths: &AppPaths, game_id: &str) -> AppResult<Vec<LuaScript>> {
    let url = index_url(game_id);
    let cache_dir = paths.lua_community_cache_dir(game_id);
    std::fs::create_dir_all(&cache_dir)?;
    let cache_file = cached_index_path(paths, game_id);

    let fetched = http_get_index(&url);
    let index: CommunityIndex = match fetched {
        Ok(raw) => {
            // Persist verbatim — if the schema ever extends, we don't lose
            // fields we didn't deserialize.
            let _ = std::fs::write(&cache_file, raw.as_bytes());
            serde_json::from_str(&raw).unwrap_or(CommunityIndex {
                scripts: Vec::new(),
            })
        }
        Err(e) => {
            tracing::info!(
                game = %game_id,
                url = %url,
                error = %e,
                "community index fetch failed; falling back to cache"
            );
            match std::fs::read_to_string(&cache_file) {
                Ok(s) => serde_json::from_str(&s).unwrap_or(CommunityIndex {
                    scripts: Vec::new(),
                }),
                Err(_) => CommunityIndex {
                    scripts: Vec::new(),
                },
            }
        }
    };

    let mut out: Vec<LuaScript> = index
        .scripts
        .into_iter()
        .filter(|e| storage::validate_slug(&e.slug).is_ok())
        .map(|e| {
            let installed = storage::is_community_installed(paths, game_id, &e.slug);
            LuaScript {
                slug: e.slug.clone(),
                name: e.name,
                source: LuaSource::Community,
                description: e.description,
                author: e.author,
                modified_unix_secs: None,
                installed,
            }
        })
        .collect();
    out.sort_by_key(|a| a.name.to_lowercase());
    Ok(out)
}

/// Cache-only counterpart to `refresh_index`. Reads the cached
/// `index.json` if present and returns the listing without hitting the
/// network. Used by `list_lua_scripts` so opening the tab is fast.
pub fn refresh_index_from_cache(paths: &AppPaths, game_id: &str) -> AppResult<Vec<LuaScript>> {
    let cache_file = cached_index_path(paths, game_id);
    let raw = match std::fs::read_to_string(&cache_file) {
        Ok(s) => s,
        Err(_) => return Ok(Vec::new()),
    };
    let index: CommunityIndex = match serde_json::from_str(&raw) {
        Ok(i) => i,
        Err(_) => return Ok(Vec::new()),
    };
    let mut out: Vec<LuaScript> = index
        .scripts
        .into_iter()
        .filter(|e| storage::validate_slug(&e.slug).is_ok())
        .map(|e| {
            let installed = storage::is_community_installed(paths, game_id, &e.slug);
            LuaScript {
                slug: e.slug.clone(),
                name: e.name,
                source: LuaSource::Community,
                description: e.description,
                author: e.author,
                modified_unix_secs: None,
                installed,
            }
        })
        .collect();
    out.sort_by_key(|a| a.name.to_lowercase());
    Ok(out)
}

/// Fetch the `.lua` body for a given community slug and persist it to the
/// cache dir. Returns the updated listing entry (with `installed = true`).
pub fn install_script(paths: &AppPaths, game_id: &str, slug: &str) -> AppResult<LuaScript> {
    storage::validate_slug(slug)?;

    // Locate the URL via the cached index first; fall back to the default
    // path convention if the entry doesn't carry a custom URL.
    let cache_file = cached_index_path(paths, game_id);
    let (name, description, author, url) = match std::fs::read_to_string(&cache_file) {
        Ok(s) => {
            let idx: CommunityIndex = serde_json::from_str(&s).map_err(AppError::Json)?;
            let entry = idx
                .scripts
                .into_iter()
                .find(|e| e.slug == slug)
                .ok_or_else(|| {
                    AppError::Other(format!("script `{slug}` not in community index"))
                })?;
            (
                entry.name,
                entry.description,
                entry.author,
                entry
                    .url
                    .unwrap_or_else(|| default_script_url(game_id, slug)),
            )
        }
        Err(_) => (
            slug.to_string(),
            None,
            None,
            default_script_url(game_id, slug),
        ),
    };

    let body =
        http_get_text(&url).map_err(|e| AppError::Other(format!("fetch {url} failed: {e}")))?;
    storage::write_community_script(paths, game_id, slug, &body)?;

    Ok(LuaScript {
        slug: slug.to_string(),
        name,
        source: LuaSource::Community,
        description,
        author,
        modified_unix_secs: None,
        installed: true,
    })
}

fn http_get_index(url: &str) -> Result<String, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(HTTP_TIMEOUT)
        .user_agent("openforge")
        .build()
        .map_err(|e| e.to_string())?;
    let resp = client.get(url).send().map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }
    resp.text().map_err(|e| e.to_string())
}

fn http_get_text(url: &str) -> Result<String, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(HTTP_TIMEOUT)
        .user_agent("openforge")
        .build()
        .map_err(|e| e.to_string())?;
    let resp = client.get(url).send().map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }
    resp.text().map_err(|e| e.to_string())
}
