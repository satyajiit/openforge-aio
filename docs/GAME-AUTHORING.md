# Adding a Game to OpenForge

This walkthrough covers discovering your first memory address and shipping it as a declarative signature in your game's crate.

## Prerequisites

- Windows 10/11 with admin rights.
- Rust 1.95+ stable (`rustup default stable`).
- Node 24 LTS + pnpm 10.
- A purchased + installed copy of the game.
- A disassembler of your choice (Cheat Engine, x64dbg, IDA Free, Ghidra). OpenForge's `extract-aob` synthesizes the AOB; the disassembler is only used to find the writing instruction's bytes.

## Game crate shape

After `openforge-cli new-game --id <slug> --name "Your Game" --process YourGame.exe`:

```
crates/games/<slug>/
├── Cargo.toml              package = openforge-game-<slug>
├── manifest.toml           game metadata (process names, version, sort order)
├── build.rs                reads manifest + signatures, emits constants
├── src/lib.rs              YourGameGame struct + register_game!()
├── signatures/             one TOML per cheat (initially empty)
└── assets/icon.png         64×64 monochrome icon
```

Each file in `signatures/` is parsed at compile time and embedded into the binary as a `DeclFeatureSrc`. The runtime instantiates a `DeclarativeFeature` for each one — no per-cheat Rust code needed.

## Signature TOML reference

```toml
[meta]
feature              = "gold"
display_name         = "Gold"
description          = "Primary currency"
tier                 = "currency"          # used to group features in tabs
discovered_in_version = "1.0.0"
verified_versions    = ["1.0.0"]
discovered_on        = "2026-06-01"

[value]
type    = "i32"                            # bool | u8 | u16 | u32 | u64 | i8 | i16 | i32 | i64 | f32 | f64
min     = 0
max     = 999999999
display = "decimal"                        # decimal | hex | float | bool | thousands
endian  = "little"

[write]
strategy = "one_shot"                      # one_shot | freeze | code_patch

# When strategy = "freeze":
# interval_ms = 250

# When strategy = "code_patch":
# original_bytes = "FF 88 24 02 00 00"
# patched_bytes  = "90 90 90 90 90 90"

[locator]
pattern = "48 8B 05 ?? ?? ?? ?? 89 88 ?? ?? ?? ?? 48 85 C0"
resolve = { kind = "rip_relative", instruction_offset = 3, operand_size = 4 }
# Optionally override the module to scan in:
# module = "MyEngine.dll"

# Optional: walk a pointer chain after the locator resolves.
# [[pointer_chain.hops]]
# deref  = true
# offset = 0x40
```

### Resolve modes

| `kind` | Computation |
|--------|-------------|
| `direct` | `target = match_address` |
| `offset` | `target = match_address + delta` (signed) |
| `rip_relative` | `target = (match_address + instruction_offset + operand_size) + sign_extend(disp)` where `disp` is read from `match_address + instruction_offset` |

### Pointer chains

For values stored at runtime through one or more pointers (common in Unreal-engine games), append `[[pointer_chain.hops]]` entries to the signature. Each hop applies in order: `if deref: addr = read_ptr(addr); addr += offset`.

## Discovery workflow

The CLI flow turns "I see 12,345 gold on the HUD" into a committed signature.

```powershell
# 1. Sanity-check the environment.
cargo run -p openforge-discover -- --game <slug> doctor

# 2. Launch the game, then attach.
cargo run -p openforge-discover -- --game <slug> attach

# 3. First scan for the value you can see (act as if Cheat Engine):
cargo run -p openforge-discover -- --game <slug> scan --feature gold --type i32 --value 12345 --name "Gold"

# 4. Earn or spend some gold. Then narrow:
cargo run -p openforge-discover -- --game <slug> narrow --feature gold --equal 14200

# Repeat narrow with --equal, --changed, --increased, etc. until one candidate remains.
# If stuck at >1 candidate, write a unique value to each via Cheat Engine and pick the one the HUD reflects:
cargo run -p openforge-discover -- --game <slug> pick --feature gold --address 0xDEADBEEF

# 5. Find the writing instruction.
#    Easiest: in Cheat Engine, right-click your address → "Find out what writes to this address" →
#    perform the in-game action → CE shows the instruction's RIP. Copy ~16 bytes around it from
#    CE's memory viewer (right-click → copy → bytes).

# 6. Capture the bytes:
cargo run -p openforge-discover -- --game <slug> capture --feature gold --rip 0x7FF612345678 --bytes "48 8B 05 d2 e5 7f 00 89 88 c0 00 00 00 48 85 c0"

# 7. Synthesize the AOB. The tool wildcards immediates + RIP-relative displacements and
#    verifies the synthesized pattern matches uniquely inside .text.
cargo run -p openforge-discover -- --game <slug> extract-aob --feature gold 0x7FF612345678

# 8. Emit the signature TOML.
cargo run -p openforge-discover -- --game <slug> emit gold

# 9. Build + sanity check.
cargo build -p openforge-game-<slug>
cargo run -p openforge-discover -- --game <slug> verify
```

After verify passes, the trainer picks the new feature up automatically on next launch.

## When the AOB isn't unique

`extract-aob` widens the window (more instructions included) up to 5 attempts. If it still can't make it unique:

1. Pass `--conservative` to start with a wider initial window.
2. Edit the emitted TOML's `[locator]` pattern by hand — include more surrounding instructions or drop a few wildcards.
3. As a last resort, fall back to a `module_offset`-style locator (a future schema extension). Until then, find a different anchor instruction — usually a nearby `mov reg, [rip+disp32]` that loads the pointer your writer uses.

## Tier conventions

Cheats group into tabs by `meta.tier`. Conventional tier names: `currency`, `combat`, `progression`, `movement`, `combat-depth`. Use any string; the UI sorts alphabetically.

## Risky cheats

Cheats whose `tier = "progression"` route writes through a confirmation dialog in the UI (story flags can soft-lock save files). If your cheat touches progression state, set the tier accordingly and document it in `meta.description`.

## Code patches (god mode etc.)

For cheats that NOP an instruction rather than write a value:

```toml
[meta]
feature      = "god_mode"
display_name = "God Mode"
tier         = "combat"
# ... discovered_on etc.

[write]
strategy       = "code_patch"
original_bytes = "89 88 C0 00 00 00"      # the original mov instruction
patched_bytes  = "90 90 90 90 90 90"      # NOP it

[locator]
pattern = "..."
resolve = { kind = "direct" }              # anchor on the instruction itself
```

No `[value]` section is needed — the UI shows a Switch (applied / reverted).

## Verifying against multiple builds

When a game patches, run:

```powershell
cargo run -p openforge-discover -- --game <slug> verify --version <new-version> --update-verified
```

Each signature whose AOB still matches uniquely gets the new version added to `meta.verified_versions`. Signatures that broke need a `refresh`:

```powershell
cargo run -p openforge-discover -- --game <slug> refresh <feature>
```

## Troubleshooting

| Symptom | Likely cause | Fix |
|---------|--------------|-----|
| `scan` reports 0 candidates | wrong type or wrong value | try a different `--type`, or try `--unknown-initial` |
| `narrow` stays at the same count | predicate didn't actually exclude anything | try a different predicate (`--changed` after acting in-game) |
| `extract-aob` says "0 matches" | wildcarded a byte you shouldn't have | run with `--conservative`; widen window |
| `extract-aob` says "N matches" | pattern not unique | widen window (more instructions); pass `--conservative` |
| Trainer shows the feature but Apply does nothing | resolve gives the wrong address | check the disassembly: are you anchored on the right instruction? does the field need a `pointer_chain`? |
| Trainer crashes the game | write strategy mismatched the value type | double-check `[value].type` matches what the game expects |
