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
    /// Target game engine. A schema-2 manifest must declare one, so this is
    /// required — there is no silent legacy default.
    #[arg(long, value_enum)]
    pub engine: EngineArg,
    /// On-disk format for this game's signature files. The per-file extension
    /// still wins at build time; this only seeds the manifest's
    /// `[engine].config_format`.
    #[arg(long, value_enum, default_value_t = ConfigFormatArg::Toml)]
    pub format: ConfigFormatArg,
}

/// Engine kinds the scaffolder can write into a manifest. Mirrors
/// `openforge_runtime::EngineKind`; the `ValueEnum` string for each arm is the
/// manifest `[engine].kind` value (validated against
/// `EngineKind::from_manifest_str` in the handler).
#[derive(ValueEnum, Clone, Copy, Debug)]
pub enum EngineArg {
    Ue5,
    Glacier2,
}

impl EngineArg {
    /// The manifest `[engine].kind` string for this engine.
    pub fn manifest_kind(self) -> &'static str {
        match self {
            EngineArg::Ue5 => "ue5",
            EngineArg::Glacier2 => "glacier2",
        }
    }
}

/// Signature config format the scaffolder writes into `[engine].config_format`.
/// Mirrors `openforge_runtime::ConfigFormat`.
#[derive(ValueEnum, Clone, Copy, Debug)]
pub enum ConfigFormatArg {
    Toml,
    Ron,
}

impl ConfigFormatArg {
    /// The manifest `[engine].config_format` string for this format.
    pub fn manifest_format(self) -> &'static str {
        match self {
            ConfigFormatArg::Toml => "toml",
            ConfigFormatArg::Ron => "ron",
        }
    }
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
