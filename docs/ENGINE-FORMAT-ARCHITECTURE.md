# OpenForge — Config-Driven Engine + Format Architecture

> Status: **Design — source of truth for implementation.**
> Owners: runtime + app + host/DLL.
> Supersedes the ad-hoc engine inference in `commands.rs` and the TOML-only assumptions across `build.rs` / `runtime`.

This document specifies how OpenForge moves from **two hard-coded engines and one hard-coded config format** to a **declarative, registry-driven** model where:

- a game **declares** its engine (`ue5` / `glacier2` / future) and its config format in the manifest,
- the right host + injected DLL are **selected from that declaration** (not inferred from a DLL filename string),
- a **second config format (RON)** is additive and reuses the existing `serde` schema verbatim,
- hex bit-patterns like `0xDEADBEEF` deserialize correctly into `i32`/`u32`/`i64` in **both** formats,
- the working **UE5 (batman-lod)** and **Glacier (007 god_mode)** paths and every shipped `.toml` keep working byte-for-byte.

---

## 1. Goal & non-goals

### Goals (the user's requirements, cited)

- **R1 — Declared engine + format.** The game definition file must declare its engine type (`ue5` / `glacier2` / future) **and** its config/meta format (`toml` / new). The host + hooks (DLL) are selected from that declaration. Truly config-driven.
- **R2 — A second, more-flexible, future-ready config format** beyond TOML.
- **R3 — Hex bit-patterns must deserialize correctly.** TOML cannot express negative/bit-pattern hex: `-0x1` is a parse error, and a bare `0xDEADBEEF` parses as `+3735928559`, which overflows `i32`. Authors are currently forced to hand-convert to signed decimal (`-559038737`) — a correctness hazard for `heap_scan` fingerprints/sentinels. An author must be able to write `0xDEADBEEF` and have it deserialize into `i32`/`u32`/`i64` by **bit reinterpretation**, and write **negative hex**.
- **R4 — Keep comments** (disqualifies raw JSON).
- **R5 — Future expressiveness:** room for computed offsets / simple conditionals.
- **R6 — Do not break** the working UE5 (batman-lod) and Glacier (007 god_mode) paths. Existing `.toml` signatures keep working. The new format is **additive**.
- **R7 — CI gates stay green:** `cargo fmt`, `cargo clippy -D warnings`, `cargo test --lib`, `openforge-cli verify-registry`, frontend `typecheck` + `build`.

### Non-goals

- **No whole-file programmable signatures (Lua/Rhai) in this work.** A Turing-complete signature file breaks the **R7 static-decidability contract**: `verify-registry` is a *parse + typecheck* pass and the frontend renders controls **offline** from the parsed spec. We cannot prove a script yields a valid `SignatureSpec` without executing untrusted author code in the CI binary. We **reserve** the seam for a future `lua`/`rhai` format that **must evaluate to a static `SignatureSpec` at parse/verify time and must never execute during resolve** (see §9, Open Question O3).
- **No rewrite of the resolve/feature engine.** Both formats produce the **identical** `SignatureSpec`; downstream code stays format-blind.
- **No change to the wire protocols** (`ue5-protocol`, `glacier-protocol`) beyond the one generalized `freeze` op (§6.3), which is landed behind the migration gate and re-validated live.

### Governing principle

> **Inferred capability is the disease.** `commands.rs:227` infers the engine from a DLL filename because `manifest.rs` has no field to declare it (E2). `build.rs:177` / `signature.rs:42` / `manifest.rs:56` hardwire TOML because nothing declares the format. **Fix the declaration layer once**, feed two trivial registries, and both classes of inference die.

---

## 2. The decision — second config format

### 2.1 Verdict: **RON (Rusty Object Notation)** + a format-independent `FlexInt` bit-reinterpret newtype

We were delegated the choice. **It is RON, decisively**, paired with a small `hex_bits` / `FlexInt<T>` `serde` helper that solves R3 **independently of the format**.

Two separable decisions, deliberately decoupled:

1. **The format** (R2/R4): **RON**. It is `serde`-native, so `ron::from_str::<SignatureSpec>` is a true drop-in for the existing `toml::from_str`. RON's docs confirm it supports **all** `serde` enum representations (externally / internally / adjacently tagged, untagged). The existing `#[serde(tag = "strategy")]` `WriteSpec`, `#[serde(tag = "kind")]` `ControlSpec`, `#[serde(tag = "kind", content = "value")]` `PredicateSpec`, and untagged `Preset` (the F6 enums) round-trip **verbatim**. Adding the format is **one registry row + a derive**, not a parallel schema. RON keeps comments (`//`, `/* */`).
2. **R3 hex** is solved **by the schema, not the grammar** — a `FlexInt<T>` newtype with a custom `Deserialize`. This is deliberate:
   - it makes the fix **identical in TOML and RON**, and
   - it **retrofits** correct hex into the **already-shipped `.toml`** corpus, closing the documented `heap_scan` hand-conversion hazard *today*.

This decoupling is the key insight that decides the format war: **once a newtype solves R3, the format choice carries no R3 burden.** KDL's prettier native unquoted `-0x1` ergonomics buy nothing extra — and KDL costs a hand-written DOM→serde bridge (its leading derive crate `knus` is **not** `serde`-based) that would have to reproduce all three tagged-enum styles and prove byte-identity against the working batman signatures. That is the exact per-format schema coupling this whole exercise exists to remove.

### 2.2 Comparison table

| Criterion | **TOML (keep, default)** | **RON (chosen 2nd)** | KDL (rejected) | JSON5 (rejected) | Lua/Rhai whole-file (deferred) |
|---|---|---|---|---|---|
| Comments (R4) | ✅ `#` | ✅ `//`, `/* */` | ✅ `//`, `/*`, slashdash `/-` | ✅ | ✅ |
| `serde`-native drop-in for `toml::from_str` | — (is the baseline) | ✅ true drop-in | ❌ `knus` not serde-based → bespoke DOM→serde bridge | ⚠️ `serde_json5`, but numbers via f64/std int | ❌ not a deserialize target |
| Round-trips F6 tagged enums verbatim | ✅ | ✅ (docs: all serde tag modes) | ❌ must hand-pin every discriminator | ⚠️ untagged/internal OK, fragile | ❌ |
| Native signed/bit hex in grammar (R3) | ❌ `-0x1` parse error; `0xDEADBEEF`→`+3.7e9` overflow on i32 | ⚠️ lexes `0xDEAD_BEEF`/`-0x1` but **range-errors** into `i32` | ✅ i128 DOM holds every literal | ⚠️ f64-shaped, float coercion risk | ✅ (but 64-bit only, no i32) |
| **R3 actually solved** | ✅ via `FlexInt` newtype | ✅ via **same** `FlexInt` newtype | ✅ in-grammar, but only inside KDL | ⚠️ needs same newtype anyway | n/a |
| Retrofits hex fix into existing `.toml` | ✅ (newtype) | ✅ (newtype) | ❌ (grammar fix is KDL-only) | ⚠️ | n/a |
| Cost to add (engine of regression) | — | **1 registry row + 1 derive** | bespoke bridge = standing regression vector vs F6 | new non-Rust dep, no gain over RON | sandbox + execute-to-validate → breaks R7 |
| New dependency | already in tree | `ron` (host/runtime side only) | `kdl` + `knus`/hand bridge | `serde_json5` | `mlua` (already in tree) |
| R7 verify-registry stays a pure static parse | ✅ | ✅ | ✅ | ✅ | ❌ would have to execute |

**Why not KDL** (it was a serious contender and the runner-up in two panels): KDL's in-grammar i128 model is a genuinely cleaner *in-format* R3 mechanism, but it is neutralized the moment we have `FlexInt`. Against that, KDL's only `serde` story is non-`serde` (`knus`) or a hand-written DOM→serde bridge that must faithfully reproduce `WriteSpec`/`PredicateSpec`/`Preset` and prove byte-identity with the TOML path — a large, permanent regression surface against R6. Paying that to win an R3 round `FlexInt` already wins for free is the wrong trade.

**Why not JSON5 / raw JSON:** JSON5 numbers funnel through f64/standard integer parsing (overflow/float-coercion on `0xDEADBEEF`→i32), it needs the same newtype anyway, and adds a non-Rust-native dep for no gain over RON. Raw JSON fails R4 (no comments).

**Why Lua/Rhai is deferred, not chosen:** strongest for R5, and `mlua` is already in-tree via `crates/ue5-lua` — but a Turing-complete *definition* format destroys R7's static-decidability contract and the offline-UI render contract. We keep the `ConfigFormat` seam open for it under the iron rule in O3.

### 2.3 Worked example — writing `0xDEADBEEF` as an `i32` fingerprint (R3)

A `heap_scan` fingerprint sentinel is a **32-bit pattern**, not an arithmetic value. The author means *“these 32 bits.”* The field's Rust type is `i32`. We want `0xDEADBEEF` → `-559038737` (the `i32` bit-reinterpretation), never `+3735928559`.

**The newtype** (in `crates/runtime/src/value.rs`):

```rust
/// Accepts an integer token OR a string ("0xDEADBEEF" / "-0x1" / "0b…" / "0o…" / "-17").
/// Strings are parsed by MAGNITUDE then BIT-REINTERPRETED into the target width.
/// Integer tokens delegate to T's own Deserialize unchanged (so existing decimals are byte-stable).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FlexInt<T>(pub T);

// Width-specific helpers used via #[serde(deserialize_with = ...)] where bit-pattern intent is explicit.
pub mod hex_bits {
    pub fn i32<'de, D: serde::Deserializer<'de>>(d: D) -> Result<i32, D::Error> { reinterp::<D, _>(d, |u| u as i32) }
    pub fn u32<'de, D: serde::Deserializer<'de>>(d: D) -> Result<u32, D::Error> { reinterp::<D, _>(d, |u| u as u32) }
    pub fn i64<'de, D: serde::Deserializer<'de>>(d: D) -> Result<i64, D::Error> { reinterp::<D, _>(d, |u| u as i64) }
    pub fn u64<'de, D: serde::Deserializer<'de>>(d: D) -> Result<u64, D::Error> { reinterp::<D, _>(d, |u| u as u64) }

    // reinterp(): if the token is an integer, delegate to the target type's Deserialize (unchanged path).
    // If it is a string: strip optional leading '-' and 0x/0b/0o, parse the MAGNITUDE as u128,
    // apply wrapping_neg() when signed, then cast `as` the target width. Width is chosen by which
    // helper the FIELD uses (type-directed) — never inferred, so sign/width can't be wrong.
}
```

**Parse trace for `0xDEADBEEF` into an `i32` field:**

```
"0xDEADBEEF"  →  strip "0x"  →  magnitude = 0xDEADBEEF = 3_735_928_559 (u128)
              →  no leading '-'  →  cast as u32 = 0xDEAD_BEEF
              →  as i32 = -559_038_737   ✅  (exact bit reinterpretation)
```

**Parse trace for `-0x1` into a `u32` field:**

```
"-0x1"  →  leading '-', strip "0x"  →  magnitude = 1
        →  signed: 1u32.wrapping_neg() = 0xFFFF_FFFF
        →  as u32 = 0xFFFF_FFFF = 4_294_967_295   ✅
```

**TOML today (the hazard):**

```toml
[heap_scan]
value = -559038737   # author was forced to hand-convert from 0xDEADBEEF — error-prone
```

**TOML after this change (hazard closed, no new format needed):**

```toml
[heap_scan]
value = "0xDEADBEEF"   # quoted; FlexInt's string arm bit-reinterprets to i32 = -559038737
```

**RON after this change (native, additive):**

```ron
// crates/games/glacier-007/signatures/example.ron
HeapScan(
    // RON also lexes the unquoted token; the field's hex_bits::i32 helper does the reinterpret.
    value: "0xDEADBEEF",   // standardized quoted form for the guaranteed bit-reinterpret across both formats
    // negative bit patterns are now expressible:
    sentinel: "-0x1",      // → i32 = -1
)
```

> **Standardization rule:** authors write the **quoted string form** (`"0xDEADBEEF"`, `"-0x1"`) for any bit-pattern field, in **both** formats. RON additionally accepts the bare integer token, but the quoted form is the one we document and lint, because it is the form whose bit-reinterpret semantics are *guaranteed identical* across TOML and RON. A bare `0xDEADBEEF` integer token in RON **range-errors** into an `i32` field — exactly why the helper, not the grammar, owns R3.

`verify-registry` lints a `FlexInt` field whose magnitude exceeds the target width even after reinterpretation (e.g. a 33-bit literal on a `hex_bits::i32` field) → CI failure, not a silent truncation.

---

## 3. Manifest v2 schema

A game declares its engine and (optionally) a default config format in a new `[engine]` block. **Absence = legacy defaults** (engine inferred as today, format = TOML), so no shipped manifest must change to keep working (R6). We additionally add an explicit `schema` version for unambiguous legacy gating (grafted from the schema-first proposal) rather than relying solely on block-absence.

### 3.1 New `serde` shape — `crates/runtime/src/manifest.rs`

```rust
#[derive(Deserialize, Serialize, Default)]
pub struct GameManifest {
    #[serde(default)] pub engine: EngineDecl,   // NEW; Default => legacy
    pub game: GameManifestBody,                 // unchanged
    // …existing [process], [[versions]], [icon] …
}

#[derive(Deserialize, Serialize, Default)]
pub struct EngineDecl {
    /// Absent on schema=1 manifests; defaults to 1.
    #[serde(default = "schema_v1")] pub schema: u8,
    /// Required when schema >= 2. None on legacy → inferred (legacy_engine_for()).
    #[serde(default)] pub kind: Option<EngineKind>,
    /// Default config format for this game's signatures dir. Per-file extension still wins.
    #[serde(default)] pub config_format: ConfigFormat,   // Toml by default
    /// Optional override of the registry's default DLL name for this engine.
    #[serde(default)] pub dll: Option<String>,
    /// Engine-namespaced constants live here as DATA (fixes E4: no more consts in runtime).
    #[serde(default)] pub ue5: Option<Ue5EngineConfig>,
    #[serde(default)] pub glacier2: Option<Glacier2EngineConfig>,
}
fn schema_v1() -> u8 { 1 }

#[derive(Deserialize, Serialize, Clone, Copy, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum EngineKind { Ue5, Glacier2 }   // open for #[serde(other)]-style future variants via registry lookup

#[derive(Deserialize, Serialize, Clone, Copy, Default)]
#[serde(rename_all = "snake_case")]
pub enum ConfigFormat { #[default] Toml, Ron }   // open for Kdl/Lua later

#[derive(Deserialize, Serialize, Default)]
pub struct Ue5EngineConfig {
    /// Was the hardcoded UOBJECT_CLASS_PRIVATE_OFFSET=0x10 at runtime/feature.rs:20 (E4 fix).
    #[serde(default = "default_uobject_class_private_offset")] pub uobject_class_private_offset: u32,
    // (SetProgressTags layout consts from feature.rs:574 also move here as data — see §6.4)
}

#[derive(Deserialize, Serialize, Default)]
pub struct Glacier2EngineConfig {
    #[serde(default = "default_type_registry_offset")] pub type_registry_offset: u32, // e.g. 0x38
}
```

> Putting engine constants in the manifest as **data** (not just relocating them to a Rust module) is strictly more extensible: a future build of the same engine with a shifted offset is a manifest edit, not a recompile.

### 3.2 Exact new manifests

**`crates/games/batman-lod/manifest.toml` (UE5):**

```toml
[engine]
schema = 2
kind = "ue5"
config_format = "toml"          # explicit; could be omitted (defaults to toml)
# dll omitted → registry default (Ue5Backend::default_dll_name())

[engine.ue5]
uobject_class_private_offset = 0x10   # was hardcoded in runtime/feature.rs:20

# [game] block is UNCHANGED from today (verbatim from the live manifest):
[game]
id                 = "batman-lod"
display_name       = "LEGO Batman: Legacy of the Dark Knight"
tagline            = "Single-player. Local writes only."
process_names      = ["LEGOBatmanLotDK-Win64-Shipping.exe", "LEGOBatmanLotDK.exe"]
primary_module     = "LEGOBatmanLotDK-Win64-Shipping.exe"
supported_versions = ["1.0.0.1"]
forbidden_services = []
requires_admin     = false
sort_order         = 100
dll_file_name      = "batman_lod_dll.dll"   # retained one release; engine.dll falls back to it (§3.3)

[icon]
path = "assets/icon.png"
```

**`crates/games/glacier-007/manifest.toml` (Glacier 2):**

```toml
[engine]
schema = 2
kind = "glacier2"
config_format = "toml"          # game still authored in TOML; opt into .ron per-file later
dll = "glacier_007_dll.dll"     # explicit (was the magic string the engine was inferred FROM)

[engine.glacier2]
type_registry_offset = 0x38   # ZTypeRegistry m_types offset (was implicit in glacier-007-dll)

# [game] block is UNCHANGED from today (verbatim from the live manifest):
[game]
id                 = "glacier-007"
display_name       = "007 First Light"
tagline            = "Single-player. Local writes only."
process_names      = ["007FirstLight.exe"]
primary_module     = "007FirstLight.exe"
supported_versions = []
forbidden_services = []
requires_admin     = false
sort_order         = 200
dll_file_name      = "glacier_007_dll.dll"   # retained one release; engine.dll falls back to it (§3.3)

[icon]
path = "assets/icon.png"
```

> Note: `glacier-007` currently ships `supported_versions = []` (Denuvo build churn). The `[engine]` block is fully orthogonal to that — version handling is unchanged by this design.

A `config_format` declared at manifest level is the **default**; an individual signature file's extension still wins, so a game can mix `.toml` and `.ron` during migration. `verify-registry` errors if a file's extension format mismatches the game's declared `config_format` only when the manifest explicitly **forces** a format (open question O5 — default is permissive mixing).

### 3.3 The `dll_file_name` deprecation shim

The existing top-level `GameManifestBody.dll_file_name` (`manifest.rs:39`) is **retained for one release** with its current `#[serde(default)]`. Resolution order at attach:

```
dll_name = manifest.engine.dll                        // new, preferred
        .or(manifest.game.dll_file_name if non-empty) // legacy field, deprecation window
        .unwrap_or(backend.default_dll_name())        // registry default
```

`legacy_engine_for(game_id)` (the schema=1 fallback in §4.3) infers `EngineKind` from the legacy `dll_file_name` string **exactly as today** — this is the *only* place the old filename inference survives, quarantined behind `schema < 2`, and `verify-registry` warns whenever it fires. Once both shipped manifests carry `schema = 2` + `[engine].kind` (Phase 3), the legacy path is dead code and `dll_file_name` can be dropped.

---

## 4. EngineBackend abstraction

### 4.1 New crate: `openforge-engine` (`crates/engine/`)

Holds **only** the dispatch trait + registry. Depends on `openforge-core` (for `Ctx`) and `openforge-runtime` (for `EngineKind`, `EngineDecl`). **No win32, no concrete engine crate** — dependency arrows point inward; the host crates register themselves.

```rust
// crates/engine/src/lib.rs
use std::{path::Path, sync::Arc};
use openforge_core::Ctx;
use openforge_runtime::manifest::{EngineKind, EngineDecl};

pub trait EngineBackend: Send + Sync + 'static {
    fn kind(&self) -> EngineKind;
    fn default_dll_name(&self) -> &'static str;          // replaces the literal at commands.rs:227

    /// Inject DLL + handshake + build a Ctx-implementing session, boxed so the app
    /// never names Ue5Session / GlacierSession concretely.
    fn attach(&self, pid: u32, dll_path: &Path, decl: &EngineDecl)
        -> anyhow::Result<Arc<dyn EngineSession>>;
}

/// The single opaque handle the app holds. Ctx is the existing core seam.
pub trait EngineSession: Ctx + Send + Sync {
    fn engine_kind(&self) -> EngineKind;
    fn main_module_base(&self) -> u64;                   // was Session::main_module_base() (attach.rs)
}

// Same inventory mechanism the codebase already uses for register_game!.
pub struct EngineRegistration { pub kind: EngineKind, pub make: fn() -> Box<dyn EngineBackend> }
inventory::collect!(EngineRegistration);

#[macro_export]
macro_rules! register_engine {
    ($ty:ty) => {
        inventory::submit! {
            $crate::EngineRegistration { kind: <$ty>::KIND, make: || Box::new(<$ty>::default()) }
        }
    };
}

pub fn backend_for(kind: EngineKind) -> Option<Box<dyn EngineBackend>> {
    inventory::iter::<EngineRegistration>().find(|r| r.kind == kind).map(|r| (r.make)())
}
```

### 4.2 Concrete backends (live in the host crates, which already link win32)

```rust
// crates/ue5-host/src/backend.rs
#[derive(Default)] pub struct Ue5Backend;
impl Ue5Backend { pub const KIND: EngineKind = EngineKind::Ue5; }
impl EngineBackend for Ue5Backend {
    fn kind(&self) -> EngineKind { EngineKind::Ue5 }
    fn default_dll_name(&self) -> &'static str { "batman_lod_dll.dll" }
    fn attach(&self, pid, dll, decl) -> anyhow::Result<Arc<dyn EngineSession>> {
        Ok(Arc::new(Ue5Session::open(pid, dll, decl.ue5.clone().unwrap_or_default())?))
    }
}
openforge_engine::register_engine!(Ue5Backend);   // also `impl EngineSession for Ue5Session`

// crates/glacier-host/src/backend.rs
#[derive(Default)] pub struct Glacier2Backend;
impl Glacier2Backend { pub const KIND: EngineKind = EngineKind::Glacier2; }
impl EngineBackend for Glacier2Backend {
    fn kind(&self) -> EngineKind { EngineKind::Glacier2 }
    fn default_dll_name(&self) -> &'static str { "glacier_007_dll.dll" }
    fn attach(&self, pid, dll, decl) -> anyhow::Result<Arc<dyn EngineSession>> {
        Ok(Arc::new(GlacierSession::open(pid, dll, decl.glacier2.clone().unwrap_or_default())?))
    }
}
openforge_engine::register_engine!(Glacier2Backend);   // also `impl EngineSession for GlacierSession`
```

`crates/bundle` adds `pub use openforge_ue5_host as _;` and `pub use openforge_glacier_host as _;` so their `inventory::submit!`s are linked (mirrors how games are linked today).

### 4.3 Attach dispatch — replacing `commands.rs:227`

**Before** (engine inferred from a DLL filename string — E1):

```rust
let is_glacier = dll_file_name == "glacier_007_dll.dll";   // commands.rs:227
// …then two ~60-line match arms + Session::ue5()/glacier() accessors (E3)…
```

**After** (manifest-driven):

```rust
let kind = manifest.engine.kind
    .unwrap_or_else(|| legacy_engine_for(&manifest.game.id));         // schema=1 fallback
let backend = openforge_engine::backend_for(kind)
    .ok_or_else(|| AppError::Other(format!("no backend for engine {kind:?}")))?;
let dll_name = manifest.engine.dll.as_deref().unwrap_or(backend.default_dll_name());
let dll_path = resolve_dll_path(dll_name)?;
let session: Arc<dyn EngineSession> =
    spawn_blocking(move || backend.attach(pid, &dll_path, &manifest.engine)).await??;
```

### 4.4 Types created / changed, by crate

| Type / item | Crate | Status |
|---|---|---|
| `EngineBackend`, `EngineSession`, `EngineRegistration`, `register_engine!`, `backend_for` | **`openforge-engine` (new)** | new |
| `Ue5Backend` (+ `impl EngineSession for Ue5Session`) | `openforge-ue5-host` | new |
| `Glacier2Backend` (+ `impl EngineSession for GlacierSession`) | `openforge-glacier-host` | new |
| `EngineKind`, `ConfigFormat`, `EngineDecl`, `Ue5EngineConfig`, `Glacier2EngineConfig` | `openforge-runtime` (`manifest.rs`) | new |
| `Session` enum + `Session::ue5()` / `Session::glacier()` accessors | `openforge-app` (`attach.rs:38/61`) | **deleted** |
| `Attached.session` field type → `Arc<dyn EngineSession>` | `openforge-app` (`attach.rs`) | changed |
| `is_glacier = dll_file_name == …` | `openforge-app` (`commands.rs:227`) | **deleted** |
| Engine-matching at call sites `commands.rs:526/828/1367` | `openforge-app` | replaced with plain `Ctx`/`EngineSession` calls |
| `pub use openforge_ue5_host as _; pub use openforge_glacier_host as _;` | `openforge-bundle` | new |

The 3 engine-gated call sites (`commands.rs:526` UE5 read-probe, `:828` UE5 pending-retry, `:1367` Glacier copy-freeze) are handled without downcast: the reflection ops are already on `Ctx` and default to `Err("not supported")`; only `Ue5Session` overrides them, and Glacier copy-freeze becomes a generic `Ctx::freeze(FreezeMode::CopySibling{..})` (§6.3). The app stops branching on engine.

---

## 5. Format-parser abstraction

### 5.1 The seam — `crates/runtime/src/format.rs`

One registry, keyed by `ConfigFormat` / file extension, producing the **same** `SignatureSpec` and `GameManifest`. Neutralizes F1–F6 without forking the schema.

```rust
// crates/runtime/src/format.rs
use serde::de::DeserializeOwned;

pub trait SpecFormat: Send + Sync {
    const FORMAT: ConfigFormat;
    fn extensions(&self) -> &'static [&'static str];
    fn parse<T: DeserializeOwned>(&self, src: &str) -> Result<T, FormatError>;  // ANY serde target
}

pub fn format_for_ext(ext: &str) -> Option<&'static dyn SpecFormat> { /* registry scan */ }
pub fn format_for(kind: ConfigFormat) -> &'static dyn SpecFormat { /* registry scan, Toml default */ }

pub fn parse_str<T: DeserializeOwned>(src: &str, fmt: ConfigFormat) -> Result<T, FormatError> {
    format_for(fmt).parse::<T>(src)
}
```

Two impls (registered via `inventory`, same pattern):

```rust
pub struct TomlFormat;
impl SpecFormat for TomlFormat {
    const FORMAT: ConfigFormat = ConfigFormat::Toml;
    fn extensions(&self) -> &'static [&'static str] { &["toml"] }
    fn parse<T: DeserializeOwned>(&self, s: &str) -> Result<T, FormatError> {
        toml::from_str(s).map_err(FormatError::from)     // BYTE-FOR-BYTE the current path
    }
}
pub struct RonFormat;
impl SpecFormat for RonFormat {
    const FORMAT: ConfigFormat = ConfigFormat::Ron;
    fn extensions(&self) -> &'static [&'static str] { &["ron"] }
    fn parse<T: DeserializeOwned>(&self, s: &str) -> Result<T, FormatError> {
        ron::from_str(s).map_err(FormatError::from)
    }
}
```

`TomlFormat` registers first and is the default — every shipped `.toml` flows through the **unchanged** `toml::from_str` (R6). The F6 tagged enums (`WriteSpec`/`ControlSpec`/`PredicateSpec`/`Preset`) deserialize through both because both targets are the **same** `serde` derives.

### 5.2 Routing the four hardwired sites

- **`SignatureSpec::parse` (`signature.rs:42`, F4)** and **`GameManifest::parse` (`manifest.rs:56`, F3)** take a `ConfigFormat` (default `Toml`) and delegate to `parse_str`. Existing `include_str!`-based test fixtures keep an explicit `parse_toml` shim.
- **`build.rs:70` (F2):** manifest parsed via `format_for_ext(manifest_path.extension())` (currently `toml::from_str(&manifest_text)`).
- **`build.rs:177` (F1):** `enumerate_signatures()` widens its filter from `path.extension() != Some("toml")` to `format_for_ext(ext).is_some()`. The generated `register_game!` output is identical regardless of source format; `build.rs` embeds the raw source + its format, and runtime re-parses via the registry.

> **Rename the format-coupled embed field.** The generated code today emits `DeclFeatureSrc { name, toml: <content> }` (`build.rs:160-162`) — the field is literally named `toml`, a leak. Rename to `DeclFeatureSrc { name, source, format }` where `format: ConfigFormat` is derived from the file extension at build time. `DeclarativeFeature::from_toml(src)` (runtime) becomes `from_source(src, format)` and dispatches through `format_for(format)`. Keep a thin `from_toml` shim (`from_source(s, ConfigFormat::Toml)`) for the test fixtures.

### 5.3 Kill the triplicated build script (F5)

The two live `build.rs` files and `_template/build.rs` are byte-identical, and each one **re-duplicates the manifest schema** ("The schema is duplicated here … to keep build-time dependencies tiny" — `build.rs:4-5`, with its own `struct Manifest`/`GameBody` mirror at `build.rs:13-39`). Extract the walker + codegen into a tiny **`openforge-buildgen`** crate (build-dependency only; deps kept light: `serde` + `toml` + `ron`). That kills both the triplication **and** the schema mirror in one move — `openforge-buildgen` gains the `[engine]`/`[format]` fields once, so future games inherit them. Each `build.rs` becomes:

```rust
// crates/games/<id>/build.rs
fn main() { openforge_buildgen::generate().expect("codegen"); }
```

Adding a future format then touches the walker **once**, not three copies.

---

## 6. Schema refactor — de-leak engine-specific blocks (fix S1)

Today `WriteSpec` (`signature.rs:414-415`, `#[serde(tag = "strategy", rename_all = "snake_case")]`) is one flat enum that mixes engine-agnostic variants (`OneShot`, `Freeze`, `CodePatch`) with **UE5-reflection-only** variants (`SetProgressTags`, `CallInstanceUFunction`, `TeleportToWaypoint`, `FreezeForMatching`, `PlayAnimMontageForMatching` — all enumerated in `SignatureSpec::effective_kind`, `signature.rs:49-55`) plus the top-level `reflection: Option<ReflectionSpec>` field (`signature.rs:33`); and `Freeze`'s `freeze_copy_offset: Option<i64>` (`signature.rs:438`) is **Glacier-DLL-only** sitting in the shared `Freeze` variant. A Glacier signature drags UE5 vocabulary into scope.

The F6 tagged enums that must survive a format swap, with confirmed locations:

| Enum | Tagging | Location |
|---|---|---|
| `WriteSpec` | internally tagged `tag = "strategy"` | `signature.rs:414` |
| `ControlSpec` | internally tagged `tag = "kind"` | `signature.rs:319` |
| `Preset` | untagged | `signature.rs:362` |
| `PredicateSpec` | adjacently tagged `tag = "kind", content = "value"` | `signature.rs:1034` |

RON's docs confirm all four representations round-trip; Phase 6 gates on a per-enum round-trip test before any live signature is ported.

### 6.1 Generic core stays minimal

`WriteSpec` keeps **only** engine-portable variants — they operate on a raw address + bytes and work on any engine:

```rust
#[serde(tag = "strategy", rename_all = "snake_case")]
pub enum WriteSpec {
    OneShot,
    Freeze(FreezeSpec),     // freeze_copy_offset REMOVED (→ generic FreezeMode, §6.3)
    CodePatch(CodePatchSpec),
}
```

Locator blocks stay generic where they truly are: `[signature]` (AOB), `[heap_scan]` (structural fingerprint — Glacier already uses it). Only `[reflection]` is UE5-specific and moves out.

### 6.2 Engine-namespaced extension bucket

```rust
pub struct SignatureSpec {
    pub meta: Meta,
    #[serde(default)] pub value: Option<ValueSpec>,
    pub write: WriteSpec,                                  // generic only
    #[serde(default)] pub signature: Option<AobSpec>,
    #[serde(default)] pub heap_scan: Option<HeapScanSpec>,
    #[serde(default)] pub control: Option<ControlSpec>,
    #[serde(default, flatten)] pub engine: EngineExt,      // namespaced extras
}

#[serde(tag = "engine", rename_all = "snake_case")]
pub enum EngineExt {
    Ue5(Ue5Ext),          // [reflection] + the 5 UE5-only write ops (now Ue5Write)
    Glacier2(GlacierExt), // future Glacier-only ops
    #[serde(other)] None, // generic signatures (most of them) carry nothing
}
```

`Ue5Ext` absorbs `reflection: Option<ReflectionSpec>` and the five UE5 write ops (renamed `Ue5Write`). The generic core then names **no** engine.

> **Backward-compat (R6) — no file edits.** The existing flat strategy strings (`set_progress_tags`, etc.) are unique. We keep them parseable via `#[serde(alias = "…")]` during a deprecation window, so **not a single shipped `.toml` is edited**. The Phase-0 golden round-trip over the real corpus is the hard gate.

### 6.3 De-leak `freeze_copy_offset` by **generalizing**, not relocating

Instead of moving `freeze_copy_offset` into a `GlacierExt` field, promote the concept to a generic `Ctx` op so the **app and schema never name "Glacier"** and engine #3 inherits copy-freeze for free:

```rust
// crates/core/src/ctx.rs
pub enum FreezeMode {
    Constant,                          // freeze to a fixed value (today's default)
    CopySibling { source_offset: i64 },// copy a sibling field each tick (was Glacier freeze_copy_offset)
}
pub trait Ctx {
    fn freeze(&self, addr: u64, value: &[u8], mode: FreezeMode) -> CtxResult<()> { /* default */ }
}
```

`FreezeSpec` carries `mode = "constant" | "copy_sibling"` + `source_offset`; the DLL decides semantics; the app calls `ctx.freeze(..)` unconditionally. **This re-expresses the working Glacier protocol-v4 freeze**, so it lands **after** the golden gate and is **re-validated live** on 007 god_mode (see Phase 4 + Risk).

### 6.4 Move misplaced runtime constants (E4) + Ctx vocabulary (E5)

- `UOBJECT_CLASS_PRIVATE_OFFSET = 0x10` (`feature.rs:20`) and the `SetProgressTags` layout consts (`feature.rs:574`) are UE5-only. They move out of generic runtime: their **values** become manifest data (`[engine.ue5]`, §3.1) and their **interpretation** moves into a `runtime::engines::ue5` module under the `Ue5Ext` arm.
- `Ctx::find_uobject` / `call_ufunction` (`ctx.rs:178/263`) get engine-neutral names (`find_object` / `call_function`, `resolve_property`→`resolve_field`) with `#[deprecated]` thin aliases for one release. Their default-`Err` bodies already prove they're optional-per-impl, so neutral names cost nothing.

### 6.5 verify-registry cross-check

A signature's `EngineExt` variant **must** match its game manifest's declared `[engine].kind`. A UE5 reflection block dropped into a Glacier game fails at **CI**, not at attach (R7).

---

## 7. De-duplication — shared crates (D1–D6)

007 was forked from UE5; the primitives are byte-duplicated. Extract two crates, split **by side of the pipe**. Pure mechanical moves, one module per commit, `cargo build` each target after each move.

### 7.1 `openforge-dll-common` (new lib; in-process, injected-DLL primitives; **zero engine semantics**)

Used by `batman-lod-dll` and `glacier-007-dll`; each deletes its copy.

| ID | Module move | Dest | Priority |
|---|---|---|---|
| D1 | `pe.rs` (PE/module/.text introspection) | `dll-common::pe` | **HIGH** |
| D2 | `seh.rs` + `seh.c` (SEH guard; `cc` build step lives here **once**) | `dll-common::seh` | MEDIUM |
| D3 | `panic_guard.rs` (`catch_unwind` request wrapper) | `dll-common::panic_guard` | MEDIUM |
| D4 | `local_reader.rs` (fault-isolated local reads) | `dll-common::local_reader` | MEDIUM |
| D5 | `log_ring.rs` (ring-buffer logging) | `dll-common::log_ring` | LOW |

Engine-specific walks **stay**: batman keeps `GUObjectArray` + `FName::ToString`; glacier keeps `ZTypeRegistry` walk + CRC32/FNV hashing + freeze thread. **Extract `glacier-007-dll/src/freeze.rs`'s shared primitives early** — gitStatus shows it untracked/mid-flight; pull the common parts into `dll-common` before it diverges further.

### 7.2 `openforge-host-common` (new lib; host-side injection + transport; **zero engine semantics**)

Used by `ue5-host` and `glacier-host`.

| ID | Move | Dest |
|---|---|---|
| D6a | `Injector` (LoadLibrary + remote thread + DLL-path resolve) | `host-common::injector` |
| D6b | `PipeHandle` (named-pipe open + length-prefixed postcard framing) | `host-common::transport` |
| D6c | Generic `Session<P: Protocol>` / `Ctx` scaffold (connect, handshake retry, request/response correlation) | `host-common::session` |

A `Protocol` trait (associated `Request`/`Response` + postcard encode/decode) lives in `host-common`; `ue5-protocol` and `glacier-protocol` impl it. **This is the structural precondition** that makes `EngineBackend::attach` build `EngineSession` over a single generic `Session<P>` — i.e. why engine #3's host is a thin impl, not a third Session/Ctx fork. The protocol enums and the engine semantics in `Ue5Session`/`GlacierSession` (UE5 offset handshake, PE-hash cache; Glacier freeze handles) stay in their crates.

---

## 8. Migration plan

Ordered, independently shippable, each phase keeps all CI gates green and both games working. **R** = reversible (pure addition or feature-flagged), **R\*** = reversible until the next phase consumes it.

| Phase | What | Reversible | Gate |
|---|---|---|---|
| **0 — Safety net** | Snapshot CI green. Add a **golden round-trip test**: parse all shipped signatures + all 3 manifests and assert the parsed structs are **byte-stable** (value-equality). Extend `verify-registry`'s existing re-parse to assert this. *This is the R6 proof every later phase must keep green.* | R | fmt/clippy/test/verify-registry/frontend |
| **1 — `FlexInt` (R3)** | Add `FlexInt`/`hex_bits` in `runtime/value.rs`; retype hex-bearing fields (`heap_scan.value` + sentinels, `Freeze` value). Integer arm delegates to `T` unchanged → existing decimals byte-stable. Allow quoted `"0x.."` in TOML *now*. Unit tests: `0xDEADBEEF`→i32 `-559038737`, `-0x1`→u32 `0xFFFFFFFF`, `-17`→i32 `-17`. | R | + new unit tests |
| **2 — Format seam** | Add `format.rs` (`SpecFormat` + registry + `TomlFormat`). Route `signature.rs:42`, `manifest.rs:56`, `build.rs:70/177` through it (F1–F4). Toml arm byte-for-byte. **Still TOML-only end-to-end.** Pure refactor. | R | golden gate |
| **3 — Manifest declares engine + format** | Add `EngineDecl` (E2) with `schema`/`kind`/`config_format` defaults; backfill both shipped manifests with explicit `[engine]`. `legacy_engine_for()` for schema=1. `verify-registry`: schema≥2 must set kind; declared kind must have a registered backend. **Nothing reads `kind` for dispatch yet.** | R | golden gate + new verify check |
| **4 — Engine backend registry** | Create `openforge-engine`; impl `Ue5Backend`/`Glacier2Backend`; `impl EngineSession`; bundle pub-uses both. Rewrite `attach.rs`/`commands.rs:227` to dispatch via `backend_for`; collapse `Session` enum → `Arc<dyn EngineSession>`; delete filename sniff (E1) + accessors (E3). Promote Glacier copy-freeze → generic `Ctx::freeze(FreezeMode::CopySibling)` (§6.3). **Live-validate batman currencies + 007 god_mode.** | R\* (enum collapse is the irreversible step; behind the gate) | golden gate + **live smoke both games** |
| **5 — Schema de-leak (S1) + E4/E5** | Split `WriteSpec` into generic core + `EngineExt{Ue5,Glacier2}`; move `[reflection]` + 5 UE5 ops under `Ue5Ext`; relocate `feature.rs:20`/`:574` consts to manifest data + `engines::ue5`; rename `Ctx` methods (E5) with `#[deprecated]` aliases. `serde` aliases keep every `.toml` parsing unchanged. verify-registry cross-checks engine vs manifest. **No signature file edited.** | R\* | golden gate (must show identical feature set before/after) |
| **6 — Add RON (R2)** | Add `ron` dep + `RonFormat` registration. Allow `signatures/*.ron`. Round-trip test each F6 enum style through RON. Port **one** non-critical glacier-007 signature to `.ron` and author a **new** `.ron` heap_scan using `"0xDEADBEEF"` to prove R3 end-to-end. Leave all working signatures as `.toml`. | R | golden gate + RON round-trip tests |
| **7 — Dedup (D1–D6)** | Extract `openforge-dll-common` (D1 first → D2–D5) and `openforge-host-common` (D6 last, most entangled). One module per commit; `cargo build` each DLL/host after each. **Live re-attach smoke both games.** | R (each move) | full CI + **live smoke** |
| **8 — Tooling + docs** | `new-game --engine <kind> --format <toml\|ron>` in the scaffolder; fold `_template/build.rs` into `openforge-buildgen` (F5); document RON+`FlexInt` authoring + the §10 walkthrough in `docs/PLAN.md`. | R | full CI |

**Why this ordering:** the safety net (P0) and the format-independent R3 fix (P1) land first with zero risk and immediate payoff; the heavy enum collapse (P4) and schema de-leak (P5) land **after** the golden gate exists; the new format (P6) only lands once the F6 enums are proven to round-trip; the mechanical dedup (P7) is last because D6 is entangled with the EngineBackend work from P4.

---

## 9. Risks & open questions

### Risks (with mitigations)

- **R-1 `FlexInt` touches the hottest struct.** A wrong `Deserialize` bound could silently change parsing of existing decimals. *Mitigation:* integer arm delegates to `T::deserialize` unchanged; gate with the Phase-0 golden value-equality test over the whole corpus.
- **R-2 RON tagged-enum fidelity (F6).** RON's docs claim all `serde` tag modes work, but `WriteSpec` (internally tagged), `PredicateSpec` (adjacently tagged), `Preset` (untagged) must all round-trip. *Mitigation:* per-variant round-trip test through both RON and TOML in Phase 6 **before** porting any live signature; if one fails, that field gets a small custom `(de)serialize`, not a schema fork.
- **R-3 Generalized `Ctx::freeze` re-expresses a working op.** The Glacier copy-freeze is the shipped protocol-v4 path. *Mitigation:* land in Phase 4 **after** the gate; live-re-validate 007 god_mode; keep the old code path until the new one is confirmed.
- **R-4 Inventory linkage is runtime, not compile-time.** If `bundle` forgets a host `pub use`, `backend_for(glacier2)` returns `None` at attach, not a build error. *Mitigation:* a test enumerating expected `EngineKind`s against the collected registry; a startup/`verify-registry` assertion that every enumerated kind has a backend.
- **R-5 `flatten` + internally-tagged `EngineExt`** is a known `serde` footgun. *Mitigation:* golden corpus gate; if it bites, fall back to a flat-on-disk-tag-string split (keep the same strategy strings, group only the Rust type).
- **R-6 Dedup touches SEH/panic_guard/local_reader** — fault-isolation primitives whose drift can crash the **game**, not just the trainer (`panic = "unwind"` + `catch_unwind` contract). *Mitigation:* diff the two forks first, document any intentional divergence, move byte-for-byte one module per commit, keep the single `cc` invocation in `dll-common`, live re-attach + DLL-iteration smoke on both games after Phase 7.
- **R-7 `ron` dep + Cargo profile.** Must not perturb `opt-level=3` deps / `panic = "unwind"`. *Mitigation:* `ron` is host/runtime-side only (never in the injected DLLs), so DLL `panic = "unwind"` is unaffected; confirm fmt/clippy clean.

### Open questions for the user

- **O1 — Manifest format.** Manifest stays **TOML** in this design (only signatures opt into RON). Confirm, or do you want the manifest itself authorable in RON? (Trivial via the same seam, but TOML manifests read cleanly and every shipped one is already TOML.)
- **O2 — Standardize on quoted hex.** We standardize on the **quoted-string** hex form (`"0xDEADBEEF"`) in both formats for the guaranteed bit-reinterpret. RON could allow bare `0xDEADBEEF` for `i64`/`u32` fields. Do you want bare hex permitted in RON where it's unambiguous, or quoted-everywhere for one rule?
- **O3 — Future programmable format.** We **reserve** a `ConfigFormat::Lua`/`Rhai` arm under the iron rule: *it must evaluate to a static `SignatureSpec` at parse/verify time and must never execute during resolve or `verify-registry`.* Confirm this is the boundary you want (vs. a narrow per-field `lua { … }` computed-scalar hatch, which we deliberately **excluded** because it dents R7's static-decidability). Computed offsets/conditionals (R5) would then live as static, declaratively-expressed forms (e.g. `base + 0x18 * index`) parsed into a small expression node — do you want that mini-expression schema specced now or deferred?
- **O4 — `EngineKind` extensibility.** Adding engine #3 needs a new `EngineKind` enum variant (one line in `runtime`). Acceptable, or do you want `EngineKind` to be a string newtype validated only against the registry (zero generic edits for engine #3, at the cost of compile-time exhaustiveness)?
- **O5 — Mixed-format games.** Default is permissive: a game can mix `.toml` and `.ron` during migration. Do you want `[engine].config_format` to be a *hard* constraint (any off-format file = CI error) instead?

---

## 10. Adding game #3 on engine #N — folder-drop walkthrough

Two scenarios prove the design.

### 10.1 Game #3 on an **existing** engine (e.g. another UE5 title, "Foo")

**Pure folder drop. Zero edits to generic code.**

```
1. Scaffold:
     cargo run -p openforge-cli -- new-game --id foo --name "Foo" --engine ue5 --format toml
   → creates crates/games/foo/{manifest.toml, build.rs, signatures/}
     build.rs is the 1-line openforge-buildgen::generate() call.

2. crates/games/foo/manifest.toml:
     [engine]
     schema = 2
     kind = "ue5"
     # dll omitted → Ue5Backend::default_dll_name()
     [engine.ue5]
     uobject_class_private_offset = 0x10
     [game]
     id = "foo"
     name = "Foo"
     [process] executables = ["Foo.exe"]

3. Author signatures (.toml or .ron) in crates/games/foo/signatures/.
   A heap_scan fingerprint with a bit pattern:
     [heap_scan]
     value = "0xDEADBEEF"     # FlexInt → i32 -559038737, no hand-conversion

4. Register the game in crates/bundle (the existing register_game! line — same as every game today).

5. Done. No edits to commands.rs / attach.rs / runtime / engine / host crates.
   attach() reads manifest.engine.kind = ue5 → backend_for(Ue5) → Ue5Backend::attach.
   verify-registry parses every signature through the format registry and cross-checks engine.
```

### 10.2 Game #3 on a **new** engine #N (e.g. "Source 2", `EngineKind::Source2`)

A new engine is a **new host crate + new DLL crate + one enum variant + one bundle line** — and it *reuses* `dll-common` and `host-common`, so it is a thin impl, not a fork.

```
1. runtime/manifest.rs: add EngineKind::Source2 (+ optional Source2EngineConfig).   [1 enum variant]

2. New DLL crate crates/source2-dll:
     depends on openforge-dll-common (pe, seh, panic_guard, local_reader, log_ring — all reused).
     Writes ONLY its engine walk (e.g. Source2's schema/entity system) + ops dispatch.

3. New host crate crates/source2-host:
     depends on openforge-host-common (Injector, PipeTransport, Session<P>) + openforge-protocol shape.
     impl Protocol for source2-protocol.
     struct Source2Backend; impl EngineBackend (KIND=Source2, default_dll_name="source2_dll.dll");
       attach() builds Source2Session over Session<P>.
     impl EngineSession for Source2Session.
     register_engine!(Source2Backend);

4. crates/bundle: pub use openforge_source2_host as _;                                [1 line]

5. Drop the game folder exactly as §10.1, with [engine].kind = "source2".

6. Done. commands.rs / attach.rs / runtime resolve / the format registry / the frontend
   are UNTOUCHED. The filename sniff is gone; dispatch is backend_for(manifest.engine.kind).
```

**The proof:** the things that used to require editing generic code — the `is_glacier` string compare (`commands.rs:227`), the `Session` enum + accessors (`attach.rs:38/61`, call sites `526/828/1367`), and the four `toml::from_str` sites (`build.rs:70/177`, `manifest.rs:56`, `signature.rs:42`) — are all gone, replaced by registry lookups. Engine #N is a self-registering crate; format #3 is one `SpecFormat` impl + one `inventory::submit!`. **Adding a game is a folder drop; adding an engine is a crate drop.**
