//! Command handler for `openforge-discover ue5-find-object`.
//!
//! Uses the injected per-game DLL (e.g. `openforge-batman-lot-dll`) via
//! `Ue5Session` for reflection. The earlier external `UeEngine` fallback
//! was retired with the in-process DLL path; see git history if needed.

use std::process::ExitCode;

use anyhow::{Result, anyhow};
use openforge_core::Target;
use openforge_ue5_host::Ue5Session;

use crate::cli::Ue5FindObjectArgs;
use crate::context::DiscoverContext;
use crate::term;

pub fn run(ctx: &DiscoverContext, args: &Ue5FindObjectArgs) -> Result<ExitCode> {
    if !args.json {
        term::header(&format!(
            "ue5-find-object: {} / filter: \"{}\"",
            ctx.game_slug, args.name
        ));
    }

    let candidates: Vec<&str> = ctx
        .manifest
        .game
        .process_names
        .iter()
        .map(String::as_str)
        .collect();
    let target = Target::attach_by_candidates(&candidates)?;
    if !args.json {
        term::ok(&format!(
            "Attached to pid {} ({})",
            target.pid, target.process_name
        ));
        term::bullet("Injecting UE5 reflection DLL and connecting...");
    }

    let dll_path = openforge_ue5_host::resolve_dll_path(&ctx.manifest.game.dll_file_name)
        .map_err(|e| anyhow!("cannot resolve per-game DLL path: {e}"))?;
    let session = Ue5Session::attach_pid(target.pid, &dll_path)
        .map_err(|e| anyhow!("UE5 DLL session attach failed: {}", e))?;

    if !args.json {
        let w = session.welcome();
        term::ok(&format!(
            "DLL up — GUObjectArray=0x{:X}  FNamePool=0x{:X}  layout_validated={}",
            w.guobject_array, w.fname_pool, w.layout_validated
        ));
        term::bullet("Walking UObject array...");
    }
    let objects = session.walk_objects()?;

    // Compile regex
    let name_regex = regex::Regex::new(&format!("(?i){}", args.name))
        .map_err(|e| anyhow!("Invalid regex pattern: {}", e))?;
    let package_regex = if let Some(pat) = &args.package {
        Some(
            regex::Regex::new(&format!("(?i){}", pat))
                .map_err(|e| anyhow!("Invalid package regex: {}", e))?,
        )
    } else {
        None
    };

    let mut match_count = 0usize;
    let mut emitted = 0usize;
    for obj in objects.iter() {
        if let Some(pr) = &package_regex
            && !pr.is_match(&obj.fqn)
        {
            continue;
        }
        if !(name_regex.is_match(&obj.fqn) || name_regex.is_match(&obj.class_name)) {
            continue;
        }
        match_count += 1;
        if emitted >= args.limit {
            continue;
        }
        if args.json {
            let line = serde_json::json!({
                "addr": format!("0x{:X}", obj.addr),
                "class": obj.class_name,
                "fqn": obj.fqn,
            });
            println!("{}", line);
        } else {
            println!(
                "  0x{:012X}  {:<32} ({})",
                obj.addr, obj.fqn, obj.class_name
            );
        }
        emitted += 1;
    }

    if !args.json {
        term::ok(&format!(
            "Completed: found {} matches across {} live UObjects (printed {})",
            match_count,
            objects.len(),
            emitted
        ));
    }
    Ok(ExitCode::SUCCESS)
}
