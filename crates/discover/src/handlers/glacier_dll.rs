//! `openforge-discover glacier-dll` — inject the per-game Glacier DLL and
//! drive the Tier-2 named-pipe stack end-to-end.
//!
//! This is the live validation harness for the injected backend: it attaches,
//! injects `glacier_007_dll.dll`, completes the handshake, enumerates modules,
//! and (optionally) runs a reflection smoke test against the in-process
//! `GlacierReflection` server — resolve a type, walk a live entity's
//! per-instance properties, resolve one by name, or write one via the guarded
//! `SetProperty` op.

use std::process::ExitCode;

use anyhow::{Result, anyhow};
use openforge_core::Target;
use openforge_glacier_host::GlacierSession;
use openforge_glacier_protocol::{GlacierValue, LogLevel};

use crate::cli::GlacierDllArgs;
use crate::context::DiscoverContext;
use crate::handlers::pick::parse_hex_addr;
use crate::term;

pub fn run(ctx: &DiscoverContext, args: &GlacierDllArgs) -> Result<ExitCode> {
    let candidates: Vec<&str> = ctx
        .manifest
        .game
        .process_names
        .iter()
        .map(String::as_str)
        .collect();
    let target = Target::attach_by_candidates(&candidates)?;
    term::header(&format!(
        "glacier-dll: {} (pid {})",
        target.process_name, target.pid
    ));

    let dll_path = match &args.dll {
        Some(p) => p.clone(),
        None => openforge_glacier_host::resolve_dll_path(&ctx.manifest.game.dll_file_name)
            .map_err(|e| anyhow!("cannot resolve Glacier DLL path: {e}"))?,
    };
    term::bullet(format!("injecting {} ...", dll_path.display()));

    let session = GlacierSession::attach_pid(target.pid, &dll_path)
        .map_err(|e| anyhow!("Glacier DLL session attach failed: {e}"))?;
    let _ = session.set_log_level(LogLevel::Debug);

    let w = session.welcome();
    term::ok(&format!(
        "handshake OK — server_version={} pid={} main_module=0x{:X} (+0x{:X})",
        w.server_version, w.pid, w.module_base, w.module_size
    ));
    session.ping().map_err(|e| anyhow!("ping failed: {e}"))?;
    term::ok("ping → pong");

    // EnumModules sanity: route a memory op across the pipe.
    {
        use openforge_core::Ctx;
        let m = session.main_module();
        term::bullet(format!(
            "main module via Ctx: {} base=0x{:X} text=+0x{:X} (0x{:X} bytes)",
            m.name, m.base, m.text_offset, m.text_size
        ));
    }

    if let Some(type_name) = &args.r#type {
        for name in type_name
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            match session.resolve_type(name) {
                Ok(Some(t)) => term::ok(&format!(
                    "type {name}: IType=0x{:X} STypeID=0x{:X} flags=0x{:04X} size={} ({})",
                    t.itype_va,
                    t.stypeid_va,
                    t.flags,
                    t.size,
                    if t.via_static { "static" } else { "registry" }
                )),
                Ok(None) => term::bullet(format!("type {name}: not found in registry")),
                Err(e) => term::bullet(format!("type {name}: error {e}")),
            }
        }
    }

    if let Some(addr_str) = &args.entity {
        let entity_va = parse_hex_addr(addr_str)? as u64;
        run_entity(&session, entity_va, args)?;
    }

    // Surface anything the DLL logged during this exchange.
    match session.drain_log(args.limit as u32) {
        Ok(lines) if !lines.is_empty() => {
            term::header("DLL log");
            for l in lines {
                term::dim(format!("  {l}"));
            }
        }
        Ok(_) => {}
        Err(e) => term::bullet(format!("drain_log: {e}")),
    }

    Ok(ExitCode::SUCCESS)
}

fn run_entity(session: &GlacierSession, entity_va: u64, args: &GlacierDllArgs) -> Result<()> {
    term::header(&format!("entity 0x{entity_va:X}"));

    match (&args.prop, &args.set) {
        // Set a property value.
        (Some(prop), Some(set_spec)) => {
            let value = parse_value(set_spec)?;
            term::bullet(format!("SetProperty {prop:?} = {value:?}"));
            match session.set_property(entity_va, prop, value) {
                Ok(true) => term::ok("write OK"),
                Ok(false) => term::bullet("property not present on this instance"),
                Err(e) => term::bullet(format!("refused/failed: {e}")),
            }
        }
        // Resolve a single property.
        (Some(prop), None) => match session.resolve_instance_property(entity_va, prop)? {
            Some(f) => {
                term::ok(&format!(
                    "{prop}: +0x{:X} crc=0x{:08X} type={} {}",
                    f.offset,
                    f.crc32,
                    f.type_name.as_deref().unwrap_or("?"),
                    if f.has_getter_setter {
                        "[getter/setter — raw write refused]"
                    } else {
                        ""
                    }
                ));
            }
            None => term::bullet(format!("property {prop:?} not present on this instance")),
        },
        // List every per-instance property.
        (None, _) => {
            let (obj_base, fields) = session.instance_properties(entity_va)?;
            term::bullet(format!(
                "obj_base=0x{obj_base:X}  properties={}",
                fields.len()
            ));
            for f in fields.iter().take(args.limit) {
                let gs = if f.has_getter_setter { " [g/s]" } else { "" };
                term::bullet(format!(
                    "+0x{:<5X} crc=0x{:08X} {:<40} {}{gs}",
                    f.offset,
                    f.crc32,
                    f.name.as_deref().unwrap_or("?"),
                    f.type_name.as_deref().unwrap_or("?")
                ));
            }
            if fields.len() > args.limit {
                term::dim(format!("  ... and {} more", fields.len() - args.limit));
            }
        }
    }
    Ok(())
}

/// Parse a `kind:value` spec into a [`GlacierValue`]. Integer values accept a
/// `0x` hex prefix; floats and bools use plain literals.
fn parse_value(spec: &str) -> Result<GlacierValue> {
    let (kind, raw) = spec
        .split_once(':')
        .ok_or_else(|| anyhow!("--set must be `kind:value` (e.g. `bool:true`, `i32:100`)"))?;
    let kind = kind.trim().to_ascii_lowercase();
    let raw = raw.trim();

    let parse_i = |r: &str| -> Result<i64> {
        if let Some(hex) = r.strip_prefix("0x").or_else(|| r.strip_prefix("0X")) {
            Ok(i64::from_str_radix(hex, 16)?)
        } else {
            Ok(r.parse::<i64>()?)
        }
    };
    let parse_u = |r: &str| -> Result<u64> {
        if let Some(hex) = r.strip_prefix("0x").or_else(|| r.strip_prefix("0X")) {
            Ok(u64::from_str_radix(hex, 16)?)
        } else {
            Ok(r.parse::<u64>()?)
        }
    };

    Ok(match kind.as_str() {
        "bool" => GlacierValue::Bool(matches!(raw, "1" | "true" | "TRUE" | "True")),
        "i8" => GlacierValue::I8(parse_i(raw)? as i8),
        "i16" => GlacierValue::I16(parse_i(raw)? as i16),
        "i32" => GlacierValue::I32(parse_i(raw)? as i32),
        "i64" => GlacierValue::I64(parse_i(raw)?),
        "u8" => GlacierValue::U8(parse_u(raw)? as u8),
        "u16" => GlacierValue::U16(parse_u(raw)? as u16),
        "u32" => GlacierValue::U32(parse_u(raw)? as u32),
        "u64" => GlacierValue::U64(parse_u(raw)?),
        "f32" => GlacierValue::F32(raw.parse::<f32>()?),
        "f64" => GlacierValue::F64(raw.parse::<f64>()?),
        other => return Err(anyhow!("unknown value kind {other:?}")),
    })
}
