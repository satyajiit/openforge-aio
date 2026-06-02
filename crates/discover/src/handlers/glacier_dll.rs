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
            // Enumerate the type's static properties (its input pins / fields).
            if args.type_props {
                match session.enumerate_type_properties(name) {
                    Ok(Some(props)) if !props.is_empty() => {
                        term::ok(&format!("  {} static propert(y/ies):", props.len()));
                        for p in props.iter().take(args.limit) {
                            term::bullet(format!(
                                "    [{}] {:<40} crc=0x{:08X} flags=0x{:X} {}",
                                p.index,
                                p.name,
                                p.crc32,
                                p.flags,
                                p.type_name.as_deref().unwrap_or("?")
                            ));
                        }
                    }
                    Ok(_) => term::dim("  (no static properties / not a class type)"),
                    Err(e) => term::bullet(format!("  enumerate props error: {e}")),
                }
            }
        }
    }

    // Raw one-shot write (e.g. repoint a node's m_humanoid ref), then exit.
    if let Some(va_s) = &args.write {
        let va = parse_hex_addr(va_s)? as u64;
        let hex = args
            .write_hex
            .as_deref()
            .ok_or_else(|| anyhow!("--write requires --write-hex (LE bytes)"))?;
        let bytes: Vec<u8> = hex
            .as_bytes()
            .chunks(2)
            .filter_map(|c| std::str::from_utf8(c).ok())
            .filter_map(|h| u8::from_str_radix(h, 16).ok())
            .collect();
        if bytes.is_empty() {
            return Err(anyhow!("--write-hex parsed to zero bytes"));
        }
        let mut before = vec![0u8; bytes.len()];
        let _ = session.read_bytes(va as usize, &mut before);
        term::bullet(format!(
            "0x{va:X} before: {}",
            before
                .iter()
                .map(|b| format!("{b:02X}"))
                .collect::<String>()
        ));
        match session.write_bytes(va as usize, &bytes) {
            Ok(()) => term::ok(&format!(
                "wrote {} bytes @ 0x{va:X}: {}",
                bytes.len(),
                bytes.iter().map(|b| format!("{b:02X}")).collect::<String>()
            )),
            Err(e) => term::bullet(format!("write failed: {e}")),
        }
        return Ok(ExitCode::SUCCESS);
    }

    // Game-thread call of an arbitrary engine fn (RCX..R9 from --gtargs), exit.
    if let Some(fn_s) = &args.gtcall {
        let fn_va = parse_hex_addr(fn_s)? as u64;
        let gargs: Vec<u64> = match &args.gtargs {
            Some(s) => s
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| parse_hex_addr(s).map(|a| a as u64))
                .collect::<Result<_>>()?,
            None => Vec::new(),
        };
        term::header(&format!(
            "game-thread call fn=0x{fn_va:X} args={:X?} (fire during active gameplay)",
            gargs
        ));
        match session.game_thread_call(fn_va, gargs) {
            Ok(ret) => term::ok(&format!("returned RAX=0x{ret:X}")),
            Err(e) => term::bullet(format!("call failed: {e}")),
        }
        return Ok(ExitCode::SUCCESS);
    }

    // Restore a prior mass-set from its revert CSV, then exit.
    if let Some(path) = &args.restore {
        return run_restore(&session, path);
    }

    // Diff a prior position snapshot to find who moved, then exit.
    if let Some(path) = &args.diff {
        return run_diff(&session, path);
    }

    // Freeze one or more addresses (re-stamp bytes on a tight loop), then exit.
    // `--freeze-addr` accepts a comma-separated list so we can pin every copy of
    // a value at once (e.g. all heap mirrors of player health) to find which one
    // is authoritative.
    if let Some(addr_s) = &args.freeze_addr {
        let addrs: Vec<u64> = addr_s
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| parse_hex_addr(s).map(|a| a as u64))
            .collect::<Result<_>>()?;
        if addrs.is_empty() {
            return Err(anyhow!("--freeze-addr parsed to zero addresses"));
        }
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
        // Optional watch set (`--addrs` without `--peek`): while freezing, sample
        // each as an f32 and remember the minimum seen. If a watched (unfrozen)
        // copy never drops below full during a damage event, the authoritative
        // source it mirrors is in the frozen set — an OBJECTIVE gate test that
        // replaces noisy by-eye "invincible vs red flash" judgement.
        let watch: Vec<u64> = match args.addrs.as_deref() {
            Some(s) if args.peek.is_none() => s
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| parse_hex_addr(s).map(|a| a as u64))
                .collect::<Result<_>>()?,
            _ => Vec::new(),
        };
        let mut wmin = vec![f32::INFINITY; watch.len()];
        term::header(&format!(
            "freeze {} addr(s) = {bytes:02X?} for {}s (~30 Hz){}",
            addrs.len(),
            args.freeze_secs,
            if watch.is_empty() {
                String::new()
            } else {
                format!(", watching {} copy(ies)", watch.len())
            }
        ));
        let iters = args.freeze_secs.saturating_mul(30);
        let mut writes = 0u64;
        let mut skipped = 0u64;
        let guard = args.freeze_guard;
        for _ in 0..iters {
            for &addr in &addrs {
                // Crash-safe guard: read-before-write. A freed/reused/garbage
                // address (the broad-freeze crash cause) won't read a plausible
                // health f32, so skip it instead of corrupting memory.
                if let Some(gmax) = guard {
                    let mut cur = [0u8; 4];
                    if session.read_bytes(addr as usize, &mut cur).is_err() {
                        skipped += 1;
                        continue;
                    }
                    let v = f32::from_le_bytes(cur);
                    if !(v.is_finite() && v > 0.0 && v <= gmax) {
                        skipped += 1;
                        continue;
                    }
                }
                if session.write_bytes(addr as usize, &bytes).is_ok() {
                    writes += 1;
                }
            }
            for (i, &wa) in watch.iter().enumerate() {
                let mut b = [0u8; 4];
                if session.read_bytes(wa as usize, &mut b).is_ok() {
                    let v = f32::from_le_bytes(b);
                    if v.is_finite() && v < wmin[i] {
                        wmin[i] = v;
                    }
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(33));
        }
        term::ok(&format!(
            "freeze done: {writes} writes over {}s across {} addr(s){}",
            args.freeze_secs,
            addrs.len(),
            if guard.is_some() {
                format!(" ({skipped} guarded skips)")
            } else {
                String::new()
            }
        ));
        if !watch.is_empty() {
            term::ok("watch mins (unfrozen copies — 150.0 == held = source is frozen):");
            for (i, &wa) in watch.iter().enumerate() {
                let m = wmin[i];
                let held = m >= 149.5;
                println!(
                    "  0x{wa:X}  min={:.3}  {}",
                    m,
                    if held { "HELD" } else { "dropped" }
                );
            }
        }
        return Ok(ExitCode::SUCCESS);
    }

    // Snapshot an explicit address list (with --peek --snapshot-out), then exit.
    if let Some(addrs) = &args.addrs
        && args.peek.is_some()
    {
        let vas: Vec<u64> = addrs
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| parse_hex_addr(s).map(|a| a as u64))
            .collect::<Result<_>>()?;
        run_snapshot(&session, &vas, args)?;
        return Ok(ExitCode::SUCCESS);
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

    // In-process HW write-breakpoint: find the instruction writing an address.
    if let Some(s) = &args.find_writer {
        let addr = parse_hex_addr(s)? as u64;
        let secs = args.writer_secs;
        let exec = args.writer_width == 0;
        term::header(&format!(
            "find-{} @ 0x{addr:X} ({}, {secs}s) — trigger it in-game now",
            if exec { "exec" } else { "writer" },
            if exec {
                "EXECUTE bp".to_string()
            } else {
                format!("width {}", args.writer_width)
            }
        ));
        let hit = session.find_writer(addr, args.writer_width, (secs * 1000) as u32)?;
        let module_base = session.main_module().base as u64;
        if exec {
            // Execute trap: RIP IS the function entry; the regs below are the
            // real call's args (RCX/RDX/R8/R9 = the live calling convention).
            term::ok(&format!(
                "EXEC trap RIP=0x{:X} (main_module+0x{:X}, thread {})",
                hit.rip,
                hit.rip.saturating_sub(module_base),
                hit.thread_id
            ));
            let hex: String = hit
                .bytes
                .iter()
                .take(24)
                .map(|b| format!("{b:02X}"))
                .collect::<Vec<_>>()
                .join(" ");
            term::bullet(format!("prologue bytes: {hex}"));
        } else {
            let (writer_rip, writer_bytes) = align_to_writer(&hit.bytes, hit.bytes_start, hit.rip);
            term::ok(&format!(
                "captured: trap RIP=0x{:X} → writer RIP=0x{:X} (thread {})",
                hit.rip, writer_rip, hit.thread_id
            ));
            if writer_rip >= module_base {
                term::bullet(format!(
                    "writer is main_module+0x{:X}",
                    writer_rip - module_base
                ));
            }
            let hex: String = writer_bytes
                .iter()
                .take(24)
                .map(|b| format!("{b:02X}"))
                .collect::<Vec<_>>()
                .join(" ");
            term::bullet(format!("writer bytes: {hex}"));
        }
        let names = [
            "rax", "rbx", "rcx", "rdx", "rsi", "rdi", "rbp", "rsp", "r8", "r9", "r10", "r11",
            "r12", "r13", "r14", "r15",
        ];
        term::dim("registers @ trap:");
        for (n, v) in names.iter().zip(hit.regs.iter()) {
            term::dim(format!("    {n:<3} = 0x{v:016X}"));
        }
        return Ok(ExitCode::SUCCESS);
    }

    // Typed live explorer: identify / read-window / walk-chain, then exit.
    if let Some(s) = &args.ident {
        let va = parse_hex_addr(s)? as u64;
        return run_ident(&session, va);
    }
    if let Some(s) = &args.read {
        let va = parse_hex_addr(s)? as u64;
        return run_read(&session, va, args);
    }
    if let Some(s) = &args.deref {
        return run_deref(&session, s);
    }
    if args.survey_firearms {
        return run_survey_firearms(&session, args);
    }
    if args.list_firearms {
        return run_list_firearms(&session);
    }
    if let Some(filter) = &args.give_weapon {
        return run_give_weapon(&session, filter);
    }
    if let Some(s) = &args.watch {
        let va = parse_hex_addr(s)? as u64;
        return run_watch(&session, va, args.watch_len, args.watch_secs);
    }
    if let Some(s) = &args.snap {
        let va = parse_hex_addr(s)? as u64;
        let out = args
            .snapshot_out
            .clone()
            .unwrap_or_else(|| std::path::PathBuf::from("region_snap.csv"));
        return run_snap(&session, va, args.watch_len, &out);
    }
    if let Some(path) = &args.snap_diff {
        return run_snap_diff(&session, path);
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

/// Walk the captured byte window forward and locate the instruction that
/// finished just before `trap_rip` (HW data breakpoints trap AFTER the write).
/// Returns the writer's own RIP + its byte slice. Falls back to the raw window.
fn align_to_writer(raw: &[u8], bytes_start: u64, trap_rip: u64) -> (u64, Vec<u8>) {
    use iced_x86::{Decoder, DecoderOptions, Instruction};
    let max_skip = raw.len().min(15);
    for start in 0..=max_skip {
        if start >= raw.len() {
            break;
        }
        let slice = &raw[start..];
        let mut decoder =
            Decoder::with_ip(64, slice, bytes_start + start as u64, DecoderOptions::NONE);
        let mut instr = Instruction::default();
        while decoder.can_decode() {
            let pos = decoder.position();
            let ip = decoder.ip();
            decoder.decode_out(&mut instr);
            if instr.is_invalid() {
                break;
            }
            let next_ip = ip + instr.len() as u64;
            if next_ip == trap_rip {
                return (ip, raw[start + pos..].to_vec());
            }
            if next_ip > trap_rip {
                break;
            }
        }
    }
    (trap_rip, raw.to_vec())
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
    movers.sort_by_key(|m| std::cmp::Reverse(m.1));
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

// ---------------------------------------------------------------------------
// Typed live explorer (ident / read / deref) with MSVC RTTI naming.
//
// Module VAs are fixed (no ASLR on the main module) so reads of an object's
// vtable + the RTTI walk (vtable → COL → TypeDescriptor → name) all happen in
// the live process and match the dump. This turns blind pointer-chasing into a
// typed object-graph walk: at any live VA we can answer "what class is this?".
// ---------------------------------------------------------------------------

fn ru64(session: &GlacierSession, va: u64) -> Option<u64> {
    let mut b = [0u8; 8];
    session.read_bytes(va as usize, &mut b).ok()?;
    Some(u64::from_le_bytes(b))
}

fn ru32(session: &GlacierSession, va: u64) -> Option<u32> {
    let mut b = [0u8; 4];
    session.read_bytes(va as usize, &mut b).ok()?;
    Some(u32::from_le_bytes(b))
}

fn rcstr(session: &GlacierSession, va: u64, max: usize) -> Option<String> {
    let mut b = vec![0u8; max];
    session.read_bytes(va as usize, &mut b).ok()?;
    let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    if end == 0 {
        return None;
    }
    Some(b[..end].iter().map(|&c| c as char).collect())
}

fn module_bounds(session: &GlacierSession) -> (u64, u64) {
    let w = session.welcome();
    (w.module_base, w.module_base + w.module_size)
}

/// `.?AVZFoo@@` / `.?AUZBar@@` → `ZFoo` / `ZBar` (keep any `@Namespace`).
fn pretty(name: &str) -> &str {
    let s = name
        .strip_prefix(".?AV")
        .or_else(|| name.strip_prefix(".?AU"))
        .unwrap_or(name);
    s.strip_suffix("@@").unwrap_or(s)
}

/// Given a *vtable* VA, resolve the MSVC RTTI class name (mangled). `None` if
/// the vtable is outside the module or the COL fails validation.
fn rtti_for_vtable(session: &GlacierSession, base: u64, end: u64, vt: u64) -> Option<String> {
    if vt < base || vt >= end {
        return None;
    }
    let col = ru64(session, vt.wrapping_sub(8))?;
    if col < base || col >= end {
        return None;
    }
    if ru32(session, col)? != 1 {
        return None; // _RTTICompleteObjectLocator.signature (x64) == 1
    }
    if ru32(session, col + 0x14)? as u64 != col - base {
        return None; // pSelf RVA must point back at the COL
    }
    let td_rva = ru32(session, col + 0xC)? as u64;
    rcstr(session, base + td_rva + 0x10, 256)
}

/// Given an *object* VA, read its vtable and resolve its class name.
fn rtti_for_object(session: &GlacierSession, base: u64, end: u64, obj: u64) -> Option<String> {
    let vt = ru64(session, obj)?;
    rtti_for_vtable(session, base, end, vt)
}

fn run_ident(session: &GlacierSession, va: u64) -> Result<ExitCode> {
    let (base, end) = module_bounds(session);
    term::header(&format!("ident 0x{va:X}"));
    match ru64(session, va) {
        Some(vt) => {
            term::bullet(format!("vtable = 0x{vt:X}"));
            match rtti_for_vtable(session, base, end, vt) {
                Some(n) => term::ok(&format!("class = {} ({n})", pretty(&n))),
                None => term::bullet("no RTTI (vtable not in module / COL invalid)"),
            }
        }
        None => term::bullet("unreadable"),
    }
    Ok(ExitCode::SUCCESS)
}

fn run_read(session: &GlacierSession, va: u64, args: &GlacierDllArgs) -> Result<ExitCode> {
    let (base, end) = module_bounds(session);
    let len = args.peek_len.max(8) & !7; // whole qwords
    let mut buf = vec![0u8; len];
    session.read_bytes(va as usize, &mut buf)?;
    term::header(&format!("read 0x{va:X} ({len} bytes as qwords)"));
    for i in 0..(len / 8) {
        let q = u64::from_le_bytes(buf[i * 8..i * 8 + 8].try_into().unwrap());
        let mut note = String::new();
        if q >= base && q < end {
            note = match rtti_for_vtable(session, base, end, q) {
                Some(n) => format!("  vtable:{}", pretty(&n)),
                None => "  [module]".to_string(),
            };
        } else if q > 0x10000
            && q < 0x0000_8000_0000_0000
            && let Some(n) = rtti_for_object(session, base, end, q)
        {
            note = format!("  -> {}", pretty(&n));
        }
        let lo = (q & 0xFFFF_FFFF) as u32;
        let f = f32::from_bits(lo);
        let fnote = if f.is_finite() && f != 0.0 && f.abs() > 1e-3 && f.abs() < 1e7 {
            format!("  f32={f}")
        } else {
            String::new()
        };
        term::bullet(format!("+0x{:<3X} 0x{q:016X}{note}{fnote}", i * 8));
    }
    Ok(ExitCode::SUCCESS)
}

/// Read `len` bytes at `va` in 4 KiB chunks (the pipe caps a single read), so
/// a large region snapshot survives the per-op size limit. Unreadable chunks
/// are left zeroed.
fn read_region(session: &GlacierSession, va: u64, len: usize) -> Vec<u8> {
    let mut buf = vec![0u8; len];
    let mut off = 0usize;
    while off < len {
        let n = (len - off).min(0x1000);
        let _ = session.read_bytes((va + off as u64) as usize, &mut buf[off..off + n]);
        off += n;
    }
    buf
}

/// Snapshot a region, wait while the user moves (game focused), re-read, and
/// list floats that changed by a real amount — a jitter-filtered "what moved"
/// scan that does not depend on alt-tab timing (the wait spans the movement).
fn run_watch(session: &GlacierSession, va: u64, len: usize, secs: u64) -> Result<ExitCode> {
    let len = len & !3;
    term::header(&format!(
        "watch 0x{va:X} ({len} bytes) for {secs}s — MOVE NOW, keep the game focused"
    ));
    let a = read_region(session, va, len);
    std::thread::sleep(std::time::Duration::from_secs(secs));
    let b = read_region(session, va, len);
    report_changes(va, &a, &b);
    Ok(ExitCode::SUCCESS)
}

/// Compare two reads of the same region: list floats that changed by a real
/// amount (jitter-filtered) and i32s that decreased into a stat-like range.
fn report_changes(va: u64, a: &[u8], b: &[u8]) {
    let len = a.len().min(b.len());
    let mut movers: Vec<(u64, f32, f32, f32)> = Vec::new();
    let mut i = 0;
    while i + 4 <= len {
        let o = f32::from_le_bytes(a[i..i + 4].try_into().unwrap());
        let n = f32::from_le_bytes(b[i..i + 4].try_into().unwrap());
        if o.is_finite()
            && n.is_finite()
            && o.to_bits() != n.to_bits()
            && o.abs() < 1.0e6
            && n.abs() < 1.0e6
        {
            let d = (n - o).abs();
            if d > 0.05 {
                movers.push((va + i as u64, o, n, d));
            }
        }
        i += 4;
    }
    movers.sort_by(|x, y| y.3.partial_cmp(&x.3).unwrap_or(std::cmp::Ordering::Equal));
    term::ok(&format!("{} float(s) changed by >0.05:", movers.len()));
    for (addr, o, n, d) in movers.iter().take(40) {
        term::bullet(format!(
            "0x{addr:X} (+0x{:X})  {o:.3} -> {n:.3}  (Δ{d:.3})",
            addr - va
        ));
    }

    let mut dec: Vec<(u64, i32, i32)> = Vec::new();
    let mut chg = 0usize;
    let mut j = 0;
    while j + 4 <= len {
        let o = i32::from_le_bytes(a[j..j + 4].try_into().unwrap());
        let n = i32::from_le_bytes(b[j..j + 4].try_into().unwrap());
        if o != n && (1..=100_000).contains(&o) {
            if n < o && (0..=100_000).contains(&n) {
                dec.push((va + j as u64, o, n));
            } else {
                chg += 1;
            }
        }
        j += 4;
    }
    dec.sort_by_key(|m| std::cmp::Reverse(m.1 - m.2));
    term::ok(&format!(
        "{} i32(s) DECREASED (range 1..100000):",
        dec.len()
    ));
    for (addr, o, n) in dec.iter().take(40) {
        term::bullet(format!(
            "0x{addr:X} (+0x{:X})  i32 {o} -> {n}  (drop {})",
            addr - va,
            o - n
        ));
    }
    term::dim(format!("({chg} other i32 changes in range, hidden)"));

    // Pointer changes: 8-aligned qwords whose value changed to a plausible
    // heap/module pointer. A slot getting filled (old 0 -> new ptr) or replaced
    // (ptr -> different ptr) is exactly what a weapon-pickup writes into an
    // inventory slot. `filled` (was null) is the strongest signal.
    let plausible_ptr = |q: u64| (0x1_0000..0x0000_8000_0000_0000).contains(&q) && (q & 0x7) == 0;
    let mut filled: Vec<(u64, u64)> = Vec::new();
    let mut repl: Vec<(u64, u64, u64)> = Vec::new();
    let mut k = 0;
    while k + 8 <= len {
        let o = u64::from_le_bytes(a[k..k + 8].try_into().unwrap());
        let n = u64::from_le_bytes(b[k..k + 8].try_into().unwrap());
        if o != n && plausible_ptr(n) {
            if o == 0 {
                filled.push((va + k as u64, n));
            } else if plausible_ptr(o) {
                repl.push((va + k as u64, o, n));
            }
        }
        k += 8;
    }
    term::ok(&format!(
        "{} qword slot(s) FILLED (null -> ptr) — weapon-into-slot candidates:",
        filled.len()
    ));
    for (addr, n) in filled.iter().take(40) {
        term::bullet(format!("0x{addr:X} (+0x{:X})  0 -> 0x{n:X}", addr - va));
    }
    term::ok(&format!(
        "{} qword ptr(s) REPLACED (ptr -> ptr):",
        repl.len()
    ));
    for (addr, o, n) in repl.iter().take(30) {
        term::bullet(format!(
            "0x{addr:X} (+0x{:X})  0x{o:X} -> 0x{n:X}",
            addr - va
        ));
    }

    // Raw qword changes that are NEITHER float/i32-stat NOR clean pointers —
    // catches HANDLE changes (u32 idx+gen, e.g. 0x1_0000_1185), which is how
    // weapon-inventory slots reference weapons. Excludes the ptr cases above.
    let mut handles: Vec<(u64, u64, u64)> = Vec::new();
    let mut m = 0;
    while m + 8 <= len {
        let o = u64::from_le_bytes(a[m..m + 8].try_into().unwrap());
        let n = u64::from_le_bytes(b[m..m + 8].try_into().unwrap());
        if o != n && !plausible_ptr(n) && !plausible_ptr(o) {
            // handle-shaped: high dword small (gen) + low dword nonzero (idx),
            // or any non-pointer value flip. Keep it tight to weapon-ish handles.
            let looks_handle = (n >> 32) <= 0xFFFF && (n as u32) != 0 && (n as u32) > 0x100;
            if looks_handle {
                handles.push((va + m as u64, o, n));
            }
        }
        m += 8;
    }
    term::ok(&format!(
        "{} handle-shaped qword(s) changed (weapon-slot candidates):",
        handles.len()
    ));
    for (addr, o, n) in handles.iter().take(40) {
        term::bullet(format!(
            "0x{addr:X} (+0x{:X})  0x{o:X} -> 0x{n:X}",
            addr - va
        ));
    }
}

/// Phase 1 of a user-timed capture: read a region and persist it (header
/// `#<va> <len>` then hex) so a later [`run_snap_diff`] re-reads the exact bytes.
fn run_snap(session: &GlacierSession, va: u64, len: usize, out: &Path) -> Result<ExitCode> {
    let len = len & !3;
    term::header(&format!("snap 0x{va:X} ({len} bytes) -> {}", out.display()));
    let buf = read_region(session, va, len);
    let hex: String = buf.iter().map(|b| format!("{b:02X}")).collect();
    std::fs::write(out, format!("#{va} {len}\n{hex}\n"))?;
    term::ok("baseline saved — now take damage / move, then run --snap-diff");
    Ok(ExitCode::SUCCESS)
}

/// Phase 2: re-read the region saved by [`run_snap`] and report what changed.
fn run_snap_diff(session: &GlacierSession, path: &Path) -> Result<ExitCode> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| anyhow!("cannot read snap {}: {e}", path.display()))?;
    let mut lines = text.lines();
    let header = lines.next().unwrap_or("").trim_start_matches('#').trim();
    let mut it = header.split_whitespace();
    let va: u64 = it
        .next()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| anyhow!("snap header must be `#<va_dec> <len_dec>`, got {header:?}"))?;
    let len: usize = it
        .next()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| anyhow!("snap header missing len"))?;
    let a = hex_to_bytes(lines.next().unwrap_or("").trim());
    term::header(&format!(
        "snap-diff 0x{va:X} ({len} bytes) from {}",
        path.display()
    ));
    let b = read_region(session, va, len);
    report_changes(va, &a, &b);
    Ok(ExitCode::SUCCESS)
}

fn rvec3(session: &GlacierSession, va: u64) -> Option<[f32; 3]> {
    let mut b = [0u8; 12];
    session.read_bytes(va as usize, &mut b).ok()?;
    Some([
        f32::from_le_bytes(b[0..4].try_into().unwrap()),
        f32::from_le_bytes(b[4..8].try_into().unwrap()),
        f32::from_le_bytes(b[8..12].try_into().unwrap()),
    ])
}

/// Survey every `ZFirearmCharacterEntity` instance and report its world
/// translation from both spatial components (primary `+0x18`, secondary
/// `+0x60`; position is the translation row of the `SMatrix43` at
/// `spatial+0x40`, i.e. `spatial+0x64`). Classifies LOCAL/held (identity
/// rotation + zero translation) vs WORLD (dropped / world-anchored), sorted
/// nearest-first to the `--player-pawn` position. This is the discriminator
/// for finding a dropped weapon lying near the player.
fn run_survey_firearms(session: &GlacierSession, args: &GlacierDllArgs) -> Result<ExitCode> {
    const FIREARM_VT: u64 = 0x1_42DF_FCA8;
    const POS_OFF: u64 = 0x64;

    let player_pos = match &args.player_pawn {
        Some(p) => {
            let pawn = parse_hex_addr(p)? as u64;
            ru64(session, pawn + 0x1C8).and_then(|sp| rvec3(session, sp + POS_OFF))
        }
        None => None,
    };
    match player_pos {
        Some(pp) => term::header(&format!(
            "firearm survey — player @ ({:.2}, {:.2}, {:.2})",
            pp[0], pp[1], pp[2]
        )),
        None => term::header("firearm survey (no --player-pawn; distances omitted)"),
    }

    let hits = session.scan_heap_for_u64_labeled(FIREARM_VT, 8, "firearm_vt")?;
    let instances: Vec<u64> = hits
        .into_iter()
        .map(|h| h as u64)
        .filter(|&h| h >= 0x1_0000_0000)
        .collect();
    term::ok(&format!("{} firearm instance(s):", instances.len()));

    let mut rows: Vec<(f32, String)> = Vec::new();
    // (firearm_va, secondary_spatial_va) for every WORLD (dropped) firearm —
    // the summon targets.
    let mut world_secondary: Vec<(u64, u64)> = Vec::new();
    for &f in &instances {
        for (tag, off) in [("p", 0x18u64), ("s", 0x60u64)] {
            let Some(spatial) = ru64(session, f + off) else {
                continue;
            };
            let Some(pos) = rvec3(session, spatial + POS_OFF) else {
                continue;
            };
            if !pos.iter().all(|v| v.is_finite()) {
                continue;
            }
            let is_local = pos.iter().all(|v| v.abs() < 1.0e-3);
            if !is_local && off == 0x60 {
                world_secondary.push((f, spatial));
            }
            let dist = player_pos.map(|pp| {
                let d = [pos[0] - pp[0], pos[1] - pp[1], pos[2] - pp[2]];
                (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
            });
            let kind = if is_local { "LOCAL/held" } else { "WORLD" };
            let dstr = dist.map(|d| format!("  dist={d:.2}")).unwrap_or_default();
            rows.push((
                dist.unwrap_or(f32::INFINITY),
                format!(
                    "0x{f:X} [{tag} spatial 0x{spatial:X}+0x64] ({:.2}, {:.2}, {:.2})  {kind}{dstr}",
                    pos[0], pos[1], pos[2]
                ),
            ));
        }
    }
    rows.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    for (_, r) in rows.iter().take(args.limit) {
        term::bullet(r.clone());
    }

    // Summon: stack every dropped firearm into a vertical column at the player,
    // each 1.5 units higher on the 3rd axis.
    if args.summon_tower {
        let Some(pp) = player_pos else {
            term::bullet("--summon-tower needs --player-pawn (player position)");
            return Ok(ExitCode::SUCCESS);
        };
        term::header(&format!(
            "summon: stacking {} dropped firearm(s) into a column at the player",
            world_secondary.len()
        ));
        for (i, (f, spatial)) in world_secondary.iter().enumerate() {
            let target = [pp[0], pp[1], pp[2] + (i as f32 + 1.0) * 1.5];
            let mut bytes = [0u8; 12];
            bytes[0..4].copy_from_slice(&target[0].to_le_bytes());
            bytes[4..8].copy_from_slice(&target[1].to_le_bytes());
            bytes[8..12].copy_from_slice(&target[2].to_le_bytes());
            match session.write_bytes((spatial + POS_OFF) as usize, &bytes) {
                Ok(()) => term::ok(&format!(
                    "  0x{f:X} -> ({:.2}, {:.2}, {:.2})",
                    target[0], target[1], target[2]
                )),
                Err(e) => term::bullet(format!("  0x{f:X} write failed: {e}")),
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// Read a Glacier `ZString` whose 16-byte descriptor starts at `field_va`
/// (len bitfield u32 @+0, low 30 bits = length; `char*` @+0x8; chars at the
/// pointer directly). Returns the decoded (lossy) text, or `None`.
fn read_zstr(session: &GlacierSession, field_va: u64) -> Option<String> {
    let len = (ru64(session, field_va)? & 0x3FFF_FFFF) as usize;
    if len == 0 || len > 128 {
        return None;
    }
    let ptr = ru64(session, field_va + 8)?;
    if ptr < 0x1_0000 {
        return None;
    }
    let mut buf = vec![0u8; len];
    session.read_bytes(ptr as usize, &mut buf).ok()?;
    Some(
        String::from_utf8_lossy(&buf)
            .trim_end_matches('\0')
            .to_string(),
    )
}

/// One weapon-type group for [`run_list_firearms`]: the per-type
/// `ZFirearmAudioDefinition` ptr, the readable model name, and each instance's
/// `(firearm VA, handle idx, handle gen)`.
type FirearmGroup = (u64, String, Vec<(u64, u32, u32)>);

/// List every present `ZFirearmCharacterEntity` by readable weapon name,
/// grouped by type (shared `ZFirearmAudioDefinition` at `firearm+0xA8`). The
/// name is `m_firearmItemType` (ZString at `audio+0x18`, e.g.
/// `Pistol_WaltherPPK`); each instance's grant handle is `firearm+0x10`
/// (idx = low u32, gen = high u32) and its pickup node is `firearm-0x3B8`.
fn run_list_firearms(session: &GlacierSession) -> Result<ExitCode> {
    const FIREARM_VT: u64 = 0x1_42DF_FCA8;
    let hits = session.scan_heap_for_u64_labeled(FIREARM_VT, 8, "firearm_vt")?;
    let instances: Vec<u64> = hits
        .into_iter()
        .map(|h| h as u64)
        .filter(|&h| h >= 0x1_0000_0000)
        .collect();
    term::header(&format!(
        "{} firearm instance(s) — by weapon type",
        instances.len()
    ));

    // (audio_def, name, [(firearm, idx, gen)])
    let mut groups: Vec<FirearmGroup> = Vec::new();
    for &fw in &instances {
        let audio = ru64(session, fw + 0xA8).unwrap_or(0);
        let name = if audio != 0 {
            read_zstr(session, audio + 0x18).unwrap_or_else(|| "<unnamed>".into())
        } else {
            "<no-audio-def>".into()
        };
        let h = ru64(session, fw + 0x10).unwrap_or(0);
        let (idx, generation) = ((h & 0xFFFF_FFFF) as u32, (h >> 32) as u32);
        match groups.iter_mut().find(|g| g.0 == audio) {
            Some(g) => g.2.push((fw, idx, generation)),
            None => groups.push((audio, name, vec![(fw, idx, generation)])),
        }
    }
    for (audio, name, members) in &groups {
        term::ok(&format!(
            "{:<26} x{}   (audio-def 0x{:X})",
            name,
            members.len(),
            audio
        ));
        for (fw, idx, generation) in members {
            term::bullet(format!(
                "fw 0x{fw:X}  handle idx=0x{idx:X} gen=0x{generation:X}  node=0x{:X}",
                fw - 0x3B8
            ));
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// Grant a weapon by model name: scan present firearms, and for every one
/// whose `m_firearmItemType` matches `filter` (case-insensitive substring),
/// fire its pickup node (`firearm-0x3B8`) via the validated game-thread call to
/// the `ZCLTriggerPlayerItemPickup` handler `0x1415305F0`. Scan + fire happen
/// in this one invocation so nodes can't go stale between the two. The grant
/// takes for whichever matched firearm is in a grantable (dropped/available)
/// state; the rest harmlessly no-op / fault-skip.
fn run_give_weapon(session: &GlacierSession, filter: &str) -> Result<ExitCode> {
    const FIREARM_VT: u64 = 0x1_42DF_FCA8;
    const PICKUP_HANDLER: u64 = 0x1_4153_05F0;
    let needle = filter.to_lowercase();
    let hits = session.scan_heap_for_u64_labeled(FIREARM_VT, 8, "firearm_vt")?;
    let instances: Vec<u64> = hits
        .into_iter()
        .map(|h| h as u64)
        .filter(|&h| h >= 0x1_0000_0000)
        .collect();
    term::header(&format!(
        "give-weapon '{filter}' — firing pickup nodes of matching firearms (stay in active gameplay)"
    ));
    let (mut matched, mut fired) = (0u32, 0u32);
    for fw in instances {
        let audio = match ru64(session, fw + 0xA8) {
            Some(a) if a != 0 => a,
            _ => continue,
        };
        let name = read_zstr(session, audio + 0x18).unwrap_or_default();
        if name.is_empty() || !name.to_lowercase().contains(&needle) {
            continue;
        }
        matched += 1;
        let node = fw - 0x3B8;
        match session.game_thread_call(PICKUP_HANDLER, vec![node, node, 0, 0]) {
            Ok(_) => {
                fired += 1;
                term::ok(&format!("{name:<22} fw=0x{fw:X} node=0x{node:X} -> fired"));
            }
            Err(e) => term::dim(format!("{name:<22} fw=0x{fw:X} -> skip ({e})")),
        }
    }
    if matched == 0 {
        term::bullet(format!(
            "no present firearm matches '{filter}' — run --list-firearms to see names"
        ));
    } else {
        term::ok(&format!(
            "fired {fired}/{matched} matching pickup node(s) — check your loadout (toggle 1<->2)"
        ));
    }
    Ok(ExitCode::SUCCESS)
}

fn run_deref(session: &GlacierSession, spec: &str) -> Result<ExitCode> {
    let (base, end) = module_bounds(session);
    let mut parts = spec.split('+').map(str::trim).filter(|s| !s.is_empty());
    let base_s = parts
        .next()
        .ok_or_else(|| anyhow!("--deref needs `VA+off+off...`"))?;
    let mut cur = parse_hex_addr(base_s)? as u64;
    let offs: Vec<i64> = parts.map(parse_signed_offset).collect::<Result<_>>()?;
    term::header(&format!("deref {spec}"));
    let n0 = rtti_for_object(session, base, end, cur)
        .map(|n| pretty(&n).to_string())
        .unwrap_or_else(|| "?".to_string());
    term::bullet(format!("0x{cur:X}  [{n0}]"));
    for off in offs {
        let at = (cur as i64).wrapping_add(off) as u64;
        let Some(q) = ru64(session, at) else {
            term::bullet(format!("  +0x{off:X} @0x{at:X} unreadable"));
            break;
        };
        let name = rtti_for_object(session, base, end, q)
            .map(|n| pretty(&n).to_string())
            .unwrap_or_else(|| "?".to_string());
        term::bullet(format!("  +0x{off:X} @0x{at:X} -> 0x{q:X}  [{name}]"));
        cur = q;
    }
    Ok(ExitCode::SUCCESS)
}
