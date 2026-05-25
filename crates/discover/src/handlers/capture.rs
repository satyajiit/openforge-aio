use std::process::ExitCode;

use anyhow::{Context, Result, anyhow};

use crate::cli::CaptureArgs;
use crate::context::DiscoverContext;
use crate::handlers::pick::parse_hex_addr;
use crate::session::{HistoryStep, Session};
use crate::term;

pub fn run(ctx: &DiscoverContext, args: &CaptureArgs) -> Result<ExitCode> {
    let path = ctx.session_path(&args.feature);
    let mut session = Session::load(&path).with_context(|| format!("load {}", path.display()))?;

    let rip = parse_hex_addr(&args.rip)?;
    let bytes = parse_hex_bytes(&args.bytes)?;
    if bytes.is_empty() {
        return Err(anyhow!("captured byte sequence is empty"));
    }

    session.captured_rip = Some(rip);
    session.captured_bytes = Some(bytes.clone());
    session.history.push(HistoryStep {
        action: format!("capture rip=0x{rip:X} bytes={}b", bytes.len()),
        matches_after: session.candidates.len(),
    });
    session.save(&path)?;

    term::ok(&format!("captured {} bytes at 0x{:X}", bytes.len(), rip));
    term::next_step(format!(
        "openforge-discover --game {} extract-aob --feature {} 0x{:X}",
        ctx.game_slug, args.feature, rip
    ));
    Ok(ExitCode::SUCCESS)
}

pub fn parse_hex_bytes(s: &str) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    for tok in s.split_whitespace() {
        if tok.len() != 2 || !tok.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(anyhow!("invalid hex byte {tok:?}"));
        }
        out.push(u8::from_str_radix(tok, 16).unwrap());
    }
    Ok(out)
}
