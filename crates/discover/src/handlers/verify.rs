use std::fs;
use std::process::ExitCode;

use anyhow::{Context, Result, anyhow};
use openforge_core::{Ctx, Pattern, Target};
use openforge_runtime::{ResolveSpec, SignatureSpec};

use crate::cli::VerifyArgs;
use crate::context::DiscoverContext;
use crate::term;

pub fn run(ctx: &DiscoverContext, args: &VerifyArgs) -> Result<ExitCode> {
    let candidates: Vec<&str> = ctx
        .manifest
        .game
        .process_names
        .iter()
        .map(String::as_str)
        .collect();
    let target = Target::attach_by_candidates(&candidates)?;
    let ctx_ref: &dyn Ctx = &target;

    let primary_module = ctx
        .manifest
        .game
        .primary_module
        .clone()
        .unwrap_or_else(|| target.main().name.clone());

    let sig_dir = ctx.signatures_dir.clone();
    if !sig_dir.exists() {
        term::warn("no signatures directory", sig_dir.display());
        return Ok(ExitCode::SUCCESS);
    }
    let mut entries: Vec<_> = fs::read_dir(&sig_dir)?
        .flatten()
        .filter(|e| e.path().extension().and_then(|s| s.to_str()) == Some("toml"))
        .collect();
    entries.sort_by_key(|e| e.file_name());

    if entries.is_empty() {
        term::warn("no signatures", "the signatures directory is empty");
        return Ok(ExitCode::SUCCESS);
    }

    term::header(&format!(
        "verify: {} ({} signatures)",
        ctx.game_slug,
        entries.len()
    ));

    let mut passed = 0;
    let mut failed = 0;
    let mut newly_passing = Vec::new();

    for entry in &entries {
        let path = entry.path();
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("?");
        let text = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;

        let parsed = match SignatureSpec::parse(&text).and_then(|s| {
            s.validate()?;
            Ok(s)
        }) {
            Ok(s) => s,
            Err(e) => {
                term::fail(stem, e);
                failed += 1;
                continue;
            }
        };

        let Some(locator) = parsed.locator.as_ref() else {
            // heap_scan signatures can't be verified statically; skip them.
            term::ok(&format!("{stem}: skipped (heap_scan locator)"));
            passed += 1;
            continue;
        };
        let module = locator
            .module
            .clone()
            .unwrap_or_else(|| primary_module.clone());

        let pattern = match Pattern::parse(&locator.pattern) {
            Ok(p) => p,
            Err(e) => {
                term::fail(stem, format!("pattern parse: {e}"));
                failed += 1;
                continue;
            }
        };

        match ctx_ref.scan_module_all(&module, &pattern) {
            Ok(hits) if hits.len() == 1 => {
                let match_addr = hits[0];
                let resolve: openforge_core::Resolve = locator.resolve.into();
                match resolve.apply(ctx_ref, match_addr) {
                    Ok(addr) => {
                        // Try a value read using the declared kind (skip code_patch)
                        if let Some(v) = &parsed.value {
                            let mut buf = vec![0u8; v.kind.size()];
                            match ctx_ref.read_bytes(addr, &mut buf) {
                                Ok(_) => {
                                    term::ok(&format!(
                                        "{stem}: addr=0x{addr:X} ok ({} bytes read)",
                                        buf.len()
                                    ));
                                    passed += 1;
                                }
                                Err(e) => {
                                    term::fail(stem, format!("read: {e}"));
                                    failed += 1;
                                }
                            }
                        } else {
                            term::ok(&format!("{stem}: addr=0x{addr:X} ok (code_patch)"));
                            passed += 1;
                        }
                        if args.update_verified && args.version.is_some() {
                            newly_passing.push((path.clone(), text.clone()));
                        }
                    }
                    Err(e) => {
                        term::fail(stem, format!("resolve: {e}"));
                        failed += 1;
                    }
                }
            }
            Ok(hits) => {
                term::fail(
                    stem,
                    format!(
                        "expected 1 match, got {} (signature ambiguous on this build)",
                        hits.len()
                    ),
                );
                failed += 1;
            }
            Err(e) => {
                term::fail(stem, format!("scan: {e}"));
                failed += 1;
            }
        }
    }

    if args.update_verified {
        let Some(ver) = &args.version else {
            return Err(anyhow!("--update-verified requires --version <v>"));
        };
        for (path, original) in &newly_passing {
            if let Err(e) = update_verified_in_text(path, original, ver) {
                term::warn(&path.display().to_string(), e);
            }
        }
    }

    term::header(&format!("verify: {passed} passed, {failed} failed"));
    Ok(if failed == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(2)
    })
}

fn update_verified_in_text(path: &std::path::Path, original: &str, version: &str) -> Result<()> {
    let mut lines: Vec<String> = original.lines().map(str::to_string).collect();
    let mut in_meta = false;
    let mut replaced = false;
    for line in &mut lines {
        let trimmed = line.trim_start();
        if trimmed.starts_with('[') {
            in_meta = trimmed.starts_with("[meta]");
            continue;
        }
        if in_meta && trimmed.starts_with("verified_versions") {
            *line = format!("verified_versions = [\"{version}\"]");
            replaced = true;
            break;
        }
    }
    if !replaced {
        // Insert after the [meta] header
        let mut new_lines = Vec::with_capacity(lines.len() + 1);
        for line in lines {
            new_lines.push(line.clone());
            if line.trim() == "[meta]" {
                new_lines.push(format!("verified_versions = [\"{version}\"]"));
            }
        }
        lines = new_lines;
    }
    let new_text = lines.join("\n");
    fs::write(path, new_text)?;
    let _ = ResolveSpec::Direct; // silence "unused" warning if it occurs
    Ok(())
}
