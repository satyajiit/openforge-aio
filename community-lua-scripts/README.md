# Community Lua Scripts

This directory holds **community-contributed Lua scripts** for every game OpenForge supports. The OpenForge desktop app fetches each game's `index.json` from this directory and lets users install scripts with one click.

No separate repo, no separate auth — drop a `.lua` file in the right subdirectory, add an entry to the index, open a PR.

## Layout

```
community-lua-scripts/
├── README.md                       (this file)
└── <game-id>/                      (matches the manifest.toml `id` of the game)
    ├── index.json                  (machine-readable script listing)
    ├── <slug>.lua                  (script bodies)
    └── …
```

Currently shipped games:

- `batman-lod/` — LEGO Batman: Legacy of the Dark Knight

## `index.json` schema

Each game's `index.json` is a single object with a `scripts` array. Slugs must be kebab-case (`[a-z0-9-]+`) and unique inside the game.

```json
{
  "scripts": [
    {
      "slug": "rapid-fire-batarang",
      "name": "Rapid-fire Batarang",
      "description": "Loops the throw animation so batarangs leave faster than the cooldown allows.",
      "author": "@your-github-handle",
      "url": "https://raw.githubusercontent.com/satyajiit/openforge-aio/main/community-lua-scripts/batman-lod/rapid-fire-batarang.lua"
    }
  ]
}
```

Fields:

| Field         | Required | Notes                                                                                                       |
|---------------|----------|-------------------------------------------------------------------------------------------------------------|
| `slug`        | yes      | Filename stem of the `.lua` body (without extension). Must match `^[a-z0-9-]+$`.                            |
| `name`        | yes      | Display name shown in the sidebar.                                                                          |
| `description` | no       | One-sentence summary, shown under the name.                                                                 |
| `author`      | no       | Your GitHub handle (e.g. `@octocat`) or display name.                                                       |
| `url`         | no       | Raw URL to the `.lua` body. Defaults to `community-lua-scripts/<game>/<slug>.lua` in this repo — usually fine to omit. |

## Adding a script

1. Fork `satyajiit/openforge-aio` and check out a branch.
2. Drop your script at `community-lua-scripts/<game-id>/<slug>.lua`.
3. Add a corresponding entry to `community-lua-scripts/<game-id>/index.json` (create the file if it doesn't exist yet — schema above).
4. Open a PR. CI will validate the JSON shape; we'll review the script body for safety + behaviour.

## Rules

- **Single-player offline only.** Same as the rest of OpenForge. Scripts that touch online state will be declined.
- **No outbound network calls.** Scripts run inside the trainer's per-game DLL with reflection + memory access; they should not need HTTP / sockets and the runtime doesn't expose them.
- **MIT-licensed original work.** Don't paste extracted CT logic from closed-source trainers. Recreating an idea from scratch is fine.
- **Stable APIs only.** The Lua bindings (UE5 reflection, key helpers, output) are still firming up — pin to documented helpers, not internal back-doors.
- **Author credit goes in the `author` field.** No header banners or watermarks inside the script body, please.

## Local testing

Before opening a PR, point your local OpenForge build at your fork to verify:

1. Push your branch to your fork.
2. Edit `crates/app/src-tauri/src/lua/community.rs` constants `INDEX_URL_TEMPLATE` + `default_script_url` to point at `https://raw.githubusercontent.com/<your-fork>/openforge-aio/<branch>/community-lua-scripts/…` temporarily.
3. `pnpm tauri:dev:fast`, open the game, switch to the Lua tab → Community.
4. Revert the constants before opening the PR.

(A cleaner per-build override mechanism is on the roadmap; for now this manual swap is the workflow.)
