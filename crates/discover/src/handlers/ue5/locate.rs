//! `openforge-discover ue5-locate` — signature-first reflection bootstrap over
//! external RPM (no DLL injection).
//!
//! This revives `crate::ue5::locate::UeEngine::attach`, the recompile-stable
//! discovery path (AOB + "Maximum number of UObjects" string-xref for
//! GUObjectArray, structural oracle for FNamePool, runtime offset probing).
//! It was orphaned when the `ue5-*` reflection commands moved to the injected
//! DLL — which carries only hardcoded build-specific RVAs and therefore breaks
//! on a game update or a different store binary.
//!
//! Running this against the live process reports the REAL GUObjectArray /
//! FNamePool addresses for the current build and the RVA drift vs the DLL's
//! `lotdk::ACTIVE` constants, which is the root-cause confirmation for a
//! post-update "0 live UObjects" failure.

use std::process::ExitCode;
use std::sync::Arc;

use anyhow::{Result, anyhow};
use openforge_core::Target;

use crate::cli::Ue5LocateArgs;
use crate::context::DiscoverContext;
use crate::term;
use crate::ue5::locate::UeEngine;

/// The GUObjectArray RVA hardcoded in the batman-lod DLL's `lotdk::ACTIVE`
/// (`crates/batman-lod-dll/src/lotdk.rs`). Kept here only to surface drift.
const BATMAN_LOD_GUOBJECT_RVA: usize = 0x0B65_C4A0;

pub fn run(ctx: &DiscoverContext, args: &Ue5LocateArgs) -> Result<ExitCode> {
    term::header(&format!(
        "ue5-locate: {} (signature-first, external RPM — no DLL inject)",
        ctx.game_slug
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
    let process_name = target.process_name.clone();
    let base = target.main().base;
    term::ok(&format!(
        "Attached to pid {pid} ({process_name}) — module base 0x{base:X}"
    ));
    term::bullet("Running signature-first GUObjectArray + FNamePool discovery...");

    let engine = UeEngine::attach(Arc::new(target))
        .map_err(|e| anyhow!("signature-first locate failed: {e}"))?;

    let guo_rva = engine.guobject_array.wrapping_sub(base);
    let fname_rva = engine.fname_pool.wrapping_sub(base);

    term::ok("Located UE5 reflection state via signatures:");
    println!(
        "  GUObjectArray       = 0x{:X}  (RVA 0x{:X})",
        engine.guobject_array, guo_rva
    );
    println!(
        "  FNamePool           = 0x{:X}  (RVA 0x{:X}, chunks_offset +0x{:X})",
        engine.fname_pool, fname_rva, engine.fname_pool_chunks_offset
    );
    println!(
        "  FUObjectItem stride = {} bytes",
        engine.fuobject_item_stride
    );
    println!(
        "  fproperty offsets validated = {}",
        engine.fproperty_offsets_validated
    );

    // Surface drift against the DLL's hardcoded RVA so the post-update break is
    // explicit. Only meaningful for batman-lod (the DLL that carries this RVA).
    if ctx.game_slug == "batman-lod" {
        if guo_rva == BATMAN_LOD_GUOBJECT_RVA {
            term::ok(&format!(
                "GUObjectArray RVA matches the DLL's hardcoded value (0x{BATMAN_LOD_GUOBJECT_RVA:X}) — no drift on this build."
            ));
        } else {
            let delta = guo_rva.abs_diff(BATMAN_LOD_GUOBJECT_RVA);
            let sign = if guo_rva > BATMAN_LOD_GUOBJECT_RVA {
                "+"
            } else {
                "-"
            };
            term::warn(
                "GUObjectArray RVA DRIFT vs DLL",
                format!(
                    "DLL hardcodes RVA 0x{BATMAN_LOD_GUOBJECT_RVA:X}, live build is RVA 0x{guo_rva:X} (delta {sign}0x{delta:X}). The injected DLL reads the stale address → walk returns 0 objects → every reflection feature reports \"no live instances\"."
                ),
            );
        }
    }

    if args.walk {
        term::bullet("Walking discovered GUObjectArray to prove names decode...");
        let objects = engine
            .walk_objects()
            .map_err(|e| anyhow!("walk_objects on discovered address failed: {e}"))?;
        term::ok(&format!("Live UObjects enumerated: {}", objects.len()));
        for obj in objects.iter().take(8) {
            println!(
                "  0x{:012X}  {:<44} ({})",
                obj.addr, obj.fqn, obj.class_name
            );
        }
    }

    Ok(ExitCode::SUCCESS)
}
