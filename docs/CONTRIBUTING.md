# Contributing to OpenForge

OpenForge is community-extensible. Every shipped game lives in its own Rust crate under `crates/games/<id>/`. To add a new game, you fork the repo, drop a folder, and open a PR.

## Scope

OpenForge is for **single-player offline games**. Pull requests that target online competitive games will be closed. The trainer probes for known anti-cheat services at startup and refuses to attach if any are running — but the bigger filter is editorial: keep the scope local-only.

## Workflow

1. **Fork** `GamesPatch/openforge` on GitHub.
2. **Clone** + set up the toolchain (Rust 1.95, Node 24, pnpm 10). See top-level README.
3. **Scaffold** a new game crate:
   ```powershell
   cargo run -p openforge-cli -- new-game --id <slug> --name "Your Game" --process YourGame.exe
   ```
   The scaffolder:
   - Copies `crates/games/_template/` to `crates/games/<slug>/` and substitutes the placeholders.
   - Edits `crates/bundle/Cargo.toml` (path dep), `crates/bundle/src/lib.rs` (re-export + `FORCE_LINK`), and the root `Cargo.toml` (workspace members).
4. **Add an icon.** Drop a 64×64 monochrome PNG at `crates/games/<slug>/assets/icon.png`.
5. **Discover the first signature** with `openforge-discover` — see [GAME-AUTHORING.md](GAME-AUTHORING.md).
6. **Verify locally:**
   ```powershell
   cargo run -p openforge-cli -- verify-registry
   cargo run -p openforge-discover -- --game <slug> verify
   cd crates/app && npx pnpm tauri dev
   ```
7. **Open a PR** with: game name, source URL (Steam/GOG/etc.), the version you verified against, and one screenshot of the trainer working in-game.

## Code style

- Rust: `cargo fmt` + warning-free `cargo check --workspace`. No `unwrap()` outside tests. Custom error types via `thiserror`; boundaries return `anyhow::Result`.
- TypeScript: strict mode with `noUncheckedIndexedAccess`. No `any` outside narrow interop. Tailwind utilities only; no inline `style` for color.
- TOML signatures: parse cleanly via `openforge_runtime::SignatureSpec::parse + validate`. `openforge-cli verify-registry` will reject the PR if not.

## Commit messages

Imperative mood, one line; an optional blank-line + body for context. Examples: `Add Hogwarts Legacy game crate`, `Refresh batman/studs AOB for v1.0.0.2`.

## What lives where

| Question | File |
|----------|------|
| How do I find an address? | [GAME-AUTHORING.md](GAME-AUTHORING.md) |
| What can the trainer write? | `crates/runtime/src/feature.rs`, `signature.rs` |
| How is IPC wired? | `crates/app/src-tauri/src/commands.rs` + `crates/app/src/lib/ipc.ts` |

## What we won't accept

- Trainers for any game with online competitive modes.
- Patterns or addresses copied from another community member without attribution.
- Game executables, save files, or copyrighted assets committed to the repo.
- Closed-source build steps. Everything must build from source with `cargo` + `pnpm`.

## License

By contributing, you agree your contribution is licensed under MIT.
