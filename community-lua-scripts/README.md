# Community Lua Scripts

Community-contributed Lua scripts, one folder per game. The OpenForge app fetches each game's `index.json` and installs scripts with one click. No separate repo or auth — drop a `.lua`, add an index entry, open a PR.

## Layout

```
community-lua-scripts/
└── <game-id>/              (matches the game's manifest id)
    ├── index.json          (script listing)
    └── <slug>.lua          (script bodies)
```

Shipped: `batman-lod/` — LEGO Batman: LotDK.

## `index.json`

A single object with a `scripts` array. Slugs are kebab-case (`^[a-z0-9-]+$`), unique per game.

```json
{ "scripts": [
  {
    "slug": "rapid-fire-batarang",
    "name": "Rapid-fire Batarang",
    "description": "Loops the throw so batarangs leave faster than the cooldown.",
    "author": "@your-handle"
  }
] }
```

`slug` + `name` are required; `description`, `author`, and `url` (raw `.lua` link — defaults to this repo) are optional.

## Adding a script

1. Fork, branch, drop your script at `community-lua-scripts/<game-id>/<slug>.lua`.
2. Add an entry to that game's `index.json` (create it if missing).
3. Open a PR — CI validates the JSON; we review the body.

## Rules

- **Single-player offline only** — same as the rest of OpenForge.
- **No network calls** — scripts run inside the trainer's per-game DLL with memory + reflection access; the runtime exposes no HTTP/sockets.
- **MIT-licensed original work** — no extracted CT logic; recreating an idea from scratch is fine.
- **Stable APIs only** — the bindings are still firming up; pin to documented helpers. Author credit goes in the `author` field, not a header banner.
