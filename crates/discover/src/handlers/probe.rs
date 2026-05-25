//! Windowed value search anchored on a known address.
//!
//! When the player-state base is already known (e.g. via a working
//! `heap_scan` signature for another field in the same object), a full RW
//! memory scan for a common value like `18` is wasteful and produces millions
//! of false matches. `probe` reads a small region around the anchor and prints
//! only the aligned positions where the value appears — typically a handful
//! of candidates that the user can disambiguate by changing the field in-game
//! and re-running with the new value.

use std::process::ExitCode;

use anyhow::{Result, anyhow};
use openforge_core::{Ctx, Target};

use crate::cli::{ProbeArgs, ScanType};
use crate::context::DiscoverContext;
use crate::handlers::pick::parse_hex_addr;
use crate::term;

pub fn run(ctx: &DiscoverContext, args: &ProbeArgs) -> Result<ExitCode> {
    let anchor = parse_hex_addr(&args.addr)?;
    let size = type_size(args.r#type);
    let alignment = args.alignment.unwrap_or(size).max(1);
    let needle = parse_needle(args.r#type, &args.value)?;

    let candidates: Vec<&str> = ctx
        .manifest
        .game
        .process_names
        .iter()
        .map(String::as_str)
        .collect();
    let target = Target::attach_by_candidates(&candidates)?;

    let start = anchor.saturating_sub(args.before);
    let total = args.before + args.after + size;
    let end = start + total;

    // Read in 4 KiB chunks so a single uncommitted/guard page doesn't abort the
    // whole window. Failed chunks are skipped — the buffer is left zero there
    // and skip-counters advance the cursor past them.
    const CHUNK: usize = 4096;
    let mut buf = vec![0u8; total];
    let ctx_ref: &dyn Ctx = &target;
    let mut readable = vec![false; total];
    let mut off = 0;
    let mut skipped_pages: usize = 0;
    while off < total {
        let take = CHUNK.min(total - off);
        match ctx_ref.read_bytes(start + off, &mut buf[off..off + take]) {
            Ok(()) => {
                for r in &mut readable[off..off + take] {
                    *r = true;
                }
            }
            Err(_) => {
                skipped_pages += 1;
            }
        }
        off += take;
    }
    if skipped_pages > 0 {
        term::bullet(format!(
            "skipped {skipped_pages} unreadable chunk(s) ({} KiB each)",
            CHUNK / 1024
        ));
    }

    let first_aligned = start.div_ceil(alignment) * alignment;
    let mut hits: Vec<(usize, i64, Vec<u8>)> = Vec::new();
    let mut addr = first_aligned;
    while addr + size <= end {
        let i = addr - start;
        // Only consider this position if the entire field is from a readable
        // chunk; otherwise a hole's zero-fill produces false matches when
        // value == 0.
        if readable[i..i + size].iter().all(|&r| r) {
            let slice = &buf[i..i + size];
            if slice == needle.as_slice() {
                let signed_offset = (addr as i64) - (anchor as i64);
                hits.push((addr, signed_offset, slice.to_vec()));
            }
        }
        addr += alignment;
    }

    term::header(&format!(
        "probe at 0x{anchor:X} ({:?}={}) window -0x{:X} / +0x{:X} align={}",
        args.r#type, args.value, args.before, args.after, alignment
    ));

    if hits.is_empty() {
        println!("no matches in window");
        term::next_step(format!(
            "openforge-discover --game {} probe --addr 0x{:X} --before 0x{:X} --after 0x{:X} --type {:?} --value <changed>   # widen or try a different value",
            ctx.game_slug,
            anchor,
            args.before * 2,
            args.after * 2,
            args.r#type
        ));
    } else {
        println!("{} hit(s):", hits.len());
        for (addr, offset, bytes) in &hits {
            let (sign, abs_off) = if *offset >= 0 {
                ("+", *offset as usize)
            } else {
                ("-", offset.unsigned_abs() as usize)
            };
            let hex: Vec<String> = bytes.iter().map(|b| format!("{b:02X}")).collect();
            println!(
                "  0x{addr:X}   anchor{sign}0x{abs_off:X}   [{}]",
                hex.join(" ")
            );
        }
        term::next_step(
            "change the in-game value by 1 and re-run with --value <new>; the offset that survives is your field".to_string(),
        );
    }

    Ok(ExitCode::SUCCESS)
}

fn type_size(t: ScanType) -> usize {
    use ScanType::*;
    match t {
        I8 | U8 | Bool => 1,
        I16 | U16 => 2,
        I32 | U32 | F32 => 4,
        I64 | U64 | F64 => 8,
    }
}

fn parse_needle(t: ScanType, s: &str) -> Result<Vec<u8>> {
    use ScanType::*;
    Ok(match t {
        I8 => (parse_int(s)? as i8).to_le_bytes().to_vec(),
        I16 => (parse_int(s)? as i16).to_le_bytes().to_vec(),
        I32 => (parse_int(s)? as i32).to_le_bytes().to_vec(),
        I64 => parse_int(s)?.to_le_bytes().to_vec(),
        U8 => (parse_uint(s)? as u8).to_le_bytes().to_vec(),
        U16 => (parse_uint(s)? as u16).to_le_bytes().to_vec(),
        U32 => (parse_uint(s)? as u32).to_le_bytes().to_vec(),
        U64 => parse_uint(s)?.to_le_bytes().to_vec(),
        F32 => s
            .parse::<f32>()
            .map_err(|e| anyhow!("bad f32 value {s:?}: {e}"))?
            .to_le_bytes()
            .to_vec(),
        F64 => s
            .parse::<f64>()
            .map_err(|e| anyhow!("bad f64 value {s:?}: {e}"))?
            .to_le_bytes()
            .to_vec(),
        Bool => vec![match s.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" => 1,
            "0" | "false" | "no" => 0,
            other => return Err(anyhow!("bad bool value {other:?}")),
        }],
    })
}

fn parse_int(s: &str) -> Result<i64> {
    let s = s.trim().replace('_', "");
    let (sign, rest) = if let Some(r) = s.strip_prefix('-') {
        (-1i64, r)
    } else {
        (1i64, s.as_str())
    };
    let n = if let Some(hex) = rest.strip_prefix("0x").or_else(|| rest.strip_prefix("0X")) {
        i64::from_str_radix(hex, 16).map_err(|e| anyhow!("bad int {s:?}: {e}"))?
    } else {
        rest.parse::<i64>()
            .map_err(|e| anyhow!("bad int {s:?}: {e}"))?
    };
    Ok(sign * n)
}

fn parse_uint(s: &str) -> Result<u64> {
    let s = s.trim().replace('_', "");
    if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        u64::from_str_radix(hex, 16).map_err(|e| anyhow!("bad uint {s:?}: {e}"))
    } else {
        s.parse::<u64>().map_err(|e| anyhow!("bad uint {s:?}: {e}"))
    }
}
