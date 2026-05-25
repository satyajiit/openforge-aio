# Releasing OpenForge

OpenForge releases are cut **manually** from a maintainer's machine. There's no GitHub Actions release workflow — building Tauri + the Rust workspace on a cold CI runner took 30–60 min, which is silly when a warm local build finishes in ~5–10 min. We trade CI convenience for human speed.

> 🛡️ **Prerequisite:** every gate enforced by `.githooks/pre-commit` (fmt, clippy, tests, typecheck) is already green on the commit you're releasing from. If you skipped the hook, run them by hand before tagging.

---

## TL;DR

```powershell
# 1. From a clean working tree at the commit you want to release:
git status                                       # must be clean
git log -1 --oneline                              # this is what ships

# 2. Tag the release (keep the `v` prefix on the tag):
$VERSION = "v0.1.0"
git -c user.name=satyajiit -c user.email=satyajiit0@gmail.com `
    tag -a $VERSION -m "OpenForge $VERSION"
git push origin $VERSION

# 3. Build the trainer (warm cache: ~5-10 min, cold cache: ~20-30 min):
cd crates/app
npx pnpm tauri:build --target x86_64-pc-windows-msvc
cd ../..

# 4. Build the per-game DLL(s) separately — they're sibling workspace crates
#    that the app's release build doesn't transitively trigger (~30 sec):
cargo build --release -p openforge-batman-lod-dll --target x86_64-pc-windows-msvc

# 5. Stage + zip the user-facing bundle:
$STAGE = "target\release-staging\openforge-$VERSION-windows-x64"
if (Test-Path "target\release-staging") { Remove-Item "target\release-staging" -Recurse -Force }
New-Item -ItemType Directory -Path $STAGE -Force | Out-Null
Copy-Item "target\x86_64-pc-windows-msvc\release\openforge.exe"      -Destination $STAGE
Copy-Item "target\x86_64-pc-windows-msvc\release\batman_lod_dll.dll" -Destination $STAGE
# Drop a plain-text README inside the zip with install steps + disclaimers.
# (See the v0.1.0 RELEASING.md history for the exact template if you need it.)
Set-Content "$STAGE\README.txt" "OpenForge $VERSION — extract + run openforge.exe as admin"
$ZIP = "target\release-staging\openforge-$VERSION-windows-x64.zip"
Compress-Archive -Path "$STAGE\*" -DestinationPath $ZIP -CompressionLevel Optimal

# 6. Publish a draft release with the zip, the .exe, and the .dll attached:
gh release create $VERSION `
    --title "OpenForge $VERSION" `
    --notes-file "docs/release-notes/$VERSION.md" `
    --draft `
    $ZIP `
    "target\x86_64-pc-windows-msvc\release\openforge.exe" `
    "target\x86_64-pc-windows-msvc\release\batman_lod_dll.dll"

# 7. Open the draft, eyeball the release notes, hit Publish:
gh release view $VERSION --web
```

---

## Detailed walkthrough

### 0. Decide the version

OpenForge follows [SemVer](https://semver.org/) loosely:

| Bump  | When                                                                                       |
|-------|---------------------------------------------------------------------------------------------|
| MAJOR | Breaking changes to TOML signature schema, IPC protocol, or DLL ABI.                       |
| MINOR | New supported game, new feature category, new top-level UI surface (e.g. Lua runtime).      |
| PATCH | Bug fixes, perf, single-feature additions to an existing game, doc/UI polish.               |

Update the version in **both** places before tagging:

```toml
# Cargo.toml (workspace root)
[workspace.package]
version = "0.1.0"

# crates/app/src-tauri/tauri.conf.json
"version": "0.1.0"
```

These should already match — the desktop app's installer name and the binary version come from `tauri.conf.json`, the Rust crate metadata comes from `Cargo.toml`.

### 1. Sanity-check the commit you're releasing

```powershell
git status                  # clean
git log -1 --oneline        # this is what end-users will run
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --lib
cargo run -p openforge-cli -- verify-registry
cd crates/app; npx pnpm typecheck; npx pnpm build; cd ../..
```

All green? Proceed. Anything red? Fix and commit before tagging.

### 2. Write the release notes

Drop a markdown file at `docs/release-notes/<tag>.md`. Format:

```markdown
# OpenForge v0.1.0

## Highlights
- One sentence per shipped headline feature.

## What's new
- Grouped bullets: Games, Features, Engine, UI, Docs.

## Fixes
- One-liners.

## Known issues
- Honest list — game version compatibility, known crashes, etc.

## Install
- Standalone `openforge.exe` — drop into a folder, run as admin.
- MSI — Windows Installer; works with policy-managed deploys.
- NSIS — the friendliest installer for end-users; shortcut + uninstaller included.

Requires Windows 10+. Single-player offline scope only. No anti-cheat-protected games.
```

Tip: pull highlights from `git log --oneline <prev-tag>..HEAD` and rewrite for users.

### 3. Tag the commit

Tags are annotated (`-a`) so they carry a message. Use the same author-override pattern we use for commits — never touch global git config:

```powershell
git -c user.name=satyajiit -c user.email=satyajiit0@gmail.com `
    tag -a v0.1.0 -m "OpenForge v0.1.0"
git push origin v0.1.0
```

If you need to move a tag that's already pushed (rare; only when the commit history was rewritten):

```powershell
git tag -d v0.1.0                       # delete locally
git push origin :refs/tags/v0.1.0       # delete on remote
# … re-create + push as above
```

### 4. Build the artifacts

Two cargo builds: the Tauri app (the .exe) and each per-game DLL separately. The app's release build does NOT transitively build sibling-crate DLLs — they're cdylibs that need to be built on their own.

```powershell
# Trainer .exe
cd crates/app
npx pnpm tauri:build --target x86_64-pc-windows-msvc
cd ../..

# Per-game DLLs (one cargo call per shipped game)
cargo build --release -p openforge-batman-lod-dll --target x86_64-pc-windows-msvc
# When more games ship, add a line per DLL crate here.
```

Output paths (relative to repo root):

| File | What it is |
|------|------------|
| `target/x86_64-pc-windows-msvc/release/openforge.exe` | The trainer binary. Standalone but requires per-game DLLs next to it on disk to actually attach. |
| `target/x86_64-pc-windows-msvc/release/batman_lod_dll.dll` | LEGO Batman: LotDK injected DLL. Must live in the same folder as `openforge.exe` at runtime (the host walks `<exe-dir>/<file>` per `dll_path.rs`). |

Timing: warm cache (you ran `tauri:dev:fast` recently) ~5-10 min for the trainer + ~30 sec per DLL. Cold cache or after `cargo clean`: ~20-30 min total.

### 5. Stage + zip the user-facing bundle

End users get a single zip with the .exe + every shipped game's DLL + a plaintext README. PowerShell snippet:

```powershell
$VERSION = "v0.1.0"
$STAGE = "target\release-staging\openforge-$VERSION-windows-x64"
if (Test-Path "target\release-staging") { Remove-Item "target\release-staging" -Recurse -Force }
New-Item -ItemType Directory -Path $STAGE -Force | Out-Null

Copy-Item "target\x86_64-pc-windows-msvc\release\openforge.exe"      -Destination $STAGE
Copy-Item "target\x86_64-pc-windows-msvc\release\batman_lod_dll.dll" -Destination $STAGE
# Add Copy-Item lines for additional game DLLs as new games ship.

# Plain-text install notes inside the zip. Keep the disclaimer short — full
# README is on github.
@"
OpenForge $VERSION — Windows x64
================================

Contains:
  openforge.exe       — the trainer
  batman_lod_dll.dll  — LEGO Batman: LotDK injected DLL

Install:
  1. Extract both files into the same folder.
  2. Run openforge.exe as administrator.
  3. Launch the game, load a save, hit Attach.

Single-player offline only. Not affiliated with any publisher.
MIT licensed. https://github.com/satyajiit/openforge-aio
"@ | Out-File -FilePath "$STAGE\README.txt" -Encoding utf8

$ZIP = "target\release-staging\openforge-$VERSION-windows-x64.zip"
Compress-Archive -Path "$STAGE\*" -DestinationPath $ZIP -CompressionLevel Optimal
```

### 6. Publish the release

Attach the zip (primary download), plus the .exe and each .dll as standalone files for users who only need to swap one piece:

```powershell
gh release create $VERSION `
    --title "OpenForge $VERSION" `
    --notes-file "docs/release-notes/$VERSION.md" `
    --draft `
    $ZIP `
    "target\x86_64-pc-windows-msvc\release\openforge.exe" `
    "target\x86_64-pc-windows-msvc\release\batman_lod_dll.dll"
```

The `--draft` flag keeps it private until you publish. Open the draft in a browser to verify everything looks right:

```powershell
gh release view v0.1.0 --web
```

When you're satisfied, click **Publish release** in the GitHub UI (or run `gh release edit v0.1.0 --draft=false`).

### 6. Tweet / post / sleep

The release is now live at `https://github.com/satyajiit/openforge-aio/releases/tag/<tag>`. The README "latest release" badge will update on its own within ~minutes.

---

## Re-running a release (something went wrong)

If you published and then noticed something broken in the artifacts:

```powershell
# Delete the release entirely (does NOT delete the tag):
gh release delete v0.1.0 --yes

# Optionally also delete the tag if you need to rebuild from a different commit:
git tag -d v0.1.0
git push origin :refs/tags/v0.1.0
```

Then redo from step 3.

For typo fixes in the release notes only, edit them in place:

```powershell
gh release edit v0.1.0 --notes-file docs/release-notes/v0.1.0.md
```

---

## Why no GitHub Actions release workflow?

We had one. It took 30–60 min on `windows-latest` to build the same artifacts a warm local build produces in ~10 min, and the `release.yml` cold-cache pain quickly stopped being worth it for a small team. The trade is:

- ✅ Releases are 5–10× faster end-to-end.
- ✅ No CI-secret juggling for signing certificates etc.
- ❌ Requires a maintainer with a Windows machine + `gh` CLI to cut a release.
- ❌ No automatic build-on-tag — if a contributor pushes a tag, nothing happens.

This is fine while OpenForge is a one-maintainer project. When that changes, restore `.github/workflows/release.yml` from git history (commit `cf43650^` or earlier) and add cargo + pnpm caching to bring the runtime back to ~10–15 min.
