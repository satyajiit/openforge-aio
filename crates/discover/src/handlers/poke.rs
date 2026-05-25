//! One-shot value write. Used during discovery to test whether a candidate
//! field is genuinely the game's source-of-truth (write persists / game logic
//! responds) vs a passive UI mirror (overwritten on the next render tick).

use std::process::ExitCode;

use anyhow::{Result, anyhow};
use openforge_core::{Ctx, Target};

use crate::cli::{PokeArgs, ScanType};
use crate::context::DiscoverContext;
use crate::handlers::pick::parse_hex_addr;
use crate::term;

pub fn run(ctx: &DiscoverContext, args: &PokeArgs) -> Result<ExitCode> {
    let addr = parse_hex_addr(&args.addr)?;

    let candidates: Vec<&str> = ctx
        .manifest
        .game
        .process_names
        .iter()
        .map(String::as_str)
        .collect();
    let target = Target::attach_by_candidates(&candidates)?;
    term::ok(&format!(
        "attached pid {} ({})",
        target.pid, target.process_name
    ));

    // Read the existing value first so the operator sees the before-state in
    // the same output (helps spot typos / wrong addresses before the write).
    let before = read_typed(&target as &dyn Ctx, addr, args.r#type)?;
    let bytes = encode_value(args.r#type, &args.value)?;
    target.write_bytes(addr, &bytes)?;
    let after = read_typed(&target as &dyn Ctx, addr, args.r#type)?;

    term::header(&format!("poke at 0x{addr:X} ({:?})", args.r#type));
    term::bullet(format!("before: {before}"));
    term::bullet(format!("wrote:  {} bytes", bytes.len()));
    term::bullet(format!("after:  {after}"));
    if after != args.value {
        term::warn(
            "value may be a passive mirror",
            format!(
                "post-write read shows {after}, not {} — likely overwritten by a UI sync tick rather than the canonical store",
                args.value
            ),
        );
    } else {
        term::ok("write committed");
    }
    Ok(ExitCode::SUCCESS)
}

fn encode_value(ty: ScanType, raw: &str) -> Result<Vec<u8>> {
    let trimmed = raw.trim();
    let parse_int_i64 = || -> Result<i64> {
        if let Some(hex) = trimmed
            .strip_prefix("0x")
            .or_else(|| trimmed.strip_prefix("0X"))
        {
            i64::from_str_radix(hex, 16).map_err(|e| anyhow!("invalid hex int: {e}"))
        } else {
            trimmed
                .parse::<i64>()
                .map_err(|e| anyhow!("invalid decimal int: {e}"))
        }
    };
    let parse_int_u64 = || -> Result<u64> {
        if let Some(hex) = trimmed
            .strip_prefix("0x")
            .or_else(|| trimmed.strip_prefix("0X"))
        {
            u64::from_str_radix(hex, 16).map_err(|e| anyhow!("invalid hex uint: {e}"))
        } else {
            trimmed
                .parse::<u64>()
                .map_err(|e| anyhow!("invalid decimal uint: {e}"))
        }
    };
    Ok(match ty {
        ScanType::I8 => (parse_int_i64()? as i8).to_le_bytes().to_vec(),
        ScanType::I16 => (parse_int_i64()? as i16).to_le_bytes().to_vec(),
        ScanType::I32 => (parse_int_i64()? as i32).to_le_bytes().to_vec(),
        ScanType::I64 => parse_int_i64()?.to_le_bytes().to_vec(),
        ScanType::U8 => (parse_int_u64()? as u8).to_le_bytes().to_vec(),
        ScanType::U16 => (parse_int_u64()? as u16).to_le_bytes().to_vec(),
        ScanType::U32 => (parse_int_u64()? as u32).to_le_bytes().to_vec(),
        ScanType::U64 => parse_int_u64()?.to_le_bytes().to_vec(),
        ScanType::F32 => trimmed
            .parse::<f32>()
            .map_err(|e| anyhow!("invalid f32: {e}"))?
            .to_le_bytes()
            .to_vec(),
        ScanType::F64 => trimmed
            .parse::<f64>()
            .map_err(|e| anyhow!("invalid f64: {e}"))?
            .to_le_bytes()
            .to_vec(),
        ScanType::Bool => {
            let v = matches!(trimmed, "true" | "True" | "1");
            vec![v as u8]
        }
    })
}

fn read_typed(ctx: &dyn Ctx, addr: usize, ty: ScanType) -> Result<String> {
    let mut buf = vec![0u8; type_size(ty)];
    ctx.read_bytes(addr, &mut buf)?;
    Ok(match ty {
        ScanType::I8 => format!("{}", i8::from_le_bytes([buf[0]])),
        ScanType::I16 => format!("{}", i16::from_le_bytes([buf[0], buf[1]])),
        ScanType::I32 => format!("{}", i32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]])),
        ScanType::I64 => format!("{}", i64::from_le_bytes(buf[..8].try_into().unwrap())),
        ScanType::U8 => format!("{}", buf[0]),
        ScanType::U16 => format!("{}", u16::from_le_bytes([buf[0], buf[1]])),
        ScanType::U32 => format!("{}", u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]])),
        ScanType::U64 => format!("{}", u64::from_le_bytes(buf[..8].try_into().unwrap())),
        ScanType::F32 => format!("{}", f32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]])),
        ScanType::F64 => format!("{}", f64::from_le_bytes(buf[..8].try_into().unwrap())),
        ScanType::Bool => format!("{}", buf[0] != 0),
    })
}

fn type_size(ty: ScanType) -> usize {
    match ty {
        ScanType::I8 | ScanType::U8 | ScanType::Bool => 1,
        ScanType::I16 | ScanType::U16 => 2,
        ScanType::I32 | ScanType::U32 | ScanType::F32 => 4,
        ScanType::I64 | ScanType::U64 | ScanType::F64 => 8,
    }
}
