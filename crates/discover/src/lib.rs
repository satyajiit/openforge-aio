//! OpenForge discovery CLI: find memory addresses, synthesize AOB signatures.
//!
//! Dev-only tool. Never ships to end users.

pub mod aob;
pub mod cli;
pub mod context;
pub mod elevation;
pub mod emit;
pub mod handlers;
pub mod pages;
#[cfg(windows)]
pub mod pointer_scan;
pub mod predicate;
pub mod scan;
pub mod session;
pub mod term;
pub mod ue5;
#[cfg(windows)]
pub mod watch_write;
pub mod workspace;

use std::process::ExitCode;

use anyhow::Result;
use clap::Parser;
use tracing::Level;
use tracing_subscriber::EnvFilter;

use crate::cli::{Cli, Command};

pub fn run() -> Result<ExitCode> {
    let cli = Cli::parse();
    init_logging(cli.log_level);
    if cli.no_color {
        unsafe { std::env::set_var("NO_COLOR", "1") };
    }

    let ws = match &cli.workspace {
        Some(p) => workspace::Workspace::from_explicit(p)?,
        None => workspace::Workspace::find_from_cwd()?,
    };

    match cli.command {
        Command::Doctor(args) => handlers::doctor::run(&args),
        Command::Attach(args) => {
            let ctx = context::DiscoverContext::load(&ws, &args.game)?;
            handlers::attach::run(&ctx)
        }
        Command::Scan(args) => {
            let ctx = context::DiscoverContext::load(&ws, &args.game)?;
            handlers::scan::run(&ctx, &args)
        }
        Command::Narrow(args) => {
            let ctx = context::DiscoverContext::load(&ws, &args.game)?;
            handlers::narrow::run(&ctx, &args)
        }
        Command::Pick(args) => {
            let ctx = context::DiscoverContext::load(&ws, &args.game)?;
            handlers::pick::run(&ctx, &args)
        }
        Command::Inspect(args) => {
            let ctx = context::DiscoverContext::load(&ws, &args.game)?;
            handlers::inspect::run(&ctx, &args)
        }
        Command::Probe(args) => {
            let ctx = context::DiscoverContext::load(&ws, &args.game)?;
            handlers::probe::run(&ctx, &args)
        }
        Command::Poke(args) => {
            let ctx = context::DiscoverContext::load(&ws, &args.game)?;
            handlers::poke::run(&ctx, &args)
        }
        Command::Capture(args) => {
            let ctx = context::DiscoverContext::load(&ws, &args.game)?;
            handlers::capture::run(&ctx, &args)
        }
        Command::ExtractAob(args) => {
            let ctx = context::DiscoverContext::load(&ws, &args.game)?;
            handlers::extract_aob::run(&ctx, &args)
        }
        #[cfg(windows)]
        Command::WatchWrite(args) => {
            let ctx = context::DiscoverContext::load(&ws, &args.game)?;
            handlers::watch_write::run(&ctx, &args)
        }
        #[cfg(not(windows))]
        Command::WatchWrite(_) => Err(anyhow::anyhow!("watch-write is only available on Windows")),
        #[cfg(windows)]
        Command::FindPointers(args) => {
            let ctx = context::DiscoverContext::load(&ws, &args.game)?;
            handlers::find_pointers::run(&ctx, &args)
        }
        #[cfg(not(windows))]
        Command::FindPointers(_) => Err(anyhow::anyhow!(
            "find-pointers is only available on Windows"
        )),
        #[cfg(windows)]
        Command::TraceChain(args) => {
            let ctx = context::DiscoverContext::load(&ws, &args.game)?;
            handlers::trace_chain::run(&ctx, &args)
        }
        #[cfg(not(windows))]
        Command::TraceChain(_) => Err(anyhow::anyhow!("trace-chain is only available on Windows")),
        Command::FindCallers(args) => {
            let ctx = context::DiscoverContext::load(&ws, &args.game)?;
            handlers::find_callers::run(&ctx, &args)
        }
        Command::Emit(args) => {
            let ctx = context::DiscoverContext::load(&ws, &args.game)?;
            handlers::emit::run(&ctx, &args)
        }
        Command::Verify(args) => {
            let ctx = context::DiscoverContext::load(&ws, &args.game)?;
            handlers::verify::run(&ctx, &args)
        }
        Command::Session(args) => {
            let ctx = context::DiscoverContext::load(&ws, &args.game)?;
            handlers::session::run(&ctx, &args)
        }
        Command::Ue5FindObject(args) => {
            let ctx = context::DiscoverContext::load(&ws, &args.game)?;
            handlers::ue5::find_object::run(&ctx, &args)
        }
        Command::Ue5FindProp(args) => {
            let ctx = context::DiscoverContext::load(&ws, &args.game)?;
            handlers::ue5::find_prop::run(&ctx, &args)
        }
        Command::Ue5FindUfunc(args) => {
            let ctx = context::DiscoverContext::load(&ws, &args.game)?;
            handlers::ue5::find_ufunc::run(&ctx, &args)
        }
        Command::Ue5DumpClass(args) => {
            let ctx = context::DiscoverContext::load(&ws, &args.game)?;
            handlers::ue5::dump_class::run(&ctx, &args)
        }
        Command::GlacierWalk(args) => {
            let ctx = context::DiscoverContext::load(&ws, &args.game)?;
            handlers::glacier_walk::run(&ctx, &args)
        }
        Command::GlacierEntity(args) => {
            let ctx = context::DiscoverContext::load(&ws, &args.game)?;
            handlers::glacier_entity::run(&ctx, &args)
        }
        Command::GlacierDll(args) => {
            let ctx = context::DiscoverContext::load(&ws, &args.game)?;
            handlers::glacier_dll::run(&ctx, &args)
        }
    }
}

fn init_logging(level: cli::LogLevel) {
    let level: Level = level.into();
    let filter = EnvFilter::builder()
        .with_default_directive(level.into())
        .from_env_lossy();
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_writer(std::io::stderr)
        .try_init();
}
