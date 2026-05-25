use std::process::ExitCode;

use anyhow::{Context, Result, anyhow};

use crate::cli::PickArgs;
use crate::context::DiscoverContext;
use crate::session::{HistoryStep, Session};
use crate::term;

pub fn run(ctx: &DiscoverContext, args: &PickArgs) -> Result<ExitCode> {
    let path = ctx.session_path(&args.feature);
    let mut session = Session::load(&path).with_context(|| format!("load {}", path.display()))?;

    let selected = if let Some(idx) = args.index {
        if idx >= session.candidates.len() {
            return Err(anyhow!(
                "index {idx} out of range ({} candidates)",
                session.candidates.len()
            ));
        }
        session.candidates[idx]
    } else if let Some(addr_str) = &args.address {
        let addr = parse_hex_addr(addr_str)?;
        if !session.candidates.contains(&addr) {
            return Err(anyhow!(
                "address 0x{:X} is not among the current candidates",
                addr
            ));
        }
        addr
    } else {
        return Err(anyhow!("pick requires either --index or --address"));
    };

    let pos = session
        .candidates
        .iter()
        .position(|&a| a == selected)
        .unwrap();
    let kept_value = session.last_values.get(pos).copied().unwrap_or(0.0);

    session.candidates = vec![selected];
    session.last_values = vec![kept_value];
    session.selected_address = Some(selected);
    session.history.push(HistoryStep {
        action: format!("pick 0x{selected:X}"),
        matches_after: 1,
    });
    session.save(&path)?;

    term::ok(&format!("picked 0x{selected:X} (= {kept_value})"));
    term::next_step(format!(
        "openforge-discover --game {} inspect 0x{:X} --disasm   # find the writing instruction",
        ctx.game_slug, selected
    ));
    Ok(ExitCode::SUCCESS)
}

pub fn parse_hex_addr(s: &str) -> Result<usize> {
    let trimmed = s.trim_start_matches("0x").trim_start_matches("0X");
    usize::from_str_radix(trimmed, 16).map_err(|e| anyhow!("invalid hex address {s:?}: {e}"))
}
