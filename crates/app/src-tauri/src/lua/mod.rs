//! Lua script subsystem (Phase A: storage + parse-only validation +
//! community index fetch).
//!
//! The actual in-game Lua runtime that executes UE4SS-style scripts lives
//! in the per-game DLL (see crates/ue5-lua, Phase B). Until that lands,
//! `run_lua_script` returns `AppError::Other("runtime not yet integrated …")`.

pub mod community;
pub mod storage;
pub mod validate;

use serde::{Deserialize, Serialize};

/// Where a script came from. Bound by serde to the lowercase string form
/// because the FE state keys off these literal values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LuaSource {
    User,
    Community,
}

/// Listing entry for the sidebar. Does NOT include script body — call
/// `read_script` for that.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LuaScript {
    pub slug: String,
    pub name: String,
    pub source: LuaSource,
    pub description: Option<String>,
    pub author: Option<String>,
    /// Unix seconds. `None` for community entries not yet installed.
    pub modified_unix_secs: Option<i64>,
    /// `true` iff source = Community AND a cached `.lua` body exists on disk.
    /// Used by the FE to render "Install" vs "Open" CTAs.
    #[serde(default)]
    pub installed: bool,
}

/// Sidecar metadata stored alongside a user script.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct LuaScriptMeta {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    /// Unix seconds.
    #[serde(default)]
    pub created_unix_secs: i64,
    /// Unix seconds.
    #[serde(default)]
    pub modified_unix_secs: i64,
}

/// Result of a parse-only validation pass.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LuaValidation {
    pub ok: bool,
    #[serde(default)]
    pub errors: Vec<LuaParseError>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LuaParseError {
    /// 1-based line number. 0 if mlua didn't surface one.
    pub line: u32,
    pub message: String,
}
