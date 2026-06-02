<div align="center">

# 🦇 LEGO Batman: Legacy of the Dark Knight

**OpenForge support module — 23 features, UE5 reflection, stable on build 1.0.0.1.**

[![Status](https://img.shields.io/badge/status-stable-brightgreen)](#)
[![Features](https://img.shields.io/badge/features-23-blue)](#-features)
[![Engine](https://img.shields.io/badge/engine-Unreal%205-313131?logo=unrealengine)](#)
[![Build](https://img.shields.io/badge/build-1.0.0.1-orange)](#)
[![License](https://img.shields.io/badge/license-MIT-blue)](../../../LICENSE)

[← OpenForge root](../../../README.md)

</div>

---

## What this is

The LEGO Batman: LotDK support module (`openforge-game-batman-lod`) for [OpenForge](../../../README.md): a `manifest.toml` declaring the game (`[engine] kind = "ue5"`, process names, the DLL to inject) and a folder of [signature files](signatures/) — one per cheat — that the declarative engine interprets at runtime. The companion injected DLL in [`crates/batman-lod-dll/`](../../batman-lod-dll/) exposes UE5 reflection over a named pipe. Every cheat is config; adding one is a signature change, not a recompile.

---

## ✨ Features

23 features, all resolved through the engine's own reflection — no fragile AOB signatures.

**💰 Currency** · Set Studs · Set WayneTech Chips · Stud Multiplier (1×–100×)

**🛡️ Combat** · Infinite Health (GAS-aware) · Unlimited Focus · One-Hit Kill · Freeze All Enemies

**🏃 Movement** · Fly Mode · Low Gravity · Super Jump · Super Speed

**📍 Teleport** · Teleport X/Y/Z · Teleport to Waypoint (`K2_TeleportTo`)

**🏆 Progression** · Unlock All Skills (30) · Unlock All Fast Travel (65 tags) · Unlock All Outfits

**🌆 World spice** · Fast Pedestrians · Bullet Trains · Demolition Derby · Goons Ignore You · NPC Dance Party

Per-feature strategy, offsets, and gotchas live in the signature TOMLs in [`signatures/`](signatures/).

---

## 📋 Requirements

- **Game**: LEGO Batman: LotDK, Steam build **1.0.0.1** (`LEGOBatmanLotDK-Win64-Shipping.exe`).
- **OS**: Windows 10+ (64-bit), **administrator** (game memory access).
- **DLL**: `batman_lod_dll.dll` — built with the bundle, no separate install.

---

## 🔬 How it works

On Attach, the host finds the process, injects `batman_lod_dll.dll`, and handshakes over a per-PID named pipe. The DLL walks `GUObjectArray` in-process and calls the engine's own `FName::ToString` (SEH-guarded) to name every live UObject — no chunk-walking, no FNamePool heuristics. Each signature declares a class path + property name; the runtime resolves it through the DLL, caches offsets per session (sub-100 ms re-attach), and applies the write. Code patches auto-revert on detach. Known engine RVAs are pinned in `crates/batman-lod-dll/src/lotdk.rs`.

---

## ⚠️ Read me

- **Attach in-game only** — load a save and control Batman first; the menu/loading screens have no live objects to resolve.
- **Build-specific** — RVAs are pinned to build 1.0.0.1; a patch can move them (fix = a contributor PR).
- **Back up saves** before progression cheats; toggle combat/world cheats off if a cutscene soft-locks.
- **Don't toggle `bCanBeDamaged`** — it triggers TT's FPV cursor-lock; we freeze GAS attributes instead.
- **Not affiliated** with TT Games, WB, DC, or LEGO. Trademarks belong to their owners.

---

## 🧩 Add a feature

```powershell
cargo run -p openforge-cli -- new-feature --game batman-lod --id <feature-id>
cargo run -p openforge-discover -- --game batman-lod attach   # then scan/narrow/pick/extract-aob/emit
cargo run -p openforge-cli -- verify-registry                 # CI gate
```

Walkthrough: [docs/GAME-AUTHORING.md](../../../docs/GAME-AUTHORING.md) · UE5 recipes: [docs/UE5-CHEAT-COOKBOOK.md](../../../docs/UE5-CHEAT-COOKBOOK.md). [MIT](../../../LICENSE).
