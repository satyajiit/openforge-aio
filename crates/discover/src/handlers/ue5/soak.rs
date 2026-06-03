//! `openforge-discover ue5-soak` — reproduce the app's attach-then-crash.
//!
//! The Tauri app holds ONE persistent DLL connection and continuously drives
//! reflection (per-feature `find_uobject` resolves, deref value reads, and a
//! re-resolve burst whenever a GC pass reallocates a cached object). One-shot
//! CLI commands (`ue5-find-object`, `ue5-dump-class`) connect → do one op →
//! disconnect, so they never reproduce the crash. This command mimics the app:
//! it keeps a single session open and hammers `find_uobject` + a property read
//! on an interval until either the soak window elapses or the pipe disconnects
//! (which, for this game, means the game process crashed).

use std::process::ExitCode;
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
use openforge_core::Target;
use openforge_ue5_host::Ue5Session;
use openforge_ue5_protocol::NamePredicate;

use crate::cli::Ue5SoakArgs;
use crate::context::DiscoverContext;
use crate::term;

pub fn run(ctx: &DiscoverContext, args: &Ue5SoakArgs) -> Result<ExitCode> {
    term::header(&format!(
        "ue5-soak: {} class=\"{}\" prop=\"{}\" {}s @ {}ms x{}/iter",
        ctx.game_slug, args.class, args.prop, args.secs, args.interval_ms, args.per_iter
    ));

    let candidates: Vec<&str> = ctx
        .manifest
        .game
        .process_names
        .iter()
        .map(String::as_str)
        .collect();
    let target = Target::attach_by_candidates(&candidates)?;
    let pid = target.pid;
    term::ok(&format!("Attached to pid {pid} ({})", target.process_name));

    let dll_path = openforge_ue5_host::resolve_dll_path(&ctx.manifest.game.dll_file_name)
        .map_err(|e| anyhow!("cannot resolve per-game DLL path: {e}"))?;
    let session = Ue5Session::attach_pid(pid, &dll_path)
        .map_err(|e| anyhow!("UE5 DLL session attach failed: {e}"))?;
    let w = session.welcome();
    term::ok(&format!(
        "DLL up — GUObjectArray=0x{:X} (persistent connection held for the whole soak)",
        w.guobject_array
    ));
    term::bullet("Soaking — driving find_uobject + property read on a loop...");

    let start = Instant::now();
    let mut iters: u64 = 0;
    let mut finds: u64 = 0;
    let mut reads: u64 = 0;
    let mut last_obj: u64 = 0;
    let mut realloc_seen: u64 = 0;

    while start.elapsed() < Duration::from_secs(args.secs) {
        iters += 1;
        for _ in 0..args.per_iter {
            match session.find_uobject(&args.class, NamePredicate::Any) {
                Ok(Some((obj, class_addr))) => {
                    finds += 1;
                    if obj != last_obj && last_obj != 0 {
                        realloc_seen += 1;
                    }
                    last_obj = obj;
                    match session.resolve_property(class_addr, &args.prop) {
                        Ok(Some(rp)) => {
                            let addr = obj.wrapping_add(rp.offset as u64);
                            match session.read_property(addr, rp.kind) {
                                Ok(_) => reads += 1,
                                Err(e) => {
                                    return crashed(
                                        ctx,
                                        &start,
                                        iters,
                                        finds,
                                        reads,
                                        "read_property",
                                        &e.to_string(),
                                    );
                                }
                            }
                        }
                        Ok(None) => {}
                        Err(e) => {
                            return crashed(
                                ctx,
                                &start,
                                iters,
                                finds,
                                reads,
                                "resolve_property",
                                &e.to_string(),
                            );
                        }
                    }
                }
                Ok(None) => {}
                Err(e) => {
                    return crashed(
                        ctx,
                        &start,
                        iters,
                        finds,
                        reads,
                        "find_uobject",
                        &e.to_string(),
                    );
                }
            }
        }
        if iters.is_multiple_of(8) {
            term::dim(format!(
                "  t={:>5.1}s iters={iters} finds={finds} reads={reads} obj-moves={realloc_seen} (last obj 0x{last_obj:X})",
                start.elapsed().as_secs_f64()
            ));
        }
        std::thread::sleep(Duration::from_millis(args.interval_ms));
    }

    term::ok(&format!(
        "Soak SURVIVED {:.1}s: {iters} iters, {finds} finds, {reads} reads, {realloc_seen} object-moves observed. Game still alive — this op pattern does NOT crash it.",
        start.elapsed().as_secs_f64()
    ));
    Ok(ExitCode::SUCCESS)
}

#[allow(clippy::too_many_arguments)]
fn crashed(
    _ctx: &DiscoverContext,
    start: &Instant,
    iters: u64,
    finds: u64,
    reads: u64,
    op: &str,
    err: &str,
) -> Result<ExitCode> {
    let disconnected = err.contains("pipe disconnected") || err.contains("disconnected");
    if disconnected {
        term::fail(
            "GAME CRASHED (pipe disconnected)",
            format!(
                "after {:.1}s during `{op}` — {iters} iters, {finds} finds, {reads} reads. \
                 This op pattern reproduced the crash. err={err}",
                start.elapsed().as_secs_f64()
            ),
        );
    } else {
        term::warn(
            &format!("`{op}` failed (not a disconnect)"),
            format!(
                "after {:.1}s — {iters} iters, {finds} finds, {reads} reads. err={err}",
                start.elapsed().as_secs_f64()
            ),
        );
    }
    Ok(ExitCode::FAILURE)
}
