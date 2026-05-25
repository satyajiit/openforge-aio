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
    /// Smoke test for `peds_fight_all`. Walks every live `Character`,
    /// filters out player/vehicles/CDOs/transients, and zeros 8 bytes at
    /// `+0xB28` (CurrentTeamTag FGameplayTag) on each. Optionally loops
    /// the writes for `--loop-seconds` to defeat BP tick re-writes (like
    /// the freeze_for_matching runtime would). Prints per-pass counts so
    /// we can tell whether NPCs are getting reset.
    PedsFightTest(PedsFightArgs),
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
    #[arg(long, default_value = "_Goon_C,_SWAT_,Arkham,TwoFace,Civilian,Population")]
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
