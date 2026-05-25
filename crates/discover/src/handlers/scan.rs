use std::process::ExitCode;

use anyhow::{Result, anyhow};
use openforge_core::{Ctx, Target};
use openforge_runtime::ValueKind;

use crate::cli::{ScanArgs, ScanType};
use crate::context::DiscoverContext;
use crate::pages;
use crate::predicate::{NumberLike, Predicate};
use crate::scan;
use crate::session::{HistoryStep, Session};
use crate::term;

pub fn run(ctx: &DiscoverContext, args: &ScanArgs) -> Result<ExitCode> {
    term::header(&format!(
        "scan: {} / {} ({:?})",
        ctx.game_slug, args.feature, args.r#type
    ));

    let candidates: Vec<&str> = ctx
        .manifest
        .game
        .process_names
        .iter()
        .map(String::as_str)
        .collect();
    let target = Target::attach_by_candidates(&candidates)?;
    let main = target.main();
    term::ok(&format!(
        "attached pid {} ({}), module size {} bytes",
        target.pid, target.process_name, main.size
    ));

    let regions = collect_regions(&target, args.strict);
    term::bullet(format!("scanning {} RW regions", regions.len()));

    macro_rules! arm {
        ($T:ty) => {{
            let pred = if let Some(s) = &args.value {
                Predicate::<$T>::Eq(<$T>::parse(s)?)
            } else if args.unknown_initial {
                Predicate::<$T>::UnknownInitial
            } else {
                return Err(anyhow!("scan requires either --value or --unknown-initial"));
            };
            let hits = scan::first_scan::<$T>(&target as &dyn Ctx, &regions, pred, args.epsilon)?;
            let cands: Vec<usize> = hits.iter().map(|(a, _)| *a).collect();
            let values: Vec<f64> = hits.iter().map(|(_, v)| v.as_f64()).collect();
            (cands, values)
        }};
    }

    let (candidates_addrs, last_values) = match args.r#type {
        ScanType::I8 => arm!(i8),
        ScanType::I16 => arm!(i16),
        ScanType::I32 => arm!(i32),
        ScanType::I64 => arm!(i64),
        ScanType::U8 => arm!(u8),
        ScanType::U16 => arm!(u16),
        ScanType::U32 => arm!(u32),
        ScanType::U64 => arm!(u64),
        ScanType::F32 => arm!(f32),
        ScanType::F64 => arm!(f64),
        ScanType::Bool => arm!(bool),
    };

    let kind = scan_type_to_kind(args.r#type);

    let session = Session {
        feature: args.feature.clone(),
        display_name: args.name.clone(),
        kind,
        strict: args.strict,
        pid: target.pid,
        primary_module: main.name.clone(),
        module_base: main.base,
        module_size: main.size,
        epsilon: args.epsilon,
        candidates: candidates_addrs,
        last_values,
        selected_address: None,
        captured_rip: None,
        captured_bytes: None,
        captured_registers: None,
        candidate_pointers: None,
        candidate_chains: None,
        extracted_pattern: None,
        extracted_resolve: None,
        history: vec![HistoryStep {
            action: match &args.value {
                Some(v) => format!("first scan eq({v})"),
                None => "first scan unknown_initial".to_string(),
            },
            matches_after: 0,
        }],
    };

    let n = session.candidates.len();
    let mut session = session;
    session.history.last_mut().unwrap().matches_after = n;
    session.save(&ctx.session_path(&args.feature))?;

    term::ok(&format!("first scan complete: {n} candidates"));
    print_sample(&session.candidates, &session.last_values, 10);
    term::next_step(format!(
        "openforge-discover --game {} narrow --feature {} --equal <new_value>",
        ctx.game_slug, args.feature
    ));

    Ok(ExitCode::SUCCESS)
}

fn collect_regions(target: &Target, strict: bool) -> Vec<pages::MemoryRegion> {
    let handle = target_handle(target);
    let mut regions = pages::enumerate_rw(handle);
    if strict {
        let primary = target.main();
        regions.retain(|r| {
            r.base >= primary.base && r.base < primary.base.saturating_add(primary.size)
        });
    }
    regions
}

fn target_handle(target: &Target) -> windows::Win32::Foundation::HANDLE {
    // We can't borrow the inner ProcessHandle directly; reach in through the public
    // surface by writing 0 bytes (a no-op that proves the HANDLE is valid) — but
    // ultimately we need the HANDLE. Re-open it for VirtualQueryEx use.
    use windows::Win32::System::Threading::{
        OpenProcess, PROCESS_QUERY_INFORMATION, PROCESS_VM_READ,
    };
    unsafe {
        OpenProcess(
            PROCESS_VM_READ | PROCESS_QUERY_INFORMATION,
            false,
            target.pid,
        )
        .expect("re-open target handle for VirtualQueryEx")
    }
}

fn scan_type_to_kind(t: ScanType) -> ValueKind {
    match t {
        ScanType::I8 => ValueKind::I8,
        ScanType::I16 => ValueKind::I16,
        ScanType::I32 => ValueKind::I32,
        ScanType::I64 => ValueKind::I64,
        ScanType::U8 => ValueKind::U8,
        ScanType::U16 => ValueKind::U16,
        ScanType::U32 => ValueKind::U32,
        ScanType::U64 => ValueKind::U64,
        ScanType::F32 => ValueKind::F32,
        ScanType::F64 => ValueKind::F64,
        ScanType::Bool => ValueKind::Bool,
    }
}

fn print_sample(addrs: &[usize], values: &[f64], n: usize) {
    for (i, addr) in addrs.iter().take(n).enumerate() {
        let v = values.get(i).copied().unwrap_or(0.0);
        term::bullet(format!("  0x{addr:X}  = {v}"));
    }
    if addrs.len() > n {
        term::dim(format!("  ... and {} more", addrs.len() - n));
    }
}
