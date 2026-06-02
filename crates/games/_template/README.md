# openforge-game-template

Template OpenForge game crate. **Don't edit directly** — copy it via the scaffolder:

```powershell
cargo run -p openforge-cli -- new-game --id <slug> --name "<Display name>" --engine <kind> --format <toml|ron>
```

`--engine` is the manifest `[engine].kind` (validated against the registered engine backends); `--format` is the default signature format. The scaffolder substitutes the `__TEMPLATE_*__` placeholders, writes a schema-2 `[engine]` block, and wires the new game into the workspace + `openforge-bundle`.

| Placeholder | Replaced with |
|---|---|
| `__TEMPLATE_NAME__` | `--name` |
| `__TEMPLATE_TAGLINE__` | `--tagline` (or a default) |
| `__TEMPLATE_PROCESS__` | the game's `.exe` name |
| `__TEMPLATE_DLL_NAME__` | the injected DLL stem |

`[engine].kind` + `config_format` are set from `--engine` / `--format`. The `_template` dir compiles cleanly so you can verify the scaffolding shape before copying. Full walkthrough: [docs/GAME-AUTHORING.md](../../../docs/GAME-AUTHORING.md).
