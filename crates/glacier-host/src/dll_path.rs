//! Resolve where the per-game Glacier DLL lives on disk.
//!
//! Search order:
//! 1. `OPENFORGE_DLL_PATH` env var, if set (must be an existing file).
//! 2. Sibling of the current exe: `<current_exe_dir>/<file_name>`.
//! 3. Walk parents looking for `target/release/<file_name>`, then
//!    `target/debug/<file_name>` (so `cargo run` from a workspace Just Works).

use std::path::{Path, PathBuf};

use crate::error::{HostError, Result};

/// Environment-variable override for the DLL path.
pub const DLL_PATH_ENV: &str = "OPENFORGE_DLL_PATH";

/// Resolve a path to the DLL file named `file_name` (e.g.
/// `"glacier_007_dll.dll"`). See module docs for the search order.
pub fn resolve_dll_path(file_name: &str) -> Result<PathBuf> {
    if file_name.is_empty() {
        return Err(HostError::InjectionFailed(
            "resolve_dll_path called with empty file_name".into(),
        ));
    }

    if let Ok(env) = std::env::var(DLL_PATH_ENV)
        && !env.is_empty()
    {
        let p = PathBuf::from(env);
        if p.is_file() {
            return Ok(p);
        }
        return Err(HostError::DllNotFound(p));
    }

    let exe = std::env::current_exe().map_err(HostError::Io)?;
    let exe_dir = exe.parent().unwrap_or_else(|| Path::new("."));

    let sibling = exe_dir.join(file_name);
    if sibling.is_file() {
        return Ok(sibling);
    }

    if let Some(found) = find_in_target_tree(exe_dir, file_name) {
        return Ok(found);
    }

    if let Ok(cwd) = std::env::current_dir()
        && let Some(found) = find_in_target_tree(&cwd, file_name)
    {
        return Ok(found);
    }

    Err(HostError::DllNotFound(sibling))
}

/// Walk parents of `start` looking for `target/{release,debug}/<file_name>`.
fn find_in_target_tree(start: &Path, file_name: &str) -> Option<PathBuf> {
    let mut cur = Some(start);
    while let Some(dir) = cur {
        for profile in ["release", "debug"] {
            let candidate = dir.join("target").join(profile).join(file_name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
        cur = dir.parent();
    }
    None
}
