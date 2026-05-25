use std::fs;
use std::process::ExitCode;

use anyhow::{Context, Result, anyhow, bail};
use chrono_lite::today_iso;

use crate::cli::{EmitArgs, EmitStrategy};
use crate::context::DiscoverContext;
use crate::emit::{
    HopSection, LocatorSection, MetaSection, PointerChainSection, SignatureDoc, ValueSection,
    WriteSection,
};
use crate::session::Session;
use crate::term;

pub fn run(ctx: &DiscoverContext, args: &EmitArgs) -> Result<ExitCode> {
    let session_path = ctx.session_path(&args.feature);
    let session = Session::load(&session_path)
        .with_context(|| format!("load session {}", session_path.display()))?;

    let strategy = resolve_strategy(args.strategy, &session);
    let hops = parse_hops(args.hops.as_deref())?;

    let pattern = session
        .extracted_pattern
        .clone()
        .ok_or_else(|| anyhow!("no extracted pattern; run `extract-aob` first"))?;
    let resolve = session
        .extracted_resolve
        .clone()
        .ok_or_else(|| anyhow!("no extracted resolve metadata"))?;

    let write = build_write_section(strategy, args, &session)?;
    let value = build_value_section(strategy, &session);
    let pointer_chain = if hops.is_empty() {
        None
    } else {
        Some(PointerChainSection { hops })
    };

    let doc = SignatureDoc {
        meta: MetaSection {
            feature: session.feature.clone(),
            display_name: session.display_name.clone(),
            description: None,
            tier: "general".into(),
            discovered_in_version: ctx.manifest.game.supported_versions.first().cloned(),
            verified_versions: ctx.manifest.game.supported_versions.clone(),
            discovered_on: Some(today_iso()),
            discovered_by: None,
        },
        value,
        write,
        locator: LocatorSection {
            pattern,
            resolve_kind: resolve.kind,
            instruction_offset: Some(resolve.instruction_offset),
            operand_size: Some(resolve.operand_size),
            delta: None,
            module: None,
        },
        pointer_chain,
    };

    let path = ctx.signature_path(&args.feature);
    if path.exists() && !args.force {
        return Err(anyhow!(
            "{} already exists; pass --force to overwrite",
            path.display()
        ));
    }

    let toml_text = doc.to_toml();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("toml.tmp");
    fs::write(&tmp, &toml_text)?;
    fs::rename(&tmp, &path)?;

    term::ok(&format!("wrote {}", path.display()));
    term::next_step(format!(
        "cargo build -p openforge-game-{}   # the trainer picks the new signature up automatically",
        ctx.game_slug
    ));
    Ok(ExitCode::SUCCESS)
}

fn resolve_strategy(arg: EmitStrategy, session: &Session) -> EmitStrategy {
    match arg {
        EmitStrategy::Auto => {
            if session.captured_bytes.is_some() {
                EmitStrategy::CodePatch
            } else {
                EmitStrategy::OneShot
            }
        }
        explicit => explicit,
    }
}

fn build_write_section(
    strategy: EmitStrategy,
    args: &EmitArgs,
    session: &Session,
) -> Result<WriteSection> {
    match strategy {
        EmitStrategy::Auto => unreachable!("resolve_strategy lifts Auto"),
        EmitStrategy::OneShot => Ok(WriteSection::OneShot),
        EmitStrategy::Freeze => Ok(WriteSection::Freeze {
            interval_ms: args.interval_ms,
        }),
        EmitStrategy::CodePatch => {
            let original = match &args.original_bytes {
                Some(s) => s.clone(),
                None => session
                    .captured_bytes
                    .as_deref()
                    .map(format_hex_bytes)
                    .ok_or_else(|| {
                        anyhow!(
                            "code_patch needs --original-bytes (or a session with captured_bytes from watch-write)"
                        )
                    })?,
            };
            let patched = args.patched_bytes.clone().ok_or_else(|| {
                anyhow!("code_patch needs --patched-bytes (e.g. \"90 90 90 90\")")
            })?;
            validate_hex_bytes_match(&original, &patched)?;
            Ok(WriteSection::CodePatch {
                original_bytes: original,
                patched_bytes: patched,
            })
        }
    }
}

fn build_value_section(strategy: EmitStrategy, session: &Session) -> Option<ValueSection> {
    // [value] is required for one_shot and freeze; not used by code_patch.
    match strategy {
        EmitStrategy::CodePatch => None,
        _ => Some(ValueSection {
            kind: session.kind,
            min: None,
            max: None,
            display: "decimal".into(),
            endian: "little".into(),
        }),
    }
}

fn parse_hops(spec: Option<&str>) -> Result<Vec<HopSection>> {
    let Some(spec) = spec else {
        return Ok(Vec::new());
    };
    let mut hops = Vec::new();
    for raw in spec.split(',') {
        let token = raw.trim();
        if token.is_empty() {
            continue;
        }
        let (kind, rest) = token
            .split_once(':')
            .ok_or_else(|| anyhow!("hop {token:?}: expected `d:<offset>` or `+:<offset>`"))?;
        let deref = match kind.trim() {
            "d" | "deref" => true,
            "+" | "add" => false,
            other => bail!("hop {token:?}: unknown kind {other:?}; use `d` or `+`"),
        };
        let offset =
            parse_signed_hex(rest.trim()).with_context(|| format!("hop {token:?}: bad offset"))?;
        hops.push(HopSection { deref, offset });
    }
    Ok(hops)
}

fn parse_signed_hex(s: &str) -> Result<i64> {
    let (sign, body) = match s.strip_prefix('-') {
        Some(body) => (-1i64, body),
        None => (1i64, s.strip_prefix('+').unwrap_or(s)),
    };
    let body = body.trim_start_matches("0x").trim_start_matches("0X");
    let mag = i64::from_str_radix(body, 16).with_context(|| format!("not hex: {body:?}"))?;
    Ok(sign * mag)
}

fn format_hex_bytes(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn validate_hex_bytes_match(original: &str, patched: &str) -> Result<()> {
    let count = |s: &str| s.split_whitespace().count();
    let a = count(original);
    let b = count(patched);
    if a == 0 {
        bail!("original_bytes is empty");
    }
    if a != b {
        bail!("original_bytes ({a} bytes) and patched_bytes ({b} bytes) must be the same length");
    }
    Ok(())
}

mod chrono_lite {
    use std::time::{SystemTime, UNIX_EPOCH};

    pub fn today_iso() -> String {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        let (y, m, d) = unix_to_ymd(now);
        format!("{y:04}-{m:02}-{d:02}")
    }

    fn unix_to_ymd(secs: i64) -> (i32, u32, u32) {
        let days = secs.div_euclid(86_400);
        let (y, m, d) = civil_from_days(days);
        (y as i32, m as u32, d as u32)
    }

    /// Howard Hinnant's "Date Algorithms" — civil_from_days for the proleptic Gregorian calendar.
    fn civil_from_days(z: i64) -> (i64, u8, u8) {
        let z = z + 719_468;
        let era = z.div_euclid(146_097);
        let doe = (z - era * 146_097) as u64;
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let y = yoe as i64 + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = (doy - (153 * mp + 2) / 5 + 1) as u8;
        let m = if mp < 10 { mp + 3 } else { mp - 9 } as u8;
        let y = if m <= 2 { y + 1 } else { y };
        (y, m, d)
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_hops, parse_signed_hex};

    #[test]
    fn parse_hops_handles_mixed_kinds() {
        let hops = parse_hops(Some("d:0x10, d:0x40, +:0x38")).unwrap();
        assert_eq!(hops.len(), 3);
        assert!(hops[0].deref);
        assert_eq!(hops[0].offset, 0x10);
        assert!(hops[1].deref);
        assert_eq!(hops[1].offset, 0x40);
        assert!(!hops[2].deref);
        assert_eq!(hops[2].offset, 0x38);
    }

    #[test]
    fn parse_hops_empty_is_ok() {
        assert!(parse_hops(None).unwrap().is_empty());
        assert!(parse_hops(Some("")).unwrap().is_empty());
        assert!(parse_hops(Some("   ,  ,  ")).unwrap().is_empty());
    }

    #[test]
    fn parse_hops_rejects_unknown_kind() {
        assert!(parse_hops(Some("x:0x10")).is_err());
    }

    #[test]
    fn parse_signed_hex_handles_neg() {
        assert_eq!(parse_signed_hex("0x10").unwrap(), 0x10);
        assert_eq!(parse_signed_hex("-0x10").unwrap(), -0x10);
        assert_eq!(parse_signed_hex("+0x10").unwrap(), 0x10);
        assert_eq!(parse_signed_hex("10").unwrap(), 0x10);
    }
}
