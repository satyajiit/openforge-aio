use std::process::ExitCode;

use anyhow::{Result, anyhow};
use openforge_core::{ProcessSnapshot, Target};

use crate::context::DiscoverContext;
use crate::term;

pub fn run(ctx: &DiscoverContext) -> Result<ExitCode> {
    term::header(&format!("attach: {}", ctx.game_slug));

    let snap = ProcessSnapshot::capture()?;

    // Refuse if any forbidden service is running.
    let forbidden: Vec<&str> = ctx
        .manifest
        .game
        .forbidden_services
        .iter()
        .filter_map(|n| {
            snap.entries
                .iter()
                .find(|p| p.name.eq_ignore_ascii_case(n))
                .map(|_| n.as_str())
        })
        .collect();
    if !forbidden.is_empty() {
        term::fail(
            "anti-cheat / forbidden process running",
            forbidden.join(", "),
        );
        return Err(anyhow!("refusing to attach: forbidden services present"));
    }

    let candidates: Vec<&str> = ctx
        .manifest
        .game
        .process_names
        .iter()
        .map(String::as_str)
        .collect();

    let target = Target::attach_by_candidates(&candidates).map_err(|e| anyhow!(e))?;
    term::ok(&format!(
        "process {} (pid {}) attached",
        target.process_name, target.pid
    ));

    let main = target.main();
    term::bullet(format!(
        "main module: {} @ 0x{:X}, size {} bytes",
        main.name, main.base, main.size
    ));
    term::bullet(format!(
        ".text @ 0x{:X}, size {} bytes",
        main.text_base(),
        main.text_size
    ));

    term::header("all modules");
    for m in target.modules() {
        term::bullet(format!(
            "{:<48} base=0x{:X} size={:>10}",
            m.name, m.base, m.size
        ));
    }

    Ok(ExitCode::SUCCESS)
}
