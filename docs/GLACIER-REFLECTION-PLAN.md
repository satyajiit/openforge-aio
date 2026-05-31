# Glacier 2 Reflection Plan

**Audience:** OpenForge contributors adding the Glacier 2 engine (IO Interactive) and its first game, *007 First Light*.
**Status:** Research complete and validated live; backend not yet built. This doc is the architecture blueprint.
**Cross-refs:** [Game Authoring](./GAME-AUTHORING.md) · [UE5 Cheat Cookbook](./UE5-CHEAT-COOKBOOK.md) · [Contributing](./CONTRIBUTING.md)

This document records what we learned reverse-engineering *007 First Light*, and specifies how a Glacier 2 backend slots into OpenForge alongside the existing UE5 reflection path. The headline: **Glacier ships a complete runtime type system (`ZTypeRegistry`) that we can walk entirely from outside the process with `ReadProcessMemory` — no DLL injection, no debugger, no fragile `.text` signature.**

**Two tiers, both in scope.** Read/write *data* features (currency, ammo, health, flags, coordinates) need only external RPM. *Active* and experimental features — calling the game's own functions, signalling entity pins, playing animations, in-engine ESP, "what happens if…" sandboxing — need code running **inside** the process: an injected DLL with MinHook detours, exactly the ZHMModSDK model. Both are core deliverables; see §5.2 and §6.

> **RPM is not a hook.** External `ReadProcessMemory`/`WriteProcessMemory` edits values the game reads — the game's code is never intercepted. A *hook* redirects the game's own execution (a detour) and can only be installed from inside the process (the DLL). Data writes ≠ hooks; the feature catalog (§5.6) classifies every feature by which it needs.

---

## 1. Why reflection, not address scanning

A value scan (Cheat-Engine style) found the live ammo field in minutes — but that address is a per-launch heap allocation, worthless to ship. Even an AOB signature only survives until the next patch reshuffles `.text`. Glacier, like UE5, exposes runtime reflection, so we key features on **type name + property name** instead:

| Approach | Survives relaunch | Survives game **update** | Per-patch maintenance |
|---|---|---|---|
| CE address | ❌ | ❌ | rediscover everything |
| AOB signature | ✅ | ⚠️ usually breaks | re-derive every signature |
| **Reflection (this plan)** | ✅ | ✅ — names are stable | **near-zero** (see §4) |

This is the same bet the project already made for UE5 (`GUObjectArray` walk). The Glacier analog is the `ZTypeRegistry` walk.

---

## 2. Engine background

Glacier 2 is IO Interactive's in-house engine (the *Hitman: World of Assassination* lineage). Its resource format (`.rpkg`), its `ZEntity` object model, and its `ZTypeRegistry` runtime reflection are all shared across the lineage. The community reference implementation is **ZHMModSDK** (`github.com/OrfeasZ/ZHMModSDK`), which targets the Denuvo-free *Hitman WoA* build. We confirmed the engine ABI is **byte-identical** on *007 First Light* (see §3), so its struct layouts port directly.

Two hash functions matter:
- **Type names** → FNV-1a-64 of the lowercased name (the `ZTypeRegistry::m_types` key).
- **Property and pin names** → CRC32 (the per-instance property id).

There is **no string pool to decode** (unlike UE5's `FName`): type and property names are plain `const char*` read directly from the metadata structs.

---

## 3. Validated findings (007 First Light)

All confirmed live against the running retail process via plain `ReadProcessMemory` — no admin, no injection, no debugger.

### 3.1 Target & protection
- Install: `…/steamapps/common/007 First Light/Retail/007FirstLight.exe` (base `0x140000000`, ~363 MB in memory).
- Engine confirmed: `ZEntity`, `ZTypeRegistry`, `ZResourceManager`, `RuntimeResourceID` strings + `chunk0/1.rpkg`.
- Eligibility: single-player, offline, only `steam_api64.dll`. No EAC/BattlEye/EQU8.
- **Denuvo Anti-Tamper present** (writable 255 MB `.text`, non-standard `.udata/.edata/.sxdata` exec sections, two `.vmp` markers). We do **not** touch Denuvo — we attach to the already-decrypted running process. Consequence: **data/reflection writes are safe; `code_patch` is the risk surface** (integrity checks may detect a patched `.text` or hit a virtualized region). Design rule: data-first, patches last and behind fail-closed byte-verification.

### 3.2 The reflection chain (offsets validated on this build)

```
type-name string (.rdata, found by content)
  └─[find-pointers / RW scan]→ ZTypeRegistry m_types node  (32 bytes)
        +0x00  u32  ZString.m_nLength | 0x80000000 flag
        +0x08  char* m_pChars        → type-name string
        +0x10  STypeID*              → the type's STypeID
        +0x18  u32  m_iNext          (hash chain; 0xFFFFFFFF = end)
                                 │
                                 ▼  STypeID
        +0x00  u16  m_nFlags        +0x02  u16 m_nTypeNum
        +0x08  IType* m_pType ───────────────┐
                                              ▼  IType
        +0x00  void* m_pTypeFunctions
        +0x08  u16   m_nTypeSize        +0x0A u8 m_nTypeAlignment
        +0x0C  u16   m_nTypeInfoFlags   (TIF_Entity=0x01, TIF_Class=0x04, TIF_Enum=0x08, …)
        +0x10  char* pszTypeName       → type-name string
        +0x18  STypeID* typeID
```

Worked example (`ZEntityImpl`): `IType` at static RVA `0x353C3A0` → `pszTypeName` = `0x142C87989` ("ZEntityImpl"), `flags` = `0x04` (TIF_Class), `typeID` = `0x146358700`; the `STypeID` there has `m_pType` = `0x14353C3A0` (round-trips). A live `m_types` node for it sits at `0x18ED22368`. Walking sibling nodes yielded real type names — `ZParticleModifierSizeEntity`, `ITEntityRefValue<IDistanceConstraint>`, `ITEntityRefValue<SMpGameplayAbilityActivateSuccessMetricsContext>`, etc. — proving full enumeration.

### 3.3 Property / entity layer

**Static property descriptors — VALIDATED on this build** (via `glacier-walk`, §6 Phase 1): `IClassType` extends `IType` with `m_nProperties` (u16 @ `+0x30`) and `m_pProperties` (@ `+0x40`) → array of `SNamedPropertyInfo` (stride `0x38`): `const char* name` @ `+0x00`, `u32 m_nPropertyID = CRC32(name)` @ `+0x08`, `STypeID* m_Type` @ `+0x10`, `u32 m_Flags` @ `+0x20`. `m_Flags & 0x10` = `E_HAS_GETTER_SETTER` (needs the engine setter, not a raw write). Confirmed by `ZSpatialEntity` decoding `m_mTransform`(SMatrix43)/`m_bVisible`(bool)/`m_eidParent` etc. with correct names/ids/types, and `ZCLSetHumanoidImmuneToDamage` decoding `m_humanoid` + `m_invulnerable`.

**Live per-instance offsets — still pending.** To read/write a property on a *live* entity we still need its byte offset: `ZEntityImpl.m_pEntityType` @ `+0x08` → `ZEntityType.m_pPropertyData` @ `+0x08` (`TArray<SPropertyData>`); `SPropertyData{ info @+0x00, m_nPropertyOffset @+0x08, CRC32 id @+0x10, flags @+0x14 }`. These ZHMModSDK offsets are unvalidated on First Light — Phase 2 validates them against the ammo oracle.

**Also found:** the player character class is `ZHumanoidCharacterEntity` (not `ZHitman5`); `ZGameTimeManager` is not a registry type (resolve via AOB global).

### 3.4 Validation oracle
- Ammo magazine field: `0x18F272A90` (i32; dynamic per launch — **not** shippable). Owned within a weapon/inventory pool (array of `{object_ptr, capacity=8}` records). The reflection-resolved ammo property **must** land on this address; that is the end-to-end test gate for §6 Phase 2.

---

## 4. The bootstrap that needs no signature

ZHMModSDK locates `ZTypeRegistry` with an AOB on `.text` that breaks every patch. We don't need it:

```
1. Find a type-name string in module .rdata by its CONTENT (e.g. "ZEntityImpl").
2. RW-scan the process for 8-byte pointers to that string VA → the m_types node(s).
3. node +0x10 = STypeID* → +0x08 = IType → enumerate properties.
```

This resolves any type **by name** without ever finding the registry singleton, and it survives game updates untouched — the only thing that moves per patch is *where* the node array sits in memory, and we find it by string content, not a byte pattern. (The registry singleton is still worth locating for fast full enumeration; do it via a structural hunt on the node-array base, not an AOB.)

---

## 5. Architecture

### 5.1 The seam is `Ctx`
Everything funnels through the `Ctx` trait (`crates/core/src/ctx.rs`). `runtime`, `Feature`, `DeclarativeFeature`, the registry, and the signature-TOML engine are all engine-agnostic — they call `read_bytes`/`write_bytes`/`scan_*`/`patch_code` and (optionally) the reflection methods. UE5 happens to implement `Ctx` via an injected DLL + named pipe; Glacier needs only a new `Ctx` impl.

`Ctx`'s reflection methods (`find_uobject`, `find_all_uobjects`, `resolve_property`, `call_ufunction`) default to an error ("only the IPC-backed `Ue5Session` does this"). The Glacier backend will instead implement its own resolution — see §5.4 on whether to extend `Ctx` or resolve inside the `DeclarativeFeature` Glacier branch.

### 5.2 Two-tier backend
Both tiers implement / feed the same `Ctx` seam, so `DeclarativeFeature` and the signature TOML don't care which is in play.

**Tier 1 — external RPM host (`crates/glacier-host`).** Covers all *data-domain* features (currency, ammo, health, gadget energy, flags, coordinates) with zero injection:

```
crates/glacier-host/      GlacierSession: open process handle, enumerate modules,
                          implement Ctx (read/write/scan via ReadProcessMemory),
                          and a GlacierReflection resolver (§4 bootstrap + §3 walk).
```

**Tier 2 — injected DLL (`crates/glacier-protocol` + `crates/glacier-dll`).** A core deliverable, not optional. Mirrors `ue5-protocol` + `batman-lod-dll`: a DLL injected into the game, hosting **MinHook** detours and serving ops over a named pipe (`GlacierSession` routes reflection/active ops to it when present). The DLL is what makes the *active* and experimental features possible — there is no external substitute for:
- **Calling the game's own functions** (the `call_ufunction` analog) and **signalling entity pins** (`SignalInputPin`/`SignalOutputPin`, CRC32 ids) — required for animations ("make Bond dance"), door unlocks via pins, and the "what happens if…" sandbox.
- **In-engine rendering** (hooking `d3d12` present/draw) for ESP/wallhack drawn in the game's own pipeline (the alternative is an external overlay window — see the catalog §5.6).
- **Behavioral hooks** (intercepting AI perception, damage, collision) and fast in-process enumeration of large live-entity sets.

Tier 1 ships first (fast win, Denuvo-trivial); Tier 2 unlocks the rest. Loader: a `DINPUT8.dll` proxy is the conventional Glacier injection path (the game already imports it).

### 5.3 Wiring into attach
`Attached.session` is currently the concrete type `Arc<Ue5Session>` (`crates/app/src-tauri/src/attach.rs`), and `commands.rs` calls `Ue5Session`-specific methods (welcome, Lua ops) that are not on `Ctx`. So a second backend needs one of:

- **Enum dispatch (recommended):** `enum Session { Ue5(Arc<Ue5Session>), Glacier(Arc<GlacierSession>) }`, with a `fn ctx(&self) -> &dyn Ctx`. Preserves each backend's non-`Ctx` methods; engine choice branches on `game.dll_file_name()` (empty ⇒ Glacier external-RPM path; non-empty ⇒ UE5 DLL path).
- **Trait object:** change `Attached.session` to `Arc<dyn Ctx>`. Minimal, but loses the UE5-specific surface — only viable if those calls are gated separately.

The attach branch point already exists: `commands.rs` reads `game.dll_file_name()` and, today, hard-requires it. Glacier games declare `dll_file_name = ""` and take the RPM path.

### 5.4 Signature TOML: the Glacier reflection block
A new locator block parallel to UE5's `[reflection]`, keyed on Glacier's name space:

```toml
[glacier_reflection]
type_name     = "ZHM_FirstLight_Weapon"          # FNV-1a-64-lower key into m_types
property_name = "m_nMagazineAmmo"                 # CRC32 id resolved on the entity
predicate     = { kind = "exact", value = "..." } # discriminator when many instances
# optional pointer-follow chain, mirroring UE5 ReflectionDerefSpec:
# deref = [ { property_name = "...", inner_offset = 0x0 } ]
```

`DeclarativeFeature::resolve` dispatches in priority order (`crates/runtime/src/feature.rs`): `[reflection]` → `[heap_scan]` → `[locator]`. Add a `[glacier_reflection]` branch calling `resolve_via_glacier_reflection`, which: looks up `type_name` (§4) → finds live instances of the type → matches `predicate` → resolves `property_name` (CRC32) to `entity + m_nPropertyOffset`, then applies any `deref`/`offset` chain. (Where this resolver physically lives — extending `Ctx` vs. a Glacier-only path that takes `&GlacierSession` — is the one open design call in §8.)

Reuse `[heap_scan]` and `[locator]` unchanged for the rare Glacier feature that needs them; they are engine-agnostic.

### 5.5 What adding the game touches
- `crates/games/glacier-007/manifest.toml` — exists already (research stub): `process_names = ["007FirstLight.exe"]`, `dll_file_name = ""`. Promote to a full game crate (`build.rs`, `lib.rs`, `register_game!`, `signatures/`).
- `crates/glacier-host/` — new (Tier 1). Then `crates/glacier-protocol/` + `crates/glacier-dll/` (Tier 2).
- `crates/bundle/src/lib.rs` — register the game.
- `crates/app/src-tauri/src/{attach.rs,commands.rs}` — the §5.3 backend branch.
- Discovery tooling: `glacier-*` discover subcommands mirroring `ue5-find-object` / `ue5-find-prop` / `ue5-dump-class`.

### 5.6 Feature catalog

The trainer targets both classic data cheats and active/experimental "what happens if…" features. Each is classified by the tier it needs (Tier 1 = external RPM data write; Tier 2 = injected DLL hook / function-call / pin). The mapping below is the **source-verified** output of the `glacier-feature-map` workflow — every named Glacier class/offset/pin/function was cross-checked against ZHMModSDK master, then adversarially re-checked. **All numeric offsets/AOBs are Hitman-WoA values and MUST be re-derived on the First Light build via the §3 reflection walk (Phase 2) before shipping** — names and CRC32/FNV ids port; numbers do not.

| Feature | Tier | Verified mechanism | First-Light gate |
|---|---|---|---|
| **Infinite ammo** | **1** | Freeze the plain magazine integer each tick (the field we located live — *not* `m_nBulletsToFire`, which is a burst counter). Never raw-write `m_nAmmoInPocket` (TCheatProtect). | field identity ✅ (oracle `0x18F272A90`) |
| **Game speed** (super-speed / slow-mo) | **1** | Write `ZGameTimeManager+0x48` (`m_fGameTimeMultiplier`); pause `+0x70`. Plain float, no getter/setter. | re-derive global sig |
| **Freeze enemy AI** | **1** | Write one bool `ZActorManager+0xED08` (`m_bDisableAIBehavior`). | **causality untested** — gating live test; re-verify offset |
| **ESP / wallhack** (boxes/labels/distance) | **1** + overlay | Walk `ActorManager.m_activatedActors` → cached `ZSpatialEntity` → `m_mTransform.Trans` (+0x44); WorldToScreen from `RenderManager→m_pDevice→m_Constants` camera fields; draw on a transparent overlay. | camera-const offsets (use `0x18/0x170/0x17C/0x188/0x194`), re-validate |
| ESP depth-tint + bone skeleton | 2 | in-engine `IRenderer` `OnDepthDraw3D` / bone-transform chain | — |
| **Teleport** — save | **1** | Read player `SMatrix43` @ entity `+0x20` (Trans `+0x44`). | — |
| Teleport — restore / fly-to | 2 | vfunc `ZSpatialEntity::SetObjectToWorldMatrixFromEditor` (vtbl 31). Raw transform write fails (dirty-flag + locomotion overwrite). | re-verify vtable index |
| **God mode** | **2** | Spawn `ZAICrippleEntity` + `SignalInputPin("SetHeroInvincible")`, plus a `ZActor_YouGotHit` detour. **Not** the `m_bIsInvincible` flag (the SDK deliberately avoids it); health is TCheatProtect. | cripple-entity resource exists? |
| **Noclip** | **2** | Spawn `ZHM5CrippleBox` (`m_bMovementAllowed` etc.) + 2 engine calls + per-frame `SetObjectToWorldMatrixFromEditor` + one `ClearScene` cleanup hook. | cripple-box resource exists? |
| **Unlock any door** | **2** | `SetProperty` through the engine setter to flip the lock-condition bool (it carries `E_HAS_GETTER_SETTER`, so raw writes are ignored); optionally `SignalInputPin` a per-prefab open pin discovered live. (No "Poll" pin exists.) | lock-bool name/id + open-pin id, live |
| **Enemies ignore you** | **2** | Spawn `zaicrippleentity.class` + `SignalInputPin("SetHeroHidden")` + suppress security-camera frame-update hooks. External "degraded" mode (zero attention floats) can't reset NPCs already in combat. | cripple-entity resource exists? |
| **Make Bond dance** / arbitrary anim | **2** | `SignalInputPin` the `ZSequenceEntity` `"Start"` pin (CRC32 `1589148299`) after reflection-wiring a 4-entity sequence graph (`m_animationResourceID`, `m_targetEntity`, slot). | clip RRIDs + spawn-wire spike, live |
| **"What happens if…" sandbox** | **2** (enum external) | Enumerate entities/pins/properties externally; *act* via self-resolved engine fn ptrs (`SignalInputPin`/`SignalOutputPin`/`SetProperty`/`NewEntity`) marshalled to the game thread by one tick-fn hook. SEH-guard every call; blocklist `*_Lowres_*`/`*_Proxy_*` spawns. | re-derive AOBs + spawn pipeline |
| **Free camera / photo mode** | **2** | `CreateFreeCameraAndControl` + `GetActiveRenderDestinationEntity`/`SetSource` + `SetObjectToWorldMatrixFromEditor` + control `SetActive`; photo mode pauses the clock. Matches ZHMModSDK FreeCam verbatim. | re-derive AOBs + `ZEngineAppCommon` offsets |

### 5.7 Cross-cutting findings (from the feature map)

- **First Light has NO `ZHM5*` / cripple-entity cheat layer — live-confirmed.** We enumerated the running image: engine types are present (`ZSpatialEntity`, `ZSequenceEntity`, `ZGameTimeManager`, `ZCameraManager`, `ZFreeCameraControlEntity`, `ZEntitySceneContext`, `ZActor`, `ZPlayer`), but `ZHitman5`, `ZActorManager`, `ZHM5ItemWeapon`, `ZHM5CrippleBox`, `ZAICrippleEntity`, `ZHM5ActionManager`, `ICharacterMovementState` and the rest of the Hitman-5 layer are **absent**. The WoA feature map's cripple-entity mechanism does not exist here. **The §5.6 WoA mechanisms (`SetHeroInvincible`, `ZHM5CrippleBox`, etc.) are superseded by the First-Light-native nodes below.**
- **First Light's gameplay layer is a `ZCL*` logic-node + `PlanScoped` modifier + act-message architecture** — and it ships *explicit native nodes* for most of what we want. Weapons attach to `Humanoid` entities; NPCs are `Agent`s; the player is `LocalPlayer`. Live-enumerated targets (every name is a reflection type confirmed in the running image, to be resolved/actuated in Phase 2):

  | Feature | First-Light-native mechanism (live-found type names) |
  |---|---|
  | Infinite ammo | `ZCLSetFirearmInfiniteAmmo`, `ZCLSetHumanoidInfiniteClipAmmo`, `ZAddPlanScopedInfiniteFirearmMagazine`, `ZAIInfiniteClipMagazineConfigurationEntity` |
  | God mode | `ZCLSetHumanoidImmuneToDamage`, `ZCLSetHumanoidUnkillableByDamage`, `ZHumanAddPlanScopedPreventReceivingDamage` / `…PreventDyingFromDamage` |
  | Set/force health | `ZCLForcePlayerHealthValues`, `ZCLSetHumanoidHealth`, `ZCLGiveHumanoidHealth`, `ZCLGetHumanoidHealth`/`…MaxHealth` |
  | Enemies ignore you | AI-perception nodes: `ZCLAICanHumanNPCsSeePlayer`, `ZCLAIIsPlayerIdentifiedByAnyNPC`, threat-ID nodes (`ZCLAIGetHighestLocalPlayerThreatIDValue`), `ZAddProhibitedFromDoingDamageInRangedCombatToAgents` |
  | Play animation / dance | `RequestPlayAnimation` act-messages (`ZClearAllRequestPlayAnimationActMessages`) + `ZSequenceEntity` + the `Gesture`/`InputAction` system |
  | Doors | interaction-based (`ZHumanUseDoor`, `ZInteractionUIProvider_Unlockable`, sabotaged-door nodes); no single "unlock" node — gate via the interaction/lock property |
  | Gadget / Q-watch energy | `ZCLAddResourceRechargeDelayToPlayer` + the player resource/recharge system |

  These are logic-graph nodes, so actuation is Tier-2 (in-process): resolve the node's backing function or the state it sets and invoke/flip it. The upside is large — **First Light natively supports these cheats**, so we align with the engine's own intent. Vocabulary *is* the deliverable: this list replaces the WoA guesses with confirmed First-Light reflection targets.
- **TCheatProtect obfuscation.** Health (`m_fHitPoints`) and reserve ammo (`m_nAmmoInPocket`) are XOR+FNV-checksum scrambled — raw external writes corrupt them. Prefer the plain magazine integer (already located) or the cripple-box.
- **More Tier-2 than first guessed.** God mode and noclip are in-process function-call features, not data writes — independent confirmation that the injected DLL is essential, not optional.
- **Offset-portable, not byte-portable.** Type/property/pin *names* + their CRC32/FNV ids are stable across the lineage; numeric offsets and AOB patterns are WoA values that must be re-derived on First Light (the first pass already mis-stated the camera-constants offsets — caught by verification).

---

## 6. Phased roadmap

- **Phase 1 — Reflection probe (research → tooling).** `glacier-host` skeleton + a discover command that runs the §4 bootstrap and enumerates types live. Done when it prints the live type list.
- **Phase 2 — Property resolution + oracle gate.** Validate §3.3 offsets live; resolve the weapon's ammo property by name and confirm it lands on the §3.4 oracle address. Done when reflection resolution == scanned address.
- **Phase 3 — First shippable feature.** A `[glacier_reflection]` ammo (or currency) signature, resolved on attach, written via the `Ctx`. Survives relaunch.
- **Phase 4 — Backend integration.** `GlacierSession` implements `Ctx`; enum-dispatch in attach; promote `glacier-007` to a full game crate; bundle registration. Trainer can attach and apply Glacier features end-to-end.
- **Phase 5 — Injected DLL (core).** `glacier-protocol` + `glacier-dll` (MinHook), injected via a `DINPUT8.dll` proxy, served over a named pipe like the UE5 DLL. Implements `SignalInputPin`/`SignalOutputPin`, the UFunction-call analog, behavioral hooks (AI/damage/collision), and a `d3d12` render hook. Hook only unprotected engine functions; verify bytes before detouring so a Denuvo-virtualized region fails closed. Done when the DLL can call a function / signal a pin on a named entity.
- **Phase 6 — Active & experimental features.** The §5.6 Tier-2 catalog: unlock doors, enemies-ignore, freeze AI, free camera, noclip-via-hook, in-engine ESP, "make Bond dance", and the interactive "what happens if…" sandbox (live UFunction-call + pin-signal console over the discover CLI, then surfaced in the app). This is where the engine's reflection pays off as a playground.

---

## 7. Decision log

- **Reflection over AOB/CE.** Update-resilience (names stable, addresses/offsets not). Mirrors the UE5 decision.
- **Two tiers: RPM-first, DLL-core.** Data features ship first via external RPM (Denuvo-trivial, no injection). The injected DLL is a *core* deliverable — not optional — because active/experimental features (function calls, pin signals, animations, in-engine ESP, the sandbox) have no external substitute. Sequence RPM before DLL so we ship value early and isolate Denuvo risk to Tier 2.
- **No `.text` signature for bootstrap.** Locate the type system by string content + pointer scan (§4), not an AOB — removes the single most update-fragile dependency.
- **Data writes over `code_patch` under Denuvo.** Denuvo checks code integrity, not heap data. Prefer property writes; gate any patch behind byte-verification that fails closed.
- **Enum dispatch over trait-object for `Attached.session`.** Preserves backend-specific (non-`Ctx`) methods both engines carry.

---

## 8. Open questions / risks

- **The `ZCL*` logic-nodes are the new top unknown (§5.7).** Cripple entities are confirmed *absent*; First Light exposes native nodes (`ZCLSetHumanoidImmuneToDamage`, `ZCLSetFirearmInfiniteAmmo`, …). Open question: how is a `ZCL` node *actuated* as a cheat — is it a callable function, a graph node we signal, or does it set a discoverable entity property/state we can flip externally? Phase 2 must RE one node end-to-end (suggest `ZCLSetHumanoidImmuneToDamage`) to establish the actuation pattern for the whole class.
- **Property-layer offsets (§3.3) unconfirmed on this build.** High confidence (type layer matched exactly) but must be validated in Phase 2 before relying on them.
- **Where the Glacier resolver lives.** Extend `Ctx` with Glacier methods (symmetry with UE5) vs. a Glacier-only resolution path taking `&GlacierSession` (keeps `Ctx` UE5-shaped). Lean toward the latter to avoid bloating the shared trait, decide in Phase 1.
- **Live-instance enumeration without a DLL.** Resolving `type_name` → *live instances* externally needs the entity manager / scene context (heap walk) or a targeted RW scan. Feasible (we found the weapon via the oracle) but the generic mechanism is unproven — prototype in Phase 2.
- **Getter/setter properties.** `E_HAS_GETTER_SETTER` properties can't be written by raw memory poke; they need the setter fn pointer invoked — which from an external process means either a remote thread or deferring those to the Phase 5 DLL.
- **Denuvo first-attach latency / online check-ins.** Slow first decrypt; expect a generous attach/quick-check timeout. Reported periodic online validation even in single-player — does not affect memory access but note it.
- **Registry singleton location.** Not strictly required (§4), but desirable for fast enumeration; find structurally, never by AOB.
