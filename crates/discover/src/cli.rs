//! Clap argument structs. Keep all CLI surface here so handlers stay testable.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};
use tracing::Level;

#[derive(Parser, Debug)]
#[command(
    name = "openforge-discover",
    version,
    about = "Find memory addresses for OpenForge game signatures.",
    long_about = None
)]
pub struct Cli {
    #[arg(long, global = true)]
    pub workspace: Option<PathBuf>,

    #[arg(long, global = true, value_enum, default_value_t = LogLevel::Info)]
    pub log_level: LogLevel,

    #[arg(long, global = true)]
    pub no_color: bool,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(ValueEnum, Clone, Copy, Debug)]
pub enum LogLevel {
    Off,
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}

impl From<LogLevel> for Level {
    fn from(l: LogLevel) -> Self {
        match l {
            LogLevel::Off | LogLevel::Error => Level::ERROR,
            LogLevel::Warn => Level::WARN,
            LogLevel::Info => Level::INFO,
            LogLevel::Debug => Level::DEBUG,
            LogLevel::Trace => Level::TRACE,
        }
    }
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Run a system preflight check.
    Doctor(DoctorArgs),
    /// Open the game's process and list discovered modules.
    Attach(AttachArgs),
    /// First scan for a value of a given type.
    Scan(ScanArgs),
    /// Narrow an existing candidate set with a follow-up predicate.
    Narrow(NarrowArgs),
    /// Reduce the candidate set to a single address.
    Pick(PickArgs),
    /// Dump bytes (and optionally disassembly) around an address.
    Inspect(InspectArgs),
    /// Conservative windowed-value search anchored on a known address. Reads a
    /// small region around `--addr` and prints offsets where the value appears.
    /// Use this when you already know the player-state base and want to find
    /// adjacent fields without a full memory scan.
    Probe(ProbeArgs),
    /// Write a single value to a specific address. Diagnostic tool used during
    /// discovery to test whether a candidate field actually feeds the game's
    /// logic (write persists) vs being a passive UI mirror (overwritten on the
    /// next render tick).
    Poke(PokeArgs),
    /// Record manually-pasted writing-instruction bytes for AOB extraction.
    Capture(CaptureArgs),
    /// Synthesize an AOB signature from captured bytes.
    ExtractAob(ExtractAobArgs),
    /// Attach as a debugger and capture the instruction that writes to the
    /// session's selected address via a hardware breakpoint.
    WatchWrite(WatchWriteArgs),
    /// Scan writable memory for 8-byte values that point near the given
    /// address. Useful for finding objects that own a dynamic field.
    FindPointers(FindPointersArgs),
    /// Recursive pointer-chain search rooted at a static module address.
    TraceChain(TraceChainArgs),
    /// Scan .text for `call <target>` sites and disassemble the preceding bytes
    /// to surface how arguments (especially rcx) are prepared by the caller.
    FindCallers(FindCallersArgs),
    /// Write the session's signature to `crates/games/<id>/signatures/<feature>.toml`.
    Emit(EmitArgs),
    /// Re-scan every committed signature and report pass/fail.
    Verify(VerifyArgs),
    /// Manage discovery session state on disk.
    Session(SessionArgs),
    /// List live UObjects matching name regex.
    Ue5FindObject(Ue5FindObjectArgs),
    /// Find properties by name across all UClasses.
    Ue5FindProp(Ue5FindPropArgs),
    /// Find UFunctions by name for code_patch targeting.
    Ue5FindUfunc(Ue5FindUfuncArgs),
    /// Dump full property list of a class.
    Ue5DumpClass(Ue5DumpClassArgs),
    /// Glacier 2: resolve a type by name via the ZTypeRegistry and enumerate
    /// its properties — external RPM, no DLL injection.
    GlacierWalk(GlacierWalkArgs),
    /// Glacier 2: walk a LIVE entity instance — decode its `ZEntityType`'s
    /// `SPropertyData` array to per-instance byte offsets, and (with `--prop`)
    /// dump a named property's resolved offset + live value. The validation
    /// harness for the instance-layer offset chain.
    GlacierEntity(GlacierEntityArgs),
    /// Glacier 2: inject the per-game DLL and drive the Tier-2 named-pipe
    /// stack end-to-end — handshake, EnumModules, and (optionally) a reflection
    /// smoke test (`--type` / `--entity` / `--prop` / `--set`). Validates the
    /// in-process `GlacierReflection` server live.
    GlacierDll(GlacierDllArgs),
}

#[derive(Args, Debug)]
pub struct GlacierWalkArgs {
    #[arg(long, required = true)]
    pub game: String,
    /// Type name to resolve, e.g. "ZEntityImpl" or "ZCLSetHumanoidImmuneToDamage".
    #[arg(long)]
    pub r#type: String,
    /// Print raw IType/IClassType header bytes for offset validation.
    #[arg(long)]
    pub raw: bool,
    /// Max properties to print.
    #[arg(long, default_value_t = 80)]
    pub limit: usize,
}

#[derive(Args, Debug)]
pub struct GlacierEntityArgs {
    #[arg(long, required = true)]
    pub game: String,
    /// VA of a live `ZEntityImpl` instance. Hex (`0x...` or bare).
    pub address: String,
    /// Resolve a single named property (CRC32 match) and dump its live value
    /// at both candidate value-bases. Omit to list every property.
    #[arg(long)]
    pub prop: Option<String>,
    /// Bytes to dump for the resolved property's value (with `--prop`).
    #[arg(long, default_value_t = 8)]
    pub width: usize,
    /// Max properties to print in list mode.
    #[arg(long, default_value_t = 200)]
    pub limit: usize,
}

#[derive(Args, Debug)]
pub struct GlacierDllArgs {
    #[arg(long, required = true)]
    pub game: String,
    /// Explicit path to the Glacier DLL. Defaults to resolving the manifest's
    /// `dll_file_name` from `target/{debug,release}` or the exe sibling.
    #[arg(long)]
    pub dll: Option<PathBuf>,
    /// Resolve a reflection type by name as a smoke test (e.g.
    /// `ZSpatialEntity`), printing its `IType` VA + flags. Repeatable via comma.
    #[arg(long)]
    pub r#type: Option<String>,
    /// Walk a live entity instance VA (hex, `0x...` or bare): lists its
    /// per-instance `SPropertyData` properties (or, with `--prop`, resolves one).
    #[arg(long)]
    pub entity: Option<String>,
    /// With `--entity`: resolve a single named property (CRC32 match).
    #[arg(long)]
    pub prop: Option<String>,
    /// With `--entity --prop`: write a typed value via the guarded `SetProperty`
    /// op. Format `kind:value`, e.g. `bool:true`, `i32:100`, `f32:1.5`,
    /// `u32:0xDEADBEEF`, `u64:42`.
    #[arg(long)]
    pub set: Option<String>,
    /// Heap-scan for LIVE entities carrying a named property (CRC32 match) and
    /// print their VAs — the anchor for targeting a specific entity. E.g.
    /// `--find-prop m_isUnkillable`.
    #[arg(long)]
    pub find_prop: Option<String>,
    /// Cap for `--find-prop` results.
    #[arg(long, default_value_t = 32)]
    pub max: u32,
    /// With `--find-prop --set kind:value`: apply the write to EVERY matched
    /// entity (mass-set) in one pipe session, snapshotting each original value
    /// to a revert CSV (`--revert-out`) so the change is cleanly reversible.
    #[arg(long)]
    pub all: bool,
    /// Path to write the revert CSV (`addr_hex,orig_bytes_hex`) during a
    /// mass-set. Defaults to `glacier_revert.csv` in the cwd.
    #[arg(long)]
    pub revert_out: Option<PathBuf>,
    /// Restore raw bytes from a revert CSV produced by a prior mass-set, then
    /// exit. Each line is `addr_hex,orig_bytes_hex`.
    #[arg(long)]
    pub restore: Option<PathBuf>,
    /// Peek a raw byte window at `entity_va + <offset>` (hex/dec). With
    /// `--entity` dumps that one window (f32/i32 decoded); with
    /// `--find-prop --all --snapshot-out` snapshots the window for every match.
    #[arg(long)]
    pub peek: Option<String>,
    /// Length of the `--peek` window in bytes.
    #[arg(long, default_value_t = 96)]
    pub peek_len: usize,
    /// With `--find-prop --all --peek`: write a position snapshot
    /// (`#off len` header then `va_hex,window_hex`) for a later `--diff`.
    #[arg(long)]
    pub snapshot_out: Option<PathBuf>,
    /// Re-read each VA's window from a prior `--snapshot-out` file and print the
    /// entities whose window changed most (max |Δf32|) — the live "who moved"
    /// discriminator. Then exit.
    #[arg(long)]
    pub diff: Option<PathBuf>,
    /// Heap-scan the live process (in-proc DLL `HeapScan`) for a u64 needle —
    /// e.g. a vtable VA to locate a singleton instance. Hex (`0x...`/bare).
    #[arg(long)]
    pub scan_u64: Option<String>,
    /// Alignment for `--scan-u64`.
    #[arg(long, default_value_t = 8)]
    pub scan_align: usize,
    /// Explicit comma-separated entity VAs (hex) to snapshot with
    /// `--peek --snapshot-out` instead of a `--find-prop` set. The "who moved"
    /// discriminator over a hand-picked candidate list.
    #[arg(long)]
    pub addrs: Option<String>,
    /// Freeze an address: re-write `--freeze-hex` bytes at this VA (hex) every
    /// ~33 ms for `--freeze-secs`. A live god-mode / infinite-value loop.
    #[arg(long)]
    pub freeze_addr: Option<String>,
    /// Little-endian bytes to stamp during `--freeze-addr` (hex, e.g. `64000000`).
    #[arg(long)]
    pub freeze_hex: Option<String>,
    /// How long to hold the `--freeze-addr` freeze, in seconds.
    #[arg(long, default_value_t = 30)]
    pub freeze_secs: u64,
    /// Fire a logic node's input pin via the engine `SignalInputPin` (protocol
    /// v3 game-thread engine call). Pass the VA of a live `ZEntityImpl` node
    /// (hex `0x...` or bare). Pin defaults to `Activate` (`0x4F1066FB`);
    /// override with `--pin`.
    #[arg(long)]
    pub fire: Option<String>,
    /// With `--fire`: raw input-pin id (decimal or `0x...`). Glacier pin ids
    /// are IOI's proprietary signed-i32 hash, NOT CRC32 — pass a literal.
    /// Omit for the `Activate` pin.
    #[arg(long)]
    pub pin: Option<String>,
    /// Max properties / log lines to print.
    #[arg(long, default_value_t = 60)]
    pub limit: usize,
}

#[derive(Args, Debug)]
pub struct Ue5FindObjectArgs {
    #[arg(long, required = true)]
    pub game: String,
    #[arg(long, required = true)]
    pub name: String,
    /// Maximum number of rows to print.
    #[arg(long, default_value_t = 200)]
    pub limit: usize,
    /// Filter on FQN by case-insensitive regex (any match).
    #[arg(long)]
    pub package: Option<String>,
    /// Emit one JSON object per match, suppressing headers/bullets.
    #[arg(long)]
    pub json: bool,
}

#[derive(Args, Debug)]
pub struct Ue5FindPropArgs {
    #[arg(long, required = true)]
    pub game: String,
    #[arg(long, required = true)]
    pub name: String,
    #[arg(long)]
    pub class: Option<String>,
    #[arg(long, value_enum)]
    pub r#type: Option<ScanType>,
    #[arg(long)]
    pub read_values: bool,
    #[arg(long)]
    pub emit_as: Option<String>,
    /// Maximum number of class-rows to print (after class dedup).
    #[arg(long, default_value_t = 200)]
    pub limit: usize,
    /// Filter on FQN by case-insensitive regex (any match).
    #[arg(long)]
    pub package: Option<String>,
    /// Emit one JSON object per match, suppressing headers/bullets.
    #[arg(long)]
    pub json: bool,
}

#[derive(Args, Debug)]
pub struct Ue5FindUfuncArgs {
    #[arg(long, required = true)]
    pub game: String,
    #[arg(long, required = true)]
    pub name: String,
    #[arg(long)]
    pub class: Option<String>,
    #[arg(long)]
    pub native_only: bool,
    #[arg(long)]
    pub emit_as: Option<String>,
    /// Maximum number of class-rows to print (after class dedup).
    #[arg(long, default_value_t = 200)]
    pub limit: usize,
    /// Filter on FQN by case-insensitive regex (any match).
    #[arg(long)]
    pub package: Option<String>,
    /// Emit one JSON object per match, suppressing headers/bullets.
    #[arg(long)]
    pub json: bool,
}

#[derive(Args, Debug)]
pub struct Ue5DumpClassArgs {
    #[arg(long, required = true)]
    pub game: String,
    #[arg(long, required = true)]
    pub name: String,
    /// Maximum number of rows to print.
    #[arg(long, default_value_t = 200)]
    pub limit: usize,
    /// Filter on FQN by case-insensitive regex (any match).
    #[arg(long)]
    pub package: Option<String>,
    /// Emit one JSON object per match, suppressing headers/bullets.
    #[arg(long)]
    pub json: bool,
}

#[derive(Args, Debug)]
pub struct DoctorArgs {}

#[derive(Args, Debug)]
pub struct AttachArgs {
    #[arg(long, required = true)]
    pub game: String,
}

#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScanType {
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

#[derive(Args, Debug)]
pub struct ScanArgs {
    #[arg(long, required = true)]
    pub game: String,
    #[arg(long, required = true)]
    pub feature: String,
    #[arg(long, value_enum)]
    pub r#type: ScanType,
    #[arg(long)]
    pub name: String,
    #[arg(long, conflicts_with = "unknown_initial")]
    pub value: Option<String>,
    #[arg(long, conflicts_with = "value")]
    pub unknown_initial: bool,
    #[arg(long)]
    pub strict: bool,
    #[arg(long, default_value_t = 1e-3)]
    pub epsilon: f64,
}

#[derive(Args, Debug)]
pub struct PokeArgs {
    #[arg(long, required = true)]
    pub game: String,
    /// Target address. Hex (`0x...` or bare).
    #[arg(long, required = true)]
    pub addr: String,
    /// Width of the value to write.
    #[arg(long, value_enum)]
    pub r#type: ScanType,
    /// Value to write. Ints accept decimal (including negative) or `0x`-hex;
    /// floats decimal; bool `true`/`false`.
    #[arg(long, required = true)]
    pub value: String,
}

#[derive(Args, Debug)]
pub struct NarrowArgs {
    #[arg(long, required = true)]
    pub game: String,
    #[arg(long, conflicts_with_all = ["not_equal","gt","ge","lt","le","between","changed","unchanged","increased","decreased","increased_by","decreased_by"])]
    pub equal: Option<String>,
    #[arg(long)]
    pub not_equal: Option<String>,
    #[arg(long)]
    pub gt: Option<String>,
    #[arg(long)]
    pub ge: Option<String>,
    #[arg(long)]
    pub lt: Option<String>,
    #[arg(long)]
    pub le: Option<String>,
    #[arg(long, num_args = 2, value_names = ["MIN","MAX"])]
    pub between: Option<Vec<String>>,
    #[arg(long)]
    pub changed: bool,
    #[arg(long)]
    pub unchanged: bool,
    #[arg(long)]
    pub increased: bool,
    #[arg(long)]
    pub decreased: bool,
    #[arg(long)]
    pub increased_by: Option<String>,
    #[arg(long)]
    pub decreased_by: Option<String>,
    #[arg(long, required = true)]
    pub feature: String,
}

#[derive(Args, Debug)]
pub struct PickArgs {
    #[arg(long, required = true)]
    pub game: String,
    #[arg(long, required = true)]
    pub feature: String,
    #[arg(long, conflicts_with = "address")]
    pub index: Option<usize>,
    #[arg(long, conflicts_with = "index")]
    pub address: Option<String>,
}

#[derive(Args, Debug)]
pub struct InspectArgs {
    #[arg(long, required = true)]
    pub game: String,
    pub address: String,
    #[arg(long, default_value_t = 64)]
    pub bytes: usize,
    #[arg(long)]
    pub disasm: bool,
    /// Hardware-breakpoint capture is not yet implemented; this flag is rejected for now.
    #[arg(long)]
    pub watch_write: bool,
}

#[derive(Args, Debug)]
pub struct ProbeArgs {
    #[arg(long, required = true)]
    pub game: String,
    /// Anchor address. Hex (`0x...` or bare).
    #[arg(long, required = true)]
    pub addr: String,
    /// Bytes to read before the anchor.
    #[arg(long, default_value_t = 0x400)]
    pub before: usize,
    /// Bytes to read after the anchor.
    #[arg(long, default_value_t = 0x400)]
    pub after: usize,
    /// Value type to search for.
    #[arg(long, value_enum)]
    pub r#type: ScanType,
    /// Target value. Ints accept decimal or `0x`-hex; floats decimal; bool `true`/`false`.
    #[arg(long, required = true)]
    pub value: String,
    /// Alignment in bytes (default = type size). Step by this when scanning.
    #[arg(long)]
    pub alignment: Option<usize>,
}

#[derive(Args, Debug)]
pub struct CaptureArgs {
    #[arg(long, required = true)]
    pub game: String,
    #[arg(long, required = true)]
    pub feature: String,
    #[arg(long)]
    pub rip: String,
    #[arg(long)]
    pub bytes: String,
}

#[derive(Args, Debug)]
pub struct ExtractAobArgs {
    #[arg(long, required = true)]
    pub game: String,
    #[arg(long, required = true)]
    pub feature: String,
    pub address: String,
    #[arg(long, default_value_t = 16)]
    pub window: usize,
    #[arg(long)]
    pub conservative: bool,
}

#[derive(Args, Debug)]
pub struct WatchWriteArgs {
    #[arg(long, required = true)]
    pub game: String,
    #[arg(long, required = true)]
    pub feature: String,
    /// Override the address (default: the feature's session-selected or single-candidate address).
    #[arg(long)]
    pub address: Option<String>,
    /// Write width in bytes (1, 2, 4, or 8).
    #[arg(long, default_value_t = 4)]
    pub width: u8,
    /// How many seconds to wait for a write before giving up.
    #[arg(long, default_value_t = 120)]
    pub timeout: u64,
    /// How many bytes to capture around the RIP. Default 48.
    #[arg(long, default_value_t = 48)]
    pub window: usize,
}

#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum EmitStrategy {
    /// Auto-detect from the session (code_patch if `captured_bytes` exists, else one_shot).
    Auto,
    OneShot,
    Freeze,
    CodePatch,
}

#[derive(Args, Debug)]
pub struct FindPointersArgs {
    #[arg(long, required = true)]
    pub game: String,
    /// The target address to find pointers to. Hex (`0x...` or bare).
    #[arg(long, required = true)]
    pub target: String,
    /// Maximum (positive) trailing offset `target - value` to accept.
    #[arg(long, default_value_t = 0x400)]
    pub max_offset: usize,
    /// Alignment, in bytes, of candidate values within a page (8 = u64).
    #[arg(long, default_value_t = 8)]
    pub alignment: usize,
    /// How many candidates to print after sorting.
    #[arg(long, default_value_t = 50)]
    pub top: usize,
    /// Persist results to the named session (creates / updates
    /// `candidate_pointers`). If omitted, no session is written.
    #[arg(long)]
    pub feature: Option<String>,
}

#[derive(Args, Debug)]
pub struct TraceChainArgs {
    #[arg(long, required = true)]
    pub game: String,
    /// The terminal address every chain must resolve to (± `max_offset`).
    #[arg(long, required = true)]
    pub target: String,
    /// Maximum number of pointer hops between the static anchor and target.
    #[arg(long, default_value_t = 4)]
    pub max_depth: usize,
    /// Per-hop maximum trailing offset.
    #[arg(long, default_value_t = 0x400)]
    pub max_offset: usize,
    /// Cap total exploration to avoid combinatorial blowup.
    #[arg(long, default_value_t = 200)]
    pub max_candidates: usize,
    /// Restrict static anchor to this module. Defaults to primary module.
    #[arg(long)]
    pub module: Option<String>,
    /// Persist results to the named session (creates / updates
    /// `candidate_chains`). If omitted, no session is written.
    #[arg(long)]
    pub feature: Option<String>,
}

#[derive(Args, Debug)]
pub struct FindCallersArgs {
    #[arg(long, required = true)]
    pub game: String,
    /// The function entry whose call sites we're looking for. Hex.
    #[arg(long, required = true)]
    pub target: String,
    /// How many bytes BEFORE each call to disassemble.
    #[arg(long, default_value_t = 64)]
    pub window_before: usize,
    /// How many bytes AFTER each call to disassemble (so you see the next op).
    #[arg(long, default_value_t = 0)]
    pub window_after: usize,
    /// Restrict to this module (default: primary module).
    #[arg(long)]
    pub module: Option<String>,
    /// Cap printed call sites; sorted by RIP ascending.
    #[arg(long, default_value_t = 32)]
    pub top: usize,
}

#[derive(Args, Debug)]
pub struct EmitArgs {
    #[arg(long, required = true)]
    pub game: String,
    pub feature: String,
    #[arg(long)]
    pub force: bool,
    /// Write strategy for the emitted signature. Defaults to auto-detect.
    #[arg(long, value_enum, default_value_t = EmitStrategy::Auto)]
    pub strategy: EmitStrategy,
    /// Comma-separated hop list applied after locator resolution. Each hop is
    /// `d:<hex_offset>` (deref-then-add) or `+:<hex_offset>` (no-deref add).
    /// Example: `d:0x10,d:0x40,d:0x18,+:0x38`.
    #[arg(long)]
    pub hops: Option<String>,
    /// Freeze interval in milliseconds. Only honored when `--strategy freeze`.
    #[arg(long, default_value_t = 250)]
    pub interval_ms: u32,
    /// Override the original bytes for a code_patch (defaults to `captured_bytes` from the session).
    #[arg(long)]
    pub original_bytes: Option<String>,
    /// Patched bytes for a code_patch. Required when `--strategy code_patch`; usually four NOPs (`90 90 90 90`).
    #[arg(long)]
    pub patched_bytes: Option<String>,
}

#[derive(Args, Debug)]
pub struct VerifyArgs {
    #[arg(long, required = true)]
    pub game: String,
    #[arg(long)]
    pub version: Option<String>,
    #[arg(long)]
    pub update_verified: bool,
}

#[derive(Args, Debug)]
pub struct SessionArgs {
    #[arg(long, required = true)]
    pub game: String,
    #[command(subcommand)]
    pub action: SessionAction,
}

#[derive(Subcommand, Debug)]
pub enum SessionAction {
    /// List all sessions for this game.
    List,
    /// Show one session in detail.
    Show { feature: String },
    /// Remove one session.
    Rm { feature: String },
    /// Remove every session for this game.
    RmAll,
}
