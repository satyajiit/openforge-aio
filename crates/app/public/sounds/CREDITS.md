# Hotkey activation sounds

These three files are played when a global hotkey toggles or fires a
feature. See `crates/app/src/lib/sounds.ts` for the playback path.

All three are by **Breviceps** on Freesound, released under
**Creative Commons 0 (CC0 1.0 Universal — Public Domain Dedication)**.
No attribution is legally required; provenance is recorded here for
maintainers.

| File              | Original title  | Freesound URL                                   |
| ----------------- | --------------- | ----------------------------------------------- |
| `hotkey-on.mp3`   | Blip Wave       | https://freesound.org/people/Breviceps/sounds/452998/ |
| `hotkey-off.mp3`  | Reverse Blip    | https://freesound.org/people/Breviceps/sounds/450612/ |
| `hotkey-fire.mp3` | Normal click    | https://freesound.org/people/Breviceps/sounds/448086/ |

License text: https://creativecommons.org/publicdomain/zero/1.0/

To replace any of these, drop a new file in this directory under the
same name (or update `SOURCES` in `sounds.ts`). Keep new files short
(< 200ms) and normalize loudness so toggles don't startle.
