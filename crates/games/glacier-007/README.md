<div align="center">

# 🔫 007 First Light

**OpenForge support module — 6 features on the Glacier 2 engine, Denuvo-safe.**

[![Status](https://img.shields.io/badge/status-beta-yellow)](#)
[![Features](https://img.shields.io/badge/features-6-blue)](#-features)
[![Engine](https://img.shields.io/badge/engine-Glacier%202-313131)](#)
[![Anti-tamper](https://img.shields.io/badge/Denuvo-safe-brightgreen)](#-how-it-works)
[![License](https://img.shields.io/badge/license-MIT-blue)](../../../LICENSE)

[← OpenForge root](../../../README.md)

</div>

---

## What this is

The 007 First Light support module (`openforge-game-glacier-007`) for [OpenForge](../../../README.md): a `manifest.toml` declaring the game (`[engine] kind = "glacier2"`, process name, the DLL to inject) and a folder of [signature files](signatures/) — one per cheat — that the declarative engine interprets at runtime. The companion injected DLL in [`crates/glacier-007-dll/`](../../glacier-007-dll/) runs the shared Glacier reflection engine in-process and serves memory + game-thread ops over a named pipe. Every cheat is config; adding one is a signature change, not a recompile.

---

## ✨ Features

6 features, all **Denuvo-safe** — data writes and reflection, no `.text` patches except two reversible code-patches (Infinite Ammo, No Reload).

**🛡️ Combat** · God Mode (authoritative-health freeze) · One-Hit Kill (melts every enemy, any type)

**🔫 Weapons** · Infinite Ammo · No Reload · Give Weapon *(Mods tab — grant any weapon present in the level by name)*

**🌍 World** · Game Speed (0.05×–10×)

Per-feature strategy, offsets, and gotchas live in the signature files in [`signatures/`](signatures/).

---

## 📋 Requirements

- **Game**: 007 First Light (`007FirstLight.exe`), Glacier 2 engine, **Denuvo**.
- **OS**: Windows 10+ (64-bit). **No administrator required** — the backend uses external RPM + an injected reflection DLL (no debugger attach).
- **DLL**: `glacier_007_dll.dll` — built with the bundle, no separate install.

---

## 🔬 How it works

On Attach, the host finds the process, injects `glacier_007_dll.dll`, and handshakes over a per-PID named pipe. The DLL exposes the game's own Glacier reflection (`ZTypeRegistry`) plus memory primitives (read/write/heap-scan), a Denuvo-safe in-process **game-thread executor** (HW-breakpoint rendezvous — no debugger, no `.text` patch), and a **find-writer** (HW write/exec breakpoint) used during discovery.

- **God Mode** freezes the player's authoritative health value-box (`current := max` each tick), re-found each session by a structural fingerprint — not a fixed address.
- **One-Hit Kill** finds *every* loaded enemy's health box by a layout-agnostic invariant — `base × scale == max`, `0 < current ≤ max` — so it covers every enemy archetype with no per-type HP list, then sets each to 1 HP on a background loop. Your own box is found first (the difficulty-invariant base-100 fingerprint) and excluded. Each write re-reads + re-validates the box first, so it can only land on a live health box.
- **Game Speed** writes `ZGameTimeManager`'s gameplay-clock multiplier, re-found by a vtable heap-scan.
- **Infinite Ammo / No Reload** are reversible `code_patch`es over the ammo-decrement sites (auto-revert on detach).
- **Give Weapon** fires a present firearm's own pickup node on the game thread — the same path a real pickup takes.

Resolved addresses are cached per session for sub-100 ms re-attach. Known per-build VAs are pinned in `crates/glacier-host/src/session.rs` and the signature files.

---

## ⚠️ Read me

- **Attach + toggle in active gameplay** — health boxes, the difficulty entity, and firearm nodes only exist in a loaded level, not on menus/cutscenes.
- **Build-specific** — Glacier has no ASLR on the main module, so VAs are stable within a build but **shift on a game update**. Re-derive them with the `re_tools.py` RTTI/disasm toolkit on a fresh `--dump-module` dump (see the discovery notes); struct *offsets* stay put, only the addresses move. Structural features (God Mode, One-Hit Kill) self-heal because they scan by shape, not address.
- **One-Hit Kill** pins enemies to 1 HP while on — they die to a single hit (or any chip damage). Toggle it off to restore normal combat; enemies you have not engaged heal/respawn as usual.
- **Give Weapon** only lists weapons present in the level right now (dropped/on racks); it grants what is there, it does not spawn arbitrary types.
- **Back up saves** before long sessions; toggle cheats off if a scripted/near-death sequence soft-locks.
- **Not affiliated** with IO Interactive, MGM, or EON. 007 and related marks belong to their owners.

---

## 🧩 Add a feature

```powershell
cargo run -p openforge-cli -- new-feature --game glacier-007 --id <feature-id>
cargo run -p openforge-discover -- glacier-dll --game glacier-007   # inject + drive the reflection/memory ops live
cargo run -p openforge-cli -- verify-registry                       # CI gate
```

The `openforge-discover glacier-dll` harness drives the live backend (resolve types, read/write, heap-scan, find-writer, game-thread call, list/give firearms, scan + one-hit-kill enemy health boxes). Walkthrough: [docs/GAME-AUTHORING.md](../../../docs/GAME-AUTHORING.md). [MIT](../../../LICENSE).
