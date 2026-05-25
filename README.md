<div align="center">

# OpenForge

**An open-source, all-in-one trainer for offline single-player PC games.**

*Because save-scumming is a craft, not a crime.*

[![Build](https://img.shields.io/github/actions/workflow/status/satyajiit/openforge-aio/ci.yml?branch=main&label=build&logo=github)](https://github.com/satyajiit/openforge-aio/actions)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Platform](https://img.shields.io/badge/platform-Windows%2010%2B-0078D6?logo=windows)](https://github.com/satyajiit/openforge-aio/releases)
[![Rust](https://img.shields.io/badge/rust-1.95%2B-orange?logo=rust)](https://www.rust-lang.org/)
[![Tauri](https://img.shields.io/badge/tauri-2.11-FFC131?logo=tauri)](https://tauri.app/)
[![React](https://img.shields.io/badge/react-19-61DAFB?logo=react)](https://react.dev/)
[![Latest Release](https://img.shields.io/github/v/release/satyajiit/openforge-aio?include_prereleases&label=release)](https://github.com/satyajiit/openforge-aio/releases)
[![Stars](https://img.shields.io/github/stars/satyajiit/openforge-aio?style=social)](https://github.com/satyajiit/openforge-aio/stargazers)

[![PRs Welcome](https://img.shields.io/badge/PRs-welcome-brightgreen.svg)](docs/CONTRIBUTING.md)
[![Single-Player Only](https://img.shields.io/badge/scope-single--player%20only-success)](#a-grown-up-disclaimer)
[![No Anti-Cheat](https://img.shields.io/badge/anti--cheat-not%20our%20circus-lightgrey)](#a-grown-up-disclaimer)
[![Made with Rust](https://img.shields.io/badge/made%20with-%F0%9F%A6%80%20Rust-CE422B)](https://www.rust-lang.org/)
[![YouTube](https://img.shields.io/badge/YouTube-%40GamesPatch-FF0000?logo=youtube&logoColor=white)](https://youtube.com/@GamesPatch)

[**▶ Subscribe on YouTube**](https://youtube.com/@GamesPatch) · [**★ Star the repo**](https://github.com/satyajiit/openforge-aio) · [**🐛 Report a bug**](https://github.com/satyajiit/openforge-aio/issues) · [**🎮 Request a game**](https://github.com/satyajiit/openforge-aio/issues/new?labels=new-game)

</div>

---

## What it is

OpenForge attaches to a running offline single-player PC game and lets you tweak it like the developers forgot to ship a debug menu — currency edits, infinite health, fly mode, fast travel unlocks, the works. One desktop app, every supported game, no per-game installers, no DRM-bothering, no online shenanigans.

The interesting bit lives under the hood: **every cheat is a TOML file, not Rust code**. Contributors author signatures; a declarative engine at runtime turns them into reads, writes, freezes, and code patches. Adding a new game is a folder drop in `crates/games/<id>/`, a few signature TOMLs, and a PR — no engine modifications, no recompile dance per cheat.

For UE5 titles, a small per-game DLL is injected on Attach and exposes the engine's own reflection (UObject graph, FName resolution, FProperty walking) over a named pipe. The trainer never has to chase brittle AOB signatures; you write `class_path = "BP_Hero_C"` + `property_name = "Health"` and the runtime does the rest. It's how the first shipped game gets twenty features without twenty handwritten address scans.

> **TL;DR:** Tauri 2 desktop app + Rust workspace + declarative TOML signatures + UE5 reflection. Drop a folder, get a trainer.

---

## ✨ Why use it

| | |
|---|---|
| 🦀 **Pure Rust core** — no Lua interpreter at the controls, no random binary blobs. | 🎨 **Modern Tauri 2 + React 19 UI** — tabs, glassmorphism, dark mode, a search bar that actually searches. |
| 📜 **Declarative TOML signatures** — every cheat is a config file, not 200 lines of Rust. | 🧠 **UE5 reflection engine** — class + property name lookup, no manual address chasing. |
| 🛡️ **Auto-revert on detach** — every code patch you apply is rolled back when the trainer closes or the game exits. | 🪶 **Sub-100ms re-attach** — resolved addresses are cached per session and validated on re-connect. |
| 🧩 **One folder = one game** — community-extensible by design. | 🆓 **MIT licensed** — fork it, ship it, sell it, just don't slap online games with it. |

---

## 🎮 Supported games & features

OpenForge currently ships full support for the game(s) below. Each entry links to a per-game README with the deeper details. Want your favourite offline title supported? [Open an issue](https://github.com/satyajiit/openforge-aio/issues/new?labels=new-game) or, better yet, [bring a PR](docs/GAME-AUTHORING.md).

<details open>
<summary><strong>🦇 LEGO Batman: Legacy of the Dark Knight</strong> &mdash; <code>stable</code> · <code>20 features</code> · <code>UE5 reflection</code> · supported build: <code>1.0.0.1</code> · <a href="crates/games/batman-lod/README.md">per-game README →</a></summary>

LEGO meets the Caped Crusader on Unreal Engine 5. OpenForge ships a full toolkit: stud edits, skill unlocks, fly, low-gravity glides, super-speed, super-jump, freeze enemies, teleport, plus a bunch of world-spice toggles (bullet trains, demolition-derby traffic, sprinting pedestrians) for when Gotham feels too quiet.

#### 💰 Currency

| Feature | Strategy | What it does |
|---|---|---|
| **Set Studs** | one-shot | Writes a custom stud balance via TT's currency reflection; persists across saves. |
| **Set WayneTech Chips** | one-shot | Same idea for WayneTechChips. |
| **Stud Multiplier** | one-shot | Pins `StudMultiplierMin`/`Max` together (presets 1×–100×). |

#### 🛡️ Combat

| Feature | Strategy | What it does |
|---|---|---|
| **Infinite Health** | freeze (100 ms) | Freezes `Health.CurrentValue` on the player's `HealthAttributeSet` (GAS-aware). |
| **Unlimited Focus** | freeze (100 ms) | Pins the combo meter at 9999. |
| **One-Hit Kill** | freeze-for-matching (200 ms) | Freezes enemy `Health` at 1.0 while filtering out player/allies/vehicles/NPCs. |
| **Freeze All Enemies** | freeze-for-matching (200 ms) | Walks the live UObject array and sets `CustomTimeDilation` to 0.0001 on Characters. |

#### 🏃 Movement

| Feature | Strategy | What it does |
|---|---|---|
| **Fly Mode** | freeze (16 ms) | Sets `MovementMode` to `MOVE_Flying`; WASD glide, no gravity. |
| **Low Gravity** | one-shot | Adjusts `GravityScale` (presets 1.0 → 0.5 → 0.15 → 0.0, negatives for upward float). |
| **Super Jump** | one-shot | Adjusts `JumpZVelocity` (presets 420 → 5000 cm/s). |
| **Super Speed** | one-shot | Adjusts `MaxWalkSpeed` (presets 600 → 5000 cm/s). |

#### 📍 Teleportation

| Feature | Strategy | What it does |
|---|---|---|
| **Teleport X / Y / Z** | one-shot | Tweak `RootComponent.RelativeLocation` per-axis. |
| **Teleport to Waypoint** | one-shot | Finds `CustomMapPinActor` and warps the player pawn via `K2_TeleportTo`. |

#### 🏆 Progression

| Feature | Strategy | What it does |
|---|---|---|
| **Unlock All Skills** | progress-tag write | Iterates `PROG_Skills` and unlocks all 30 combat/exploration nodes via `TtGameProgressStatics`. |
| **Unlock All Fast Travel** | progress-tag write | Activates all 65 fast-travel unlock tags (~9 usable terminals — fast-travel from anywhere). |

#### 🌆 World spice

| Feature | Strategy | What it does |
|---|---|---|
| **Fast Pedestrians** | one-shot | Scales `CrowdStatelessWanderSettings.WalkSpeedMetresPerSecond` (1.34 → 20 m/s). |
| **Bullet Trains** | freeze-for-matching (250 ms) | Freezes `TrackSplineComponent.MoveSpeed` at 5× stock — Gotham gets the metro it deserves. |
| **Demolition Derby** | freeze (200 ms) | Cranks `MassTrafficSettings` chaos: turn-speed scale + four variance fields, presets 0.6 → 2.0. |

</details>

### Coming soon / on the wishlist

PRs welcome on any single-player offline title. Want your favourite added? Open an issue with a `[new-game]` tag.

### Add your own game

```powershell
cargo run -p openforge-cli -- new-game --id <slug> --name "Your Game"
```

This scaffolds `crates/games/<slug>/` from the template, wires it into the bundle, and prints the next steps. Full walkthrough lives in **[docs/GAME-AUTHORING.md](docs/GAME-AUTHORING.md)** and the UE5-specific recipe book in **[docs/UE5-CHEAT-COOKBOOK.md](docs/UE5-CHEAT-COOKBOOK.md)**.

---

## 🚀 Quick start (end-users)

1. Grab the latest installer from **[Releases](https://github.com/satyajiit/openforge-aio/releases)**.
2. Run as **Administrator** (game memory access needs elevation — OpenForge prompts if you forget).
3. Launch your game.
4. Hit **Attach** in OpenForge. Toggle cheats. Be merciful with the cutscenes.

> 💾 Always back up your save before applying progression cheats. We're good at our job; the game's serializer occasionally is not.

---

## 🛠️ Build from source

Prerequisites: **Rust 1.95+** (edition 2024), **Node 24 LTS**, **pnpm 10**, Windows 10+.

```powershell
# Verify everything compiles
cargo check --workspace

# Frontend deps
cd crates/app
npx --yes pnpm@10 install
npx pnpm typecheck
npx pnpm build

# Run the desktop app (pick a profile)
npx pnpm tauri:dev          # debug — fastest rebuild, slow scans
npx pnpm tauri:dev:fast     # dev-fast — release-opt deps + debug our code (recommended for real testing)
npx pnpm tauri:dev:release  # full release — fastest runtime, slowest rebuild

# Discovery pipeline (per game, for contributors)
cargo run -p openforge-discover -- --game <slug> doctor
cargo run -p openforge-discover -- --game <slug> scan --feature gold --type i32 --value 12345 --name "Gold"
```

CI gates: `cargo fmt --check`, `cargo clippy -D warnings`, `cargo test --workspace --lib`, `openforge-cli verify-registry`, frontend `pnpm typecheck` + `pnpm build`. All must pass.

---

## 🧱 Tech stack

- **Rust 1.95** workspace (edition 2024) — eight crates: `core`, `runtime`, `discover`, `cli`, `bundle`, `app`, `ue5-host`, `ue5-protocol`
- **Tauri 2.11** + **windows-rs 0.61** for the desktop shell
- **React 19** + **Vite 6** + **TypeScript 5.7** + **Tailwind CSS 4** (CSS-first, `@theme`) + **shadcn/ui** (new-york)
- **postcard** wire protocol for the in-process DLL pipe
- **iced-x86** for instruction disassembly during signature extraction
- **MIT licensed**, contributor-friendly, no patent-encumbered deps

Per-crate architecture lives next to the code — start with `crates/runtime/src/feature.rs` (the declarative engine), `crates/app/src-tauri/src/commands.rs` (the IPC surface), and `crates/ue5-host/src/session.rs` (the in-process reflection bridge).

---

## ⚠️ A grown-up disclaimer

OpenForge exists for **offline single-player PC games**. That scope is the project, not a footnote. Specifically:

- 🚫 **No online or competitive titles, ever.** No PvP, no MMOs, no online co-op against strangers. Don't try to make it work — we won't accept PRs that do.
- 🚫 **No anti-cheat bypassing.** BattlEye, EAC, EQU8, VAC, Ricochet — if it's there, we walk away. The first line of every PR template asks; the answer is always "no."
- 🚫 **Not affiliated with any game publisher or developer.** All trademarks belong to their owners. We just like their games enough to wear out the save-file.
- ⚠️ **Use at your own risk.** Trainers poke at process memory; rare crashes are part of the deal. Back up saves before progression cheats. Don't mess with mid-cutscene state unless you enjoy soft-locks.
- 🎓 **Built for personal use, learning, game preservation, modding research, and accessibility.** Discovery walkthroughs are public so anyone curious about reverse engineering UE5 internals can follow along.
- 🤝 **You own your trainer use.** If a game's EULA says no trainers, that's between you and the EULA. We don't ship anti-detection or evasion code, and we never will.

If any of that gives you pause: cool, the project might not be for you. If it sounds like exactly the kind of community-built tinker-tool you've been looking for — welcome.

---

## 🤝 Contributing

OpenForge gets better the more people drop new games into `crates/games/`. The contribution guide is **[docs/CONTRIBUTING.md](docs/CONTRIBUTING.md)**, but the headline rules are short:

- 🎯 **Single-player offline only.** No exceptions.
- 📝 **Signatures must be original MIT-licensed work.** Don't paste from WeMod, MrAntiFun, or Cheat Evolution `.ct` extractions. Public FearlessRevolution threads + GitHub-hosted CTs are fine to adapt and re-derive.
- ✅ **CI is the merge gate.** `cargo fmt`, `clippy -D warnings`, `cargo test --lib`, `verify-registry`, frontend `typecheck` + `build` — all green or it doesn't ship.
- 💬 **Discuss before refactoring shared infra.** Single-game PRs can land fast; engine changes deserve a chat first.

Open a GitHub Discussion for architecture-level proposals before sinking a weekend into one.

---

## 📚 Documentation index

| Doc | What's inside |
|---|---|
| [docs/CONTRIBUTING.md](docs/CONTRIBUTING.md) | PR workflow, code style, what we will / won't accept. |
| [docs/GAME-AUTHORING.md](docs/GAME-AUTHORING.md) | End-to-end walkthrough of adding a new game. |
| [docs/UE5-CHEAT-COOKBOOK.md](docs/UE5-CHEAT-COOKBOOK.md) | Per-cheat-category recipes for UE5 titles. |
| [crates/games/batman-lod/README.md](crates/games/batman-lod/README.md) | LEGO Batman: LotDK — features, build notes, disclaimers. |

---

## 💛 Support the project

If OpenForge saved you a Cheat Engine session, an evening of pointer-chasing, or just made a quiet evening with a single-player game a bit more fun — there are two easy ways to say thanks:

### ⭐ Star the repo

It's free, it's two clicks, it genuinely helps more contributors find the project.

> [**Star OpenForge on GitHub →**](https://github.com/satyajiit/openforge-aio)

### ▶ Subscribe on YouTube

Discovery walkthroughs, new-game previews, "here's how I reverse-engineered this cheat" deep-dives, contributor spotlights, the occasional rant about UE5 internals.

> [**Subscribe to @GamesPatch →**](https://youtube.com/@GamesPatch)

You can also **[open an issue](https://github.com/satyajiit/openforge-aio/issues)** for bugs, feature ideas, or new-game requests. PRs are even better.

---

## 📄 License

[MIT](LICENSE) © 2026 OpenForge contributors. Use it, fork it, ship it — just keep the license file and don't put it anywhere online-multiplayer.

---

<div align="center">

*Made with 🦀, 🎮, and an unreasonable amount of late-night save-scumming.*

[⬆ Back to top](#openforge)

</div>
