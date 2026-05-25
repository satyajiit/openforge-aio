use std::process::ExitCode;

use anyhow::{Context, Result, anyhow};
use openforge_core::Target;

use crate::aob::{self, AobResolveKind};
use crate::cli::ExtractAobArgs;
use crate::context::DiscoverContext;
use crate::handlers::pick::parse_hex_addr;
use crate::session::{ExtractedResolve, HistoryStep, Session};
use crate::term;

pub fn run(ctx: &DiscoverContext, args: &ExtractAobArgs) -> Result<ExitCode> {
    let path = ctx.session_path(&args.feature);
    let mut session = Session::load(&path).with_context(|| format!("load {}", path.display()))?;

    let rip = parse_hex_addr(&args.address)?;
    let bytes = session
        .captured_bytes
        .clone()
        .ok_or_else(|| anyhow!("no captured bytes in session; run `capture` first"))?;

    let candidates: Vec<&str> = ctx
        .manifest
        .game
        .process_names
        .iter()
        .map(String::as_str)
        .collect();
    let target = Target::attach_by_candidates(&candidates)?;

    let module = session.primary_module.clone();
    let start_n = if args.conservative { 3 } else { 1 };
    let max_n = if args.conservative { 8 } else { 5 };

    let syn = aob::synthesize_unique(&target, &module, &bytes, rip as u64, start_n, max_n)?;

    term::header(&format!(
        "extracted AOB ({} instructions)",
        syn.instructions_included
    ));
    term::bullet(format!("pattern: {}", syn.pattern));
    let kind_str = match syn.resolve_kind {
        AobResolveKind::Direct => "direct",
        AobResolveKind::RipRelative => "rip_relative",
    };
    if syn.resolve_kind == AobResolveKind::RipRelative {
        term::bullet(format!(
            "resolve: {{ kind = \"rip_relative\", instruction_offset = {}, operand_size = {} }}",
            syn.instruction_offset, syn.operand_size
        ));
    } else {
        term::bullet("resolve: { kind = \"direct\" }".to_string());
    }
    term::ok(&format!("verified uniquely in module {module}"));

    session.extracted_pattern = Some(syn.pattern);
    session.extracted_resolve = Some(ExtractedResolve {
        kind: kind_str.to_string(),
        instruction_offset: syn.instruction_offset,
        operand_size: syn.operand_size,
    });
    session.history.push(HistoryStep {
        action: format!("extract-aob (n={})", syn.instructions_included),
        matches_after: 1,
    });
    session.save(&path)?;

    term::next_step(format!(
        "openforge-discover --game {} emit {}",
        ctx.game_slug, args.feature
    ));
    Ok(ExitCode::SUCCESS)
}
