//! Generic typed scanners. Read pages once; iterate; apply predicate.

use anyhow::Result;
use indicatif::{ProgressBar, ProgressStyle};
use openforge_core::Ctx;

use crate::pages::MemoryRegion;
use crate::predicate::{NumberLike, Predicate};

/// Read each `MemoryRegion`, slide over it, apply the predicate to every value
/// of type `T` at every byte offset. Returns `(address, current_value)` tuples
/// for every match.
pub fn first_scan<T: NumberLike>(
    ctx: &dyn Ctx,
    regions: &[MemoryRegion],
    predicate: Predicate<T>,
    eps: f64,
) -> Result<Vec<(usize, T)>> {
    let total: u64 = regions.iter().map(|r| r.size as u64).sum();
    let bar = make_bar(total, "scan");
    let mut out = Vec::new();

    for region in regions {
        let mut buf = vec![0u8; region.size];
        if let Err(e) = ctx.read_bytes(region.base, &mut buf) {
            tracing::debug!(base = format!("0x{:X}", region.base), error = %e, "skip region");
            bar.inc(region.size as u64);
            continue;
        }
        let stride = 1usize;
        let last = region.size.saturating_sub(T::SIZE);
        let mut i = 0usize;
        while i <= last {
            let v = T::from_le_bytes(&buf[i..i + T::SIZE]);
            if predicate.matches(v, None, eps) {
                out.push((region.base + i, v));
            }
            i += stride;
        }
        bar.inc(region.size as u64);
    }
    bar.finish_and_clear();
    Ok(out)
}

/// Re-read each previous candidate, apply the predicate against the new value.
pub fn next_scan<T: NumberLike>(
    ctx: &dyn Ctx,
    previous: &[(usize, T)],
    predicate: Predicate<T>,
    eps: f64,
) -> Result<Vec<(usize, T)>> {
    let bar = make_bar(previous.len() as u64, "narrow");
    let mut out = Vec::with_capacity(previous.len() / 4);

    let mut buf = [0u8; 16]; // big enough for any NumberLike
    for &(addr, prev_v) in previous {
        let slice = &mut buf[..T::SIZE];
        match ctx.read_bytes(addr, slice) {
            Ok(()) => {
                let v = T::from_le_bytes(slice);
                if predicate.matches(v, Some(prev_v), eps) {
                    out.push((addr, v));
                }
            }
            Err(_) => {
                // Address went away (region unmapped). Drop silently.
            }
        }
        bar.inc(1);
    }
    bar.finish_and_clear();
    Ok(out)
}

fn make_bar(total: u64, label: &str) -> ProgressBar {
    let bar = ProgressBar::new(total);
    let style = ProgressStyle::with_template(&format!(
        "{label} {{spinner}} [{{wide_bar}}] {{percent:>3}}% ({{eta}})"
    ))
    .unwrap_or(ProgressStyle::default_bar())
    .progress_chars("=> ");
    bar.set_style(style);
    bar
}
