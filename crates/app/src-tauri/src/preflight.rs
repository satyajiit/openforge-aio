use openforge_core::ProcessSnapshot;
use openforge_runtime::Game;

use crate::elevation::is_elevated;
use crate::types::{PreflightCheck, PreflightReport};

pub fn evaluate(game: &dyn Game) -> PreflightReport {
    let snap = ProcessSnapshot::capture().ok();

    let running = snap
        .as_ref()
        .map(|s| {
            game.process_names()
                .iter()
                .any(|n| s.entries.iter().any(|p| p.name.eq_ignore_ascii_case(n)))
        })
        .unwrap_or(false);

    let admin = if game.requires_admin() {
        is_elevated()
    } else {
        true
    };

    let no_anti_cheat = snap
        .as_ref()
        .map(|s| {
            !game
                .forbidden_services()
                .iter()
                .any(|n| s.entries.iter().any(|p| p.name.eq_ignore_ascii_case(n)))
        })
        .unwrap_or(true);

    // We can only know the version after attach; preflight treats it as "ok"
    // (not blocking) and the attach command may emit a version_warning.
    let version_known_unblocking = true;

    PreflightReport {
        checks: vec![
            PreflightCheck {
                id: "running".into(),
                ok: running,
            },
            PreflightCheck {
                id: "version".into(),
                ok: version_known_unblocking,
            },
            PreflightCheck {
                id: "admin".into(),
                ok: admin,
            },
            PreflightCheck {
                id: "no_anti_cheat".into(),
                ok: no_anti_cheat,
            },
        ],
    }
}
