use std::process::ExitCode;

use anyhow::{Result, anyhow};
use iced_x86::{Decoder, DecoderOptions, Formatter, Instruction, NasmFormatter};
use openforge_core::{Ctx, Target};

use crate::cli::InspectArgs;
use crate::context::DiscoverContext;
use crate::handlers::pick::parse_hex_addr;
use crate::term;

pub fn run(ctx: &DiscoverContext, args: &InspectArgs) -> Result<ExitCode> {
    if args.watch_write {
        return Err(anyhow!(
            "--watch-write requires hardware-breakpoint capture (not yet implemented). \
             Use an external disassembler to find the writing instruction, then \
             `openforge-discover capture --rip <addr> --bytes \"...\"`."
        ));
    }

    let addr = parse_hex_addr(&args.address)?;
    let candidates: Vec<&str> = ctx
        .manifest
        .game
        .process_names
        .iter()
        .map(String::as_str)
        .collect();
    let target = Target::attach_by_candidates(&candidates)?;

    let half = args.bytes.max(16);
    let start = addr.saturating_sub(half);
    let total = half * 2;
    let mut buf = vec![0u8; total];
    let ctx_ref: &dyn Ctx = &target;
    ctx_ref.read_bytes(start, &mut buf)?;

    term::header(&format!("inspect: 0x{addr:X}"));
    if let Some(m) = target.modules().iter().find(|m| m.contains(addr)) {
        term::bullet(format!(
            "in module {} (base 0x{:X}, +0x{:X})",
            m.name,
            m.base,
            addr - m.base
        ));
    }

    // Hex dump
    term::header("bytes");
    let mut col = 0;
    let mut line = String::new();
    for (i, b) in buf.iter().enumerate() {
        if col == 0 {
            line.push_str(&format!("0x{:X}: ", start + i));
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

    if args.disasm {
        term::header("disasm");
        let mut decoder = Decoder::with_ip(64, &buf, start as u64, DecoderOptions::NONE);
        let mut formatter = NasmFormatter::new();
        let mut instr = Instruction::default();
        let mut text = String::new();
        while decoder.can_decode() {
            decoder.decode_out(&mut instr);
            text.clear();
            formatter.format(&instr, &mut text);
            let marker = if instr.ip() == addr as u64 {
                "→"
            } else {
                " "
            };
            println!("{marker} 0x{:X}  {text}", instr.ip());
        }
    }

    Ok(ExitCode::SUCCESS)
}
