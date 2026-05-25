//! `openforge-discover find-pointers` — scan writable memory for 8-byte
//! values that land inside `[target - max_offset, target]`. Static-anchor
//! candidates (i.e. those whose source lives in a module image) are labelled
//! with `[STATIC] <module>+<offset>`; the rest are heap / VirtualAlloc / TLS.

use std::process::ExitCode;

use anyhow::{Context, Result};
use openforge_core::Target;

use crate::cli::FindPointersArgs;
use crate::context::DiscoverContext;
use crate::handlers::pick::parse_hex_addr;
use crate::pointer_scan;
use crate::session::{HistoryStep, PointerHit, Session};
use crate::term;

pub fn run(ctx: &DiscoverContext, args: &FindPointersArgs) -> Result<ExitCode> {
    let target_addr = parse_hex_addr(&args.target)?;

    let candidates: Vec<&str> = ctx
        .manifest
        .game
        .process_names
        .iter()
        .map(String::as_str)
        .collect();
    let target = Target::attach_by_candidates(&candidates)?;
    term::header(&format!(
        "find-pointers: {} → 0x{:X} (± 0x{:X})",
        target.process_name, target_addr, args.max_offset,
    ));
    term::ok(&format!(
        "attached pid {} ({}), scanning RW memory",
        target.pid, target.process_name,
    ));

    let handle = open_read_handle(target.pid);
    let hits = pointer_scan::scan_for_pointers_to(
        handle,
        target_addr,
        args.max_offset,
        args.alignment,
        0, // default chunk size
    )?;
    close_handle(handle);

    let modules = target.modules();
    let mut enriched: Vec<PointerHit> = hits
        .into_iter()
        .map(|c| {
            let (module_name, module_offset) = modules
                .iter()
                .find(|m| m.contains(c.source_addr))
                .map(|m| (Some(m.name.clone()), Some(c.source_addr - m.base)))
                .unwrap_or((None, None));
            PointerHit {
                source_addr: c.source_addr,
                value: c.value,
                trailing_offset: c.trailing_offset,
                module_name,
                module_offset,
            }
        })
        .collect();

    // Sort: ascending trailing offset, then ascending source address.
    enriched.sort_by(|a, b| {
        a.trailing_offset
            .cmp(&b.trailing_offset)
            .then(a.source_addr.cmp(&b.source_addr))
    });

    let total = enriched.len();
    term::ok(&format!(
        "Found {total} pointers to 0x{:X} (± 0x{:X})",
        target_addr, args.max_offset,
    ));

    let static_count = enriched.iter().filter(|h| h.module_name.is_some()).count();
    if static_count > 0 {
        term::bullet(format!(
            "{static_count} of {total} candidates fall inside a loaded module"
        ));
    }

    print_hits(&enriched, args.top);

    if let Some(feature) = &args.feature {
        let path = ctx.session_path(feature);
        let mut session =
            Session::load(&path).with_context(|| format!("load session {}", path.display()))?;
        let recorded = enriched.len();
        session.candidate_pointers = Some(enriched);
        session.history.push(HistoryStep {
            action: format!(
                "find-pointers target=0x{target_addr:X} max_offset=0x{:X} hits={recorded}",
                args.max_offset
            ),
            matches_after: session.candidates.len(),
        });
        session.save(&path)?;
        term::ok(&format!(
            "persisted {recorded} candidates to session {}",
            path.display()
        ));
    }

    Ok(ExitCode::SUCCESS)
}

fn print_hits(hits: &[PointerHit], top: usize) {
    let n = hits.len().min(top);
    for (i, h) in hits.iter().take(n).enumerate() {
        let prefix = if h.module_name.is_some() {
            "[STATIC] "
        } else {
            "         "
        };
        let module_tag = match (&h.module_name, h.module_offset) {
            (Some(name), Some(off)) => format!("  {name}+0x{off:X}"),
            _ => String::new(),
        };
        term::bullet(format!(
            "{prefix}[{i:>3}] source=0x{:X}  value=0x{:X}  +0x{:X}{module_tag}",
            h.source_addr, h.value, h.trailing_offset,
        ));
    }
    if hits.len() > n {
        term::dim(format!("  ... and {} more", hits.len() - n));
    }
}

fn open_read_handle(pid: u32) -> windows::Win32::Foundation::HANDLE {
    use windows::Win32::System::Threading::{
        OpenProcess, PROCESS_QUERY_INFORMATION, PROCESS_VM_READ,
    };
    unsafe {
        OpenProcess(PROCESS_VM_READ | PROCESS_QUERY_INFORMATION, false, pid)
            .expect("OpenProcess(PROCESS_VM_READ | PROCESS_QUERY_INFORMATION)")
    }
}

fn close_handle(h: windows::Win32::Foundation::HANDLE) {
    use windows::Win32::Foundation::CloseHandle;
    unsafe {
        let _ = CloseHandle(h);
    }
}
