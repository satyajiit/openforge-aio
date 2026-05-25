//! Command handler for `openforge-discover ue5-dump-class`.
//!
//! Uses the injected per-game OpenForge DLL (e.g. `openforge-batman-lod-dll`) via `Ue5Session` for reflection.

use std::collections::HashMap;
use std::process::ExitCode;

use anyhow::{Result, anyhow};
use openforge_core::Target;
use openforge_ue5_host::Ue5Session;

use crate::cli::Ue5DumpClassArgs;
use crate::context::DiscoverContext;
use crate::term;

pub fn run(ctx: &DiscoverContext, args: &Ue5DumpClassArgs) -> Result<ExitCode> {
    if !args.json {
        term::header(&format!(
            "ue5-dump-class: {} / class: \"{}\"",
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

    let class_regex = regex::Regex::new(&format!("(?i){}", args.name))
        .map_err(|e| anyhow!("Invalid class name regex: {}", e))?;
    let package_regex = if let Some(pat) = &args.package {
        Some(
            regex::Regex::new(&format!("(?i){}", pat))
                .map_err(|e| anyhow!("Invalid package regex: {}", e))?,
        )
    } else {
        None
    };

    // Collect matching instances, grouping by class_ptr so we dump each class
    // once with the full instance list.
    let mut by_class: HashMap<u64, (String, Vec<(u64, String)>)> = HashMap::new();
    let mut class_order: Vec<u64> = Vec::new();

    for obj in objects.iter() {
        if !class_regex.is_match(&obj.class_name) {
            continue;
        }
        if let Some(pr) = &package_regex
            && !pr.is_match(&obj.fqn)
        {
            continue;
        }
        if obj.class_ptr == 0 {
            continue;
        }
        let entry = by_class.entry(obj.class_ptr).or_insert_with(|| {
            class_order.push(obj.class_ptr);
            (obj.class_name.clone(), Vec::new())
        });
        entry.1.push((obj.addr, obj.fqn.clone()));
    }

    if class_order.is_empty() {
        if !args.json {
            term::fail(
                "Dump Failed",
                format!("no active class instances matched \"{}\"", args.name),
            );
        }
        return Ok(ExitCode::SUCCESS);
    }

    let mut printed = 0usize;
    for class_ptr in class_order {
        if printed >= args.limit {
            break;
        }
        let (class_name, instances) = match by_class.get(&class_ptr) {
            Some(v) => v,
            None => continue,
        };

        let props = session.walk_properties_cached(class_ptr)?;
        let funcs = session.walk_functions_cached(class_ptr)?;

        if args.json {
            let row = serde_json::json!({
                "class": class_name,
                "class_ptr": format!("0x{:X}", class_ptr),
                "instances": instances.iter()
                    .map(|(a, fqn)| serde_json::json!({"addr": format!("0x{:X}", a), "fqn": fqn}))
                    .collect::<Vec<_>>(),
                "properties": props.iter().map(|p| serde_json::json!({
                    "name": p.name,
                    "offset": p.offset,
                    "size": p.size,
                    "kind": p.kind,
                    "defined_in_class": p.defined_in_class,
                    "inherited": p.defined_in_class != *class_name,
                })).collect::<Vec<_>>(),
                "functions": funcs.iter().map(|f| serde_json::json!({
                    "name": f.name,
                    "addr": format!("0x{:X}", f.addr),
                    "native_func_ptr": format!("0x{:X}", f.native_func_ptr),
                    "is_native": f.is_native,
                    "defined_in_class": f.defined_in_class,
                    "inherited": f.defined_in_class != *class_name,
                })).collect::<Vec<_>>(),
            });
            println!("{}", row);
        } else {
            term::header(&format!(
                "class {} (class_ptr 0x{:X})",
                class_name, class_ptr
            ));
            println!("instances:");
            for (addr, fqn) in instances {
                println!("    0x{:X}  ({})", addr, fqn);
            }

            term::header(&format!("class properties for {}", class_name));
            for prop in props.iter() {
                let inherit = if prop.defined_in_class != *class_name {
                    format!(" (inherited from {})", prop.defined_in_class)
                } else {
                    String::new()
                };
                println!(
                    "  +0x{:04X}  {:<32} {:<16}{}",
                    prop.offset, prop.name, prop.kind, inherit
                );
            }

            term::header(&format!("class functions for {}", class_name));
            for func in funcs.iter() {
                let status_str = if func.is_native { "NATIVE" } else { "BP" };
                let inherit = if func.defined_in_class != *class_name {
                    format!(" (inherited from {})", func.defined_in_class)
                } else {
                    String::new()
                };
                println!(
                    "  {:<32} 0x{:012X}   {:<8}{}",
                    func.name, func.native_func_ptr, status_str, inherit
                );
            }
        }

        printed += 1;
    }

    // Keep `target` referenced so the OpenProcess handle stays alive for the
    // session lifetime. (The DLL holds its own handle to itself; this is for
    // any external reads we might add later via this handler.)
    let _ = target;

    Ok(ExitCode::SUCCESS)
}
