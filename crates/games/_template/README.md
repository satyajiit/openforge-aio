# openforge-game-template

Template OpenForge game crate. **Do not edit directly.** Copy this directory via:

```
cargo run -p openforge-cli -- new-game --id <slug> --name "<Display name>"
```

The scaffolder substitutes the `__TEMPLATE_*__` placeholders below and wires the
new game into the workspace + `openforge-bundle`.

## Placeholders substituted

| Placeholder | Replaced with |
|-------------|---------------|
| `__TEMPLATE_NAME__` | The `--name` value |
| `__TEMPLATE_TAGLINE__` | The `--tagline` value (or a sensible default) |
| `__TEMPLATE_PROCESS__` | The `--process` value (the game's `.exe` name) |

The `_template` directory itself compiles cleanly so contributors can verify the
scaffolding shape before copying.
