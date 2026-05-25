# UE5 Cheat Cookbook

**Audience:** OpenForge contributors authoring signatures for UE5 games.
**Prerequisite:** the per-game injected DLL is running (`openforge-batman-lot-dll` for LotDK) and a `Ue5Session` is open on the named pipe. The DLL serves reflection ops (`WalkObjects`, `WalkProperties`, `WalkFunctions`) and the memory ops the recipes here invoke.
**Cross-refs:** [Game Authoring](./GAME-AUTHORING.md) · [Contributing](./CONTRIBUTING.md)

This document is the **practical playbook** for using the UE5 reflection engine to author signatures for the common cheat categories. Every recipe follows the same four-step structure: **Find → Validate → Author → Verify**.

---

## The universal UE5 discovery workflow

Before any specific recipe, the workflow is always:

```
1. attach to game
   $ openforge-discover --game <id> attach

2. find the property OR function by name regex
   $ openforge-discover --game <id> ue5-find-prop  --name <regex>
   $ openforge-discover --game <id> ue5-find-ufunc --name <regex>

3. (optional) dump the full class to see neighboring fields
   $ openforge-discover --game <id> ue5-dump-class --name "<FullyQualifiedClass>"

4. emit the signature TOML
   $ openforge-discover --game <id> ue5-find-prop  --name <regex> --emit-as <feature_id>
   # or
   $ openforge-discover --game <id> ue5-find-ufunc --name <regex> --emit-as <feature_id>

5. rebuild + verify
   $ cargo build -p openforge-game-<id>
   $ openforge-discover --game <id> verify --feature <feature_id>
```

A typical UE5 cheat goes from "I want it" to "shipped signature" in **under 10 minutes**.

---

## Recipe 1 — God Mode (preferred: property flag)

**Effort:** ~3 minutes. Works if the game exposes a flag.

### Find

```bash
openforge-discover --game batman ue5-find-prop \
    --name "bGod|bImmortal|bInvuln|bInvincible|bDamageImmune" \
    --type bool
```

Sample output:

```
== class BatCharacter (instance 0x108AB1240) ==
  bIsImmortal      bool   offset +0x0428   current = false
== class BatVehicle (instance 0x108D52040) ==
  bDamageImmune    bool   offset +0x0314   current = false
```

### Validate

- Read current value with `--read-values`. Should be `false` for the player pawn during normal play.
- Toggle in-game via cheat console if available; re-read; confirm the offset changes.

### Author

```toml
# crates/games/<id>/signatures/god_mode.toml
[meta]
feature      = "god_mode"
display_name = "God Mode"
tagline      = "He is the night."
icon         = "shield"
tier         = "combat"

[value]
type = "bool"

[write]
strategy = "one_shot"

[heap_scan]
# Anchor: BatCharacter vtable in the main module. Walk the heap looking for
# a u64 in module range, validate via the property's known offset value.
value_type   = "u64"
value        = 0x140A12340       # vtable for BatCharacter
field_offset = 0x428             # property Offset_Internal

[[heap_scan.validators]]
field_type      = "u64"
relative_offset = -0x428         # back to the vtable position
min             = 0x140000000
max             = 0x180000000

[control]
kind = "switch"
```

### Verify

```bash
openforge-discover --game <id> verify --feature god_mode
```

---

## Recipe 2 — God Mode (fallback: NOP TakeDamage)

**Effort:** ~4 minutes. Works on every UE5 game even without a flag.

### Find

```bash
openforge-discover --game batman ue5-find-ufunc \
    --name "TakeDamage|ApplyDamage" \
    --native-only
```

Sample output:

```
== class BatCharacter (instance 0x108AB1240) ==
  TakeDamage   0x140A5B7C0  NATIVE  void(float, FDamageEvent&, AController*, AActor*)
```

### Validate

- Read the first 16 bytes at `0x140A5B7C0` via `inspect --disasm`. Should see a standard x64 prologue (`40 53 48 83 EC ?? ...` or `48 89 5C 24 08 ...`).
- Confirm the function is `void`-returning (UFunction layout reports `ReturnValueOffset`).

### Author

`ue5-find-ufunc --emit-as god_mode --strategy code_patch` writes this automatically:

```toml
[meta]
feature      = "god_mode"
display_name = "God Mode"
tagline      = "Damage refuses to take."
tier         = "combat"

[locator]
# Synthesized AOB — 16 bytes from TakeDamage entry, verified unique in .text.
pattern = "40 53 48 83 EC 20 48 8B D9 ?? ?? ?? ?? ?? ?? ??"
resolve = { kind = "direct" }

[write]
strategy       = "code_patch"
original_bytes = "40 53 48 83 EC 20"      # first 6 bytes of prologue
patched_bytes  = "C3 90 90 90 90 90"      # RET; 5x NOP

[control]
kind = "switch"
```

### Verify

```bash
openforge-discover --game <id> verify --feature god_mode
```

### Gotchas

- **Don't** patch a Blueprint-trampoline UFunction's `Func` pointer — it points at `ProcessInternal` (engine-wide BP interpreter). `--native-only` in `ue5-find-ufunc` prevents this.
- For non-`void` UFunctions: `RET` returns garbage in the return registers. Don't use this pattern; instead, NOP the damage-application *inside* the function (use `watch-write` to find the writer).

---

## Recipe 3 — Currency (chips, gold bricks, skill bricks, etc.)

**Effort:** ~2 minutes per currency.

### Find

```bash
openforge-discover --game batman ue5-find-prop \
    --name "chip|brick|stud|coin|currency|count" \
    --class "Save|Profile|Player" \
    --type i32
```

Sample:

```
== class BatSaveGame (instance 0x12345A000) ==
  WayneChipsCount      i32   +0x0148   = 3
  SkillBricksCount     i32   +0x014C   = 7
  GoldBrickCount       i32   +0x0150   = 142
== class BatPlayerState (instance 0x104C72C40) ==
  CurrentStuds         i32   +0x0038   = 19_842
```

### Validate

- For persistent currencies (chips, skill bricks): the value should match the HUD.
- For session currencies (studs): same.
- Cross-check by spending/earning one unit; re-run with `--read-values`; confirm the field changed by ±1.

### Author

```toml
# crates/games/<id>/signatures/wayne_chips.toml
[meta]
feature      = "wayne_chips"
display_name = "Wayne Chips"
tagline      = "Bruce can afford anything."
icon         = "coins"
tier         = "currency"

[value]
type = "i32"
min  = 0
max  = 999_999_999

[write]
strategy = "one_shot"

[heap_scan]
value_type   = "u64"
value        = 0x140B22480              # vtable for BatSaveGame (from ue5-find-prop output)
field_offset = 0x148                    # WayneChipsCount.Offset_Internal

[[heap_scan.validators]]
field_type      = "u64"
relative_offset = -0x148
min             = 0x140000000
max             = 0x180000000

[[heap_scan.validators]]
# Sanity: the value itself must be a non-negative int that fits a reasonable cap
field_type      = "i32"
relative_offset = 0
min             = 0
max             = 999_999_999

[control]
kind = "input"
presets = [1000, 10_000, 100_000, 999_999_999]
```

### Tip — no max cap visible

If the game doesn't show a max cap (chips don't have one), we use the **vtable as fingerprint** instead of a max-value sentinel. The vtable lives in `.text` so it's process-stable; the `relative_offset = -<field_offset>` validator confirms we're inside the right object.

---

## Recipe 4 — Health (current + max)

**Effort:** ~3 minutes for the pair.

### Find

```bash
openforge-discover --game batman ue5-find-prop \
    --name "^health$|^hp$|^hitpoints$|currenthealth|maxhealth|maxhp" \
    --type f32
```

Sample:

```
== class BatCharacter (instance 0x108AB1240) ==
  Health        f32   +0x0240   = 100.0
  MaxHealth     f32   +0x0244   = 100.0
```

### Author (current health, freeze-via-set semantics)

```toml
[meta]
feature      = "lock_health"
display_name = "Lock Health"
tagline      = "Bring the cowl. Skip the bruises."
icon         = "heart"
tier         = "combat"

[value]
type = "f32"

[write]
strategy = "one_shot"

[heap_scan]
value_type   = "u64"
value        = 0x140A12340       # BatCharacter vtable
field_offset = 0x240             # Health.Offset_Internal

[[heap_scan.validators]]
field_type      = "u64"
relative_offset = -0x240
min             = 0x140000000
max             = 0x180000000

[control]
kind = "input"
presets = [100, 500, 1000, 9999]
```

### Author (max health)

Same template, `field_offset = 0x244`, `feature = "max_health"`, `presets = [100, 500, 1000, 9999]`.

### Gotcha — runtime Pawn replacement

UE5 sometimes destroys and re-spawns the player pawn (on death, level transition, cutscene). The `BatCharacter` instance address changes. Our `heap_scan` re-resolves on attach + `quick_check` validates on cache hit, so this is handled — but a Lock-Health that's *currently applied* across a pawn respawn will land on a dead object's memory. Solution: pair with code-patch god mode (Recipe 1/2) which doesn't depend on object address stability.

---

## Recipe 5 — Damage Multiplier

**Effort:** ~3 minutes.

### Find

```bash
openforge-discover --game batman ue5-find-prop \
    --name "damage.*mult|damage.*scale|incoming.*damage|outgoing.*damage" \
    --type f32
```

Sample:

```
== class BatCharacter (instance 0x108AB1240) ==
  IncomingDamageScale  f32   +0x02F0   = 1.0
  OutgoingDamageScale  f32   +0x02F4   = 1.0
```

### Author

Two cheats — one feature each:

```toml
# damage_taken_multiplier.toml — set to 0.0 for immune, < 1.0 for resistance
[meta]
feature      = "damage_taken_multiplier"
display_name = "Damage Taken Multiplier"
[heap_scan]
value_type   = "u64"
value        = 0x140A12340
field_offset = 0x2F0
# ... validators as above ...
[value]
type = "f32"
min  = 0.0
max  = 100.0
[control]
kind = "input"
presets = [0.0, 0.5, 1.0]
```

```toml
# damage_dealt_multiplier.toml — set to 999.0 for one-hit kills
[meta]
feature      = "damage_dealt_multiplier"
display_name = "Damage Dealt Multiplier"
[heap_scan]
value_type   = "u64"
value        = 0x140A12340
field_offset = 0x2F4
# ... validators ...
[value]
type = "f32"
[control]
kind = "input"
presets = [1.0, 5.0, 100.0, 999.0]
```

---

## Recipe 6 — Movement (speed, jump, glide)

**Effort:** ~5 minutes for the trio.

### Find

```bash
openforge-discover --game batman ue5-find-prop \
    --name "maxwalkspeed|maxrunspeed|jumpzvelocity|gravity|glide.*speed" \
    --type f32
```

These typically live on `CharacterMovementComponent`, not the pawn itself. The output groups by class:

```
== class BatCharacterMovement (instance 0x10A2B40) ==
  MaxWalkSpeed     f32   +0x0290   = 600.0
  MaxRunSpeed      f32   +0x0294   = 900.0
  JumpZVelocity    f32   +0x0298   = 540.0
  GravityScale     f32   +0x0260   = 1.0
```

### Author (movement speed)

```toml
[meta]
feature      = "max_walk_speed"
display_name = "Walk Speed"
tagline      = "Cape gets aerodynamic."
icon         = "wind"
tier         = "movement"

[value]
type = "f32"

[write]
strategy = "one_shot"

[heap_scan]
value_type   = "u64"
value        = 0x140C5A100       # CharacterMovement vtable
field_offset = 0x290

[[heap_scan.validators]]
field_type      = "u64"
relative_offset = -0x290
min             = 0x140000000
max             = 0x180000000

[control]
kind = "input"
presets = [600, 1200, 2000, 5000]
```

### Gotcha — networked games

UE5 networking validates speed bounds server-side. Single-player titles (LEGO Batman fits) don't care. Don't ship movement cheats for online multiplayer games (forbidden by CONTRIBUTING.md anyway).

---

## Recipe 7 — Bitfield unlocks (characters, vehicles, story flags)

**Effort:** ~10 minutes. Riskier — bitfield writes can break save consistency.

### Find

```bash
openforge-discover --game batman ue5-find-prop \
    --name "unlock|owned|collected|completed"
```

Bitfields usually appear as `TArray<uint8>` or `TBitArray`:

```
== class BatSaveGame (instance 0x12345A000) ==
  CharacterUnlocks   TArray<uint8>  +0x0500   (32 elements = 256 bits)
  VehicleUnlocks     TArray<uint8>  +0x0540   (12 elements = 96 bits)
  StoryFlags         TArray<uint8>  +0x0560   (?? elements)
```

### Author

Bitfields **don't** use the simple one_shot pattern. They need a writer that fills the array with 0xFF. Two options:

**(a) Custom Rust Feature** in `crates/games/<id>/src/features/character_unlocks.rs` (escape-hatch from declarative):

```rust
impl Feature for CharacterUnlocks {
    fn write(&self, ctx: &dyn Ctx, addr: usize, _v: Value) -> RuntimeResult<()> {
        // Read the TArray header: { T* Data; int32 Num; int32 Max }
        let data_ptr = read_u64(ctx, addr)?;
        let count    = read_i32(ctx, addr + 8)? as usize;
        let buf = vec![0xFFu8; count];
        ctx.write_bytes(data_ptr as usize, &buf)?;
        Ok(())
    }
}
```

**(b) Multi-write declarative extension** (post-v0.4 — not yet supported):

```toml
# Future syntax. Not implemented in v0.1.
[write]
strategy = "array_fill"
array_kind = "uint8"
fill_byte  = 0xFF
```

### Gotchas

- **Save corruption.** Story flags often encode mission state in specific bits; setting all to 1 can break cutscene triggers or lock the player out of progression. Risky writes route through a Dialog confirmation ("Type APPLY") and offer to back up the save folder first (planned for v0.4).
- **TArray relocation.** UE5's `TArray<T>` is a `(ptr, num, max)` struct. The `ptr` can change on resize. For long-held cheats, prefer one_shot writes that read the pointer fresh each time, not freeze loops that cache it.

---

## Recipe 8 — Inventory (ammo, gadget counts)

**Effort:** ~3 minutes.

### Find

```bash
openforge-discover --game batman ue5-find-prop \
    --name "ammo|count|charges" \
    --class ".*Inventory.*|.*Gadget.*|.*Weapon.*" \
    --type i32
```

### Author

Same one_shot heap_scan pattern as currencies. The anchor is the inventory component's vtable.

---

## Cross-cutting tips

### Anchor choice — vtable beats max-cap-value when no cap exists

- **If the game has a `MaxXxx` companion field** (`MaxHealth`, `MaxStuds`): use the cap value as the `heap_scan.value` (e.g. `9_999_999` for studs). Simplest fingerprint.
- **If no cap exists** (chips, skill bricks): use the **UClass vtable address** as the `heap_scan.value`. Vtables live in `.text` (the main module), are process-stable for the lifetime of the game, and are unique per class.

The vtable is `Read the first u64 of any live instance of the target class`. The `ue5-find-prop --emit-as` path does this automatically.

### Validator chain — always include the module-range check

Every `heap_scan` should include a validator that confirms the vtable (at `relative_offset = -<field_offset>`) is in the game's main-module address range. This kills 99% of false positives from random heap values.

```toml
[[heap_scan.validators]]
field_type      = "u64"
relative_offset = -<field_offset>
min             = 0x140000000      # typical main module base for x64 PIE-disabled
max             = 0x180000000      # 1 GB above; covers any realistic module size
```

(The actual module range for your game is `target.main().base` to `target.main().base + target.main().size`; the trainer can substitute exact values at build time, but the broad `[0x140000000, 0x180000000]` range works for all standard Windows x64 builds.)

### When reflection finds zero hits

- Try a wider regex: `--name "(?i)chip"` (case-insensitive substring).
- Game may strip reflection metadata in shipping builds (extremely rare; UE5's reflection is required by Blueprint VM).
- Check the FUObjectItem stride: `attach --verbose` prints the detected stride; if it's wrong, manual probing has to disambiguate.

### Anti-tamper / detection

xmodhub's article on this game noted UE5's crash handler "flags external debuggers". Our reflection scans use `OpenProcess` + `ReadProcessMemory` only — same syscalls as Task Manager. We never call `DebugActiveProcess` or `SetThreadContext` (those are debugger-territory and are what UE5's handler detects). If the game crashes mid-scan, file an issue with the call site — it's the first thing to investigate.

### Verification habits

Always run `verify --feature <id>` after authoring. The signature must resolve on a freshly-launched game. If it works once but fails after a level transition, suspect:
- Object reallocation (heap_scan should re-resolve — confirm `quick_check` is the failure point)
- Vtable changed (game updated; re-derive from `ue5-find-prop`)
- Module base shifted (ASLR-ish? UE5 normally pins base addresses, but exotic launchers can move them)

---

## Recipe-to-cheat mapping for LEGO Batman: LotDK (v0.2 + v0.3)

| Cheat | Recipe | Status |
|---|---|---|
| Lock Studs | Manual code_patch (pre-reflection) | ✅ v0.1 |
| Set Studs | Manual heap_scan via max-cap (pre-reflection) | ✅ v0.1 |
| Wayne Chips | Recipe 3 | 🔲 v0.2 |
| Skill Bricks | Recipe 3 | 🔲 v0.2 |
| Gold Bricks | Recipe 3 | 🔲 v0.3 |
| Current Health | Recipe 4 | 🔲 v0.2 |
| Max Health | Recipe 4 | 🔲 v0.2 |
| God Mode | Recipe 1 or 2 | 🔲 v0.2 |
| Damage Taken Mult | Recipe 5 | 🔲 v0.2 |
| Damage Dealt Mult | Recipe 5 | 🔲 v0.3 |
| Walk Speed | Recipe 6 | 🔲 v0.3 |
| Jump Height | Recipe 6 | 🔲 v0.3 |
| Glide Speed | Recipe 6 | 🔲 v0.3 |
| Stud Magnet Range | Recipe 6 (find by `magnet.*range|pickup.*radius`) | 🔲 v0.3 |
| Infinite Batarangs | Recipe 8 | 🔲 v0.3 |
| Character Unlocks | Recipe 7 | 🔲 v0.4 (risky — save backup required) |
| Vehicle Unlocks | Recipe 7 | 🔲 v0.4 |
| Story Flags | Recipe 7 | 🔲 v0.4 |
| Minikit Counts | Recipe 3 | 🔲 v0.4 |

---

## Future-proofing for other UE5 games

The recipes above apply to **every UE5 title** with no game-specific changes:

| Game (UE5) | Status | Notes |
|---|---|---|
| LEGO Batman: LotDK | Current | First shipped game. Reference implementation. |
| Future TT Games LEGO titles | Likely | Same engine vendor (TT Games on UE5); recipes 1-8 should drop in unchanged. |
| Other UE5 single-player titles | Likely | Property/UFunction names vary, but the engine layout is identical. Adapt regexes per game. |
| UE4 games (older LEGOs, etc.) | TBD | UE4 used `UProperty` (UObject-derived) instead of `FProperty`. Would need a Phase 6 — UProperty walker. ~150 LOC. |

Adding a new UE5 game:

1. Follow [GAME-AUTHORING.md](./GAME-AUTHORING.md) to scaffold the crate.
2. Open the game, attach via `openforge-discover attach`.
3. Run `ue5-find-object` to confirm reflection works (expect to see the game's PlayerController / Character classes in the output).
4. For each desired cheat, follow the matching recipe above.
5. Open a PR with the signature TOMLs + manifest update.

Total time per game with a normal feature set (currencies + health + god mode + movement): **~30 minutes**.

---

## When reflection isn't enough

Some cases still need manual work even after reflection:

- **Encrypted properties.** Rare. Some titles XOR sensitive fields with a runtime key. Detect when the read value at a known offset isn't sane; locate the decryption routine via writer references.
- **Non-UObject-stored state.** Lua-scripted games (e.g. some indie titles using a UE5 wrapper) may hold game state in a Lua VM, not UObjects. Reflection won't see it; fall back to `scan + narrow`.
- **Functions whose bodies are JIT'd or hot-patched at runtime.** Very rare. Code-patch strategy won't survive next launch. Use property write instead if possible.
- **Save-game manipulation when the game is closed.** Out of scope for v0.x — that's a save editor, not a trainer.

For these, fall back to the manual `scan + narrow + watch-write + find-callers + extract-aob + emit` pipeline. The reflection engine is a fast path, not the only path.
