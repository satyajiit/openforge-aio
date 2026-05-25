use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};

#[derive(Parser, Debug)]
#[command(
    name = "openforge-cli",
    version,
    about = "OpenForge scaffolder + registry lints (dev-only)."
)]
pub struct Cli {
    #[arg(long, global = true)]
    pub workspace: Option<PathBuf>,

    #[arg(long, global = true)]
    pub no_color: bool,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Create a new game crate from the `_template`.
    NewGame(NewGameArgs),
    /// Create a signature TOML stub.
    NewFeature(NewFeatureArgs),
    /// Lint every game crate against the registry invariants.
    VerifyRegistry(VerifyRegistryArgs),
    /// Print every shipped game's metadata.
    ListGames(ListGamesArgs),
}

#[derive(Args, Debug)]
pub struct NewGameArgs {
    #[arg(long)]
    pub id: String,
    #[arg(long)]
    pub name: String,
    #[arg(long)]
    pub tagline: Option<String>,
    #[arg(long)]
    pub process: Option<String>,
}

#[derive(ValueEnum, Clone, Copy, Debug)]
pub enum FeatureKind {
    I8,
    I16,
    I32,
    I64,
    U8,
    U16,
    U32,
    U64,
    F32,
    F64,
    Bool,
}

#[derive(ValueEnum, Clone, Copy, Debug)]
pub enum WriteStrategy {
    OneShot,
    Freeze,
    CodePatch,
}

#[derive(Args, Debug)]
pub struct NewFeatureArgs {
    #[arg(long)]
    pub game: String,
    #[arg(long)]
    pub feature: String,
    #[arg(long)]
    pub name: String,
    #[arg(long, value_enum)]
    pub kind: FeatureKind,
    #[arg(long, value_enum, default_value_t = WriteStrategy::OneShot)]
    pub strategy: WriteStrategy,
    #[arg(long)]
    pub tier: Option<String>,
    #[arg(long)]
    pub description: Option<String>,
}

#[derive(Args, Debug)]
pub struct VerifyRegistryArgs {
    #[arg(long)]
    pub strict: bool,
}

#[derive(Args, Debug)]
pub struct ListGamesArgs {
    #[arg(long)]
    pub markdown: bool,
}
