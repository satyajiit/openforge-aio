//! Command handler for `openforge-discover ue5-find-ufunc`.
//!
//! Uses the injected per-game OpenForge DLL (e.g. `openforge-batman-lod-dll`) via `Ue5Session` for reflection.

use std::collections::HashMap;
use std::process::ExitCode;

use anyhow::{Result, anyhow};
use openforge_core::Target;
use openforge_ue5_host::Ue5Session;
use openforge_ue5_protocol::UeObjectRef;

use crate::cli::Ue5FindUfuncArgs;
use crate::context::DiscoverContext;
use crate::term;
use crate::ue5::emit::emit_ufunction_signature;

pub fn run(ctx: &DiscoverContext, args: &Ue5FindUfuncArgs) -> Result<ExitCode> {
    if !args.json {
        term::header(&format!(
            "ue5-find-ufunc: {} / filter: \"{}\"",
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
    let layout_validated = session.welcome().layout_validated;
    if !args.json {
        term::ok(&format!("DLL up — layout_validated={}", layout_validated));
        term::bullet("Walking UObject array...");
    }
    let objects = session.walk_objects()?;

    let name_regex = regex::Regex::new(&format!("(?i){}", args.name))
        .map_err(|e| anyhow!("Invalid function name regex: {}", e))?;
    let class_filter = if let Some(class_pat) = &args.class {
        Some(
            regex::Regex::new(&format!("(?i){}", class_pat))
                .map_err(|e| anyhow!("Invalid class name regex: {}", e))?,
        )
    } else {
        None
    };
    let package_regex = if let Some(pat) = &args.package {
        Some(
            regex::Regex::new(&format!("(?i){}", pat))
                .map_err(|e| anyhow!("Invalid package regex: {}", e))?,
        )
    } else {
        None
    };

    // Group instances by class_ptr.
    let mut by_class: HashMap<u64, (String, Vec<&UeObjectRef>)> = HashMap::new();
    let mut class_order: Vec<u64> = Vec::new();
    for obj in objects.iter() {
        if obj.class_ptr == 0 {
            continue;
        }
        if let Some(cf) = &class_filter
            && !cf.is_match(&obj.class_name)
        {
            continue;
        }
        if let Some(pr) = &package_regex
            && !pr.is_match(&obj.fqn)
        {
            continue;
        }
        let entry = by_class.entry(obj.class_ptr).or_insert_with(|| {
            class_order.push(obj.class_ptr);
            (obj.class_name.clone(), Vec::new())
        });
        entry.1.push(obj);
    }

    if !args.json {
        term::header("matching functions");
    }

    let mut printed_classes = 0usize;
    let mut emitted_signature = false;

    for class_ptr in class_order {
        if printed_classes >= args.limit {
            break;
        }
        let (class_name, instances) = match by_class.get(&class_ptr) {
            Some(v) => v,
            None => continue,
        };

        let funcs_arc = match session.walk_functions_cached(class_ptr) {
            Ok(f) => f,
            Err(_) => continue,
        };

        let mut matched_funcs = Vec::new();
        for func in funcs_arc.iter() {
            if !name_regex.is_match(&func.name) {
                continue;
            }
            if args.native_only && !func.is_native {
                continue;
            }
            matched_funcs.push(func.clone());
        }

        if matched_funcs.is_empty() {
            continue;
        }

        if args.json {
            for inst in instances {
                for func in &matched_funcs {
                    let row = serde_json::json!({
                        "class": class_name,
                        "instance": format!("0x{:X}", inst.addr),
                        "fqn": inst.fqn,
                        "func": {
                            "name": func.name,
                            "addr": format!("0x{:X}", func.addr),
                            "native_func_ptr": format!("0x{:X}", func.native_func_ptr),
                            "is_native": func.is_native,
                            "defined_in_class": func.defined_in_class,
                        }
                    });
                    println!("{}", row);
                }
            }
        } else {
            println!("== class {} ({} instances) ==", class_name, instances.len());
            for inst in instances {
                println!("    instance 0x{:X}  ({})", inst.addr, inst.fqn);
            }
            for func in &matched_funcs {
                let status_str = if func.is_native { "NATIVE" } else { "BP" };
                println!(
                    "  {:<32} 0x{:012X}   {:<8}",
                    func.name, func.native_func_ptr, status_str
                );
            }
        }

        if let Some(emit_id) = &args.emit_as
            && !emitted_signature
            && let Some(first_native) = matched_funcs.iter().find(|f| f.is_native)
        {
            match emit_ufunction_signature(
                &target,
                ctx,
                emit_id,
                class_name,
                &first_native.name,
                first_native.native_func_ptr,
                layout_validated,
            ) {
                Ok(()) => {
                    if !args.json {
                        term::ok(&format!("Emitted signature for feature \"{}\"", emit_id));
                    }
                    emitted_signature = true;
                }
                Err(e) => {
                    if !args.json {
                        term::fail("Failed to emit signature", e);
                    } else {
                        eprintln!("emit failed: {}", e);
                    }
                }
            }
        }

        printed_classes += 1;
    }

    Ok(ExitCode::SUCCESS)
}
