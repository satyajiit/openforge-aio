//! Golden-corpus parse stability test — the safety net for the config-driven
//! engine+format migration.
//!
//! Walks every shipped game's `manifest.toml` and `signatures/*.toml`, parses
//! them with the real [`GameManifest::parse`] / [`SignatureSpec::parse`] +
//! [`SignatureSpec::validate`] paths, and snapshots the `Debug` of every
//! parsed value into a single committed golden file. Any later phase
//! (FlexInt, the format-parser seam, RON, schema de-leak) that changes how the
//! existing TOML corpus deserializes will change this snapshot and fail the
//! test — so a drift is always a deliberate, reviewed `UPDATE_GOLDEN=1` step,
//! never silent.
//!
//! Lives in `src/` (not `tests/`) on purpose: CI runs `cargo test --lib`, which
//! skips integration tests. To regenerate after an intended schema change:
//! `UPDATE_GOLDEN=1 cargo test -p openforge-runtime --lib golden`.

use std::path::{Path, PathBuf};

use crate::format::ConfigFormat;
use crate::manifest::GameManifest;
use crate::signature::SignatureSpec;

/// `crates/games` relative to this crate's manifest dir (`crates/runtime`).
fn games_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("games")
}

fn golden_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("golden")
        .join("corpus.snapshot")
}

/// Forward-slash, workspace-relative display so the snapshot is identical on
/// every OS regardless of path separator.
fn rel_display(p: &Path) -> String {
    let games = games_dir();
    let stripped = p.strip_prefix(&games).unwrap_or(p);
    stripped.to_string_lossy().replace('\\', "/")
}

/// Shipped game directories (skip `_template` and any `_`-prefixed scaffold),
/// sorted for determinism.
fn game_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(games_dir())
        .expect("read crates/games")
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| !n.starts_with('_'))
        })
        .collect();
    dirs.sort();
    dirs
}

/// Collect every signature source file in `dir` — both `.toml` and `.ron` —
/// sorted by full filename so the snapshot order is deterministic and stable
/// across the two formats (`game_speed.ron` < `god_mode.toml`, etc.).
fn signature_files_in(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.path())
                .filter(|p| {
                    p.extension()
                        .and_then(|x| x.to_str())
                        .and_then(ConfigFormat::from_extension)
                        .is_some()
                })
                .collect()
        })
        .unwrap_or_default();
    files.sort();
    files
}

/// Outcome of walking + parsing the whole corpus.
struct Corpus {
    snapshot: String,
    manifests: usize,
    signatures: usize,
}

/// Build the full corpus snapshot (manifests first, then signatures, both in
/// stable path order), parsing through the real runtime paths.
fn build_corpus() -> Corpus {
    let mut snapshot = String::new();
    let mut manifests = 0usize;
    let mut signatures = 0usize;

    for game in game_dirs() {
        let manifest = game.join("manifest.toml");
        if manifest.is_file() {
            let text = std::fs::read_to_string(&manifest)
                .unwrap_or_else(|e| panic!("read {}: {e}", manifest.display()));
            let parsed = GameManifest::parse(&text)
                .unwrap_or_else(|e| panic!("parse manifest {}: {e}", manifest.display()));
            snapshot.push_str(&format!("=== MANIFEST {} ===\n", rel_display(&manifest)));
            snapshot.push_str(&format!("{parsed:#?}\n\n"));
            manifests += 1;
        }
    }

    for game in game_dirs() {
        for sig in signature_files_in(&game.join("signatures")) {
            let text = std::fs::read_to_string(&sig)
                .unwrap_or_else(|e| panic!("read {}: {e}", sig.display()));
            let fmt = sig
                .extension()
                .and_then(|x| x.to_str())
                .and_then(ConfigFormat::from_extension)
                .unwrap_or_else(|| panic!("unsupported signature extension: {}", sig.display()));
            let parsed = SignatureSpec::parse_with(&text, fmt)
                .unwrap_or_else(|e| panic!("parse signature {}: {e}", sig.display()));
            parsed
                .validate()
                .unwrap_or_else(|e| panic!("validate signature {}: {e}", sig.display()));
            snapshot.push_str(&format!("=== SIGNATURE {} ===\n", rel_display(&sig)));
            snapshot.push_str(&format!("{parsed:#?}\n\n"));
            signatures += 1;
        }
    }

    Corpus {
        snapshot,
        manifests,
        signatures,
    }
}

/// Normalize line endings so a CRLF checkout on Windows compares equal to the
/// LF-authored snapshot.
fn normalize(s: &str) -> String {
    s.replace("\r\n", "\n")
}

#[test]
fn corpus_parses_and_matches_golden() {
    let corpus = build_corpus();

    // Guard against a broken walk silently "passing" with an empty corpus.
    assert!(
        corpus.manifests >= 2,
        "expected >=2 game manifests, found {} — corpus walk is broken",
        corpus.manifests
    );
    assert!(
        corpus.signatures >= 20,
        "expected >=20 signatures, found {} — corpus walk is broken",
        corpus.signatures
    );

    let path = golden_path();

    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create golden dir");
        }
        std::fs::write(&path, corpus.snapshot.as_bytes()).expect("write golden snapshot");
        eprintln!(
            "UPDATE_GOLDEN: wrote {} ({} manifests, {} signatures)",
            path.display(),
            corpus.manifests,
            corpus.signatures
        );
        return;
    }

    let expected = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "golden snapshot missing ({}): {e}\n\
             generate it with: UPDATE_GOLDEN=1 cargo test -p openforge-runtime --lib golden",
            path.display()
        )
    });

    let expected_norm = normalize(&expected);
    let snapshot_norm = normalize(&corpus.snapshot);

    if expected_norm != snapshot_norm {
        // Find the first differing line to make the failure actionable.
        let exp_lines: Vec<&str> = expected_norm.lines().collect();
        let got_lines: Vec<&str> = snapshot_norm.lines().collect();
        let mut first_diff = None;
        for (i, (a, b)) in exp_lines.iter().zip(got_lines.iter()).enumerate() {
            if a != b {
                first_diff = Some((i + 1, a.to_string(), b.to_string()));
                break;
            }
        }
        let detail = match first_diff {
            Some((line, a, b)) => {
                format!("first difference at line {line}:\n  golden: {a}\n  parsed: {b}")
            }
            None => format!(
                "snapshots differ in length (golden {} lines, parsed {} lines)",
                exp_lines.len(),
                got_lines.len()
            ),
        };
        panic!(
            "corpus snapshot drifted from golden.\n{detail}\n\n\
             If this change is intentional, regenerate with:\n  \
             UPDATE_GOLDEN=1 cargo test -p openforge-runtime --lib golden"
        );
    }
}
