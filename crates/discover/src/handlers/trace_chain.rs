//! `openforge-discover trace-chain` — recursive DFS pointer-chain discovery.
//!
//! Starting from the target address, at each level we scan writable memory
//! for 8-byte values that point near the current frontier address. Any source
//! address that falls inside the configured static module is recorded as a
//! chain candidate; the search keeps descending from non-static sources until
//! either `max_depth` is exhausted or `max_candidates` accepted chains have
//! been recorded. Each candidate is verified by replaying the chain via
//! remote reads — chains that don't land within `target ± max_offset` are
//! dropped before persistence.

use std::collections::HashSet;
use std::process::ExitCode;

use anyhow::{Context, Result, anyhow};
use openforge_core::{Module, Target};
use windows::Win32::Foundation::HANDLE;

use crate::cli::TraceChainArgs;
use crate::context::DiscoverContext;
use crate::handlers::pick::parse_hex_addr;
use crate::pointer_scan;
use crate::session::{ChainCandidate, ChainHopSpec, HistoryStep, Session};
use crate::term;

/// Cap per-level breadth to avoid combinatorial blow-up. The brief mentions
/// 64; we honour it.
const PER_LEVEL_BREADTH: usize = 64;

pub fn run(ctx: &DiscoverContext, args: &TraceChainArgs) -> Result<ExitCode> {
    let target_addr = parse_hex_addr(&args.target)?;

    let candidates: Vec<&str> = ctx
        .manifest
        .game
        .process_names
        .iter()
        .map(String::as_str)
        .collect();
    let target = Target::attach_by_candidates(&candidates)?;
    let anchor_module_name = args
        .module
        .clone()
        .unwrap_or_else(|| target.main().name.clone());
    let anchor_module = target
        .modules()
        .iter()
        .find(|m| m.name.eq_ignore_ascii_case(&anchor_module_name))
        .ok_or_else(|| anyhow!("module {anchor_module_name:?} not present in target process"))?
        .clone();

    term::header(&format!(
        "trace-chain: {} → 0x{:X} (anchor: {})",
        target.process_name, target_addr, anchor_module.name,
    ));
    term::ok(&format!(
        "attached pid {} ({}), max_depth={} max_offset=0x{:X} max_candidates={}",
        target.pid, target.process_name, args.max_depth, args.max_offset, args.max_candidates,
    ));

    let handle = open_read_handle(target.pid);
    let modules_snapshot: Vec<Module> = target.modules().to_vec();

    let mut chains: Vec<ChainCandidate> = Vec::new();
    let mut visited: HashSet<usize> = HashSet::new();
    visited.insert(target_addr);

    dfs(
        handle,
        target_addr,
        &Vec::new(),
        0,
        args.max_depth,
        args.max_offset,
        args.max_candidates,
        &anchor_module,
        &modules_snapshot,
        &mut visited,
        &mut chains,
    );

    // Verify each candidate by replaying the chain live.
    let lo = target_addr.saturating_sub(args.max_offset);
    let hi = target_addr;
    let mut verified: Vec<ChainCandidate> = chains
        .into_iter()
        .filter(|c| {
            let anchor_addr = anchor_module.base + c.module_offset;
            let hops: Vec<(bool, i64)> = c.hops.iter().map(|h| (h.deref, h.offset)).collect();
            match pointer_scan::walk_chain(handle, anchor_addr, &hops) {
                Some(end) => end >= lo && end <= hi,
                None => false,
            }
        })
        .collect();

    close_handle(handle);

    // Sort: shorter chains first, then lower module_offset.
    verified.sort_by(|a, b| {
        a.hops
            .len()
            .cmp(&b.hops.len())
            .then(a.module_offset.cmp(&b.module_offset))
    });

    term::ok(&format!(
        "Chain candidates to 0x{:X} ({} found):",
        target_addr,
        verified.len()
    ));
    print_chains(&verified, &anchor_module.name);

    if let Some(feature) = &args.feature {
        let path = ctx.session_path(feature);
        let mut session =
            Session::load(&path).with_context(|| format!("load session {}", path.display()))?;
        let recorded = verified.len();
        session.candidate_chains = Some(verified);
        session.history.push(HistoryStep {
            action: format!(
                "trace-chain target=0x{target_addr:X} depth={} hits={recorded}",
                args.max_depth
            ),
            matches_after: session.candidates.len(),
        });
        session.save(&path)?;
        term::ok(&format!(
            "persisted {recorded} chains to session {}",
            path.display()
        ));
    }

    Ok(ExitCode::SUCCESS)
}

#[allow(clippy::too_many_arguments)]
fn dfs(
    handle: HANDLE,
    current_addr: usize,
    hops_so_far: &[ChainHopSpec],
    depth: usize,
    max_depth: usize,
    max_offset: usize,
    max_candidates: usize,
    anchor_module: &Module,
    all_modules: &[Module],
    visited: &mut HashSet<usize>,
    out: &mut Vec<ChainCandidate>,
) {
    if depth >= max_depth || out.len() >= max_candidates {
        return;
    }

    let mut hits = match pointer_scan::scan_for_pointers_to(handle, current_addr, max_offset, 8, 0)
    {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(
                addr = format!("0x{current_addr:X}"),
                error = %e,
                "pointer scan failed at level"
            );
            return;
        }
    };

    // Drop self-references and already-visited sources.
    hits.retain(|h| h.source_addr != current_addr && !visited.contains(&h.source_addr));

    // Cap breadth: prefer small trailing offsets, which are far more likely to
    // be honest object-internal pointers than wildly-offset coincidences.
    hits.sort_by_key(|h| h.trailing_offset);
    hits.truncate(PER_LEVEL_BREADTH);

    for hit in hits {
        if out.len() >= max_candidates {
            return;
        }
        // Each new hop is prepended at level k: chain walks the *outermost*
        // hop first (the one closest to the static anchor) and arrives at the
        // target after the *innermost* one. We add the current hop to the
        // *front* so the static anchor lands at index 0.
        let mut new_hops: Vec<ChainHopSpec> = Vec::with_capacity(hops_so_far.len() + 1);
        new_hops.push(ChainHopSpec {
            deref: true,
            offset: hit.trailing_offset as i64,
        });
        new_hops.extend(hops_so_far.iter().cloned());

        if anchor_module.contains(hit.source_addr) {
            out.push(ChainCandidate {
                module_name: anchor_module.name.clone(),
                module_offset: hit.source_addr - anchor_module.base,
                hops: new_hops.clone(),
            });
            // A static hit is the natural terminus for that branch — descend
            // no deeper from a static slot (those slots are written to by the
            // loader, not by other heap pointers we care about).
            continue;
        }

        // Skip descending through *other* modules' image space — those are
        // either data sections we can't anchor on or .text we shouldn't.
        if all_modules.iter().any(|m| m.contains(hit.source_addr))
            && !anchor_module.contains(hit.source_addr)
        {
            continue;
        }

        visited.insert(hit.source_addr);
        dfs(
            handle,
            hit.source_addr,
            &new_hops,
            depth + 1,
            max_depth,
            max_offset,
            max_candidates,
            anchor_module,
            all_modules,
            visited,
            out,
        );
    }
}

fn print_chains(chains: &[ChainCandidate], anchor_name: &str) {
    for (i, c) in chains.iter().enumerate() {
        let hops_str = c
            .hops
            .iter()
            .map(|h| {
                let sign = if h.offset >= 0 { "+" } else { "-" };
                format!("(deref) {sign}0x{:X}", h.offset.unsigned_abs())
            })
            .collect::<Vec<_>>()
            .join(" → ");
        term::bullet(format!(
            "[{i}] {anchor_name}+0x{:X} → {hops_str}",
            c.module_offset,
        ));
    }
    if chains.is_empty() {
        term::dim("  (none — try increasing --max-depth or --max-offset)");
    }
}

fn open_read_handle(pid: u32) -> HANDLE {
    use windows::Win32::System::Threading::{
        OpenProcess, PROCESS_QUERY_INFORMATION, PROCESS_VM_READ,
    };
    unsafe {
        OpenProcess(PROCESS_VM_READ | PROCESS_QUERY_INFORMATION, false, pid)
            .expect("OpenProcess(PROCESS_VM_READ | PROCESS_QUERY_INFORMATION)")
    }
}

fn close_handle(h: HANDLE) {
    use windows::Win32::Foundation::CloseHandle;
    unsafe {
        let _ = CloseHandle(h);
    }
}
