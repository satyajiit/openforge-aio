use std::process::ExitCode;

use anyhow::{Context, Result, anyhow};
use openforge_core::{Ctx, Target};
use openforge_runtime::ValueKind;

use crate::cli::NarrowArgs;
use crate::context::DiscoverContext;
use crate::predicate::{NumberLike, Predicate};
use crate::scan;
use crate::session::{HistoryStep, Session};
use crate::term;

pub fn run(ctx: &DiscoverContext, args: &NarrowArgs) -> Result<ExitCode> {
    let path = ctx.session_path(&args.feature);
    let mut session =
        Session::load(&path).with_context(|| format!("load session {}", path.display()))?;

    let candidates: Vec<&str> = ctx
        .manifest
        .game
        .process_names
        .iter()
        .map(String::as_str)
        .collect();
    let target = Target::attach_by_candidates(&candidates)?;

    let label = pick_label(args)?;
    let (addrs, values) = match session.kind {
        ValueKind::I8 => narrow_typed::<i8>(args, &session, &target as &dyn Ctx)?,
        ValueKind::I16 => narrow_typed::<i16>(args, &session, &target as &dyn Ctx)?,
        ValueKind::I32 => narrow_typed::<i32>(args, &session, &target as &dyn Ctx)?,
        ValueKind::I64 => narrow_typed::<i64>(args, &session, &target as &dyn Ctx)?,
        ValueKind::U8 => narrow_typed::<u8>(args, &session, &target as &dyn Ctx)?,
        ValueKind::U16 => narrow_typed::<u16>(args, &session, &target as &dyn Ctx)?,
        ValueKind::U32 => narrow_typed::<u32>(args, &session, &target as &dyn Ctx)?,
        ValueKind::U64 => narrow_typed::<u64>(args, &session, &target as &dyn Ctx)?,
        ValueKind::F32 => narrow_typed::<f32>(args, &session, &target as &dyn Ctx)?,
        ValueKind::F64 => narrow_typed::<f64>(args, &session, &target as &dyn Ctx)?,
        ValueKind::Bool => narrow_typed::<bool>(args, &session, &target as &dyn Ctx)?,
    };
    let n = addrs.len();
    session.candidates = addrs;
    session.last_values = values;
    session.history.push(HistoryStep {
        action: label.clone(),
        matches_after: n,
    });
    session.save(&path)?;

    term::ok(&format!("narrow ({label}) complete: {n} candidates"));
    print_sample(&session.candidates, &session.last_values, 10);
    if n == 1 {
        term::next_step(format!(
            "openforge-discover --game {} inspect 0x{:X} --disasm",
            ctx.game_slug, session.candidates[0]
        ));
    } else if n > 1 {
        term::next_step(format!(
            "openforge-discover --game {} narrow --feature {} --changed   (act in-game first)",
            ctx.game_slug, args.feature
        ));
    } else {
        term::warn(
            "no candidates remain",
            "the predicate excluded every previous candidate; try a different predicate",
        );
    }
    Ok(ExitCode::SUCCESS)
}

fn narrow_typed<T: NumberLike + 'static>(
    args: &NarrowArgs,
    session: &Session,
    ctx: &dyn Ctx,
) -> Result<(Vec<usize>, Vec<f64>)> {
    let predicate = build_predicate::<T>(args)?;
    let previous: Vec<(usize, T)> = session
        .candidates
        .iter()
        .zip(session.last_values.iter())
        .map(|(&a, &v)| (a, cast_f64_to::<T>(v)))
        .collect();
    let hits = scan::next_scan::<T>(ctx, &previous, predicate, session.epsilon)?;
    let addrs = hits.iter().map(|(a, _)| *a).collect();
    let values = hits.iter().map(|(_, v)| v.as_f64()).collect();
    Ok((addrs, values))
}

fn pick_label(args: &NarrowArgs) -> Result<String> {
    if args.equal.is_some() {
        return Ok("equal".into());
    }
    if args.not_equal.is_some() {
        return Ok("not_equal".into());
    }
    if args.gt.is_some() {
        return Ok("gt".into());
    }
    if args.ge.is_some() {
        return Ok("ge".into());
    }
    if args.lt.is_some() {
        return Ok("lt".into());
    }
    if args.le.is_some() {
        return Ok("le".into());
    }
    if args.between.is_some() {
        return Ok("between".into());
    }
    if args.changed {
        return Ok("changed".into());
    }
    if args.unchanged {
        return Ok("unchanged".into());
    }
    if args.increased {
        return Ok("increased".into());
    }
    if args.decreased {
        return Ok("decreased".into());
    }
    if args.increased_by.is_some() {
        return Ok("increased_by".into());
    }
    if args.decreased_by.is_some() {
        return Ok("decreased_by".into());
    }
    Err(anyhow!("narrow requires exactly one predicate flag"))
}

fn build_predicate<T: NumberLike>(args: &NarrowArgs) -> Result<Predicate<T>> {
    if let Some(s) = &args.equal {
        return Ok(Predicate::Eq(T::parse(s)?));
    }
    if let Some(s) = &args.not_equal {
        return Ok(Predicate::Ne(T::parse(s)?));
    }
    if let Some(s) = &args.gt {
        return Ok(Predicate::Gt(T::parse(s)?));
    }
    if let Some(s) = &args.ge {
        return Ok(Predicate::Ge(T::parse(s)?));
    }
    if let Some(s) = &args.lt {
        return Ok(Predicate::Lt(T::parse(s)?));
    }
    if let Some(s) = &args.le {
        return Ok(Predicate::Le(T::parse(s)?));
    }
    if let Some(parts) = &args.between {
        if parts.len() != 2 {
            return Err(anyhow!("--between requires MIN MAX"));
        }
        return Ok(Predicate::Between(
            T::parse(&parts[0])?,
            T::parse(&parts[1])?,
        ));
    }
    if args.changed {
        return Ok(Predicate::Changed);
    }
    if args.unchanged {
        return Ok(Predicate::Unchanged);
    }
    if args.increased {
        return Ok(Predicate::Increased);
    }
    if args.decreased {
        return Ok(Predicate::Decreased);
    }
    if let Some(s) = &args.increased_by {
        return Ok(Predicate::IncreasedBy(T::parse(s)?));
    }
    if let Some(s) = &args.decreased_by {
        return Ok(Predicate::DecreasedBy(T::parse(s)?));
    }
    Err(anyhow!("narrow requires exactly one predicate flag"))
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

#[allow(clippy::cast_lossless)]
fn cast_f64_to<T: NumberLike + 'static>(v: f64) -> T {
    use std::any::TypeId;
    if TypeId::of::<T>() == TypeId::of::<bool>() {
        let b: bool = v != 0.0;
        unsafe { std::mem::transmute_copy(&b) }
    } else if TypeId::of::<T>() == TypeId::of::<i8>() {
        unsafe { std::mem::transmute_copy(&(v as i8)) }
    } else if TypeId::of::<T>() == TypeId::of::<i16>() {
        unsafe { std::mem::transmute_copy(&(v as i16)) }
    } else if TypeId::of::<T>() == TypeId::of::<i32>() {
        unsafe { std::mem::transmute_copy(&(v as i32)) }
    } else if TypeId::of::<T>() == TypeId::of::<i64>() {
        unsafe { std::mem::transmute_copy(&(v as i64)) }
    } else if TypeId::of::<T>() == TypeId::of::<u8>() {
        unsafe { std::mem::transmute_copy(&(v as u8)) }
    } else if TypeId::of::<T>() == TypeId::of::<u16>() {
        unsafe { std::mem::transmute_copy(&(v as u16)) }
    } else if TypeId::of::<T>() == TypeId::of::<u32>() {
        unsafe { std::mem::transmute_copy(&(v as u32)) }
    } else if TypeId::of::<T>() == TypeId::of::<u64>() {
        unsafe { std::mem::transmute_copy(&(v as u64)) }
    } else if TypeId::of::<T>() == TypeId::of::<f32>() {
        unsafe { std::mem::transmute_copy(&(v as f32)) }
    } else if TypeId::of::<T>() == TypeId::of::<f64>() {
        unsafe { std::mem::transmute_copy(&v) }
    } else {
        unreachable!("unknown NumberLike type")
    }
}
