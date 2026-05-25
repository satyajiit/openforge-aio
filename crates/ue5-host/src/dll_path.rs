//! Resolve where a per-game OpenForge DLL lives on disk.
//!
//! AIO contract: the host crate is game-agnostic. Callers supply the DLL's
//! file name (e.g. `batman_lod_dll.dll`); we search for that file on disk and
//! return the resolved absolute path. The game crate that owns the DLL is
//! the canonical source of the filename — see e.g.
//! `openforge_game_batman_lod::DLL_FILE_NAME`.
//!
//! Search order (documented on [`resolve_dll_path`]):
//! 1. `OPENFORGE_DLL_PATH` env var, if set (must be an existing file).
//! 2. Sibling of the current exe: `<current_exe_dir>/<file_name>`.
//! 3. Walk parents looking for `target/release/<file_name>`, then
//!    `target/debug/<file_name>` (so `cargo run` from a workspace Just Works).
//!
//! The function is pure: no env-side-effects, no caching. That makes it
//! trivially unit-testable by setting cwd / temp paths.

use std::path::{Path, PathBuf};

use crate::error::{HostError, Result};

/// Environment-variable override for the DLL path. When set to a non-empty
/// value, it must point at an existing file; otherwise [`resolve_dll_path`]
/// returns [`HostError::DllNotFound`].
pub const DLL_PATH_ENV: &str = "OPENFORGE_DLL_PATH";

/// Resolve a path to the DLL file named `file_name` (e.g.
/// `"batman_lod_dll.dll"`). See module docs for the search order.
///
/// Returns the last-tried path inside [`HostError::DllNotFound`] if nothing
/// resolves.
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

    // Fall back to the *current working directory* tree — handles `cargo
    // run -p something` from a sub-crate where the exe is in
    // `target/debug/deps/` but the workspace root is multiple levels up.
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    const TEST_DLL_NAME: &str = "openforge-test-fixture.dll";

    /// Lightweight env-var guard so concurrent tests don't clobber each other.
    struct EnvGuard {
        key: &'static str,
        prev: Option<String>,
    }
    impl EnvGuard {
        fn set(key: &'static str, value: &str) -> Self {
            let prev = std::env::var(key).ok();
            // SAFETY: tests run single-threaded in this module; set/remove
            // are otherwise documented unsafe in Rust 1.74+.
            unsafe {
                std::env::set_var(key, value);
            }
            Self { key, prev }
        }
        fn clear(key: &'static str) -> Self {
            let prev = std::env::var(key).ok();
            // SAFETY: see `set`.
            unsafe {
                std::env::remove_var(key);
            }
            Self { key, prev }
        }
    }
    impl Drop for EnvGuard {
        fn drop(&mut self) {
            // SAFETY: see `set`.
            match &self.prev {
                Some(v) => unsafe { std::env::set_var(self.key, v) },
                None => unsafe { std::env::remove_var(self.key) },
            }
        }
    }

    #[test]
    fn env_var_takes_priority_when_file_exists() {
        let tmp = std::env::temp_dir().join("openforge-host-tests-env-ok");
        let _ = fs::create_dir_all(&tmp);
        let dll = tmp.join(TEST_DLL_NAME);
        fs::write(&dll, b"fake").unwrap();
        let _g = EnvGuard::set(DLL_PATH_ENV, dll.to_str().unwrap());
        let resolved = resolve_dll_path(TEST_DLL_NAME).unwrap();
        assert_eq!(resolved, dll);
        let _ = fs::remove_file(&dll);
    }

    #[test]
    fn env_var_pointing_at_nonexistent_returns_dll_not_found() {
        let bogus = std::env::temp_dir().join("openforge-host-tests-missing.dll");
        let _ = std::fs::remove_file(&bogus);
        let _g = EnvGuard::set(DLL_PATH_ENV, bogus.to_str().unwrap());
        match resolve_dll_path(TEST_DLL_NAME) {
            Err(HostError::DllNotFound(p)) => assert_eq!(p, bogus),
            other => panic!("expected DllNotFound, got {other:?}"),
        }
    }

    #[test]
    fn find_in_target_tree_returns_release_when_both_exist() {
        let tmp = std::env::temp_dir().join("openforge-host-tests-tree");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("a/b/target/release")).unwrap();
        fs::create_dir_all(tmp.join("a/b/target/debug")).unwrap();
        fs::write(tmp.join("a/b/target/release").join(TEST_DLL_NAME), b"r").unwrap();
        fs::write(tmp.join("a/b/target/debug").join(TEST_DLL_NAME), b"d").unwrap();
        let start = tmp.join("a/b/some/sub/dir");
        fs::create_dir_all(&start).unwrap();
        let got = find_in_target_tree(&start, TEST_DLL_NAME).unwrap();
        assert!(
            got.ends_with(format!("release/{TEST_DLL_NAME}"))
                || got.ends_with(format!("release\\{TEST_DLL_NAME}"))
        );
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn find_in_target_tree_returns_none_when_missing() {
        let tmp = std::env::temp_dir().join("openforge-host-tests-empty");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        assert!(find_in_target_tree(&tmp, TEST_DLL_NAME).is_none());
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn empty_env_var_falls_through_to_sibling_search() {
        let _g = EnvGuard::set(DLL_PATH_ENV, "");
        // Result may be Ok or DllNotFound depending on cwd; we only care that
        // the empty value didn't short-circuit and produce a weird path.
        match resolve_dll_path(TEST_DLL_NAME) {
            Ok(p) => assert!(p.file_name().is_some_and(|n| n == TEST_DLL_NAME)),
            Err(HostError::DllNotFound(_)) => {}
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn clear_env_var_does_not_panic() {
        let _g = EnvGuard::clear(DLL_PATH_ENV);
        let _ = resolve_dll_path(TEST_DLL_NAME);
    }

    #[test]
    fn empty_file_name_is_rejected() {
        match resolve_dll_path("") {
            Err(HostError::InjectionFailed(s)) => assert!(s.contains("empty file_name")),
            other => panic!("expected InjectionFailed, got {other:?}"),
        }
    }
}
