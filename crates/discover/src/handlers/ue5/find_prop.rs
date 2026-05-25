//! Command handler for `openforge-discover ue5-find-prop`.
//!
//! Uses the injected per-game OpenForge DLL (e.g. `openforge-batman-lod-dll`) via `Ue5Session` for reflection.

use std::collections::HashMap;
use std::process::ExitCode;

use anyhow::{Result, anyhow};
use openforge_core::{Ctx, Target};
use openforge_ue5_host::Ue5Session;
use openforge_ue5_protocol::UeObjectRef;

use crate::cli::{ScanType, Ue5FindPropArgs};
use crate::context::DiscoverContext;
use crate::term;
use crate::ue5::emit::emit_property_signature;

pub fn run(ctx: &DiscoverContext, args: &Ue5FindPropArgs) -> Result<ExitCode> {
    if !args.json {
        term::header(&format!(
            "ue5-find-prop: {} / filter: \"{}\"",
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

    // Compile regexes
    let name_regex = regex::Regex::new(&format!("(?i){}", args.name))
        .map_err(|e| anyhow!("Invalid property name regex: {}", e))?;
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

    // Group instances by class_ptr so we walk each class only once.
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
        term::header("matching properties");
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

        let props_arc = match session.walk_properties_cached(class_ptr) {
            Ok(p) => p,
            Err(_) => continue,
        };

        // Filter properties.
        let mut matched_props = Vec::new();
        for prop in props_arc.iter() {
            if !name_regex.is_match(&prop.name) {
                continue;
            }
            if let Some(st) = args.r#type {
                let match_t = match st {
                    ScanType::I8 | ScanType::U8 => prop.kind == "ByteProperty",
                    ScanType::I16 | ScanType::U16 => prop.kind == "IntProperty" && prop.size == 2,
                    ScanType::I32 | ScanType::U32 => prop.kind == "IntProperty" && prop.size == 4,
                    ScanType::I64 | ScanType::U64 => prop.kind == "Int64Property",
                    ScanType::F32 => prop.kind == "FloatProperty",
                    ScanType::F64 => prop.kind == "DoubleProperty",
                    ScanType::Bool => prop.kind == "BoolProperty",
                };
                if !match_t {
                    continue;
                }
            }
            matched_props.push(prop.clone());
        }

        if matched_props.is_empty() {
            continue;
        }

        if args.json {
            for inst in instances {
                for prop in &matched_props {
                    let row = serde_json::json!({
                        "class": class_name,
                        "instance": format!("0x{:X}", inst.addr),
                        "fqn": inst.fqn,
                        "prop": {
                            "name": prop.name,
                            "offset": prop.offset,
                            "size": prop.size,
                            "kind": prop.kind,
                            "defined_in_class": prop.defined_in_class,
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
            for prop in &matched_props {
                let mut val_str = String::new();
                if args.read_values && !instances.is_empty() {
                    // Read the value from the FIRST instance only (best effort).
                    let val_addr = instances[0].addr + prop.offset as u64;
                    val_str = read_prop_value(&target, prop.kind.as_str(), prop.size, val_addr);
                }
                println!(
                    "  +0x{:04X}  {:<32} {:<16}{}",
                    prop.offset, prop.name, prop.kind, val_str
                );
            }
        }

        // Emit signature: once, off the first matched prop of the first
        // surviving class. Use a real instance pointer for vtable read.
        if let Some(emit_id) = &args.emit_as
            && !emitted_signature
            && let Some(first_inst) = instances.first()
            && let Some(first_prop) = matched_props.first()
        {
            match emit_property_signature(
                &target,
                ctx,
                emit_id,
                class_name,
                first_inst.addr,
                first_prop,
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

fn read_prop_value(target: &Target, kind: &str, size: u32, val_addr: u64) -> String {
    let addr = val_addr as usize;
    match kind {
        "IntProperty" | "Int16Property" => {
            if size == 4 {
                let mut buf = [0u8; 4];
                if target.read_bytes(addr, &mut buf).is_ok() {
                    return format!(" = {}", i32::from_le_bytes(buf));
                }
            } else {
                let mut buf = [0u8; 2];
                if target.read_bytes(addr, &mut buf).is_ok() {
                    return format!(" = {}", i16::from_le_bytes(buf));
                }
            }
        }
        "Int64Property" => {
            let mut buf = [0u8; 8];
            if target.read_bytes(addr, &mut buf).is_ok() {
                return format!(" = {}", i64::from_le_bytes(buf));
            }
        }
        "FloatProperty" => {
            let mut buf = [0u8; 4];
            if target.read_bytes(addr, &mut buf).is_ok() {
                return format!(" = {}", f32::from_le_bytes(buf));
            }
        }
        "DoubleProperty" => {
            let mut buf = [0u8; 8];
            if target.read_bytes(addr, &mut buf).is_ok() {
                return format!(" = {}", f64::from_le_bytes(buf));
            }
        }
        "BoolProperty" => {
            let mut buf = [0u8; 1];
            if target.read_bytes(addr, &mut buf).is_ok() {
                return format!(" = {}", buf[0] != 0);
            }
        }
        "ByteProperty" => {
            let mut buf = [0u8; 1];
            if target.read_bytes(addr, &mut buf).is_ok() {
                return format!(" = {}", buf[0]);
            }
        }
        _ => {}
    }
    String::new()
}
