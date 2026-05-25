//! Dev-only CLI for driving the UE5 DLL host: inject, eject, diagnose.
//!
//! Build with: `cargo build --release -p openforge-ue5-host --features cli`
//!
//! Examples:
//! ```text
//! openforge-ue5-host inject  --pid 12345
//! openforge-ue5-host eject   --pid 12345
//! openforge-ue5-host attach  --pid 12345 [--ping] [--walk N] [--drain-log]
//! openforge-ue5-host pipe-probe --pid 12345
//! ```

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use clap::{Parser, Subcommand};
use openforge_ue5_host::{
    DEFAULT_CONNECT_TIMEOUT, Injector, Ue5Client, Ue5Session,
    protocol::{LogLevel, NamePredicate, PropKind, PropValue, pipe_name_for_pid},
};

#[derive(Parser, Debug)]
#[command(
    name = "openforge-ue5-host",
    version,
    about = "Drive the UE5 DLL host (dev tooling)"
)]
struct Cli {
    #[arg(long, global = true)]
    verbose: bool,

    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Inject the DLL into a process.
    Inject(PidArgs),
    /// Try to eject the DLL via remote FreeLibrary. Often fails because the
    /// worker thread keeps the refcount alive.
    Eject(PidArgs),
    /// Eject then re-inject the DLL.
    Reinject(PidArgs),
    /// Attach (inject if needed) and run a diagnose sequence: handshake,
    /// ping, optional walk-objects head, optional drain-log.
    Attach(AttachArgs),
    /// Open the pipe, send Hello, print the Welcome — without going through
    /// `Ue5Session` (so we don't fail on layout_validated=false).
    PipeProbe(PidArgs),
    /// Dump every FProperty + UFunction on a UClass instance found
    /// in the running game. Use this to design the per-feature TOML
    /// reflection block — confirm the class exists, the property names you
    /// expect are there with the right `kind`, and check whether a setter
    /// UFunction is available for HUD-refresh correctness.
    Discover(DiscoverArgs),
    /// Call a UFunction by name on a live UObject via the
    /// engine's `ProcessEvent` dispatcher. Use this to test that the
    /// CallUFunction wire op works against a known-safe function (e.g.
    /// `IsSpectator` on a PlayerState) before designing features that
    /// invoke game-state-changing functions.
    CallFn(CallFnArgs),
    /// Live test: iterate the `Rules` TArray on a
    /// `TtGameProgressRuleSet` data asset, extract each rule's RuleID FName,
    /// and call `TtGameProgressStatics::SetGameProgressValue` for every
    /// rule. Use this to bulk-grant skill bricks (`--ruleset PROGR_SkillBricks`)
    /// or category-level skill tags (`--ruleset PROGR_Skills`). Skill-tree
    /// granular unlocks live one level deeper (each rule's `Values` TArray);
    /// `--recurse` will be added once flat call is proven safe.
    GrantProgressFromRules(GrantArgs),
    /// Raw `ReadBytes` against an arbitrary game-process address. Useful for
    /// hand-inspecting struct layouts when discovery doesn't reveal them
    /// (e.g. native-only structs inside TArray entries).
    ReadBytes(ReadBytesArgs),
    /// Probe-mode vehicle hijack. Finds the live player pawn, walks every
    /// Pawn UObject whose class name contains the configured substring,
    /// reads each candidate's world location via `K2_GetActorLocation`,
    /// and reports the nearest N. With `--commit`, calls
    /// `APlayerController::Possess(nearestVehicle)` on the live player
    /// controller to hand control of the vehicle to the player.
    PossessNearestVehicle(PossessArgs),
    /// End-to-end scripted-driver smoke test: Possess() the nearest vehicle,
    /// then loop `SetThrottleInput(--throttle)` on its `VehicleMovementComp`
    /// at ~50Hz for `--duration-ms` to see if the AIController concedes
    /// control. Auto-brakes + re-Possesses the original Batman pawn at the
    /// end. If the bike rolls forward, the scripted-driver feature is
    /// viable; if it stays glued in place, AI is dominating each tick.
    VehicleDriveTest(DriveTestArgs),
    /// Civilian-brawl probe: finds N nearest civilians to the player and
    /// drives them through scripted hit reactions / animations to test
    /// whether each technique produces a visible effect.
    ///
    /// `--technique attacked`   → calls `Attacked(BeingAttacked=true)` on
    ///                            each civilian every tick.
    /// `--technique player-reaction` → calls `PlayerReaction(player, true)`
    ///                                 with an OTHER civilian as the
    ///                                 "player" actor (testing whether the
    ///                                 reaction works with non-player input).
    /// `--technique brawl-pair` → real puppet-show: pair up nearby civilians,
    ///                            face them, alternate `Attacked(true)`
    ///                            calls — the actual feature.
    CivilianBrawlTest(CivilianBrawlArgs),
    /// Dump FProperty children at a raw UStruct address. UFunction is a
    /// UStruct, so this gives the parameter layout of a function. Find the
    /// function's address in a `discover-pipe` dump (the `addr` column) and
    /// paste it here to see ParmsSize / parameter list.
    WalkPropsAt(WalkPropsAtArgs),
    /// Mirror of `discover` that connects to an already-injected DLL pipe
    /// (no LoadLibraryW dance). Use when the Tauri app already owns the
    /// DLL and a second injection would fail.
    DiscoverPipe(DiscoverPipeArgs),
    /// Connect to an already-injected DLL pipe (no injection attempt) and
    /// walk every UObject. Prints unique class names with instance counts,
    /// optionally filtered by case-insensitive substring set. Use this to
    /// discover what NPC / civilian classes are live in the current scene
    /// without re-injecting the DLL (avoids "LoadLibraryW returned NULL"
    /// when the Tauri app already loaded the DLL from its bundled path).
    WalkClasses(WalkClassesArgs),
    /// Smoke test for `peds_fight_all`. Walks every live `Character`,
    /// filters out player/vehicles/CDOs/transients, and zeros 8 bytes at
    /// `+0xB28` (CurrentTeamTag FGameplayTag) on each. Optionally loops
    /// the writes for `--loop-seconds` to defeat BP tick re-writes (like
    /// the freeze_for_matching runtime would). Prints per-pass counts so
    /// we can tell whether NPCs are getting reset.
    PedsFightTest(PedsFightArgs),
}

#[derive(Parser, Debug)]
struct CivilianBrawlArgs {
    #[arg(long)]
    pid: u32,
    /// `attacked` | `player-reaction` | `brawl-pair`.
    #[arg(long, default_value = "attacked")]
    technique: String,
    /// Max number of civilian candidates to consider (sorted by distance).
    #[arg(long, default_value_t = 8)]
    top: u32,
    /// Distance cutoff from player, in UE units (1u ≈ 1cm).
    #[arg(long, default_value_t = 3000.0)]
    max_distance: f64,
    /// How long to keep ticking, in milliseconds.
    #[arg(long, default_value_t = 8000)]
    duration_ms: u32,
    /// Tick interval — gap between successive UFunction calls.
    #[arg(long, default_value_t = 400)]
    tick_ms: u32,
    /// Comma-separated class-name substrings (case-insensitive). Default
    /// catches both ambient population minifigs and quest civilians.
    #[arg(long, default_value = "BP_Population_Minifig,BP_Civilian_")]
    class_substrings: String,
    /// (brawl-pair only) Limit the number of pairs that fight simultaneously.
    /// 1 = single clean visible brawl near player. 0 = all pairs.
    #[arg(long, default_value_t = 1)]
    max_pairs: u32,
    /// (brawl-pair only) Maximum starting distance between pair partners
    /// (UE units). Pairs whose members are farther apart are skipped.
    #[arg(long, default_value_t = 600.0)]
    pair_max_apart: f64,
    /// (brawl-pair only) Distance at which a pair stops running toward
    /// each other and starts the attack/hit cycle (UE units). Once they're
    /// closer than this, the brawl proper begins.
    #[arg(long, default_value_t = 180.0)]
    engage_distance: f64,
    /// (brawl-pair only) How far each civilian advances toward the other
    /// per tick during the approach phase (UE units). Step ÷ tick-ms
    /// approximates run speed; e.g. 25 @ 100ms = ~250u/s walk.
    #[arg(long, default_value_t = 35.0)]
    approach_step: f64,
}

#[derive(Parser, Debug)]
struct WalkPropsAtArgs {
    #[arg(long)]
    pid: u32,
    /// Raw UStruct/UFunction address (hex with optional 0x prefix).
    #[arg(long, value_parser = parse_hex_u64)]
    addr: u64,
}

#[derive(Parser, Debug)]
struct DiscoverPipeArgs {
    #[arg(long)]
    pid: u32,
    #[arg(long)]
    class: String,
    #[arg(long, default_value = "any", value_parser = parse_predicate)]
    predicate: NamePredicate,
    #[arg(long, default_value_t = 0)]
    max_props: u32,
    #[arg(long, default_value_t = 0)]
    max_funcs: u32,
    #[arg(long)]
    no_funcs: bool,
    /// Dump the function list using the parent class chain pointer (super)
    /// of the matched class rather than the class itself. Use when child
    /// BP classes have no overrides and we want to see what the base
    /// inherits from C++.
    #[arg(long)]
    walk_super: bool,
}

#[derive(Parser, Debug)]
struct WalkClassesArgs {
    #[arg(long)]
    pid: u32,
    /// Comma-separated case-insensitive substrings. An object's class_name
    /// must contain at least one. Empty = print all unique class names.
    #[arg(long, default_value = "")]
    filter: String,
    /// Also print up to N live FQN-paths per matched class (so we can pick
    /// real instances). 0 = only print counts.
    #[arg(long, default_value_t = 3)]
    sample: u32,
    /// Limit printed classes (sorted by descending instance count). 0 = all.
    #[arg(long, default_value_t = 60)]
    top: u32,
    /// Drop CDOs / *_GEN_VARIABLE / Transient hits from the sample list.
    #[arg(long, default_value_t = true)]
    live_only: bool,
}

#[derive(Parser, Debug)]
struct PedsFightArgs {
    #[arg(long)]
    pid: u32,
    #[arg(long)]
    dll: PathBuf,
    /// How long to keep re-writing the team tag every ~250ms. 0 = write
    /// once and exit. 5+ recommended so you can watch the brawl spin up.
    #[arg(long, default_value_t = 10)]
    loop_seconds: u32,
    /// Comma-separated class-name substrings (case-insensitive). Any match
    /// includes the object. Default covers the full ped set: combatants
    /// (goons / SWAT / Arkham / TwoFace) AND civilians / population NPCs.
    #[arg(
        long,
        default_value = "_Goon_C,_SWAT_,Arkham,TwoFace,Civilian,Population"
    )]
    class_substrings: String,
    /// 8 hex bytes (16 hex chars, no spaces) to write at +0xB28 on each
    /// match. Default `0000000000000000` zeros the tag. Pass
    /// `66c8787d36948c74` to stamp every NPC with the JokerGang tag —
    /// Path-C diagnostic: if SWAT / Arkham / TwoFace stop fighting Joker
    /// goons after the write, CurrentTeamTag IS the field combat AI reads,
    /// and we just need to find a different value for "hostile to all".
    #[arg(long, default_value = "0000000000000000")]
    write_hex: String,
}

#[derive(Parser, Debug)]
struct DriveTestArgs {
    #[arg(long)]
    pid: u32,
    #[arg(long)]
    dll: PathBuf,
    /// How long to apply throttle, in milliseconds.
    #[arg(long, default_value_t = 1500)]
    duration_ms: u32,
    /// Throttle value (0.0–1.0). Negative is reverse on most builds.
    #[arg(long, default_value_t = 1.0)]
    throttle: f32,
    /// Tick interval — gap between successive throttle writes. Sets the
    /// scripted-driver "input refresh rate". Lower = more chances to win
    /// vs AIController re-writes; higher = lighter IPC load.
    #[arg(long, default_value_t = 20)]
    tick_ms: u32,
    /// Class-name substring filter when searching for the nearest car.
    #[arg(long, default_value = "VEH")]
    class_substring: String,
    /// Distance cutoff in UE units (default 100m).
    #[arg(long, default_value_t = 10000.0)]
    max_distance: f64,
}

#[derive(Parser, Debug)]
struct PossessArgs {
    #[arg(long)]
    pid: u32,
    #[arg(long)]
    dll: PathBuf,
    /// Class-name substring filter (case-insensitive). Default catches every
    /// `BP_VEH_*` and `BP_Population_VEH_*` variant.
    #[arg(long, default_value = "VEH")]
    class_substring: String,
    /// How many candidates to print in the report, sorted by ascending
    /// distance from the player.
    #[arg(long, default_value_t = 8)]
    top: usize,
    /// Distance cutoff in UE units. Default 10000 = ~100m. Candidates
    /// farther than this are excluded from the ranking.
    #[arg(long, default_value_t = 10000.0)]
    max_distance: f64,
    /// Without this flag, the command only reports the ranked candidates and
    /// exits. With it, the top candidate is hand-possessed via
    /// `APlayerController::Possess` — actually flipping control of the
    /// vehicle to the player.
    #[arg(long)]
    commit: bool,
    /// Restore mode: skip the nearest-vehicle search entirely and call
    /// `Possess(<hex>)` on the live player controller against this raw obj
    /// address. Use after a failed hijack to re-Possess the original Batman
    /// pawn (whose addr was printed by the previous run).
    #[arg(long, value_parser = parse_hex_u64)]
    restore_pawn: Option<u64>,
}

#[derive(Parser, Debug)]
struct ReadBytesArgs {
    #[arg(long)]
    pid: u32,
    #[arg(long)]
    dll: PathBuf,
    /// Hex address (e.g. `0x1E0015B00`).
    #[arg(long, value_parser = parse_hex_u64)]
    addr: u64,
    /// Number of bytes to read.
    #[arg(long, default_value_t = 64)]
    len: u32,
}

fn parse_hex_u64(s: &str) -> Result<u64, String> {
    let s = s.trim().trim_start_matches("0x").trim_start_matches("0X");
    u64::from_str_radix(s, 16).map_err(|e| format!("bad hex addr `{s}`: {e}"))
}

#[derive(Parser, Debug)]
struct GrantArgs {
    #[arg(long)]
    pid: u32,
    #[arg(long)]
    dll: PathBuf,
    /// Substring of the TtGameProgressRuleSet data asset name. Example:
    /// `PROGR_SkillBricks` or `PROGR_Skills`.
    #[arg(long)]
    ruleset: String,
    /// Value to write into each tag. UE5 progress values are `uint8`, so
    /// the practical range is 0..=255. `1` is "unlocked"/"completed" for
    /// most boolean-like progress entries; bricks may want higher.
    #[arg(long, default_value_t = 1)]
    value: u8,
    /// Set bOnlyIfHigher in the SetGameProgressValue call. Safer when the
    /// game already has higher values for some tags; defaults off so the
    /// grant is unconditional.
    #[arg(long)]
    only_if_higher: bool,
    /// Dry-run: read+print the tags but don't actually call
    /// SetGameProgressValue. Use this first to confirm the strides/offsets
    /// look right before firing.
    #[arg(long)]
    dry_run: bool,
    /// Use TtGameProgressLiveData::ApplyOverride (instance method on live
    /// LiveData) instead of TtGameProgressStatics::SetGameProgressValue.
    /// ApplyOverride bypasses the rule-condition gate that SetGameProgressValue
    /// enforces — use this when SetGameProgressValue keeps returning false
    /// despite valid tags.
    #[arg(long)]
    via_override: bool,
    /// Treat `--ruleset` as a `TtGameProgressDefinitionSet` instead of a
    /// `TtGameProgressRuleSet`. DefinitionSets store `ProgressDefinitions`
    /// (TArray of 16-byte ClassPtr+DataPtr pairs); each data instance has a
    /// registered FGameplayTag at +0x4C. Use this for PROG_Skills (32
    /// per-skill tags) rather than PROGR_Skills (4 category-level tags).
    #[arg(long)]
    from_definitions: bool,
    /// Read-only: extract tags and call GetGameProgressValue per tag to
    /// report current state, but skip the SetGameProgressValue grant pass.
    /// Diagnostic — use to figure out what value the game uses for
    /// "unlocked" before committing a grant.
    #[arg(long)]
    read_only: bool,
}

#[derive(Parser, Debug)]
struct CallFnArgs {
    #[arg(long)]
    pid: u32,
    #[arg(long)]
    dll: PathBuf,
    /// Class to locate (same semantics as `discover --class`).
    #[arg(long)]
    class: String,
    /// Predicate for picking the live instance (see `discover` doc).
    #[arg(long, default_value = "any", value_parser = parse_predicate)]
    predicate: NamePredicate,
    /// UFunction name (case-insensitive).
    #[arg(long)]
    function: String,
    /// Parameter bytes as a hex string with optional spaces, e.g.
    /// `"FF 00 00 00"` for the i32 `255`. Empty = call with zero
    /// arguments (return-only).
    #[arg(long, default_value = "")]
    params: String,
}

#[derive(Parser, Debug)]
struct PidArgs {
    #[arg(long)]
    pid: u32,
    /// Path to the per-game DLL. The host crate is game-agnostic; the dev
    /// runner must point at the DLL it wants injected (or its file name for
    /// `eject`). Set the `OPENFORGE_DLL_PATH` env var to override.
    #[arg(long)]
    dll: PathBuf,
}

#[derive(Parser, Debug)]
struct AttachArgs {
    #[arg(long)]
    pid: u32,
    /// Path to the per-game DLL. Required because the host crate is
    /// game-agnostic; the dev runner must say which DLL to inject.
    #[arg(long)]
    dll: PathBuf,
    /// Send a Ping after handshake.
    #[arg(long)]
    ping: bool,
    /// Walk N objects after handshake (0 = skip).
    #[arg(long, default_value_t = 0)]
    walk: u32,
    /// Drain up to N log lines from the DLL.
    #[arg(long, default_value_t = 0)]
    drain_log: u32,
    /// Set DLL log level before draining.
    #[arg(long, value_parser = parse_log_level)]
    log_level: Option<LogLevel>,
}

#[derive(Parser, Debug)]
struct DiscoverArgs {
    #[arg(long)]
    pid: u32,
    /// Path to the per-game DLL (host crate is game-agnostic).
    #[arg(long)]
    dll: PathBuf,
    /// Class to look up by name (case-insensitive). Examples:
    /// `ULegoPlayerState`, `ALegoCharacter`, `AGameModeBase`.
    #[arg(long)]
    class: String,
    /// Multi-instance discriminator. Accepted forms:
    ///   `any`              — first match in GUObjectArray order (default)
    ///   `exact:<NAME>`     — object name matches exactly
    ///   `contains:<SUB>`   — FQN contains substring
    ///   `prefix:<PFX>`     — FQN starts with prefix
    #[arg(long, default_value = "any", value_parser = parse_predicate)]
    predicate: NamePredicate,
    /// Limit FProperty dump (0 = unlimited).
    #[arg(long, default_value_t = 0)]
    max_props: u32,
    /// Limit UFunction dump (0 = unlimited).
    #[arg(long, default_value_t = 0)]
    max_funcs: u32,
    /// Skip the UFunction dump (only show FProperties).
    #[arg(long)]
    no_funcs: bool,
    /// Walk the *found object* as a UStruct (e.g. ScriptStruct / UFunction)
    /// instead of walking its metaclass. Use this to dump the field layout of
    /// a ScriptStruct (`--class ScriptStruct --predicate exact:TtGameProgressRule
    /// --as-struct`) or the parameter layout of a UFunction.
    #[arg(long)]
    as_struct: bool,
    /// Read the current value of these properties on the found object after
    /// dumping the schema. Use the property name as it appears in the
    /// FProperty list (e.g. `--read Total` for DinnerCurrency_Studs). Pass
    /// multiple times for several reads. The CLI resolves each via
    /// ResolveProperty (which infers the right PropKind from the FFieldClass
    /// name), then issues a single ReadProperty per name.
    #[arg(long, value_name = "PROPERTY")]
    read: Vec<String>,
}

fn parse_predicate(s: &str) -> Result<NamePredicate, String> {
    if s.eq_ignore_ascii_case("any") {
        return Ok(NamePredicate::Any);
    }
    if let Some(rest) = s.strip_prefix("exact:") {
        return Ok(NamePredicate::Exact(rest.to_string()));
    }
    if let Some(rest) = s.strip_prefix("contains:") {
        return Ok(NamePredicate::Contains(rest.to_string()));
    }
    if let Some(rest) = s.strip_prefix("prefix:") {
        return Ok(NamePredicate::FqnPrefix(rest.to_string()));
    }
    Err(format!(
        "expected `any`, `exact:NAME`, `contains:SUB`, or `prefix:PFX`; got `{s}`"
    ))
}

fn parse_log_level(s: &str) -> Result<LogLevel, String> {
    match s.to_ascii_lowercase().as_str() {
        "off" => Ok(LogLevel::Off),
        "error" => Ok(LogLevel::Error),
        "warn" => Ok(LogLevel::Warn),
        "info" => Ok(LogLevel::Info),
        "debug" => Ok(LogLevel::Debug),
        "trace" => Ok(LogLevel::Trace),
        other => Err(format!("unknown log level: {other}")),
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let level = if cli.verbose {
        tracing::Level::DEBUG
    } else {
        tracing::Level::INFO
    };
    let _ = tracing_subscriber::fmt()
        .with_max_level(level)
        .with_target(false)
        .try_init();

    match run(cli.cmd) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn parse_hex_bytes(s: &str) -> Result<Vec<u8>, String> {
    let s = s.trim();
    if s.is_empty() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for tok in s.split_whitespace() {
        if tok.len() != 2 || !tok.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(format!(
                "bad hex byte `{tok}`; expect 2 hex digits per token, space-separated"
            ));
        }
        out.push(u8::from_str_radix(tok, 16).unwrap());
    }
    Ok(out)
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(n.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

/// Call `K2_GetActorLocation` on `obj_addr` (an Actor instance) and decode
/// the returned FVector3d (UE5 LWC: 3 × f64 = 24 bytes; world location).
/// Returns `Err` if the function isn't found on the actor's class chain —
/// i.e. the obj_addr was a non-Actor UObject (component, dataasset, etc.).
fn call_get_actor_location(
    session: &Ue5Session,
    obj_addr: u64,
    class_addr: u64,
) -> Result<[f64; 3], Box<dyn std::error::Error>> {
    // ParmsSize for K2_GetActorLocation is exactly 24 bytes (FVector3d
    // return at +0; 3 × f64 under UE5 LWC). The DLL rejects oversize blobs
    // (would silently drop "arguments") so we size precisely.
    let params = vec![0u8; 24];
    let ret = session
        .call_ufunction(obj_addr, class_addr, "K2_GetActorLocation", params)?
        .ok_or("K2_GetActorLocation not on class chain")?;
    if ret.len() < 24 {
        return Err(format!(
            "K2_GetActorLocation returned {} bytes, expected ≥24",
            ret.len()
        )
        .into());
    }
    let x = f64::from_le_bytes(ret[0..8].try_into().unwrap());
    let y = f64::from_le_bytes(ret[8..16].try_into().unwrap());
    let z = f64::from_le_bytes(ret[16..24].try_into().unwrap());
    Ok([x, y, z])
}

fn dll_file_name_from(path: &std::path::Path) -> Result<String, Box<dyn std::error::Error>> {
    match path.file_name().and_then(|s| s.to_str()) {
        Some(name) if !name.is_empty() => Ok(name.to_owned()),
        _ => Err(format!("{} has no usable file name", path.display()).into()),
    }
}

fn run(cmd: Cmd) -> Result<(), Box<dyn std::error::Error>> {
    match cmd {
        Cmd::Inject(a) => {
            println!("injecting {} into pid {}", a.dll.display(), a.pid);
            Injector::inject(a.pid, &a.dll)?;
            println!("injection ok");
        }
        Cmd::Eject(a) => {
            let name = dll_file_name_from(&a.dll)?;
            println!("ejecting {name} from pid {}", a.pid);
            let unloaded = Injector::eject(a.pid, &name)?;
            if unloaded {
                println!("DLL unloaded");
            } else {
                println!("FreeLibrary ran but DLL still listed (worker thread holding refcount)");
            }
        }
        Cmd::Reinject(a) => {
            println!("reinjecting {} into pid {}", a.dll.display(), a.pid);
            Injector::reinject(a.pid, &a.dll)?;
            println!("reinjection ok");
        }
        Cmd::Attach(a) => {
            println!("attaching to pid {} via {}", a.pid, a.dll.display());
            let session = Ue5Session::attach_pid(a.pid, &a.dll)?;
            let w = session.welcome();
            println!(
                "welcome: pid={} guobject=0x{:X} fname_pool=0x{:X} chunks_off=+0x{:X} stride={} validated={}\n  \
                 offsets: class=+0x{:X} name=+0x{:X} outer=+0x{:X} super=+0x{:X} children=+0x{:X} child_props=+0x{:X}\n  \
                          ffield: class=+0x{:X} next=+0x{:X} name=+0x{:X}  fprop: off=+0x{:X} size=+0x{:X}  ufield_next=+0x{:X}  ufunc: flags=+0x{:X} func=+0x{:X}",
                w.pid,
                w.guobject_array,
                w.fname_pool,
                w.fname_pool_chunks_offset,
                w.fuobject_item_stride,
                w.layout_validated,
                w.offsets.uobject_class_private,
                w.offsets.uobject_name_private,
                w.offsets.uobject_outer_private,
                w.offsets.ustruct_super_struct,
                w.offsets.ustruct_children,
                w.offsets.ustruct_child_properties,
                w.offsets.ffield_class_private,
                w.offsets.ffield_next,
                w.offsets.ffield_name_private,
                w.offsets.fproperty_offset_internal,
                w.offsets.fproperty_element_size,
                w.offsets.ufield_next,
                w.offsets.ufunction_flags,
                w.offsets.ufunction_func
            );
            if let Some(level) = a.log_level {
                session.set_log_level(level)?;
                println!("log level set to {level:?}");
            }
            if a.ping {
                session.ping()?;
                println!("ping ok");
            }
            if a.walk > 0 {
                let objs = session.walk_objects()?;
                println!(
                    "walked {} objects; first {}:",
                    objs.len(),
                    a.walk.min(objs.len() as u32)
                );
                for obj in objs.iter().take(a.walk as usize) {
                    println!(
                        "  0x{:012X}  {:<32} ({})",
                        obj.addr, obj.fqn, obj.class_name
                    );
                }
            }
            if a.drain_log > 0 {
                let lines = session.drain_log(a.drain_log)?;
                println!("--- DLL log ({} lines) ---", lines.len());
                for line in lines {
                    println!("{line}");
                }
            }
        }
        Cmd::Discover(a) => {
            println!("attaching to pid {} via {}", a.pid, a.dll.display());
            let session = Ue5Session::attach_pid(a.pid, &a.dll)?;
            let w = session.welcome();
            if !w.layout_validated {
                eprintln!(
                    "warning: DLL reports layout_validated=false; \
                     property offsets/sizes may be wrong"
                );
            }
            println!(
                "looking up class `{}` (predicate {:?})",
                a.class, a.predicate
            );
            let (obj_addr, class_addr) = match session.find_uobject(&a.class, a.predicate)? {
                Some(pair) => pair,
                None => {
                    println!(
                        "FindUObject({}) returned NotFound — class isn't loaded yet. \
                         If the game is on the main menu, start a level and try again.",
                        a.class
                    );
                    return Ok(());
                }
            };
            println!("found: obj=0x{:X}  class=0x{:X}", obj_addr, class_addr);

            // `--as-struct` walks the FOUND object as a UStruct (works for
            // UScriptStruct + UFunction, both of which are UStructs). The DLL's
            // WalkProperties just reads child_properties at +0x50 — doesn't
            // care if the address is a UClass or any other UStruct.
            let walk_addr = if a.as_struct { obj_addr } else { class_addr };
            let props = session.walk_properties_cached(walk_addr)?;
            let prop_cap = if a.max_props == 0 {
                props.len()
            } else {
                (a.max_props as usize).min(props.len())
            };
            println!(
                "--- FProperties ({} total{}) ---",
                props.len(),
                if prop_cap < props.len() {
                    format!(", showing first {prop_cap}")
                } else {
                    String::new()
                }
            );
            println!(
                "{:<40} {:<22} {:>8} {:>6}   {}",
                "name", "kind", "offset", "size", "defined_in_class"
            );
            for p in props.iter().take(prop_cap) {
                println!(
                    "{:<40} {:<22} {:>8} {:>6}   {}",
                    truncate(&p.name, 40),
                    truncate(&p.kind, 22),
                    format!("+0x{:X}", p.offset),
                    p.size,
                    p.defined_in_class
                );
            }

            if !a.read.is_empty() {
                println!("--- Property reads ---");
                for prop_name in &a.read {
                    let resolved = match session.resolve_property(class_addr, prop_name)? {
                        Some(r) => r,
                        None => {
                            println!("  {prop_name}: <not found on class chain>");
                            continue;
                        }
                    };
                    let target_addr = obj_addr + resolved.offset as u64;
                    let value = session.read_property(target_addr, resolved.kind)?;
                    println!(
                        "  {:<32} = {:?}  (at 0x{:X} +0x{:X}, kind {:?}, size {})",
                        prop_name, value, obj_addr, resolved.offset, resolved.kind, resolved.size
                    );
                }
            }

            if !a.no_funcs {
                let funcs = session.walk_functions_cached(class_addr)?;
                let func_cap = if a.max_funcs == 0 {
                    funcs.len()
                } else {
                    (a.max_funcs as usize).min(funcs.len())
                };
                println!(
                    "--- UFunctions ({} total{}) ---",
                    funcs.len(),
                    if func_cap < funcs.len() {
                        format!(", showing first {func_cap}")
                    } else {
                        String::new()
                    }
                );
                println!(
                    "{:<48} {:<8} {:<18}   {}",
                    "name", "native", "addr", "defined_in_class"
                );
                for f in funcs.iter().take(func_cap) {
                    println!(
                        "{:<48} {:<8} 0x{:<16X}   {}",
                        truncate(&f.name, 48),
                        if f.is_native { "native" } else { "script" },
                        f.addr,
                        f.defined_in_class
                    );
                }
            }
        }
        Cmd::CallFn(a) => {
            println!("attaching to pid {} via {}", a.pid, a.dll.display());
            let session = Ue5Session::attach_pid(a.pid, &a.dll)?;
            let (obj_addr, class_addr) = match session.find_uobject(&a.class, a.predicate)? {
                Some(p) => p,
                None => {
                    println!(
                        "FindUObject({}) returned NotFound — class isn't loaded yet.",
                        a.class
                    );
                    return Ok(());
                }
            };
            println!("found: obj=0x{:X}  class=0x{:X}", obj_addr, class_addr);
            let params = parse_hex_bytes(&a.params)?;
            println!(
                "calling `{}` with {} parameter byte(s): {:02X?}",
                a.function,
                params.len(),
                params
            );
            match session.call_ufunction(obj_addr, class_addr, &a.function, params)? {
                Some(ret) => {
                    println!("CallOk: {} return byte(s): {:02X?}", ret.len(), ret);
                    if ret.len() == 1 {
                        println!("  → as bool: {}", ret[0] != 0);
                    } else if ret.len() == 4 {
                        let v = i32::from_le_bytes([ret[0], ret[1], ret[2], ret[3]]);
                        println!("  → as i32: {v}");
                    } else if ret.len() == 8 {
                        let v = i64::from_le_bytes([
                            ret[0], ret[1], ret[2], ret[3], ret[4], ret[5], ret[6], ret[7],
                        ]);
                        println!("  → as i64: {v}");
                    }
                }
                None => println!(
                    "UFunction `{}` not found on class chain of 0x{:X}",
                    a.function, class_addr
                ),
            }
        }
        Cmd::GrantProgressFromRules(a) => {
            // Strategy:
            //   1. Find the named TtGameProgressRuleSet data asset (e.g.
            //      PROGR_SkillBricks). Read its `Rules` TArray header at
            //      +0x30 (data ptr, num, max).
            //   2. Each rule is a `TtGameProgressRule` ScriptStruct laid out
            //      as [RuleID FName +0x00 (8 bytes), RuleCondition Struct
            //      +0x08 (16 bytes), Values TArray +0x18 (16 bytes),
            //      bProcessOnLoad bool +0x28 (1 byte)]. With UE5 alignment
            //      max field = 8 (TArray ptr), the struct rounds to 48 bytes.
            //      We read the whole TArray in one ReadBytes (16 * 48 bytes).
            //   3. Find TtGameProgressStatics CDO and a live UObject to use
            //      as WorldContextObject (TtGameProgressLiveData is the
            //      cleanest — guaranteed to share the world with the game
            //      progress system).
            //   4. For each rule, build the 49-byte SetGameProgressValue
            //      param blob and CallUFunction. Tag bytes are the raw FName
            //      we just read — no decode needed because both sides of the
            //      protocol speak raw FName.
            //
            // No DLL update required.
            const RULES_ARRAY_OFFSET: u64 = 0x30;
            const TARRAY_HEADER_SIZE: u32 = 16;
            // Empirically 56 bytes (0x38) for TtGameProgressRule on this
            // build — verified via hex dump on 2026-05-24. The exposed
            // FProperties stop at +0x28 (bProcessOnLoad), with 15 trailing
            // bytes that look like padding/internal scratch.
            const RULE_STRIDE: usize = 56;
            const RULE_ID_OFFSET: usize = 0x00;
            const FNAME_SIZE: usize = 8;
            const SET_PROGRESS_PARMS_SIZE: usize = 0x31; // 49 bytes
            const PARM_WORLDCTX: usize = 0x00;
            const PARM_TAG: usize = 0x08;
            const PARM_VALUE: usize = 0x10;
            const PARM_ONLY_IF_HIGHER: usize = 0x11;

            println!("attaching to pid {} via {}", a.pid, a.dll.display());
            let session = Ue5Session::attach_pid(a.pid, &a.dll)?;

            // --- 1. Find the asset (ruleset OR definitionset) -----------
            // DefinitionSet schema differs:
            //   - TArray is `ProgressDefinitions` at +0x50 (not Rules at +0x30)
            //   - Each entry is 16 bytes (ClassPtr+DataPtr); the registered
            //     gameplay tag lives at +0x4C of the data instance.
            const DEFS_ARRAY_OFFSET: u64 = 0x50;
            const DEF_ENTRY_STRIDE: usize = 16;
            const DEF_ENTRY_DATA_PTR_OFFSET: usize = 8;
            const DEF_INSTANCE_TAG_OFFSET: u64 = 0x4C;

            let (asset_class, array_offset) = if a.from_definitions {
                ("TtGameProgressDefinitionSet", DEFS_ARRAY_OFFSET)
            } else {
                ("TtGameProgressRuleSet", RULES_ARRAY_OFFSET)
            };
            let predicate = NamePredicate::Contains(a.ruleset.clone());
            let (ruleset_addr, _ruleset_class) =
                match session.find_uobject(asset_class, predicate)? {
                    Some(p) => p,
                    None => {
                        eprintln!("no {asset_class} matching `{}` is loaded", a.ruleset);
                        return Ok(());
                    }
                };
            println!(
                "{asset_class} `{}` found at 0x{:X}",
                a.ruleset, ruleset_addr
            );

            // --- 2. Read the TArray header ------------------------------
            let header = match session.read_property(
                ruleset_addr + array_offset,
                PropKind::Bytes(TARRAY_HEADER_SIZE),
            )? {
                PropValue::Bytes(b) => b,
                other => {
                    return Err(format!("expected Bytes for array header, got {other:?}").into());
                }
            };
            if header.len() != 16 {
                return Err(format!("short Rules header: {} bytes", header.len()).into());
            }
            let data_ptr = u64::from_le_bytes(header[0..8].try_into().unwrap());
            let num = i32::from_le_bytes(header[8..12].try_into().unwrap());
            let max = i32::from_le_bytes(header[12..16].try_into().unwrap());
            println!(
                "Rules TArray: data=0x{:X} num={} max={} (stride={} bytes)",
                data_ptr, num, max, RULE_STRIDE
            );
            if num <= 0 || data_ptr == 0 {
                eprintln!("empty rules array — nothing to grant");
                return Ok(());
            }
            let num = num as usize;
            let entry_stride = if a.from_definitions {
                DEF_ENTRY_STRIDE
            } else {
                RULE_STRIDE
            };
            let bytes_to_read = (num * entry_stride) as u32;
            let body = match session.read_property(data_ptr, PropKind::Bytes(bytes_to_read))? {
                PropValue::Bytes(b) => b,
                other => {
                    return Err(format!("expected Bytes for array body, got {other:?}").into());
                }
            };
            if body.len() != num * entry_stride {
                return Err(format!(
                    "short array body: got {} bytes, expected {}",
                    body.len(),
                    num * entry_stride
                )
                .into());
            }

            // Extract the real GameplayTag. Schema branch:
            //  - DefinitionSet: TArray<{ClassPtr, DataPtr}>. Tag at DataPtr+0x4C.
            //  - RuleSet:       TArray<TtGameProgressRule>. RuleID at +0x00 is
            //                   a non-registered identifier; real tag at
            //                   Values[0]+0x00 (Values TArray header at rule+0x18).
            const RULE_VALUES_OFFSET: usize = 0x18;
            let mut tags: Vec<[u8; 8]> = Vec::with_capacity(num);
            for i in 0..num {
                let tag = if a.from_definitions {
                    let entry_off = i * DEF_ENTRY_STRIDE;
                    let data_ptr_def = u64::from_le_bytes(
                        body[entry_off + DEF_ENTRY_DATA_PTR_OFFSET
                            ..entry_off + DEF_ENTRY_DATA_PTR_OFFSET + 8]
                            .try_into()
                            .unwrap(),
                    );
                    if data_ptr_def == 0 {
                        eprintln!("  [{i:2}] null definition data ptr");
                        [0u8; 8]
                    } else {
                        let tag_bytes = match session.read_property(
                            data_ptr_def + DEF_INSTANCE_TAG_OFFSET,
                            PropKind::Bytes(8),
                        )? {
                            PropValue::Bytes(b) => b,
                            _ => return Err("definition tag read returned wrong kind".into()),
                        };
                        let mut t = [0u8; 8];
                        t.copy_from_slice(&tag_bytes);
                        t
                    }
                } else {
                    let values_header_off = i * RULE_STRIDE + RULE_VALUES_OFFSET;
                    let values_data_ptr = u64::from_le_bytes(
                        body[values_header_off..values_header_off + 8]
                            .try_into()
                            .unwrap(),
                    );
                    let values_num = i32::from_le_bytes(
                        body[values_header_off + 8..values_header_off + 12]
                            .try_into()
                            .unwrap(),
                    );
                    if values_data_ptr == 0 || values_num <= 0 {
                        eprintln!("  [{i:2}] empty Values array — skipping");
                        [0u8; 8]
                    } else {
                        let tag_bytes =
                            match session.read_property(values_data_ptr, PropKind::Bytes(8))? {
                                PropValue::Bytes(b) => b,
                                _ => return Err("Values[0] tag read returned wrong kind".into()),
                            };
                        let mut t = [0u8; 8];
                        t.copy_from_slice(&tag_bytes);
                        t
                    }
                };
                tags.push(tag);
            }
            println!("--- {} tags extracted ---", tags.len());
            for (i, t) in tags.iter().enumerate() {
                let idx = u32::from_le_bytes(t[0..4].try_into().unwrap());
                let num = u32::from_le_bytes(t[4..8].try_into().unwrap());
                println!("  [{i:2}] FName{{ index: {idx} (0x{idx:X}), number: {num} }}");
            }

            if a.dry_run {
                // Debug: read a wider slice (up to 256 bytes) and hex-dump
                // so the operator can eyeball the real struct stride. The
                // assumed 48-byte stride is just a first guess from the
                // FProperty offset list; real layout may have a vtable,
                // base class, or padding we missed.
                let dump_len = (num.min(4) * RULE_STRIDE).max(256) as u32;
                let dump = match session.read_property(data_ptr, PropKind::Bytes(dump_len))? {
                    PropValue::Bytes(b) => b,
                    _ => unreachable!(),
                };
                println!(
                    "--- raw hex dump of first {} bytes from 0x{:X} ---",
                    dump.len(),
                    data_ptr
                );
                for (i, chunk) in dump.chunks(16).enumerate() {
                    print!("  +0x{:03X}: ", i * 16);
                    for b in chunk {
                        print!("{:02X} ", b);
                    }
                    print!(" |");
                    for b in chunk {
                        let c = *b;
                        print!(
                            "{}",
                            if (0x20..=0x7E).contains(&c) {
                                c as char
                            } else {
                                '.'
                            }
                        );
                    }
                    println!("|");
                }
                println!("dry-run: not calling SetGameProgressValue.");
                return Ok(());
            }

            // --- 3. Find world context (live LiveData) + statics CDO -----
            let (world_ctx_addr, _) = match session.find_uobject(
                "TtGameProgressLiveData",
                NamePredicate::Contains("/Engine/Transient/".to_string()),
            )? {
                Some(p) => p,
                None => {
                    return Err(
                        "no live TtGameProgressLiveData found — game probably on main menu".into(),
                    );
                }
            };
            println!("world context (live LiveData): 0x{:X}", world_ctx_addr);

            let (statics_addr, statics_class) =
                match session.find_uobject("TtGameProgressStatics", NamePredicate::Any)? {
                    Some(p) => p,
                    None => return Err("TtGameProgressStatics CDO not found".into()),
                };
            println!(
                "statics CDO: 0x{:X}  class: 0x{:X}",
                statics_addr, statics_class
            );

            // --- 3b. Verify tags via GetGameProgressValue read ----------
            // If these return sensible uint8 values (0 for unfound bricks),
            // the tags ARE valid and the issue is something specific to
            // SetGameProgressValue. If they all return 0xFF or unstable,
            // RuleID isn't the actual gameplay tag and we need to look in
            // each rule's Values TArray instead.
            const GET_PARMS_SIZE: usize = 0x11; // 17 bytes: WorldCtx(8) + Tag(8) + ReturnByte(1)
            println!("--- GetGameProgressValue read-back ---");
            for (i, tag) in tags.iter().enumerate() {
                let mut params = vec![0u8; GET_PARMS_SIZE];
                params[0..8].copy_from_slice(&world_ctx_addr.to_le_bytes());
                params[8..16].copy_from_slice(tag);
                let res = session.call_ufunction(
                    statics_addr,
                    statics_class,
                    "GetGameProgressValue",
                    params,
                )?;
                match res {
                    Some(ret) if ret.len() >= GET_PARMS_SIZE => {
                        println!(
                            "  [{i:2}] Get → current value: {} (raw byte 0x{:02X})",
                            ret[0x10], ret[0x10]
                        );
                    }
                    Some(ret) => println!("  [{i:2}] short Get return: {} bytes", ret.len()),
                    None => println!("  [{i:2}] GetGameProgressValue not found"),
                }
            }

            if a.read_only {
                println!("--- read-only: skipping SetGameProgressValue grant ---");
                let mut histo = std::collections::BTreeMap::<u8, usize>::new();
                for (i, tag) in tags.iter().enumerate() {
                    let mut params = vec![0u8; GET_PARMS_SIZE];
                    params[0..8].copy_from_slice(&world_ctx_addr.to_le_bytes());
                    params[8..16].copy_from_slice(tag);
                    let res = session.call_ufunction(
                        statics_addr,
                        statics_class,
                        "GetGameProgressValue",
                        params,
                    )?;
                    if let Some(ret) = res
                        && ret.len() >= GET_PARMS_SIZE
                    {
                        let v = ret[0x10];
                        *histo.entry(v).or_default() += 1;
                        let _ = i;
                    }
                }
                println!("--- value histogram ({} tags) ---", tags.len());
                for (v, n) in &histo {
                    println!("  value {v:3}: {n} tags");
                }
                return Ok(());
            }

            // --- 4. Per-tag grant call -----------------------------------
            // Two paths:
            //   (a) static SetGameProgressValue (default) — runs the engine's
            //       rule-condition validation; returns false if the rule's
            //       gate isn't met.
            //   (b) ApplyOverride on the live LiveData instance — bypasses
            //       rule gates by *overriding* the entry directly. The
            //       FGameProgressOverride struct is ~64 bytes; only Tag +
            //       OverrideValue are needed for a minimal grant.
            // Override struct = 64; + Source FString = 16; + ChangeFlags enum = 1.
            // Total ApplyOverride ParmsSize = 0x51 = 81. Earlier 64-byte attempt
            // truncated and the function silently no-op'd.
            const APPLY_OVERRIDE_PARMS_SIZE: usize = 0x51; // 81 bytes
            const OV_TAG: usize = 0x00;
            const OV_VALUE: usize = 0x08;
            const OV_CHANGE_FLAGS: usize = 0x50;
            // OverrideNameValue +0xC (FName, 8) stays 0
            // OverrideState +0x14 (Enum, 1) stays 0
            // bUseState +0x15 (bool, 1) stays 0 = use value, not state
            // bIncreasesOnly +0x16 (bool, 1) stays 0
            // SearchByTags +0x18 (FGameplayTagContainer, 32) stays 0 = no filter
            // SearchMethod +0x38 (Enum, 1) stays 0

            let mut ok = 0usize;
            let mut fail = 0usize;
            for (i, tag) in tags.iter().enumerate() {
                if a.via_override {
                    // Build FGameProgressOverride and call on LiveData instance.
                    // We need the LiveData's class addr too; refetch.
                    let (live_obj, live_class) = session
                        .find_uobject(
                            "TtGameProgressLiveData",
                            NamePredicate::Contains("/Engine/Transient/".to_string()),
                        )?
                        .ok_or("live LiveData missing")?;
                    let mut params = vec![0u8; APPLY_OVERRIDE_PARMS_SIZE];
                    params[OV_TAG..OV_TAG + FNAME_SIZE].copy_from_slice(tag);
                    params[OV_VALUE] = a.value;
                    // ChangeFlags=1 ("Notify"/"Default" — guess) so any
                    // observers wake up; without this, the override may be
                    // recorded but not surfaced to the rule system or UI.
                    params[OV_CHANGE_FLAGS] = 1;
                    let res =
                        session.call_ufunction(live_obj, live_class, "ApplyOverride", params)?;
                    match res {
                        Some(_) => {
                            println!("  [{i:2}] ApplyOverride dispatched (void return)");
                            ok += 1;
                        }
                        None => {
                            eprintln!("  [{i:2}] ApplyOverride not found on LiveData");
                            fail += 1;
                        }
                    }
                } else {
                    let mut params = vec![0u8; SET_PROGRESS_PARMS_SIZE];
                    params[PARM_WORLDCTX..PARM_WORLDCTX + 8]
                        .copy_from_slice(&world_ctx_addr.to_le_bytes());
                    params[PARM_TAG..PARM_TAG + FNAME_SIZE].copy_from_slice(tag);
                    params[PARM_VALUE] = a.value;
                    params[PARM_ONLY_IF_HIGHER] = if a.only_if_higher { 1 } else { 0 };
                    let res = session.call_ufunction(
                        statics_addr,
                        statics_class,
                        "SetGameProgressValue",
                        params,
                    )?;
                    match res {
                        Some(ret) if ret.len() >= SET_PROGRESS_PARMS_SIZE => {
                            let return_value = ret[0x30] != 0;
                            println!("  [{i:2}] SetGameProgressValue → ReturnValue={return_value}");
                            if return_value {
                                ok += 1;
                            } else {
                                fail += 1;
                            }
                        }
                        Some(ret) => {
                            println!("  [{i:2}] short return: {} bytes", ret.len());
                            fail += 1;
                        }
                        None => {
                            eprintln!("  [{i:2}] SetGameProgressValue not found on CDO");
                            fail += 1;
                        }
                    }
                }
            }
            println!("done: ok={ok} fail={fail} (of {})", tags.len());

            // --- 5. Read-back AFTER grant to confirm state changed -------
            println!("--- post-grant GetGameProgressValue ---");
            for (i, tag) in tags.iter().enumerate() {
                let mut params = vec![0u8; GET_PARMS_SIZE];
                params[0..8].copy_from_slice(&world_ctx_addr.to_le_bytes());
                params[8..16].copy_from_slice(tag);
                let res = session.call_ufunction(
                    statics_addr,
                    statics_class,
                    "GetGameProgressValue",
                    params,
                )?;
                if let Some(ret) = res {
                    if ret.len() >= GET_PARMS_SIZE {
                        println!("  [{i:2}] Get → now: {}", ret[0x10]);
                    }
                }
            }
        }
        Cmd::ReadBytes(a) => {
            println!("attaching to pid {} via {}", a.pid, a.dll.display());
            let session = Ue5Session::attach_pid(a.pid, &a.dll)?;
            let bytes = match session.read_property(a.addr, PropKind::Bytes(a.len))? {
                PropValue::Bytes(b) => b,
                other => return Err(format!("expected Bytes, got {other:?}").into()),
            };
            println!("--- {} bytes from 0x{:X} ---", bytes.len(), a.addr);
            for (i, chunk) in bytes.chunks(16).enumerate() {
                print!("  +0x{:03X}: ", i * 16);
                for b in chunk {
                    print!("{:02X} ", b);
                }
                print!(" |");
                for b in chunk {
                    let c = *b;
                    print!(
                        "{}",
                        if (0x20..=0x7E).contains(&c) {
                            c as char
                        } else {
                            '.'
                        }
                    );
                }
                println!("|");
            }
        }
        Cmd::PossessNearestVehicle(a) => {
            println!("attaching to pid {} via {}", a.pid, a.dll.display());
            let session = Ue5Session::attach_pid(a.pid, &a.dll)?;

            // --- 1. Find live PlayerController + PlayerState ---------------
            let live_pred = NamePredicate::Contains("/PersistentLevel/".to_string());
            let (pc_addr, pc_class) = session
                .find_uobject("BP_DinnerPlayerController_C", live_pred.clone())?
                .ok_or("no live BP_DinnerPlayerController_C — not in a level?")?;
            println!("PlayerController:  0x{:X}  class=0x{:X}", pc_addr, pc_class);

            // Restore-mode short-circuit: skip nearest-search, just Possess
            // the supplied hex address. Useful for putting the player back
            // in their original pawn after a failed-input hijack.
            if let Some(restore_addr) = a.restore_pawn {
                println!("restore mode: Possess(0x{:X})", restore_addr);
                let mut params = vec![0u8; 8];
                params[0..8].copy_from_slice(&restore_addr.to_le_bytes());
                let res = session.call_ufunction(pc_addr, pc_class, "Possess", params)?;
                match res {
                    Some(_) => println!("Possess() dispatched (void return)."),
                    None => return Err("Possess UFunction missing".into()),
                }
                return Ok(());
            }

            let (ps_addr, ps_class) = session
                .find_uobject("BP_DinnerPlayerState_C", live_pred)?
                .ok_or("no live BP_DinnerPlayerState_C")?;
            println!("PlayerState:       0x{:X}  class=0x{:X}", ps_addr, ps_class);

            // --- 2. Resolve PawnPrivate and read the live pawn pointer -----
            let pawn_prop = session
                .resolve_property(ps_class, "PawnPrivate")?
                .ok_or("PawnPrivate property not on PlayerState class chain")?;
            let pawn_addr_bytes = match session
                .read_property(ps_addr + pawn_prop.offset as u64, PropKind::Bytes(8))?
            {
                PropValue::Bytes(b) => b,
                other => return Err(format!("PawnPrivate read wrong kind: {other:?}").into()),
            };
            let pawn_addr = u64::from_le_bytes(pawn_addr_bytes.as_slice().try_into().unwrap());
            if pawn_addr == 0 {
                return Err("PlayerState.PawnPrivate is null — player has no pawn".into());
            }
            println!("Player Pawn:       0x{:X}", pawn_addr);

            // --- 3. Walk objects + locate player pawn's class --------------
            let objs = session.walk_objects()?;
            let pawn_class_addr = objs
                .iter()
                .find(|o| o.addr == pawn_addr)
                .map(|o| o.class_ptr)
                .ok_or("player pawn not found in walk_objects (just-spawned?)")?;

            // --- 4. K2_GetActorLocation on the player pawn -----------------
            // FVector3d return; ParmsSize = 24. We allocate 32 for safety
            // (DLL truncates to actual ParmsSize).
            let player_loc = call_get_actor_location(&session, pawn_addr, pawn_class_addr)?;
            println!(
                "Player Location:   ({:.1}, {:.1}, {:.1})",
                player_loc[0], player_loc[1], player_loc[2]
            );

            // --- 5. Filter to live vehicle Pawn candidates -----------------
            let filter = a.class_substring.to_uppercase();
            let mut candidates: Vec<(u64, u64, String, String, [f64; 3], f64)> = Vec::new();
            for obj in objs.iter() {
                if obj.fqn.contains("Default__") || obj.fqn.contains("_GEN_VARIABLE") {
                    continue;
                }
                // Skip Lowres/Proxy LOD-stubs — Possess() on these crashes
                // the game (verified 2026-05-25). Highres siblings are
                // fully-formed and safe.
                let cn_upper = obj.class_name.to_uppercase();
                if cn_upper.contains("LOWRES") || cn_upper.contains("_PROXY_") {
                    continue;
                }
                if !cn_upper.contains(&filter) {
                    continue;
                }
                if obj.addr == pawn_addr {
                    continue; // don't possess ourselves
                }
                let loc = match call_get_actor_location(&session, obj.addr, obj.class_ptr) {
                    Ok(v) => v,
                    Err(_) => continue, // not an actor (e.g. a component), skip
                };
                let dx = loc[0] - player_loc[0];
                let dy = loc[1] - player_loc[1];
                let dz = loc[2] - player_loc[2];
                let dist = (dx * dx + dy * dy + dz * dz).sqrt();
                if dist > a.max_distance {
                    continue;
                }
                candidates.push((
                    obj.addr,
                    obj.class_ptr,
                    obj.class_name.clone(),
                    obj.fqn.clone(),
                    loc,
                    dist,
                ));
            }
            candidates.sort_by(|a, b| a.5.partial_cmp(&b.5).unwrap_or(std::cmp::Ordering::Equal));

            println!(
                "--- {} candidate vehicles within {} units (top {}) ---",
                candidates.len(),
                a.max_distance,
                a.top
            );
            for (i, (addr, _cls, name, fqn, loc, dist)) in candidates.iter().take(a.top).enumerate()
            {
                println!(
                    "  [{i:2}] {dist:8.1}u  0x{addr:X}  {name}  ({:.0},{:.0},{:.0})  {fqn}",
                    loc[0], loc[1], loc[2]
                );
            }

            if candidates.is_empty() {
                return Err("no vehicle candidates within range".into());
            }

            if !a.commit {
                println!(
                    "--- dry-run mode (no --commit). Re-run with --commit to call Possess() ---"
                );
                return Ok(());
            }

            // --- 6. Possess the nearest candidate --------------------------
            let (target_addr, _target_class, target_name, target_fqn, _, target_dist) =
                &candidates[0];
            println!(
                "calling Possess(0x{:X}) — target: {} {} ({:.1}u away)",
                target_addr, target_name, target_fqn, target_dist
            );

            // APlayerController::Possess takes a single APawn* argument.
            // ParmsSize = 8 (one pointer, no return). DLL pads.
            let mut params = vec![0u8; 8];
            params[0..8].copy_from_slice(&target_addr.to_le_bytes());
            let res = session.call_ufunction(pc_addr, pc_class, "Possess", params)?;
            match res {
                Some(_) => println!("Possess() dispatched (void return)."),
                None => return Err("Possess UFunction not found on PlayerController".into()),
            }
        }
        Cmd::VehicleDriveTest(a) => {
            println!("attaching to pid {} via {}", a.pid, a.dll.display());
            let session = Ue5Session::attach_pid(a.pid, &a.dll)?;

            // 1. Player controller + state + Batman pawn ---------------------
            let live_pred = NamePredicate::Contains("/PersistentLevel/".to_string());
            let (pc_addr, pc_class) = session
                .find_uobject("BP_DinnerPlayerController_C", live_pred.clone())?
                .ok_or("no live BP_DinnerPlayerController_C")?;
            let (ps_addr, ps_class) = session
                .find_uobject("BP_DinnerPlayerState_C", live_pred)?
                .ok_or("no live BP_DinnerPlayerState_C")?;
            let pawn_prop = session
                .resolve_property(ps_class, "PawnPrivate")?
                .ok_or("PawnPrivate missing")?;
            let pawn_addr_bytes = match session
                .read_property(ps_addr + pawn_prop.offset as u64, PropKind::Bytes(8))?
            {
                PropValue::Bytes(b) => b,
                o => return Err(format!("PawnPrivate read wrong kind: {o:?}").into()),
            };
            let batman_addr = u64::from_le_bytes(pawn_addr_bytes.as_slice().try_into().unwrap());
            if batman_addr == 0 {
                return Err("PlayerState.PawnPrivate is null".into());
            }
            println!("Batman Pawn: 0x{:X}", batman_addr);

            // 2. Walk objects + find player class + player loc + nearest VEH -
            let objs = session.walk_objects()?;
            let batman_class = objs
                .iter()
                .find(|o| o.addr == batman_addr)
                .map(|o| o.class_ptr)
                .ok_or("Batman pawn not in walk_objects")?;
            let player_loc = call_get_actor_location(&session, batman_addr, batman_class)?;
            println!(
                "Player Location: ({:.1}, {:.1}, {:.1})",
                player_loc[0], player_loc[1], player_loc[2]
            );

            let filter = a.class_substring.to_uppercase();
            let mut candidates: Vec<(u64, u64, String, f64)> = Vec::new();
            for obj in objs.iter() {
                if obj.fqn.contains("Default__") || obj.fqn.contains("_GEN_VARIABLE") {
                    continue;
                }
                // CRITICAL: filter out *_Lowres_* / *_LowRes_* / *_Proxy_*
                // LOD-stub actors. They're stripped-down stand-ins for distant
                // traffic and crash the game when Possess() tries to wire
                // input/camera into a half-built pawn (confirmed
                // 2026-05-25 — Possess on `BP_VEH_TRAFFIC_VEH_Sedan_E2_A_Lowres_C`
                // killed the process).
                let cn_upper = obj.class_name.to_uppercase();
                if cn_upper.contains("LOWRES") || cn_upper.contains("_PROXY_") {
                    continue;
                }
                if !cn_upper.contains(&filter) {
                    continue;
                }
                if obj.addr == batman_addr {
                    continue;
                }
                let loc = match call_get_actor_location(&session, obj.addr, obj.class_ptr) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                let dx = loc[0] - player_loc[0];
                let dy = loc[1] - player_loc[1];
                let dz = loc[2] - player_loc[2];
                let dist = (dx * dx + dy * dy + dz * dz).sqrt();
                if dist > a.max_distance {
                    continue;
                }
                candidates.push((obj.addr, obj.class_ptr, obj.class_name.clone(), dist));
            }
            candidates.sort_by(|a, b| a.3.partial_cmp(&b.3).unwrap_or(std::cmp::Ordering::Equal));
            let target = candidates.first().ok_or("no Highres vehicle nearby")?;
            let (veh_addr, veh_class, veh_name, veh_dist) =
                (target.0, target.1, &target.2, target.3);
            println!("Target: {} 0x{:X} at {:.1}u", veh_name, veh_addr, veh_dist);

            // 3. Possess vehicle ---------------------------------------------
            let mut possess_params = vec![0u8; 8];
            possess_params[0..8].copy_from_slice(&veh_addr.to_le_bytes());
            session
                .call_ufunction(pc_addr, pc_class, "Possess", possess_params.clone())?
                .ok_or("Possess UFunction missing")?;
            println!("Possess(vehicle) dispatched.");

            // 4. Find the vehicle movement component on the vehicle pawn ---
            // The FProperty is `VehicleMovementComponent` (the subobject's
            // FQN-name `VehicleMovementComp` is just an abbreviation; reflection
            // resolves the full property name). Verified 2026-05-25 against
            // BP_VEH_TRAFFIC_Taxi_E3_A_Highres_C at +0x350.
            let vmc_prop = session
                .resolve_property(veh_class, "VehicleMovementComponent")?
                .ok_or("VehicleMovementComponent property not on vehicle class")?;
            let vmc_bytes = match session
                .read_property(veh_addr + vmc_prop.offset as u64, PropKind::Bytes(8))?
            {
                PropValue::Bytes(b) => b,
                o => return Err(format!("VMC read wrong kind: {o:?}").into()),
            };
            let vmc_addr = u64::from_le_bytes(vmc_bytes.as_slice().try_into().unwrap());
            if vmc_addr == 0 {
                return Err("VehicleMovementComp pointer is null".into());
            }
            let vmc_class = objs
                .iter()
                .find(|o| o.addr == vmc_addr)
                .map(|o| o.class_ptr)
                .ok_or("VehicleMovementComp not in walk_objects")?;
            println!(
                "VehicleMovementComp: 0x{:X} (class 0x{:X})",
                vmc_addr, vmc_class
            );

            // 5. Loop SetThrottleInput at tick rate --------------------------
            let throttle_bytes = a.throttle.to_le_bytes();
            let iterations = (a.duration_ms / a.tick_ms).max(1);
            println!(
                "Driving for {}ms: {} ticks @ {}ms apart, throttle={:.2}",
                a.duration_ms, iterations, a.tick_ms, a.throttle
            );
            let start = std::time::Instant::now();
            let mut hits = 0u32;
            for _ in 0..iterations {
                let _ = session.call_ufunction(
                    vmc_addr,
                    vmc_class,
                    "SetThrottleInput",
                    throttle_bytes.to_vec(),
                )?;
                hits += 1;
                std::thread::sleep(std::time::Duration::from_millis(a.tick_ms as u64));
            }
            let elapsed = start.elapsed();
            println!("Drive loop done: {hits} calls in {:?}", elapsed);

            // 6. Brake: throttle=0, brake=1.0 --------------------------------
            let zero = 0f32.to_le_bytes().to_vec();
            let one = 1f32.to_le_bytes().to_vec();
            session.call_ufunction(vmc_addr, vmc_class, "SetThrottleInput", zero.clone())?;
            session.call_ufunction(vmc_addr, vmc_class, "SetBrakeInput", one)?;
            session.call_ufunction(vmc_addr, vmc_class, "SetSteeringInput", zero)?;
            println!("Braked.");
            std::thread::sleep(std::time::Duration::from_millis(500));

            // 7. Restore Batman ----------------------------------------------
            let mut restore_params = vec![0u8; 8];
            restore_params[0..8].copy_from_slice(&batman_addr.to_le_bytes());
            session
                .call_ufunction(pc_addr, pc_class, "Possess", restore_params)?
                .ok_or("Possess(Batman) UFunction missing")?;
            println!("Restored Batman pawn.");
        }
        Cmd::PedsFightTest(a) => {
            println!("attaching to pid {} via {}", a.pid, a.dll.display());
            let session = Ue5Session::attach_pid(a.pid, &a.dll)?;

            const TAG_OFFSET: u64 = 0xB28;
            const TAG_BYTES: usize = 8;

            let exclude_substrings: &[&str] =
                &["_Playable_", "_VEH_", "Default__", "/Engine/Transient/"];
            let class_includes: Vec<String> = a
                .class_substrings
                .split(',')
                .map(|s| s.trim().to_ascii_uppercase())
                .filter(|s| !s.is_empty())
                .collect();
            println!("class-name include filter: {:?}", class_includes);

            // Parse --write-hex into 8 bytes. Strict: must be exactly 16 hex
            // chars (FName.index u32 LE at +0, FName.number u32 LE at +4).
            let hex = a.write_hex.trim().replace(' ', "").to_ascii_lowercase();
            if hex.len() != TAG_BYTES * 2 {
                return Err(format!(
                    "--write-hex must be exactly {} hex chars (got {})",
                    TAG_BYTES * 2,
                    hex.len()
                )
                .into());
            }
            let mut tag_bytes = vec![0u8; TAG_BYTES];
            for i in 0..TAG_BYTES {
                tag_bytes[i] = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16)
                    .map_err(|e| format!("bad hex pair at offset {i}: {e}"))?;
            }
            println!(
                "writing 8-byte tag: {}",
                tag_bytes
                    .iter()
                    .map(|b| format!("{:02X}", b))
                    .collect::<Vec<_>>()
                    .join(" ")
            );

            let do_pass = |session: &Ue5Session,
                           label: &str,
                           tag: &[u8]|
             -> Result<usize, Box<dyn std::error::Error>> {
                // The DLL's find_all_uobjects only does exact class-name
                // matching. Every NPC here is a deep BP subclass, so we
                // instead use walk_objects() + host-side filter on class_name
                // substrings (catches BP_*Goon*, BP_*SWAT*, etc.).
                let objs = session.walk_objects()?;
                let mut scanned = 0usize;
                let mut written = 0usize;
                let mut skipped = 0usize;
                for obj in objs.iter() {
                    let cn_upper = obj.class_name.to_ascii_uppercase();
                    if !class_includes.iter().any(|s| cn_upper.contains(s)) {
                        continue;
                    }
                    scanned += 1;
                    if exclude_substrings.iter().any(|s| obj.fqn.contains(s)) {
                        skipped += 1;
                        continue;
                    }
                    if session
                        .write_property(obj.addr + TAG_OFFSET, PropValue::Bytes(tag.to_vec()))
                        .is_ok()
                    {
                        written += 1;
                    }
                }
                println!("[{label}] scanned={scanned} written={written} skipped={skipped}");
                Ok(written)
            };

            // Pass 0: one-shot write. If `--loop-seconds 0`, we're done.
            do_pass(&session, "pass0", &tag_bytes)?;
            if a.loop_seconds == 0 {
                println!("--- single-pass mode; exiting ---");
                return Ok(());
            }

            // Sustained pass: write every 250ms for the requested duration.
            // Simulates the freeze_for_matching runtime loop so we can see
            // if NPCs hold the written tag or BP tick reverts it.
            const TICK_MS: u64 = 250;
            let total_iters = (a.loop_seconds as u64 * 1000) / TICK_MS;
            println!(
                "--- sustained mode: {} iterations @ {}ms = ~{}s ---",
                total_iters, TICK_MS, a.loop_seconds
            );
            for i in 1..=total_iters {
                std::thread::sleep(std::time::Duration::from_millis(TICK_MS));
                let label = format!("pass{i}");
                do_pass(&session, &label, &tag_bytes)?;
            }
            println!("--- done ---");
        }
        Cmd::CivilianBrawlTest(a) => {
            // We need walk_objects + K2_GetActorLocation + targeted UFunction
            // dispatch. All of these are available on Ue5Session (which
            // gives us cached object/class lookup) — but we can't go
            // through `attach_pid` (it re-injects). Drop to Ue5Client and
            // re-implement the bits we need.
            println!("connecting to pid {} (no injection)", a.pid);
            let mut client = Ue5Client::connect(a.pid, Duration::from_secs(5))?;

            // ---- 1. Player pawn (for distance-from-player filter) ----------
            let live_pred = NamePredicate::Contains("/PersistentLevel/".to_string());
            let (ps_addr, ps_class) = client
                .find_uobject("BP_DinnerPlayerState_C", live_pred.clone())?
                .ok_or("no live BP_DinnerPlayerState_C")?;
            // PawnPrivate is on PlayerState — 8 bytes (object pointer).
            let pawn_prop = client
                .resolve_property(ps_class, "PawnPrivate")?
                .ok_or("PawnPrivate missing on PlayerState")?;
            let pawn_bytes = client.read_bytes(ps_addr + pawn_prop.offset as u64, 8)?;
            let player_addr = u64::from_le_bytes(pawn_bytes.as_slice().try_into().unwrap());
            if player_addr == 0 {
                return Err("PlayerState.PawnPrivate is null — no live player pawn".into());
            }

            // We need each candidate's class_ptr to call UFunctions, and we
            // need the player's class_ptr to call K2_GetActorLocation on it.
            // walk_objects gives us everything in one shot.
            let objs = client.walk_objects(None)?;
            let player_class = objs
                .iter()
                .find(|o| o.addr == player_addr)
                .map(|o| o.class_ptr)
                .ok_or("player pawn not in walk_objects")?;

            // Inline K2_GetActorLocation caller (24-byte FVector3d return).
            let get_loc = |client: &mut Ue5Client,
                           obj: u64,
                           cls: u64|
             -> Result<[f64; 3], Box<dyn std::error::Error>> {
                let buf = vec![0u8; 24];
                let ret = client
                    .call_ufunction(obj, cls, "K2_GetActorLocation", buf)?
                    .ok_or("K2_GetActorLocation not on class chain")?;
                if ret.len() < 24 {
                    return Err(format!("K2_GetActorLocation returned {} bytes", ret.len()).into());
                }
                let x = f64::from_le_bytes(ret[0..8].try_into().unwrap());
                let y = f64::from_le_bytes(ret[8..16].try_into().unwrap());
                let z = f64::from_le_bytes(ret[16..24].try_into().unwrap());
                Ok([x, y, z])
            };

            let player_loc = get_loc(&mut client, player_addr, player_class)?;
            println!(
                "player pawn 0x{:X} @ ({:.0}, {:.0}, {:.0})",
                player_addr, player_loc[0], player_loc[1], player_loc[2]
            );

            // ---- 2. Build civilian candidate list --------------------------
            let class_filters: Vec<String> = a
                .class_substrings
                .split(',')
                .map(|s| s.trim().to_ascii_uppercase())
                .filter(|s| !s.is_empty())
                .collect();
            println!("class filters: {:?}", class_filters);

            #[derive(Debug, Clone)]
            struct Civilian {
                addr: u64,
                class_ptr: u64,
                class_name: String,
                loc: [f64; 3],
                dist: f64,
            }
            let mut civilians: Vec<Civilian> = Vec::new();
            for obj in objs.iter() {
                if obj.fqn.contains("Default__")
                    || obj.fqn.contains("_GEN_VARIABLE")
                    || obj.fqn.contains("/Engine/Transient/")
                {
                    continue;
                }
                let cn_upper = obj.class_name.to_ascii_uppercase();
                if !class_filters.iter().any(|s| cn_upper.contains(s)) {
                    continue;
                }
                let loc = match get_loc(&mut client, obj.addr, obj.class_ptr) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                let dx = loc[0] - player_loc[0];
                let dy = loc[1] - player_loc[1];
                let dz = loc[2] - player_loc[2];
                let dist = (dx * dx + dy * dy + dz * dz).sqrt();
                if dist > a.max_distance {
                    continue;
                }
                civilians.push(Civilian {
                    addr: obj.addr,
                    class_ptr: obj.class_ptr,
                    class_name: obj.class_name.clone(),
                    loc,
                    dist,
                });
            }
            civilians.sort_by(|a, b| {
                a.dist
                    .partial_cmp(&b.dist)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            let cap = (a.top as usize).min(civilians.len());
            civilians.truncate(cap);
            println!(
                "--- {} civilians within {:.0}u ---",
                civilians.len(),
                a.max_distance
            );
            for (i, c) in civilians.iter().enumerate() {
                println!(
                    "  [{:2}] 0x{:X}  {:<32}  {:.0}u",
                    i, c.addr, c.class_name, c.dist
                );
            }
            if civilians.is_empty() {
                return Err("no civilians nearby — walk closer to a crowd".into());
            }

            let iterations = (a.duration_ms / a.tick_ms).max(1);
            let tick_dur = Duration::from_millis(a.tick_ms as u64);

            match a.technique.as_str() {
                // ---------------------------------------------------------------
                "attacked" => {
                    // Toggle Attacked(true) on every civilian every tick.
                    // 1-byte parameter blob = [0x01].
                    println!(
                        "\ntechnique=attacked  {} ticks × {}ms; calling Attacked(true) on {} civilians per tick\n",
                        iterations,
                        a.tick_ms,
                        civilians.len()
                    );
                    let parm = vec![1u8];
                    for tick in 0..iterations {
                        let mut hits = 0u32;
                        for c in &civilians {
                            match client.call_ufunction(
                                c.addr,
                                c.class_ptr,
                                "Attacked",
                                parm.clone(),
                            ) {
                                Ok(Some(_)) => hits += 1,
                                Ok(None) => {} // function not on class chain
                                Err(e) => eprintln!("  call err on 0x{:X}: {}", c.addr, e),
                            }
                        }
                        println!("[tick {:>3}] Attacked(true) hits={}", tick, hits);
                        std::thread::sleep(tick_dur);
                    }
                    // Cool-down: call Attacked(false) once to clear the flag.
                    let parm_off = vec![0u8];
                    for c in &civilians {
                        let _ = client.call_ufunction(
                            c.addr,
                            c.class_ptr,
                            "Attacked",
                            parm_off.clone(),
                        );
                    }
                    println!("cool-down: Attacked(false) sent to all candidates");
                }
                // ---------------------------------------------------------------
                "player-reaction" => {
                    // PlayerReaction(Player: AActor*, bAllowReaction: bool).
                    // ParmsSize=9; layout: u64 player_ptr @ +0, u8 bool @ +8.
                    // Use the FIRST civilian as the fake "player" for every
                    // other civilian to react to.
                    if civilians.len() < 2 {
                        return Err("need ≥2 civilians for player-reaction test".into());
                    }
                    let fake_player_addr = civilians[0].addr;
                    println!(
                        "\ntechnique=player-reaction  fake-player=0x{:X}\n  {} ticks × {}ms across {} other civilians\n",
                        fake_player_addr,
                        iterations,
                        a.tick_ms,
                        civilians.len() - 1
                    );
                    let mut parm = vec![0u8; 9];
                    parm[0..8].copy_from_slice(&fake_player_addr.to_le_bytes());
                    parm[8] = 1; // bAllowReaction = true
                    for tick in 0..iterations {
                        let mut hits = 0u32;
                        for c in &civilians[1..] {
                            match client.call_ufunction(
                                c.addr,
                                c.class_ptr,
                                "PlayerReaction",
                                parm.clone(),
                            ) {
                                Ok(Some(_)) => hits += 1,
                                Ok(None) => {}
                                Err(e) => eprintln!("  call err on 0x{:X}: {}", c.addr, e),
                            }
                        }
                        println!("[tick {:>3}] PlayerReaction hits={}", tick, hits);
                        std::thread::sleep(tick_dur);
                    }
                }
                // ---------------------------------------------------------------
                "brawl-pair" => {
                    // Real puppet-show: pair nearest-neighbours, face them,
                    // and alternate attack + hit-react montages each tick.
                    // The combination produces visible brawl behaviour
                    // because PlayAnimMontage slots directly into the
                    // AnimInstance — no BT involvement needed.
                    if civilians.len() < 2 {
                        return Err("need ≥2 civilians to pair".into());
                    }
                    let mut taken = vec![false; civilians.len()];
                    let mut pairs: Vec<(usize, usize, f64)> = Vec::new();
                    for i in 0..civilians.len() {
                        if taken[i] {
                            continue;
                        }
                        let mut best: Option<(usize, f64)> = None;
                        for j in (i + 1)..civilians.len() {
                            if taken[j] {
                                continue;
                            }
                            let dx = civilians[i].loc[0] - civilians[j].loc[0];
                            let dy = civilians[i].loc[1] - civilians[j].loc[1];
                            let dz = civilians[i].loc[2] - civilians[j].loc[2];
                            let d = (dx * dx + dy * dy + dz * dz).sqrt();
                            if d > a.pair_max_apart {
                                continue;
                            }
                            if best.is_none() || d < best.unwrap().1 {
                                best = Some((j, d));
                            }
                        }
                        if let Some((j, d)) = best {
                            taken[i] = true;
                            taken[j] = true;
                            pairs.push((i, j, d));
                        }
                    }
                    // Limit pairs to keep brawl visible. Pairs are already in
                    // distance-from-player order (civilians is sorted ascending),
                    // so the first N are the closest to the player.
                    if a.max_pairs > 0 {
                        pairs.truncate(a.max_pairs as usize);
                    }
                    if pairs.is_empty() {
                        return Err(format!(
                            "no pairs within --pair-max-apart={:.0}u — try widening or moving closer to a crowd",
                            a.pair_max_apart
                        )
                        .into());
                    }
                    println!(
                        "\ntechnique=brawl-pair  {} pairs (max-apart={:.0}u, engage<{:.0}u, step={:.0}u):",
                        pairs.len(),
                        a.pair_max_apart,
                        a.engage_distance,
                        a.approach_step
                    );
                    for (i, j, d) in &pairs {
                        println!("  pair {:>2} ⟷ {:>2}  apart={:.0}u", i, j, d);
                    }

                    // Attack montages we'll cycle the attacker through.
                    // All Minifig-compatible (verified animating on
                    // BP_Population_Minifig_C 2026-05-25).
                    let attack_montages: &[(u64, &str)] = &[
                        (0x001BEB71A400, "AM_D0_AttackFwd_Chain_LtoL_Minifig"),
                        (0x001BEB719C00, "AM_D0_AttackFwd_Chain_RtoR_Minifig"),
                        (0x001BEB71A000, "AM_D0_AttackRight_Start_RtoL_Minifig"),
                        (0x001BEB719E00, "AM_D1_AttackRight_Start_LtoR_Minifig"),
                        (0x001BE2E7C600, "AM_InterceptionAttack_Ground_Minifig"),
                        (0x001BEB71AA00, "AM_JumpAttack_Minifig"),
                    ];
                    // Hit-react montages for the victim.
                    let hit_montages: &[(u64, &str)] = &[
                        (0x001C4A7ABC00, "AM_Stunned_Minifig"),
                        (0x001BDDB2B800, "AM_KnockBack_Heavy_Idle_Minifig"),
                        (0x001C3D7AAE00, "AM_Takehit_BatClaw_FaceUp_Minifig"),
                        (0x001C51481800, "AM_GrabAndThrow_DamageReact_Minifig_E1"),
                        (0x001C4D319200, "AM_CounterThrow_HitWall_Minifig"),
                    ];
                    // Death montage for the KO finisher. Generic rolling
                    // ragdoll — looks like a knock-out fall, plays cleanly
                    // on Minifig skeleton.
                    const DEATH_MONTAGE: u64 = 0x001BE35D5000;
                    // Engage-ticks per pair before the "KO" finisher fires.
                    // After KO, the pair sits out for a few ticks (loser
                    // stays down) before re-engaging.
                    const ENGAGE_TICKS_TO_KO: u32 = 8;
                    const KO_COOLDOWN_TICKS: u32 = 12;

                    // NOTE: `Destroy Umbrella` was attempted as a pre-brawl
                    // hook but crashed the game (2026-05-25). The BP body
                    // does `GetAnimInstance` + DynamicCast to BPI_Animation
                    // and may not be safe to invoke on every civilian — only
                    // those actively holding an umbrella. Skipping for now;
                    // re-introduce only if we can pre-filter by
                    // `bUmbrella Out` (FProperty at +0x9B1) being true.

                    // Re-read player position once we're about to start, since
                    // the user may have moved between candidate selection and
                    // the first tick. (Not currently used but cheap to have.)

                    let make_montage_params = |m_addr: u64, rate: f32| -> Vec<u8> {
                        let mut p = vec![0u8; 24];
                        p[0..8].copy_from_slice(&m_addr.to_le_bytes());
                        p[8..12].copy_from_slice(&rate.to_le_bytes());
                        p
                    };

                    // K2_SetActorRotation params: FRotator (24B LWC) +
                    // bTeleportPhysics (u8) + ReturnValue (u8) = 26B.
                    // The DLL hard-rejects oversize blobs (ParmsSize=26),
                    // so we send exactly 26 bytes.
                    let make_rot_params = |yaw_deg: f64| -> Vec<u8> {
                        let mut p = vec![0u8; 26];
                        // FRotator: Pitch (f64) + Yaw (f64) + Roll (f64)
                        p[0..8].copy_from_slice(&0f64.to_le_bytes());
                        p[8..16].copy_from_slice(&yaw_deg.to_le_bytes());
                        p[16..24].copy_from_slice(&0f64.to_le_bytes());
                        // p[24] bTeleportPhysics = false; p[25] return slot
                        p
                    };

                    // K2_AddActorWorldOffset params: FVector (24B LWC) +
                    // bSweep (u8) + FHitResult OutSweepHitResult (~136B) +
                    // bTeleport (u8). ParmsSize is ~162; the DLL hard-rejects
                    // blobs larger than ParmsSize. Send only the leading
                    // 25 bytes (FVector + bSweep=false) — the DLL zero-pads
                    // the remainder so HitResult is zeroed and bTeleport=0.
                    let make_offset_params = |dx: f64, dy: f64, dz: f64| -> Vec<u8> {
                        let mut p = vec![0u8; 25];
                        p[0..8].copy_from_slice(&dx.to_le_bytes());
                        p[8..16].copy_from_slice(&dy.to_le_bytes());
                        p[16..24].copy_from_slice(&dz.to_le_bytes());
                        p[24] = 0; // bSweep = false
                        p
                    };

                    // Per-pair engage tracking: how many brawl ticks have
                    // we spent in range (used to know when to swap roles
                    // and when to fire the KO finisher).
                    #[derive(Clone, Copy)]
                    enum PairState {
                        Active {
                            engage: u32,
                        },
                        KO {
                            cooldown_left: u32,
                            loser_idx: usize,
                        },
                    }
                    let mut states: Vec<PairState> =
                        vec![PairState::Active { engage: 0 }; pairs.len()];

                    for tick in 0..iterations {
                        let atk_m = attack_montages[(tick as usize) % attack_montages.len()];
                        let hit_m = hit_montages[(tick as usize) % hit_montages.len()];
                        let atk_parm = make_montage_params(atk_m.0, 1.0);
                        let hit_parm = make_montage_params(hit_m.0, 1.0);
                        let death_parm = make_montage_params(DEATH_MONTAGE, 1.0);
                        // (victim_idx, hit_montage_or_death)
                        let mut victims_to_recoil: Vec<(usize, Vec<u8>)> = Vec::new();
                        let mut phase_summary = String::new();

                        for (pair_idx, (i, j, _)) in pairs.iter().enumerate() {
                            let a_loc = match get_loc(
                                &mut client,
                                civilians[*i].addr,
                                civilians[*i].class_ptr,
                            ) {
                                Ok(v) => v,
                                Err(_) => continue,
                            };
                            let b_loc = match get_loc(
                                &mut client,
                                civilians[*j].addr,
                                civilians[*j].class_ptr,
                            ) {
                                Ok(v) => v,
                                Err(_) => continue,
                            };
                            let dx = b_loc[0] - a_loc[0];
                            let dy = b_loc[1] - a_loc[1];
                            let dist = (dx * dx + dy * dy).sqrt().max(1.0);
                            let ux = dx / dist;
                            let uy = dy / dist;
                            let yaw_ab = dy.atan2(dx) * 180.0 / std::f64::consts::PI;
                            let yaw_ba = yaw_ab + 180.0;

                            match states[pair_idx] {
                                PairState::KO {
                                    mut cooldown_left,
                                    loser_idx,
                                } => {
                                    // Loser stays down, winner just stands.
                                    // Re-assert death montage every few
                                    // ticks so the engine doesn't drift the
                                    // loser back to idle.
                                    if cooldown_left % 4 == 0 {
                                        let _ = client.call_ufunction(
                                            civilians[loser_idx].addr,
                                            civilians[loser_idx].class_ptr,
                                            "PlayAnimMontage",
                                            death_parm.clone(),
                                        );
                                    }
                                    cooldown_left = cooldown_left.saturating_sub(1);
                                    states[pair_idx] = if cooldown_left == 0 {
                                        PairState::Active { engage: 0 }
                                    } else {
                                        PairState::KO {
                                            cooldown_left,
                                            loser_idx,
                                        }
                                    };
                                    phase_summary
                                        .push_str(&format!("[{} KO·{}]", pair_idx, cooldown_left));
                                }
                                PairState::Active { engage } => {
                                    if dist > a.engage_distance {
                                        // APPROACH: step both toward each other.
                                        states[pair_idx] = PairState::Active { engage: 0 };
                                        let step = a.approach_step;
                                        let _ = client.call_ufunction(
                                            civilians[*i].addr,
                                            civilians[*i].class_ptr,
                                            "K2_AddActorWorldOffset",
                                            make_offset_params(ux * step, uy * step, 0.0),
                                        );
                                        let _ = client.call_ufunction(
                                            civilians[*j].addr,
                                            civilians[*j].class_ptr,
                                            "K2_AddActorWorldOffset",
                                            make_offset_params(-ux * step, -uy * step, 0.0),
                                        );
                                        // Strong facing: spam rotation calls
                                        // so Mass has less room to overwrite.
                                        for _ in 0..2 {
                                            let _ = client.call_ufunction(
                                                civilians[*i].addr,
                                                civilians[*i].class_ptr,
                                                "K2_SetActorRotation",
                                                make_rot_params(yaw_ab),
                                            );
                                            let _ = client.call_ufunction(
                                                civilians[*j].addr,
                                                civilians[*j].class_ptr,
                                                "K2_SetActorRotation",
                                                make_rot_params(yaw_ba),
                                            );
                                        }
                                        phase_summary
                                            .push_str(&format!("[{} app {:.0}u]", pair_idx, dist));
                                    } else if engage >= ENGAGE_TICKS_TO_KO {
                                        // KO finisher: pick the current
                                        // victim as loser, fire death montage.
                                        let in_block = engage / 3;
                                        let loser_idx = if in_block % 2 == 0 { *j } else { *i };
                                        let winner_idx = if loser_idx == *i { *j } else { *i };
                                        // Winner does a final swing.
                                        let win_yaw =
                                            if winner_idx == *i { yaw_ab } else { yaw_ba };
                                        let lose_yaw =
                                            if loser_idx == *i { yaw_ab } else { yaw_ba };
                                        let _ = client.call_ufunction(
                                            civilians[winner_idx].addr,
                                            civilians[winner_idx].class_ptr,
                                            "K2_SetActorRotation",
                                            make_rot_params(win_yaw),
                                        );
                                        let _ = client.call_ufunction(
                                            civilians[loser_idx].addr,
                                            civilians[loser_idx].class_ptr,
                                            "K2_SetActorRotation",
                                            make_rot_params(lose_yaw),
                                        );
                                        let _ = client.call_ufunction(
                                            civilians[winner_idx].addr,
                                            civilians[winner_idx].class_ptr,
                                            "PlayAnimMontage",
                                            atk_parm.clone(),
                                        );
                                        // Queue death montage on loser
                                        // ~300ms in (after the winner's hit
                                        // lands).
                                        victims_to_recoil.push((loser_idx, death_parm.clone()));
                                        states[pair_idx] = PairState::KO {
                                            cooldown_left: KO_COOLDOWN_TICKS,
                                            loser_idx,
                                        };
                                        phase_summary.push_str(&format!(
                                            "[{} KO! winner={}]",
                                            pair_idx, winner_idx
                                        ));
                                    } else {
                                        // ENGAGE: regular swing. Keep this
                                        // dead simple — one rotation per
                                        // civilian, one PlayAnimMontage on
                                        // the attacker, queue the victim
                                        // recoil for after a short delay.
                                        // NO mid-engage WorldOffset — root
                                        // motion from the attack montage
                                        // can conflict (observed crash
                                        // 2026-05-25 during the engage
                                        // phase of a 25s run).
                                        let in_block = engage / 3;
                                        let (attacker_idx, victim_idx) = if in_block % 2 == 0 {
                                            (*i, *j)
                                        } else {
                                            (*j, *i)
                                        };
                                        states[pair_idx] = PairState::Active { engage: engage + 1 };
                                        let attacker = &civilians[attacker_idx];
                                        let victim = &civilians[victim_idx];
                                        let atk_yaw =
                                            if attacker_idx == *i { yaw_ab } else { yaw_ba };
                                        let vic_yaw =
                                            if victim_idx == *i { yaw_ab } else { yaw_ba };
                                        let _ = client.call_ufunction(
                                            attacker.addr,
                                            attacker.class_ptr,
                                            "K2_SetActorRotation",
                                            make_rot_params(atk_yaw),
                                        );
                                        let _ = client.call_ufunction(
                                            victim.addr,
                                            victim.class_ptr,
                                            "K2_SetActorRotation",
                                            make_rot_params(vic_yaw),
                                        );
                                        let _ = client.call_ufunction(
                                            attacker.addr,
                                            attacker.class_ptr,
                                            "PlayAnimMontage",
                                            atk_parm.clone(),
                                        );
                                        victims_to_recoil.push((victim_idx, hit_parm.clone()));
                                        phase_summary.push_str(&format!(
                                            "[{} HIT eng={}/{}]",
                                            pair_idx,
                                            engage + 1,
                                            ENGAGE_TICKS_TO_KO
                                        ));
                                    }
                                }
                            }
                        }
                        // ~300ms into engage swings, fire the recoils / KO
                        // collapse on victims.
                        if !victims_to_recoil.is_empty() {
                            std::thread::sleep(Duration::from_millis(300));
                            for (vidx, parm) in &victims_to_recoil {
                                let _ = client.call_ufunction(
                                    civilians[*vidx].addr,
                                    civilians[*vidx].class_ptr,
                                    "PlayAnimMontage",
                                    parm.clone(),
                                );
                            }
                        }
                        println!(
                            "[tick {:>3}] atk={} hit={} {}",
                            tick, atk_m.1, hit_m.1, phase_summary
                        );
                        let remain =
                            (a.tick_ms as i64) - if victims_to_recoil.is_empty() { 0 } else { 300 };
                        if remain > 0 {
                            std::thread::sleep(Duration::from_millis(remain as u64));
                        }
                    }
                    println!("brawl-pair complete (montages will play out naturally)");
                }
                // ---------------------------------------------------------------
                "play-montage" => {
                    // Direct animation drive via UCharacter::PlayAnimMontage.
                    // Bypasses every BP / BT / GAS path — slots the montage
                    // straight into the AnimInstance's montage slot.
                    //
                    // Params (24B total, ParmsSize=24):
                    //   +0x00  AnimMontage* (u64 LE)
                    //   +0x08  InPlayRate   (f32)
                    //   +0x0C  StartSectionName (FName u32 idx + u32 number)
                    //   +0x14  ReturnValue  (f32, engine fills in duration)
                    //
                    // The montage addresses are hard-coded from a walk_objects
                    // dump (look for `/Game/Animation/LEGOfig/_Shared/Takehit/`
                    // and `/Game/Animation/LEGOfig/_Shared/AI_Behaviour/`).
                    // If these change across game patches, re-discover.
                    let montages: &[(u64, &str)] = &[
                        (0x001C4A7ABC00, "AM_Stunned_Minifig"),
                        (0x001C4964D000, "AM_Panic1_Minifig"),
                        (0x001BEB102400, "AM_Panic2_Minifig"),
                        (0x001C3F5B3200, "AM_Shudder_2_Pedestrian"),
                        (0x001C3D7AAE00, "AM_Takehit_BatClaw_FaceUp_Minifig"),
                        (0x001C4B891C00, "AM_FearGetAttention"),
                    ];
                    println!(
                        "\ntechnique=play-montage  cycling {} montages × {} ticks × {}ms on {} civilians\n",
                        montages.len(),
                        iterations,
                        a.tick_ms,
                        civilians.len()
                    );

                    let make_montage_params = |m_addr: u64, rate: f32| -> Vec<u8> {
                        let mut p = vec![0u8; 24];
                        p[0..8].copy_from_slice(&m_addr.to_le_bytes());
                        p[8..12].copy_from_slice(&rate.to_le_bytes());
                        // StartSectionName = NAME_None (idx=0, number=0)
                        p[12..16].copy_from_slice(&0u32.to_le_bytes());
                        p[16..20].copy_from_slice(&0u32.to_le_bytes());
                        // ReturnValue slot stays 0
                        p
                    };

                    for tick in 0..iterations {
                        let (m_addr, m_name) = montages[(tick as usize) % montages.len()];
                        let parm = make_montage_params(m_addr, 1.0);
                        let mut hits = 0u32;
                        let mut errs: Vec<String> = Vec::new();
                        for c in &civilians {
                            match client.call_ufunction(
                                c.addr,
                                c.class_ptr,
                                "PlayAnimMontage",
                                parm.clone(),
                            ) {
                                Ok(Some(ret)) => {
                                    hits += 1;
                                    if ret.len() >= 24 {
                                        // Return-value duration is at +0x14.
                                        let dur = f32::from_le_bytes(
                                            ret[0x14..0x18].try_into().unwrap_or_default(),
                                        );
                                        if dur > 0.0 && hits <= 3 {
                                            println!(
                                                "    civ 0x{:X} -> duration {:.2}s",
                                                c.addr, dur
                                            );
                                        }
                                    }
                                }
                                Ok(None) => errs.push(format!("0x{:X} NotFound", c.addr)),
                                Err(e) => errs.push(format!("0x{:X} {}", c.addr, e)),
                            }
                        }
                        println!(
                            "[tick {:>3}] PlayAnimMontage({})  hits={}/{}",
                            tick,
                            m_name,
                            hits,
                            civilians.len()
                        );
                        if !errs.is_empty() && tick == 0 {
                            for e in &errs {
                                println!("    err: {}", e);
                            }
                        }
                        std::thread::sleep(tick_dur);
                    }
                }
                // ---------------------------------------------------------------
                "vehicle-collision" => {
                    // VehicleCollision(ImpactDirection: f64) — civilian's
                    // got-hit-by-car path. This is the most physical
                    // reaction available: usually plays a knockdown
                    // ragdoll. Cycle the direction so consecutive hits
                    // don't accumulate into the same animation slot.
                    println!(
                        "\ntechnique=vehicle-collision  {} ticks × {}ms; calling VehicleCollision() on {} civilians per tick\n",
                        iterations,
                        a.tick_ms,
                        civilians.len()
                    );
                    for tick in 0..iterations {
                        // Sweep direction in 45° increments (radians, since
                        // many UE collision-direction APIs treat the f64 as
                        // an angle in radians; if BP just consumes the
                        // magnitude, this still varies the input).
                        let dir = ((tick as f64) * std::f64::consts::FRAC_PI_4)
                            % (2.0 * std::f64::consts::PI);
                        let mut parm = vec![0u8; 8];
                        parm.copy_from_slice(&dir.to_le_bytes());
                        let mut hits = 0u32;
                        for c in &civilians {
                            match client.call_ufunction(
                                c.addr,
                                c.class_ptr,
                                "VehicleCollision",
                                parm.clone(),
                            ) {
                                Ok(Some(_)) => hits += 1,
                                Ok(None) => {}
                                Err(e) => eprintln!("  call err on 0x{:X}: {}", c.addr, e),
                            }
                        }
                        println!(
                            "[tick {:>3}] VehicleCollision({:.2}) hits={}",
                            tick, dir, hits
                        );
                        std::thread::sleep(tick_dur);
                    }
                }
                // ---------------------------------------------------------------
                "player-dodge" => {
                    // PlayerDodge(PlayerDodgeDirection: Byte). Direction
                    // enum — we sweep through 0..4 to test which enum
                    // values produce visible animations.
                    println!(
                        "\ntechnique=player-dodge  {} ticks × {}ms; calling PlayerDodge() on {} civilians per tick\n",
                        iterations,
                        a.tick_ms,
                        civilians.len()
                    );
                    for tick in 0..iterations {
                        let dir_enum: u8 = (tick % 4) as u8;
                        let parm = vec![dir_enum];
                        let mut hits = 0u32;
                        for c in &civilians {
                            match client.call_ufunction(
                                c.addr,
                                c.class_ptr,
                                "PlayerDodge",
                                parm.clone(),
                            ) {
                                Ok(Some(_)) => hits += 1,
                                Ok(None) => {}
                                Err(e) => eprintln!("  call err on 0x{:X}: {}", c.addr, e),
                            }
                        }
                        println!(
                            "[tick {:>3}] PlayerDodge(dir={}) hits={}",
                            tick, dir_enum, hits
                        );
                        std::thread::sleep(tick_dur);
                    }
                }
                // ---------------------------------------------------------------
                "dance" => {
                    // Make every NPC nearby dance / do a funny animation.
                    // Each NPC gets a montage from a rotating list; the
                    // selection cycles per-NPC so the crowd doesn't all
                    // do the same move in sync. Tick cadence should be
                    // close to the average montage length (~2.5-3s) so
                    // a new dance starts as the previous one fades.
                    //
                    // No pairing, no facing, no movement. Just montages.
                    //
                    // CRITICAL: only animate NPCs within LOD range. Beyond
                    // ~1500u characters get torpor'd (Mass-only, no
                    // AnimInstance), and PlayAnimMontage on a half-allocated
                    // pawn crashed the game 2026-05-25.
                    const LOD_RANGE: f64 = 1500.0;
                    let in_range: Vec<&Civilian> =
                        civilians.iter().filter(|c| c.dist <= LOD_RANGE).collect();
                    if in_range.is_empty() {
                        return Err(format!(
                            "no NPCs within {:.0}u of player — walk closer to a crowd",
                            LOD_RANGE
                        )
                        .into());
                    }
                    println!(
                        "--- {} NPCs in animation-safe LOD range (≤{:.0}u) ---",
                        in_range.len(),
                        LOD_RANGE
                    );
                    let dance_montages: &[(u64, &str)] = &[
                        (0x001C48BFAC00, "AM_Dance_Pogo_Minifig"),
                        (0x001C4964C000, "AM_Dance_HipHop_Minifig"),
                        (0x00010824EC00, "AM_Dance_Clap_Minifig"),
                        (0x001C4964F800, "AM_Clapping_Minifig"),
                        (0x001C4A7A5C00, "AM_Laugh_Minifig"),
                        (0x001BEB10B600, "AM_Gesture_Happy_Minifig"),
                        (0x001C4964D400, "AM_Gesture_Happy2_Minifig"),
                        (0x001BEB102000, "AM_Gesture_Happy3_Minifig"),
                        (0x001C3EDB0A00, "AM_GoonTaunt1_Minifig"),
                        (0x001C4964EA00, "AM_GoonTaunt2_Minifig"),
                        (0x000108249600, "AM_GoonTaunt3_Minifig"),
                        (0x001C3F5BA000, "AM_GoonTaunt4_Minifig"),
                        (0x001C4F702800, "AM_BuildIt_Celebrate_Minifig"),
                        (0x001C4F702400, "AM_Collectable_Celebrate_Minifig"),
                    ];
                    let make_montage_params = |m_addr: u64, rate: f32| -> Vec<u8> {
                        let mut p = vec![0u8; 24];
                        p[0..8].copy_from_slice(&m_addr.to_le_bytes());
                        p[8..12].copy_from_slice(&rate.to_le_bytes());
                        p
                    };
                    println!(
                        "\ntechnique=dance  {} dance montages × {} ticks × {}ms on {} NPCs\n",
                        dance_montages.len(),
                        iterations,
                        a.tick_ms,
                        in_range.len()
                    );
                    for tick in 0..iterations {
                        let mut hits = 0u32;
                        // Per-NPC stable rotation: each NPC picks
                        // (idx + tick) mod len so the crowd looks varied
                        // but each individual NPC moves through the list.
                        for (npc_idx, c) in in_range.iter().enumerate() {
                            let pick = (tick as usize + npc_idx) % dance_montages.len();
                            let (m_addr, _) = dance_montages[pick];
                            let parm = make_montage_params(m_addr, 1.0);
                            if let Ok(Some(_)) =
                                client.call_ufunction(c.addr, c.class_ptr, "PlayAnimMontage", parm)
                            {
                                hits += 1;
                            }
                        }
                        println!("[tick {:>3}] dance hits={}/{}", tick, hits, in_range.len());
                        std::thread::sleep(tick_dur);
                    }
                }
                // ---------------------------------------------------------------
                "demo-all" => {
                    // Run each technique once for ~3s with a 2s gap so the
                    // user can watch the screen and see which one produces
                    // visible motion. Use this when we don't yet know which
                    // BP function is wired up to animation.
                    let demo_dur = Duration::from_millis(3500);
                    let gap = Duration::from_millis(1500);
                    let demo_tick = Duration::from_millis(350);

                    let demos: &[(&str, Box<dyn Fn(u32) -> (String, Vec<u8>)>)] = &[
                        (
                            "Attacked",
                            Box::new(|_| ("Attacked(true)".into(), vec![1u8])),
                        ),
                        (
                            "PlayerDodge",
                            Box::new(|t: u32| {
                                (format!("PlayerDodge(dir={})", t % 4), vec![(t % 4) as u8])
                            }),
                        ),
                        (
                            "VehicleCollision",
                            Box::new(|t: u32| {
                                let dir = ((t as f64) * std::f64::consts::FRAC_PI_4)
                                    % (2.0 * std::f64::consts::PI);
                                let mut p = vec![0u8; 8];
                                p.copy_from_slice(&dir.to_le_bytes());
                                (format!("VehicleCollision({:.2})", dir), p)
                            }),
                        ),
                    ];
                    for (fn_name, builder) in demos {
                        println!("\n=== demoing {fn_name} for ~3.5s ===");
                        let demo_start = std::time::Instant::now();
                        let mut t: u32 = 0;
                        while demo_start.elapsed() < demo_dur {
                            let (label, parm) = builder(t);
                            let mut hits = 0u32;
                            for c in &civilians {
                                if let Ok(Some(_)) = client.call_ufunction(
                                    c.addr,
                                    c.class_ptr,
                                    fn_name,
                                    parm.clone(),
                                ) {
                                    hits += 1;
                                }
                            }
                            println!("  [t={:>2}] {}  hits={}", t, label, hits);
                            t += 1;
                            std::thread::sleep(demo_tick);
                        }
                        println!("  (gap)");
                        std::thread::sleep(gap);
                    }
                    // Clear Attacked flag at the end.
                    let parm_off = vec![0u8];
                    for c in &civilians {
                        let _ = client.call_ufunction(
                            c.addr,
                            c.class_ptr,
                            "Attacked",
                            parm_off.clone(),
                        );
                    }
                }
                other => {
                    return Err(format!(
                        "unknown technique `{other}` — expected `attacked`, `player-reaction`, `player-dodge`, `vehicle-collision`, `brawl-pair`, or `demo-all`"
                    )
                    .into());
                }
            }

            client.disconnect()?;
        }
        Cmd::WalkPropsAt(a) => {
            println!("connecting to pid {} (no injection)", a.pid);
            let mut client = Ue5Client::connect(a.pid, Duration::from_secs(5))?;
            let props = client.walk_properties(a.addr)?;
            // ParmsSize lives at +0xB6 in UFunction (u16). Also peek it.
            let ps_bytes = client.read_bytes(a.addr + 0xB6, 2)?;
            let parms_size = u16::from_le_bytes(ps_bytes.as_slice().try_into().unwrap());
            println!(
                "--- UStruct at 0x{:X} — ParmsSize=+0xB6={} ---",
                a.addr, parms_size
            );
            println!(
                "{:<40} {:<22} {:>8} {:>6}",
                "name", "kind", "offset", "size"
            );
            for p in props.iter() {
                println!(
                    "{:<40} {:<22} {:>8} {:>6}",
                    truncate(&p.name, 40),
                    truncate(&p.kind, 22),
                    format!("+0x{:X}", p.offset),
                    p.size,
                );
            }
            client.disconnect()?;
        }
        Cmd::DiscoverPipe(a) => {
            println!("connecting to pid {} (no injection)", a.pid);
            let mut client = Ue5Client::connect(a.pid, Duration::from_secs(5))?;
            let (obj_addr, class_addr) = match client.find_uobject(&a.class, a.predicate)? {
                Some(pair) => pair,
                None => {
                    println!("FindUObject({}) returned NotFound", a.class);
                    return Ok(());
                }
            };
            println!("found: obj=0x{:X}  class=0x{:X}", obj_addr, class_addr);

            // Optionally walk the parent class instead of the matched class.
            // The class's super_struct lives at +0x40 (ustruct_super_struct).
            let walk_addr = if a.walk_super {
                let super_bytes = client.read_bytes(class_addr + 0x40, 8)?;
                let super_addr = u64::from_le_bytes(super_bytes.as_slice().try_into().unwrap());
                if super_addr == 0 {
                    println!("class has no super; falling back to class itself");
                    class_addr
                } else {
                    println!("walking super class at 0x{:X}", super_addr);
                    super_addr
                }
            } else {
                class_addr
            };

            let props = client.walk_properties(walk_addr)?;
            let prop_cap = if a.max_props == 0 {
                props.len()
            } else {
                (a.max_props as usize).min(props.len())
            };
            println!(
                "--- FProperties ({} total{}) ---",
                props.len(),
                if prop_cap < props.len() {
                    format!(", showing first {prop_cap}")
                } else {
                    String::new()
                }
            );
            println!(
                "{:<40} {:<22} {:>8} {:>6}   {}",
                "name", "kind", "offset", "size", "defined_in_class"
            );
            for p in props.iter().take(prop_cap) {
                println!(
                    "{:<40} {:<22} {:>8} {:>6}   {}",
                    truncate(&p.name, 40),
                    truncate(&p.kind, 22),
                    format!("+0x{:X}", p.offset),
                    p.size,
                    p.defined_in_class
                );
            }
            if !a.no_funcs {
                let funcs = client.walk_functions(walk_addr)?;
                let func_cap = if a.max_funcs == 0 {
                    funcs.len()
                } else {
                    (a.max_funcs as usize).min(funcs.len())
                };
                println!(
                    "--- UFunctions ({} total{}) ---",
                    funcs.len(),
                    if func_cap < funcs.len() {
                        format!(", showing first {func_cap}")
                    } else {
                        String::new()
                    }
                );
                println!(
                    "{:<48} {:<8} {:<18}   {}",
                    "name", "native", "addr", "defined_in_class"
                );
                for f in funcs.iter().take(func_cap) {
                    println!(
                        "{:<48} {:<8} 0x{:<16X}   {}",
                        truncate(&f.name, 48),
                        if f.is_native { "native" } else { "script" },
                        f.addr,
                        f.defined_in_class
                    );
                }
            }
            client.disconnect()?;
        }
        Cmd::WalkClasses(a) => {
            // Bypass Ue5Session::attach_pid (which tries to inject). The
            // Tauri app already injected the DLL — we just open the pipe.
            println!("connecting to pid {} (no injection)", a.pid);
            let mut client = Ue5Client::connect(a.pid, Duration::from_secs(5))?;
            let objs = client.walk_objects(None)?;
            println!("walked {} objects", objs.len());

            let filters: Vec<String> = a
                .filter
                .split(',')
                .map(|s| s.trim().to_ascii_uppercase())
                .filter(|s| !s.is_empty())
                .collect();

            use std::collections::HashMap;
            #[derive(Default)]
            struct ClassStats {
                count: u32,
                live_samples: Vec<(u64, String)>,
            }
            let mut by_class: HashMap<String, ClassStats> = HashMap::new();
            for obj in objs.iter() {
                let cn_upper = obj.class_name.to_ascii_uppercase();
                if !filters.is_empty() && !filters.iter().any(|s| cn_upper.contains(s)) {
                    continue;
                }
                let entry = by_class.entry(obj.class_name.clone()).or_default();
                entry.count += 1;
                let is_live = !obj.fqn.contains("Default__")
                    && !obj.fqn.contains("_GEN_VARIABLE")
                    && !obj.fqn.contains("/Engine/Transient/");
                if (entry.live_samples.len() as u32) < a.sample && (!a.live_only || is_live) {
                    entry.live_samples.push((obj.addr, obj.fqn.clone()));
                }
            }

            let mut rows: Vec<(String, ClassStats)> = by_class.into_iter().collect();
            rows.sort_by(|a, b| b.1.count.cmp(&a.1.count));
            let cap = if a.top == 0 {
                rows.len()
            } else {
                (a.top as usize).min(rows.len())
            };
            println!(
                "--- {} classes match (filter={:?}); top {} by instance count ---",
                rows.len(),
                filters,
                cap
            );
            for (class_name, stats) in rows.iter().take(cap) {
                println!("{:5} × {}", stats.count, class_name);
                for (addr, fqn) in &stats.live_samples {
                    println!("        0x{:012X}  {}", addr, fqn);
                }
            }
            client.disconnect()?;
        }
        Cmd::PipeProbe(a) => {
            // `pipe-probe` doesn't need the DLL on disk — the DLL is
            // assumed already injected. We keep `--dll` on the args group
            // for ergonomics but ignore it here.
            let _ = a.dll;
            let name = pipe_name_for_pid(a.pid);
            println!("probing {name} (timeout {:?})", DEFAULT_CONNECT_TIMEOUT);
            let mut client = Ue5Client::connect(a.pid, Duration::from_secs(5))?;
            let w = client.welcome().clone();
            println!(
                "welcome: pid={} guobject=0x{:X} fname_pool=0x{:X} chunks_off=+0x{:X} stride={} validated={}\n  \
                 offsets: class=+0x{:X} name=+0x{:X} outer=+0x{:X} super=+0x{:X} children=+0x{:X} child_props=+0x{:X}\n  \
                          ffield: class=+0x{:X} next=+0x{:X} name=+0x{:X}  fprop: off=+0x{:X} size=+0x{:X}  ufield_next=+0x{:X}  ufunc: flags=+0x{:X} func=+0x{:X}",
                w.pid,
                w.guobject_array,
                w.fname_pool,
                w.fname_pool_chunks_offset,
                w.fuobject_item_stride,
                w.layout_validated,
                w.offsets.uobject_class_private,
                w.offsets.uobject_name_private,
                w.offsets.uobject_outer_private,
                w.offsets.ustruct_super_struct,
                w.offsets.ustruct_children,
                w.offsets.ustruct_child_properties,
                w.offsets.ffield_class_private,
                w.offsets.ffield_next,
                w.offsets.ffield_name_private,
                w.offsets.fproperty_offset_internal,
                w.offsets.fproperty_element_size,
                w.offsets.ufield_next,
                w.offsets.ufunction_flags,
                w.offsets.ufunction_func
            );
            // Drain whatever logs the DLL has buffered so we can see locate
            // diagnostics without going through `Ue5Session`'s validated
            // gate.
            client.set_log_level(LogLevel::Trace)?;
            let lines = client.drain_log(256)?;
            println!("--- DLL log ring ({} lines) ---", lines.len());
            for line in lines {
                println!("{line}");
            }
            client.disconnect()?;
        }
    }
    Ok(())
}
