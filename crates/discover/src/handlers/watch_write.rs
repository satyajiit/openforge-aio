use std::process::ExitCode;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use iced_x86::{Decoder, DecoderOptions, Instruction};
use openforge_core::ProcessSnapshot;

use crate::cli::WatchWriteArgs;
use crate::context::DiscoverContext;
use crate::elevation;
use crate::handlers::pick::parse_hex_addr;
use crate::session::{HistoryStep, Session};
use crate::term;
use crate::watch_write;

pub fn run(ctx: &DiscoverContext, args: &WatchWriteArgs) -> Result<ExitCode> {
    if !elevation::is_admin() {
        term::warn(
            "not elevated",
            "Win32 debugger attach typically requires admin. Run from an elevated shell.",
        );
    }
    let _ = elevation::try_enable_debug_privilege();

    let path = ctx.session_path(&args.feature);
    let mut session =
        Session::load(&path).with_context(|| format!("load session {}", path.display()))?;

    let addr = resolve_address(&session, args)?;

    let pid = resolve_pid(ctx, &session)?;

    term::header(&format!(
        "watch-write: {} / {} @ 0x{:X} (pid {pid})",
        ctx.game_slug, args.feature, addr,
    ));
    term::dim("Perform an in-game action that changes this value (earn, spend, take damage, etc).");

    let result = watch_write::watch_write(
        pid,
        addr,
        args.width,
        Duration::from_secs(args.timeout),
        args.window,
    )?;

    // The trap RIP is the address AFTER the writing instruction (HW data
    // breakpoints fire as traps, not faults). Find the writer by decoding
    // forward through the captured window and locating an instruction whose
    // `IP + length == trap_rip`. Save writer-aligned bytes so extract-aob can
    // synthesise the AOB without any further fiddling.
    let trap_rip = result.rip;
    let (writer_bytes, writer_rip) =
        align_to_writer(result.bytes, result.bytes_start_addr, trap_rip);

    term::ok(&format!(
        "captured write: trap RIP=0x{trap_rip:X} → writer RIP=0x{writer_rip:X} on thread {} ({} bytes from writer)",
        result.thread_id,
        writer_bytes.len(),
    ));
    let hex_preview: String = writer_bytes
        .iter()
        .take(32)
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(" ");
    term::bullet(format!(
        "bytes (first 32 from writer): {hex_preview}{}",
        if writer_bytes.len() > 32 { " ..." } else { "" }
    ));

    let regs = &result.registers;
    eprintln!(
        "  registers @ trap:\n    \
         rax = 0x{:016X}  rbx = 0x{:016X}  rcx = 0x{:016X}  rdx = 0x{:016X}\n    \
         rsi = 0x{:016X}  rdi = 0x{:016X}  rbp = 0x{:016X}  rsp = 0x{:016X}\n    \
         r8  = 0x{:016X}  r9  = 0x{:016X}  r10 = 0x{:016X}  r11 = 0x{:016X}\n    \
         r12 = 0x{:016X}  r13 = 0x{:016X}  r14 = 0x{:016X}  r15 = 0x{:016X}",
        regs.rax,
        regs.rbx,
        regs.rcx,
        regs.rdx,
        regs.rsi,
        regs.rdi,
        regs.rbp,
        regs.rsp,
        regs.r8,
        regs.r9,
        regs.r10,
        regs.r11,
        regs.r12,
        regs.r13,
        regs.r14,
        regs.r15,
    );

    session.captured_rip = Some(writer_rip);
    session.captured_bytes = Some(writer_bytes);
    session.captured_registers = Some(result.registers.clone());
    session.history.push(HistoryStep {
        action: format!(
            "watch-write trap=0x{trap_rip:X} writer=0x{writer_rip:X} thread={}",
            result.thread_id,
        ),
        matches_after: session.candidates.len(),
    });
    session.save(&path)?;

    term::next_step(format!(
        "openforge-discover --game {} extract-aob --feature {} 0x{:X}",
        ctx.game_slug, args.feature, writer_rip,
    ));
    Ok(ExitCode::SUCCESS)
}

/// Walk the captured byte window forward and locate the instruction that
/// finished just before the trap. Returns the writer-aligned slice + writer's
/// own start address. Falls back to the raw window if no alignment fits.
fn align_to_writer(raw: Vec<u8>, bytes_start_addr: usize, trap_rip: usize) -> (Vec<u8>, usize) {
    // Try every possible starting alignment (the first byte of the captured
    // window may sit in the middle of a previous instruction).
    let max_skip = raw.len().min(15);
    for start_offset in 0..=max_skip {
        if start_offset >= raw.len() {
            break;
        }
        let slice = &raw[start_offset..];
        let mut decoder = Decoder::with_ip(
            64,
            slice,
            (bytes_start_addr + start_offset) as u64,
            DecoderOptions::NONE,
        );
        let mut instr = Instruction::default();
        while decoder.can_decode() {
            let pos_before = decoder.position();
            let ip_before = decoder.ip();
            decoder.decode_out(&mut instr);
            if instr.is_invalid() {
                break;
            }
            let next_ip = ip_before + instr.len() as u64;
            if next_ip == trap_rip as u64 {
                let writer_buf_offset = start_offset + pos_before;
                return (raw[writer_buf_offset..].to_vec(), ip_before as usize);
            }
            if next_ip > trap_rip as u64 {
                break;
            }
        }
    }
    (raw, bytes_start_addr)
}

fn resolve_address(session: &Session, args: &WatchWriteArgs) -> Result<usize> {
    if let Some(s) = &args.address {
        return parse_hex_addr(s);
    }
    if let Some(addr) = session.selected_address {
        return Ok(addr);
    }
    if session.candidates.len() == 1 {
        return Ok(session.candidates[0]);
    }
    bail!(
        "no target address: session has {} candidates and no selected_address. \
         Pass --address 0x... or run `pick --index <i>` first.",
        session.candidates.len()
    );
}

fn resolve_pid(ctx: &DiscoverContext, session: &Session) -> Result<u32> {
    // Prefer the live process matching one of the manifest's candidates.
    let snap = ProcessSnapshot::capture()?;
    let names: Vec<&str> = ctx
        .manifest
        .game
        .process_names
        .iter()
        .map(String::as_str)
        .collect();
    if let Some(info) = snap.find_by_candidates(&names) {
        return Ok(info.pid);
    }
    // Fallback to the pid captured in the session (might be stale).
    if session.pid != 0 {
        return Ok(session.pid);
    }
    Err(anyhow!(
        "no running process matches {:?}. Start the game first.",
        ctx.manifest.game.process_names
    ))
}
