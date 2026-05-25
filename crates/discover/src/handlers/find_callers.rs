//! `find-callers` — scan .text for `call <target>` (E8 disp32) sites and
//! disassemble the preceding bytes. Used to discover how the caller prepared
//! `rcx` (the `this` pointer in Windows x64 ABI) before invoking the writer.
//!
//! If a caller does `mov rcx, [rip+disp32]`, the displacement points at a
//! static slot in `.data` / `.bss` — that's an instant 1-hop static anchor for
//! a pointer chain, no recursive scanning needed.

use std::process::ExitCode;

use anyhow::{Result, anyhow};
use iced_x86::{
    Code, Decoder, DecoderOptions, Formatter, Instruction, NasmFormatter, OpKind, Register,
};
use openforge_core::{Ctx, Target};

use crate::cli::FindCallersArgs;
use crate::context::DiscoverContext;
use crate::handlers::pick::parse_hex_addr;
use crate::term;

pub fn run(ctx: &DiscoverContext, args: &FindCallersArgs) -> Result<ExitCode> {
    let target_rip = parse_hex_addr(&args.target)?;

    let candidates: Vec<&str> = ctx
        .manifest
        .game
        .process_names
        .iter()
        .map(String::as_str)
        .collect();
    let proc = Target::attach_by_candidates(&candidates)?;

    let module = match &args.module {
        Some(name) => proc
            .modules()
            .iter()
            .find(|m| m.name.eq_ignore_ascii_case(name))
            .ok_or_else(|| anyhow!("module {name:?} not loaded"))?,
        None => proc.main(),
    };
    let text_base = module.text_base();
    let text_size = module.text_size;

    term::header(&format!("find-callers: {} → 0x{target_rip:X}", module.name));
    term::bullet(format!(
        "scanning {} .text bytes from 0x{:X}",
        text_size, text_base
    ));

    let mut text = vec![0u8; text_size];
    let ctx_ref: &dyn Ctx = &proc;
    ctx_ref.read_bytes(text_base, &mut text)?;

    // Linear scan for the byte 0xE8 then verify the disp32 lands on target.
    // Cheap and correct enough: E8 also appears inside other instruction
    // operands and as embedded data, but we only ACT on hits whose computed
    // target equals our address — so false positives are silently discarded.
    let mut hits: Vec<usize> = Vec::new();
    let mut i = 0;
    while i + 5 <= text.len() {
        if text[i] == 0xE8 {
            let disp = i32::from_le_bytes([text[i + 1], text[i + 2], text[i + 3], text[i + 4]]);
            let next_ip = (text_base + i + 5) as i64;
            let target = next_ip.wrapping_add(disp as i64) as usize;
            if target == target_rip {
                hits.push(text_base + i);
            }
        }
        i += 1;
    }

    if hits.is_empty() {
        term::warn("no callers", "no `call <target>` sites found in .text");
        return Ok(ExitCode::SUCCESS);
    }

    term::ok(&format!("found {} call site(s)", hits.len()));
    let formatter_factory = || {
        let mut f = NasmFormatter::new();
        f.options_mut().set_uppercase_hex(true);
        f.options_mut().set_hex_prefix("0x");
        f.options_mut().set_hex_suffix("");
        f
    };

    let limit = args.top.min(hits.len());
    let mut static_anchors: Vec<(usize, usize, Register)> = Vec::new();

    for (idx, &call_rip) in hits.iter().take(limit).enumerate() {
        term::header(&format!(
            "[{idx:2}] call site 0x{call_rip:X}  (module+0x{:X})",
            call_rip - text_base
        ));

        let before = args.window_before.max(8);
        let after = args.window_after;
        let win_start = call_rip.saturating_sub(before).max(text_base);
        let win_end = (call_rip + 5 + after).min(text_base + text_size);
        let off_start = win_start - text_base;
        let off_end = win_end - text_base;
        let window = &text[off_start..off_end];

        // Walk the window forward, decoding instructions starting at each byte
        // boundary that produces a clean decode chain landing exactly on
        // call_rip. iced-x86 has no native "decode backwards" so we use the
        // greedy-from-start approach — works for compiler-emitted code because
        // basic block boundaries are usually `int3` padding (0xCC) which we
        // skip.
        let chain = decode_chain_ending_at(window, win_start as u64, call_rip as u64);
        if chain.is_empty() {
            term::bullet("(could not align disassembly; raw hex follows)".to_string());
            print_hex(window, win_start);
            continue;
        }

        let mut formatter = formatter_factory();
        let mut text_buf = String::new();
        for instr in &chain {
            text_buf.clear();
            formatter.format(instr, &mut text_buf);
            let marker = if instr.ip() == call_rip as u64 {
                "→"
            } else {
                " "
            };
            println!("  {marker} 0x{:X}  {text_buf}", instr.ip());

            // Highlight rcx-setup instructions
            if writes_to_register(instr, Register::RCX) {
                let note = describe_rcx_source(instr);
                println!("       ↳ rcx ← {note}");
                if let Some(disp) = rip_relative_load_disp(instr) {
                    let static_addr = (instr.next_ip() as i64).wrapping_add(disp) as usize;
                    let rva = static_addr.saturating_sub(text_base);
                    println!(
                        "       ★ STATIC LOAD: [{}+0x{:X}]  (abs 0x{:X})",
                        module.name, rva, static_addr
                    );
                    static_anchors.push((call_rip, static_addr, Register::RCX));
                }
            }
        }
    }

    if !static_anchors.is_empty() {
        term::header("STATIC RCX SOURCES");
        for (call_rip, slot, _) in &static_anchors {
            let rva = slot.saturating_sub(text_base);
            term::ok(&format!(
                "call@0x{call_rip:X} loads rcx from {}+0x{:X}  (abs 0x{:X})",
                module.name, rva, slot
            ));
        }
        term::next_step(format!(
            "openforge-discover inspect 0x{:X} --bytes 32 --disasm  # confirm the load",
            static_anchors[0].0
        ));
    } else {
        term::warn(
            "no static load",
            "no `mov rcx, [rip+disp]` found in any window — rcx is being prepared from another register; \
             widen --window-before or follow that register's source upstream",
        );
    }

    Ok(ExitCode::SUCCESS)
}

/// Decode forward from each candidate start offset; pick the one whose
/// instruction stream lands cleanly on `target_ip`. Returns the chain leading
/// up to (and including) the call.
fn decode_chain_ending_at(bytes: &[u8], base_ip: u64, target_ip: u64) -> Vec<Instruction> {
    let mut best: Vec<Instruction> = Vec::new();
    for start_off in 0..bytes.len().min(64) {
        let slice = &bytes[start_off..];
        let start_ip = base_ip + start_off as u64;
        let mut decoder = Decoder::with_ip(64, slice, start_ip, DecoderOptions::NONE);
        let mut chain = Vec::with_capacity(16);
        let mut hit = false;
        while decoder.can_decode() {
            let mut instr = Instruction::default();
            decoder.decode_out(&mut instr);
            if instr.is_invalid() {
                break;
            }
            chain.push(instr);
            if instr.ip() == target_ip {
                hit = true;
                break;
            }
            if instr.ip() > target_ip {
                break;
            }
        }
        if hit && chain.len() > best.len() {
            best = chain;
        }
    }
    best
}

fn writes_to_register(instr: &Instruction, reg: Register) -> bool {
    // op0 is the destination for the instruction forms we care about (MOV, LEA, XOR, etc).
    instr.op_count() > 0
        && instr.op0_kind() == OpKind::Register
        && instr.op0_register().full_register() == reg
}

fn rip_relative_load_disp(instr: &Instruction) -> Option<i64> {
    // Look for a memory operand whose base is RIP (encoded as RIP-relative).
    for op_idx in 0..instr.op_count() {
        if instr.op_kind(op_idx) == OpKind::Memory && instr.is_ip_rel_memory_operand() {
            return Some(instr.memory_displacement64() as i64 - instr.next_ip() as i64);
        }
    }
    None
}

fn describe_rcx_source(instr: &Instruction) -> String {
    let mut s = String::new();
    let mut f = NasmFormatter::new();
    f.options_mut().set_uppercase_hex(true);
    f.options_mut().set_hex_prefix("0x");
    f.options_mut().set_hex_suffix("");
    f.format(instr, &mut s);

    // Tag well-known shapes for quick reading.
    let code = instr.code();
    let tag = match code {
        Code::Mov_r64_imm64 => "[immediate]",
        Code::Lea_r64_m => "[lea]",
        Code::Mov_r64_rm64 => {
            if instr.is_ip_rel_memory_operand() {
                "[RIP-relative]"
            } else if instr.op1_kind() == OpKind::Register {
                "[from register]"
            } else {
                "[memory deref]"
            }
        }
        _ => "[other]",
    };
    format!("{s}  {tag}")
}

fn print_hex(bytes: &[u8], base: usize) {
    let mut col = 0;
    let mut line = String::new();
    for (i, b) in bytes.iter().enumerate() {
        if col == 0 {
            line.push_str(&format!("    0x{:X}: ", base + i));
        }
        line.push_str(&format!("{b:02X} "));
        col += 1;
        if col == 16 {
            println!("{line}");
            line.clear();
            col = 0;
        }
    }
    if !line.is_empty() {
        println!("{line}");
    }
}
