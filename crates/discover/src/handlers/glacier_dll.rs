//! `openforge-discover glacier-dll` — inject the per-game Glacier DLL and
//! drive the Tier-2 named-pipe stack end-to-end.
//!
//! This is the live validation harness for the injected backend: it attaches,
//! injects `glacier_007_dll.dll`, completes the handshake, enumerates modules,
//! and (optionally) runs a reflection smoke test against the in-process
//! `GlacierReflection` server — resolve a type, walk a live entity's
//! per-instance properties, resolve one by name, or write one via the guarded
//! `SetProperty` op.

use std::path::Path;
use std::process::ExitCode;

use anyhow::{Result, anyhow};
use openforge_core::{Ctx, Target};
use openforge_glacier_host::GlacierSession;
use openforge_glacier_protocol::{GlacierValue, LogLevel, NodeFire};

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

    // Restore a prior mass-set from its revert CSV, then exit.
    if let Some(path) = &args.restore {
        return run_restore(&session, path);
    }

    // Diff a prior position snapshot to find who moved, then exit.
    if let Some(path) = &args.diff {
        return run_diff(&session, path);
    }

    // Freeze an address (re-stamp bytes on a tight loop), then exit.
    if let Some(addr_s) = &args.freeze_addr {
        let addr = parse_hex_addr(addr_s)? as u64;
        let hex = args
            .freeze_hex
            .as_deref()
            .ok_or_else(|| anyhow!("--freeze-addr requires --freeze-hex (LE bytes)"))?;
        let bytes: Vec<u8> = hex
            .as_bytes()
            .chunks(2)
            .filter_map(|c| std::str::from_utf8(c).ok())
            .filter_map(|h| u8::from_str_radix(h, 16).ok())
            .collect();
        if bytes.is_empty() {
            return Err(anyhow!("--freeze-hex parsed to zero bytes"));
        }
        term::header(&format!(
            "freeze 0x{addr:X} = {bytes:02X?} for {}s (~30 Hz)",
            args.freeze_secs
        ));
        let iters = args.freeze_secs.saturating_mul(30);
        let mut writes = 0u64;
        for _ in 0..iters {
            if session.write_bytes(addr as usize, &bytes).is_ok() {
                writes += 1;
            }
            std::thread::sleep(std::time::Duration::from_millis(33));
        }
        term::ok(&format!(
            "freeze done: {writes} writes over {}s",
            args.freeze_secs
        ));
        return Ok(ExitCode::SUCCESS);
    }

    // Snapshot an explicit address list (with --peek --snapshot-out), then exit.
    if let Some(addrs) = &args.addrs {
        if args.peek.is_some() {
            let vas: Vec<u64> = addrs
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| parse_hex_addr(s).map(|a| a as u64))
                .collect::<Result<_>>()?;
            run_snapshot(&session, &vas, args)?;
            return Ok(ExitCode::SUCCESS);
        }
    }

    // In-proc heap scan for a u64 needle (e.g. a vtable VA), then exit.
    if let Some(v) = &args.scan_u64 {
        let needle = parse_hex_addr(v)? as u64;
        term::header(&format!(
            "heap-scan for u64 0x{needle:X} (align {})",
            args.scan_align
        ));
        let hits = session.scan_heap_for_u64_labeled(needle, args.scan_align, "scan_u64")?;
        term::ok(&format!("{} hit(s):", hits.len()));
        for h in hits.iter().take(args.limit) {
            term::bullet(format!("0x{h:X}"));
        }
        if hits.len() > args.limit {
            term::dim(format!("  ... and {} more", hits.len() - args.limit));
        }
        return Ok(ExitCode::SUCCESS);
    }

    if let Some(prop) = &args.find_prop {
        term::header(&format!("find entities with {prop:?} (max {})", args.max));
        match session.find_entities_with_property(prop, args.max) {
            Ok(vas) if vas.is_empty() => {
                term::bullet("no live entities carry that property right now");
            }
            Ok(vas) => {
                term::ok(&format!("{} entit(y/ies) found:", vas.len()));
                // Mass-set: apply the write to every match in this one session.
                if args.all && args.set.is_some() {
                    run_mass_set(&session, prop, args.set.as_deref().unwrap(), &vas, args)?;
                } else if args.all && args.peek.is_some() {
                    run_snapshot(&session, &vas, args)?;
                } else {
                    for va in &vas {
                        term::bullet(format!("0x{va:X}"));
                    }
                }
            }
            Err(e) => term::bullet(format!("find failed: {e}")),
        }
    }

    if let Some(fire_str) = &args.fire {
        let node_va = parse_hex_addr(fire_str)? as u64;
        let (fire, pin_label) = match &args.pin {
            Some(p) => {
                let id = parse_u32(p)?;
                (NodeFire::SignalInputPin(id), format!("0x{id:X}"))
            }
            None => (NodeFire::Activate, "Activate (0x4F1066FB)".to_string()),
        };
        term::header(&format!("fire node 0x{node_va:X} — pin {pin_label}"));
        match session.fire_node(node_va, Vec::new(), fire) {
            Ok(true) => term::ok("SignalInputPin call returned (no crash)"),
            Ok(false) => term::bullet("node not present / nothing fired"),
            Err(e) => term::bullet(format!("fire failed: {e}")),
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

    // Raw window dump at entity_va + offset, decoded as f32/i32 per 4 bytes.
    if let Some(off_s) = &args.peek {
        let off = parse_signed_offset(off_s)?;
        let addr = (entity_va as i64).wrapping_add(off) as u64;
        let mut buf = vec![0u8; args.peek_len];
        session.read_bytes(addr as usize, &mut buf)?;
        term::ok(&format!(
            "peek 0x{:X} (+0x{off:X}, {} bytes)",
            addr, args.peek_len
        ));
        for (i, chunk) in buf.chunks(4).enumerate() {
            if chunk.len() < 4 {
                break;
            }
            let b: [u8; 4] = chunk.try_into().unwrap();
            let off_here = off + (i as i64) * 4;
            let f = f32::from_le_bytes(b);
            let iv = i32::from_le_bytes(b);
            let plausible = f.is_finite() && f != 0.0 && f.abs() > 0.01 && f.abs() < 1.0e7;
            term::bullet(format!(
                "+0x{:<4X} {:02X?}  i32={:<12} f32={}{}",
                off_here,
                b,
                iv,
                f,
                if plausible { "  <- coord?" } else { "" }
            ));
        }
        return Ok(());
    }

    match (&args.prop, &args.set) {
        // Set a property value, with a before/after read-back.
        (Some(prop), Some(set_spec)) => {
            let value = parse_value(set_spec)?;
            let before = session
                .resolve_instance_property(entity_va, prop)?
                .and_then(|f| read_field_value(session, entity_va, f.offset));
            if let Some(b) = &before {
                term::bullet(format!("before: {b}"));
            }
            term::bullet(format!("SetProperty {prop:?} = {value:?}"));
            match session.set_property(entity_va, prop, value) {
                Ok(true) => {
                    term::ok("write OK");
                    if let Some(f) = session.resolve_instance_property(entity_va, prop)?
                        && let Some(a) = read_field_value(session, entity_va, f.offset)
                    {
                        term::ok(&format!("after:  {a}"));
                    }
                }
                Ok(false) => term::bullet("property not present on this instance"),
                Err(e) => term::bullet(format!("refused/failed: {e}")),
            }
        }
        // Resolve a single property + read its live value.
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
                if let Some(v) = read_field_value(session, entity_va, f.offset) {
                    term::bullet(format!("live value: {v}"));
                }
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

/// Mass-set: write `set_spec` to `prop` on EVERY matched entity in this one
/// pipe session, snapshotting each original 8 bytes at the value address to a
/// revert CSV first so the whole change can be undone with `--restore`.
fn run_mass_set(
    session: &GlacierSession,
    prop: &str,
    set_spec: &str,
    vas: &[u64],
    args: &GlacierDllArgs,
) -> Result<()> {
    let value = parse_value(set_spec)?;
    let revert_path = args
        .revert_out
        .clone()
        .unwrap_or_else(|| std::path::PathBuf::from("glacier_revert.csv"));
    term::header(&format!(
        "mass-set {prop:?} = {value:?} on {} entities (revert -> {})",
        vas.len(),
        revert_path.display()
    ));

    let mut csv =
        String::from("# addr_hex,orig_bytes_hex (restore with: glacier-dll --restore <this>)\n");
    let mut written = 0usize;
    let mut refused = 0usize;
    let mut missing = 0usize;
    for &va in vas {
        // Resolve the per-instance offset, snapshot the original bytes, write.
        let field = match session.resolve_instance_property(va, prop) {
            Ok(Some(f)) => f,
            Ok(None) => {
                missing += 1;
                continue;
            }
            Err(_) => {
                missing += 1;
                continue;
            }
        };
        let addr = va.wrapping_add(8).wrapping_add(field.offset as u64);
        let mut orig = [0u8; 8];
        if session.read_bytes(addr as usize, &mut orig).is_ok() {
            let hex: String = orig.iter().map(|b| format!("{b:02X}")).collect();
            csv.push_str(&format!("0x{addr:X},{hex}\n"));
        }
        match session.set_property(va, prop, value.clone()) {
            Ok(true) => written += 1,
            Ok(false) => missing += 1,
            Err(_) => refused += 1,
        }
    }
    std::fs::write(&revert_path, csv)?;
    term::ok(&format!(
        "mass-set done: {written} written, {refused} refused, {missing} missing. Revert CSV: {}",
        revert_path.display()
    ));
    term::dim(format!(
        "  undo with: openforge-discover glacier-dll --game {} --restore {}",
        args.game,
        revert_path.display()
    ));
    Ok(())
}

/// Snapshot a raw byte window (`entity_va + offset`, `peek_len` bytes) for
/// every matched entity to `--snapshot-out` for a later [`run_diff`]. The file
/// header records the window so the diff re-reads the exact same bytes.
fn run_snapshot(session: &GlacierSession, vas: &[u64], args: &GlacierDllArgs) -> Result<()> {
    let off = parse_signed_offset(args.peek.as_deref().unwrap())?;
    let len = args.peek_len;
    let out = args
        .snapshot_out
        .clone()
        .unwrap_or_else(|| std::path::PathBuf::from("glacier_snapshot.csv"));
    term::header(&format!(
        "snapshot window +0x{off:X} ({len} bytes) for {} entities -> {}",
        vas.len(),
        out.display()
    ));
    let mut csv = format!("#{off} {len}\n");
    let mut ok = 0usize;
    for &va in vas {
        let addr = (va as i64).wrapping_add(off) as u64;
        let mut buf = vec![0u8; len];
        if session.read_bytes(addr as usize, &mut buf).is_ok() {
            let hex: String = buf.iter().map(|b| format!("{b:02X}")).collect();
            csv.push_str(&format!("0x{va:X},{hex}\n"));
            ok += 1;
        }
    }
    std::fs::write(&out, csv)?;
    term::ok(&format!("snapshot written: {ok}/{} entities", vas.len()));
    Ok(())
}

/// Re-read each VA's window from a snapshot and rank entities by the largest
/// absolute f32 delta in the window — the "who moved" discriminator. The
/// player is the humanoid that moved most when the user moved.
fn run_diff(session: &GlacierSession, path: &Path) -> Result<ExitCode> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| anyhow!("cannot read snapshot {}: {e}", path.display()))?;
    let mut lines = text.lines();
    let header = lines.next().unwrap_or("");
    let (off, len) = parse_snapshot_header(header)
        .ok_or_else(|| anyhow!("snapshot header must be `#<off> <len>`, got {header:?}"))?;
    term::header(&format!(
        "diff window +0x{off:X} ({len} bytes) from {}",
        path.display()
    ));

    let mut movers: Vec<(u64, usize, Vec<u8>, Vec<u8>)> = Vec::new();
    let mut read_ok = 0usize;
    for line in lines {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((va_s, hex_s)) = line.split_once(',') else {
            continue;
        };
        let va = parse_hex_addr(va_s.trim())? as u64;
        let old = hex_to_bytes(hex_s.trim());
        let addr = (va as i64).wrapping_add(off) as u64;
        let mut buf = vec![0u8; len];
        if session.read_bytes(addr as usize, &mut buf).is_err() {
            continue;
        }
        read_ok += 1;
        // Raw byte-change count over the window (interpretation-agnostic).
        let changed = old.iter().zip(buf.iter()).filter(|(o, n)| o != n).count();
        if changed > 0 {
            movers.push((va, changed, old, buf));
        }
    }
    movers.sort_by(|a, b| b.1.cmp(&a.1));
    term::ok(&format!(
        "re-read {read_ok} entities; {} changed (raw bytes); top movers:",
        movers.len()
    ));
    for (va, changed, old, new) in movers.iter().take(15) {
        term::bullet(format!("0x{va:X}  {changed}/{len} bytes changed"));
        // Show the f32-interpreted changes at coord magnitude.
        for i in (0..old.len().min(new.len())).step_by(4) {
            if i + 4 > old.len() {
                break;
            }
            let o = f32::from_le_bytes(old[i..i + 4].try_into().unwrap());
            let n = f32::from_le_bytes(new[i..i + 4].try_into().unwrap());
            if o.to_bits() != n.to_bits() && o.is_finite() && n.is_finite() && n.abs() < 1.0e7 {
                term::dim(format!("    +0x{:<4X} {o:.3} -> {n:.3}", off + (i as i64)));
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn parse_snapshot_header(h: &str) -> Option<(i64, usize)> {
    let h = h.trim_start_matches('#').trim();
    let mut it = h.split_whitespace();
    let off = it.next()?.parse::<i64>().ok()?;
    let len = it.next()?.parse::<usize>().ok()?;
    Some((off, len))
}

fn hex_to_bytes(hex: &str) -> Vec<u8> {
    hex.as_bytes()
        .chunks(2)
        .filter_map(|c| std::str::from_utf8(c).ok())
        .filter_map(|h| u8::from_str_radix(h, 16).ok())
        .collect()
}

/// Parse a signed offset (`0x...`/decimal, optional leading `-`).
fn parse_signed_offset(s: &str) -> Result<i64> {
    let t = s.trim();
    let (neg, t) = t.strip_prefix('-').map(|r| (true, r)).unwrap_or((false, t));
    let v = if let Some(hex) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        i64::from_str_radix(hex, 16)?
    } else {
        t.parse::<i64>()?
    };
    Ok(if neg { -v } else { v })
}

/// Restore raw bytes from a revert CSV (`addr_hex,orig_bytes_hex`) produced by
/// [`run_mass_set`]. Writes each original byte image back over the pipe.
fn run_restore(session: &GlacierSession, path: &Path) -> Result<ExitCode> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| anyhow!("cannot read revert CSV {}: {e}", path.display()))?;
    term::header(&format!("restore from {}", path.display()));
    let mut restored = 0usize;
    let mut failed = 0usize;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((addr_s, bytes_s)) = line.split_once(',') else {
            continue;
        };
        let addr = parse_hex_addr(addr_s.trim())? as u64;
        let bytes: Vec<u8> = bytes_s
            .trim()
            .as_bytes()
            .chunks(2)
            .filter_map(|c| std::str::from_utf8(c).ok())
            .filter_map(|h| u8::from_str_radix(h, 16).ok())
            .collect();
        if bytes.is_empty() {
            continue;
        }
        match session.write_bytes(addr as usize, &bytes) {
            Ok(()) => restored += 1,
            Err(_) => failed += 1,
        }
    }
    term::ok(&format!(
        "restore done: {restored} restored, {failed} failed"
    ));
    Ok(ExitCode::SUCCESS)
}

/// Read the 8 raw bytes at a resolved field's live address (`obj_base +
/// offset`, where `obj_base == entity_va + 8`) and format the common
/// interpretations. Returns `None` if the address is unreadable.
fn read_field_value(session: &GlacierSession, entity_va: u64, offset: i64) -> Option<String> {
    use openforge_core::Ctx;
    let obj_base = entity_va.wrapping_add(8);
    let addr = (obj_base as i64).wrapping_add(offset) as u64;
    let mut buf = [0u8; 8];
    session.read_bytes(addr as usize, &mut buf).ok()?;
    let u = u64::from_le_bytes(buf);
    let i32v = i32::from_le_bytes(buf[..4].try_into().unwrap());
    let f32v = f32::from_bits(u32::from_le_bytes(buf[..4].try_into().unwrap()));
    Some(format!(
        "0x{addr:X}: {:02X?}  (i32={i32v} u64=0x{u:X} f32={f32v})",
        buf
    ))
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

/// Parse a u32 pin id: decimal, or `0x`-prefixed hex.
fn parse_u32(s: &str) -> Result<u32> {
    let t = s.trim();
    if let Some(hex) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        Ok(u32::from_str_radix(hex, 16)?)
    } else {
        Ok(t.parse::<u32>()?)
    }
}
