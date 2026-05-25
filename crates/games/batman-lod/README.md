<div align="center">

# 🦇 LEGO Batman: Legacy of the Dark Knight

**OpenForge support module &mdash; 20 features, UE5 reflection, stable on build 1.0.0.1.**

[![Status](https://img.shields.io/badge/status-stable-brightgreen)](#)
[![Features](https://img.shields.io/badge/features-20-blue)](#features)
[![Engine](https://img.shields.io/badge/engine-Unreal%205-313131?logo=unrealengine)](#)
[![Build](https://img.shields.io/badge/supported%20build-1.0.0.1-orange)](#)
[![Approach](https://img.shields.io/badge/approach-UE5%20reflection-purple)](#how-it-works)
[![License](https://img.shields.io/badge/license-MIT-blue)](../../../LICENSE)

[← Back to OpenForge root](../../../README.md)

</div>

---

## What this is

This crate (`openforge-game-batman-lod`) is the LEGO Batman: Legacy of the Dark Knight support module for [OpenForge](../../../README.md). It contains:

- A **manifest** describing the game (process names, supported builds, the DLL to inject).
- A folder of **TOML signatures** &mdash; one per cheat &mdash; that the OpenForge declarative engine interprets at runtime.
- The companion **injected DLL** lives in [`crates/batman-lod-dll/`](../../batman-lod-dll/) and exposes UE5 reflection over a named pipe.

No game-specific Rust glue lives in this crate beyond a tiny `BatmanGame` registration shim. Every cheat is configuration. Editing or adding a feature is a TOML change, not a recompile of the engine. Walk Gotham like you own the dev-menu.

---

## ✨ Features

20 features across six categories. All resolved through the engine's own reflection &mdash; no fragile AOB signatures, no hand-edited offsets.

### 💰 Currency

| ID | Display | Strategy | Description |
|---|---|---|---|
| `set_studs` | Set Studs | `one_shot` | Writes a custom stud balance via TT's currency reflection; survives saves. |
| `set_chips` | Set WayneTech Chips | `one_shot` | Same idea for the WayneTechChips wallet. |
| `stud_multiplier` | Stud Multiplier | `one_shot` | Pins `StudMultiplierMin/Max` together (presets 1×, 2×, 5×, 10×, 100×). |

### 🛡️ Combat

| ID | Display | Strategy | Description |
|---|---|---|---|
| `lock_health` | Infinite Health | `freeze` 100 ms | Freezes `Health.CurrentValue` on the player's `HealthAttributeSet` (GAS-aware). |
| `unlimited_focus` | Unlimited Focus | `freeze` 100 ms | Pins the combo meter (`FocusAttributeSet.Focus.CurrentValue`) at 9999. |
| `one_hit_kill` | One-Hit Kill | `freeze_for_matching` 200 ms | Freezes enemy `Health` at 1.0; filters out player/allies/vehicles/NPCs. |
| `freeze_all_enemies` | Freeze All Enemies | `freeze_for_matching` 200 ms | Sets `CustomTimeDilation = 0.0001` on every live `Character`. 10 000× slowdown. |

### 🏃 Movement

| ID | Display | Strategy | Description |
|---|---|---|---|
| `mod_fly` | Fly Mode | `freeze` 16 ms | Pins `MovementMode = MOVE_Flying`; WASD glide, no gravity. |
| `mod_low_gravity` | Low Gravity | `one_shot` | Adjusts `GravityScale` (presets 1.0 → 0.5 → 0.15 → 0.0; negatives for upward float). |
| `mod_super_jump` | Super Jump | `one_shot` | Adjusts `JumpZVelocity` (presets 420 → 1000 → 2000 → 5000 cm/s). |
| `mod_super_speed` | Super Speed | `one_shot` | Adjusts `MaxWalkSpeed` (presets 600 → 1000 → 2500 → 5000 cm/s). |

### 📍 Teleportation

| ID | Display | Strategy | Description |
|---|---|---|---|
| `teleport_x` | Teleport X | `one_shot` | Manual X-coordinate adjustment via `RootComponent.RelativeLocation`. |
| `teleport_y` | Teleport Y | `one_shot` | Manual Y-coordinate adjustment. |
| `teleport_z` | Teleport Z | `one_shot` | Manual Z (height) adjustment. |
| `teleport_to_waypoint` | Teleport to Waypoint | UFunction call | Finds the `CustomMapPinActor` and warps the player pawn via `K2_TeleportTo`. |

### 🏆 Progression

| ID | Display | Strategy | Description |
|---|---|---|---|
| `unlock_all_skills` | Unlock All Skills | progress-tag write | Iterates `PROG_Skills`, unlocks all 30 combat / exploration nodes via `TtGameProgressStatics.SetGameProgressValue`. |
| `unlock_all_fast_travel` | Unlock All Fast Travel | progress-tag write | Activates all 65 `PROG_FastTravelUnlock` tags. Fast-travel from anywhere &mdash; no map-marker dance. |

### 🌆 World spice

| ID | Display | Strategy | Description |
|---|---|---|---|
| `mod_fast_peds` | Fast Pedestrians | `one_shot` | Scales `CrowdStatelessWanderSettings.WalkSpeedMetresPerSecond` (1.34 → 20 m/s). |
| `mod_fast_trains` | Bullet Trains | `freeze_for_matching` 250 ms | Freezes `TrackSplineComponent.MoveSpeed` at 2500 cm/s (5× stock). ~36 splines in HUB_GothamCity. |
| `mod_demolition_derby` | Demolition Derby | `freeze` 200 ms | Cranks `MassTrafficSettings` chaos: `TurnSpeedScale` + four variance fields (presets 0.6 → 1.0 → 2.0). |

---

## 📋 Requirements

- **Game**: LEGO Batman: Legacy of the Dark Knight, Steam build **1.0.0.1**.
- **OS**: Windows 10+ (64-bit).
- **Privileges**: Administrator (for game-process memory access).
- **DLL**: `batman_lod_dll.dll` &mdash; built automatically as part of `openforge-bundle`. No separate install step.
- **Storage**: ~30 MB for the trainer + DLL.

Verified against `LEGOBatmanLotDK-Win64-Shipping.exe` and `LEGOBatmanLotDK.exe`.

---

## 🔬 How it works

When you hit Attach in OpenForge, the host:

1. Finds the running game process (sub-millisecond toolhelp32 scan).
2. Resolves and injects `batman_lod_dll.dll` into the game's address space.
3. Performs a Hello handshake over a per-PID named pipe (`\\.\pipe\openforge-<pid>`).
4. The DLL walks `GUObjectArray` in-process and calls the engine's own `FName::ToString` (under SEH guard) to name every live UObject &mdash; **no external chunk-walking, no FNamePool decoding heuristics**.
5. Each feature TOML in [`signatures/`](signatures/) declares the UE5 class path and property name it needs. The runtime resolves those through the DLL, caches the offsets per-session, and applies the appropriate write (`one_shot`, `freeze`, `code_patch`, or UFunction call).
6. On detach (manual or game-exit), the DLL auto-restores every code patch it applied. Pipe disconnect is the trigger.

The known engine RVAs (`FName::ToString = 0x01138230`, `GUObjectArray = 0x0B65C490`, `UObject::ProcessEvent = 0x014AB884`) are baked into `crates/batman-lod-dll/src/lotdk.rs` and verified against the build's UE4SS reference dump. Re-attach after the first session is sub-100 ms thanks to the per-session resolver cache.

---

## 🛠️ Build & test

From the workspace root:

```powershell
# Build just this game module
cargo build -p openforge-game-batman-lod

# Build the injected DLL
cargo build -p openforge-batman-lod-dll --release

# Verify the signature TOMLs parse (CI gate)
cargo run -p openforge-cli -- verify-registry

# Full bundle (DLL + game module + everything else)
cargo build -p openforge-bundle --release

# Test
cargo test -p openforge-game-batman-lod
```

To live-test cheats, run the desktop app from `crates/app/` and Attach to a running game (see the [root README](../../../README.md#-build-from-source)).

---

## ⚠️ Disclaimers (read me)

- **Build-specific.** RVAs and offsets are pinned to Steam build **1.0.0.1**. A patch can move them; if the trainer fails to attach after a game update, the fix is a contributor PR (or an issue with the new build number).
- **Single-player only.** LEGO Batman: LotDK is offline single-player by design. Don't use the trainer with any future online mode if one ships.
- **Back up saves before progression cheats.** `unlock_all_skills` and `unlock_all_fast_travel` write through the engine's own progression system, but anything that touches save state deserves a backup first.
- **Mid-cutscene caution.** `one_hit_kill`, `demolition_derby`, and `freeze_all_enemies` can disrupt scripted sequences. If a scene soft-locks, toggle the feature off and reload the last checkpoint.
- **TT-specific FPV pitfall**: don't toggle the engine's `bCanBeDamaged` flag &mdash; it triggers TT's first-person-view cursor-lock and ruins your day. We freeze through GAS attributes instead. (If you're authoring a new combat feature, this is the gotcha.)
- **Lowres / Proxy vehicle pitfall**: calling `Possess()` on `*_Lowres_*` or `*_Proxy_*` traffic vehicles crashes the game. Stick to Highres variants if you're scripting around traffic.
- **Not affiliated** with TT Games, Warner Bros. Interactive, Warner Bros. Discovery, DC, or LEGO. All trademarks belong to their respective owners.

---

## 🧩 Contributing a new feature

The full discovery walkthrough lives in **[docs/GAME-AUTHORING.md](../../../docs/GAME-AUTHORING.md)**; the UE5-specific recipe book is in **[docs/UE5-CHEAT-COOKBOOK.md](../../../docs/UE5-CHEAT-COOKBOOK.md)**. Short version:

```powershell
# Scaffold the TOML
cargo run -p openforge-cli -- new-feature --game batman-lod --id <feature-id>

# Iterate against the running game with the discover CLI
cargo run -p openforge-discover -- --game batman-lod doctor
cargo run -p openforge-discover -- --game batman-lod attach
# ... scan / narrow / pick / extract-aob / emit ...

# Verify the signature parses + the registry stays green
cargo run -p openforge-cli -- verify-registry
```

Then run the desktop app and Attach to confirm the feature behaves before opening a PR.

---

## 📄 License

[MIT](../../../LICENSE) &mdash; same as the rest of OpenForge.
