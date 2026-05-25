//! UE4SS-compatible Lua API surface installed on the worker VM.
//!
//! Community UE4SS scripts call into a small, well-known set of globals:
//!   * `StaticFindObject(path)` / `FindAllOf(class)` — object discovery.
//!   * `obj:GetFullName()`, `obj:GetClass():GetFName():ToString()`,
//!     `obj:type()`, `obj:IsValid()`, `obj:GetIndex()` — object inspection.
//!   * `ExecuteInGameThread(fn)`, `ExecuteWithDelay(ms, fn)`,
//!     `LoopAsync(ms, fn)` — scheduling.
//!   * `RegisterKeyBind(vk, fn)` — input.
//!   * `Key.F5` etc. — vk constants.
//!
//! Each binding wraps a host call (or a scheduler interaction) in a closure
//! that translates `Err(String)` from the engine into `mlua::Error::external`
//! so user scripts can catch them with `pcall`. The shape is "thin shim" by
//! design — no business logic lives here, it all delegates to
//! [`crate::LuaEngineHost`].
//!
//! ## What's intentionally NOT here (v1)
//!
//! UE4SS exposes generated method bindings (`pc:K2_GetPawn()` becomes a
//! direct call). Replicating that requires per-class metatables driven by
//! the engine's UFunction chain, which is a substantial project. v1 ships
//! a single generic escape hatch instead:
//!
//! ```lua
//! local raw_bytes = obj:CallFunction("K2_GetPawn", "")
//! ```
//!
//! The caller is responsible for encoding the parameter blob and decoding
//! the return blob. A v2 generated-binding layer can be added on top
//! without changing this trait.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::Ordering as AOrdering;
use std::time::{Duration, Instant};

use mlua::{
    AnyUserData, Function, Lua, MetaMethod, MultiValue, RegistryKey, UserData, UserDataMethods,
    Value, Variadic,
};
use openforge_ue5_protocol::{NamePredicate, PropKind, PropValue};
use parking_lot::Mutex;

use crate::host::{LuaEngineHost, UFunctionParam};
use crate::output::OutputBuffer;
use crate::workers::{DelayedTask, Workers};

/// Pre-resolved UFunction plan: the parameter layout + total buffer size
/// every call to this UFunction needs to allocate. Cached per-class on
/// [`BindingsCtx::class_metas`]; first attribute access pays the host
/// round-trip, subsequent calls are local.
pub(crate) struct UFunctionPlan {
    /// Original casing of the function name (for tracing / errors).
    pub name: String,
    /// UFunction* in game memory. Kept for debugging; the actual dispatch
    /// resolves by name against the class's function chain DLL-side.
    #[allow(dead_code)]
    pub ufunction_addr: u64,
    pub params: Vec<UFunctionParam>,
    /// Total parm-buffer size = max(end_of_last_param, 1). UE5 expects
    /// exactly this many bytes; the DLL zero-pads under.
    pub parm_buf_size: usize,
}

/// Per-class lookup cache: `lowercase(function_name) -> plan`. Built lazily
/// on the first attribute access of any `UObjectHandle` for that class; an
/// entry of `None` is a negative cache (the name is not a UFunction on
/// this class — fall through to a Lua error).
#[derive(Default)]
pub(crate) struct ClassMetaCache {
    pub functions: HashMap<String, Option<Arc<UFunctionPlan>>>,
    /// True iff we've already paid for the full `list_class_functions`
    /// call on this class — used so we only round-trip once per class.
    pub functions_enumerated: bool,
}

/// Shared state captured by every binding closure.
///
/// Cloned (Arc) per-binding; closures only read.
pub struct BindingsCtx {
    pub host: Arc<dyn LuaEngineHost>,
    pub output: Arc<OutputBuffer>,
    pub workers: Arc<Workers>,
    pub shutdown: Arc<std::sync::atomic::AtomicBool>,
    /// Per-class metatable cache. Keyed by `class_addr`. Each entry holds
    /// resolved [`UFunctionPlan`]s indexed by lowercase function name so
    /// case-insensitive UE5 lookups are O(1) after the first miss.
    pub(crate) class_metas: Mutex<HashMap<u64, ClassMetaCache>>,
}

impl BindingsCtx {
    /// Construct with empty caches. Public so the runtime can build one.
    pub fn new(
        host: Arc<dyn LuaEngineHost>,
        output: Arc<OutputBuffer>,
        workers: Arc<Workers>,
        shutdown: Arc<std::sync::atomic::AtomicBool>,
    ) -> Self {
        Self {
            host,
            output,
            workers,
            shutdown,
            class_metas: Mutex::new(HashMap::new()),
        }
    }
}

/// Userdata wrapper for a live UObject. Carries just the raw pointer; the
/// class pointer is resolved lazily on first method call that needs it
/// (typically via `host.class_name_of` + a subsequent `find_uobject` if
/// the script wants to chain into property reads).
#[derive(Clone)]
struct UObjectHandle {
    addr: u64,
    /// Cached class pointer, populated when [`find_all_uobjects`] returns
    /// it directly. Zero means "not yet known".
    class_addr: u64,
}

impl UObjectHandle {
    fn with_class(addr: u64, class_addr: u64) -> Self {
        Self { addr, class_addr }
    }
}

impl UserData for UObjectHandle {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("GetFullName", |lua, this, ()| {
            let ctx = ctx_from_lua(lua)?;
            let name = ctx.host.full_name_of(this.addr).map_err(host_err)?;
            Ok(name)
        });
        methods.add_method("GetClass", |lua, this, ()| {
            let ctx = ctx_from_lua(lua)?;
            let cls = ctx.host.class_name_of(this.addr).map_err(host_err)?;
            Ok(UClassHandle { name: cls })
        });
        methods.add_method("IsValid", |_, this, ()| Ok(this.addr != 0));
        methods.add_method("GetIndex", |_, this, ()| {
            // UE4SS-compat: scripts use this as a stable id for `print`
            // debugging. The lower 32 bits of the UObject pointer change
            // per-run but are constant within a run, which matches the
            // ergonomic expectation.
            Ok((this.addr & 0xFFFF_FFFF) as u32)
        });
        methods.add_method("type", |lua, this, ()| {
            let ctx = ctx_from_lua(lua)?;
            ctx.host.class_name_of(this.addr).map_err(host_err)
        });
        methods.add_method(
            "CallFunction",
            |lua, this, (name, params): (String, mlua::String)| {
                if this.class_addr == 0 {
                    return Err(mlua::Error::external(
                        "CallFunction requires a class-addr-carrying UObject; this handle was \
                         produced without class info (e.g. via StaticFindObject in v1)",
                    ));
                }
                let ctx = ctx_from_lua(lua)?;
                let params_bytes = params.as_bytes().to_vec();
                let ret = ctx
                    .host
                    .call_ufunction(this.addr, this.class_addr, &name, params_bytes)
                    .map_err(host_err)?;
                lua.create_string(&ret)
            },
        );
        // Generic property read: `obj:ReadInt32("Health")` etc. — the
        // single shape covers all numeric widths.
        methods.add_method(
            "ReadProperty",
            |lua, this, (name, kind_str): (String, String)| {
                if this.class_addr == 0 {
                    return Err(mlua::Error::external(
                        "ReadProperty requires a class-addr-carrying UObject",
                    ));
                }
                let ctx = ctx_from_lua(lua)?;
                let resolved = ctx
                    .host
                    .resolve_property(this.class_addr, &name)
                    .map_err(host_err)?
                    .ok_or_else(|| {
                        mlua::Error::external(format!("property `{name}` not found on class"))
                    })?;
                let kind = parse_prop_kind(&kind_str, resolved.kind)?;
                let value = ctx
                    .host
                    .read_property(this.addr.wrapping_add(resolved.offset as u64), kind)
                    .map_err(host_err)?;
                prop_value_to_lua(lua, value)
            },
        );
        methods.add_method(
            "WriteProperty",
            |lua, this, (name, value): (String, Value)| {
                if this.class_addr == 0 {
                    return Err(mlua::Error::external(
                        "WriteProperty requires a class-addr-carrying UObject",
                    ));
                }
                let ctx = ctx_from_lua(lua)?;
                let resolved = ctx
                    .host
                    .resolve_property(this.class_addr, &name)
                    .map_err(host_err)?
                    .ok_or_else(|| {
                        mlua::Error::external(format!("property `{name}` not found on class"))
                    })?;
                let pv = lua_to_prop_value(value, resolved.kind)?;
                ctx.host
                    .write_property(this.addr.wrapping_add(resolved.offset as u64), pv)
                    .map_err(host_err)?;
                Ok(())
            },
        );
        methods.add_meta_method(MetaMethod::ToString, |_, this, ()| {
            Ok(format!("UObject(0x{:X})", this.addr))
        });
        // -- v2: generated UFunction metatable -------------------------------
        //
        // `__index` is consulted when a key misses every statically-bound
        // method above. We treat the key as a UE5 UFunction name, look it
        // up against the object's class (cached per class_addr), and build
        // a Lua function that marshals args -> param buffer, dispatches
        // through `call_ufunction`, then unmarshals OUT/RETURN slots back
        // to Lua values.
        //
        // Misses (the name is not a UFunction on this class chain) bubble
        // up as a Lua-visible error mentioning the key — Lua's default
        // "attempt to call a nil value" would be much less debuggable.
        methods.add_meta_method(MetaMethod::Index, |lua, this, key: mlua::Value| {
            let key_str = match &key {
                mlua::Value::String(s) => s.to_str()?.to_string(),
                other => {
                    return Err(mlua::Error::external(format!(
                        "UObject index must be a string method name, got {}",
                        other.type_name()
                    )));
                }
            };
            if this.class_addr == 0 {
                return Err(mlua::Error::external(format!(
                    "method `{key_str}` requires a class-addr-carrying UObject; this handle was \
                     produced without class info (chain through GetClass() / FindAllOf to get one)"
                )));
            }
            let ctx = ctx_from_lua(lua)?;
            let plan =
                resolve_ufunction_plan(&ctx, this.class_addr, &key_str)?.ok_or_else(|| {
                    mlua::Error::external(format!(
                        "no UFunction or built-in method named `{key_str}` on class \
                         (0x{:X})",
                        this.class_addr
                    ))
                })?;
            let obj_addr = this.addr;
            let class_addr = this.class_addr;
            let plan_clone = plan.clone();
            // Generated dispatcher: Lua's `obj:Method(...)` desugars to
            // `obj.Method(obj, ...)`, so we receive `self` as the first
            // positional arg. Strip it before marshalling — UE5
            // parameters never include the implicit receiver.
            let dispatcher = lua.create_function(move |lua, mut args: Variadic<mlua::Value>| {
                if !args.is_empty() {
                    let _self = args.remove(0);
                }
                let ctx = ctx_from_lua(lua)?;
                invoke_generated(lua, &ctx, obj_addr, class_addr, &plan_clone, args)
            })?;
            Ok(mlua::Value::Function(dispatcher))
        });
    }
}

/// Userdata for `obj:GetClass()`. We carry the class name (cheap) and not
/// the UClass pointer — most scripts only need
/// `cls:GetFName():ToString()` for printing, and `obj:type()` already
/// covers the same use case. If a script needs the pointer for further
/// reflection it should use `FindAllOf` (which surfaces both).
#[derive(Clone)]
struct UClassHandle {
    name: String,
}

impl UserData for UClassHandle {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("GetFName", |_, this, ()| {
            Ok(FNameHandle {
                name: this.name.clone(),
            })
        });
        methods.add_meta_method(MetaMethod::ToString, |_, this, ()| {
            Ok(format!("UClass({})", this.name))
        });
    }
}

#[derive(Clone)]
struct FNameHandle {
    name: String,
}

impl UserData for FNameHandle {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_method("ToString", |_, this, ()| Ok(this.name.clone()));
        methods.add_meta_method(MetaMethod::ToString, |_, this, ()| Ok(this.name.clone()));
    }
}

fn host_err(s: String) -> mlua::Error {
    mlua::Error::external(s)
}

/// Pull the [`BindingsCtx`] back out of the Lua app-data slot. Installed
/// once by [`install`]; every binding closure goes through here so we
/// don't capture per-closure Arcs (less monomorphization).
fn ctx_from_lua(lua: &Lua) -> mlua::Result<Arc<BindingsCtx>> {
    lua.app_data_ref::<Arc<BindingsCtx>>()
        .map(|r| r.clone())
        .ok_or_else(|| mlua::Error::external("BindingsCtx not installed on Lua state"))
}

fn parse_prop_kind(name: &str, fallback: PropKind) -> mlua::Result<PropKind> {
    let kind = match name.to_ascii_lowercase().as_str() {
        "" | "auto" => fallback,
        "i8" => PropKind::I8,
        "i16" => PropKind::I16,
        "i32" => PropKind::I32,
        "i64" => PropKind::I64,
        "u8" => PropKind::U8,
        "u16" => PropKind::U16,
        "u32" => PropKind::U32,
        "u64" => PropKind::U64,
        "f32" => PropKind::F32,
        "f64" => PropKind::F64,
        "bool" => PropKind::Bool,
        other => {
            return Err(mlua::Error::external(format!(
                "unsupported property kind: {other}"
            )));
        }
    };
    Ok(kind)
}

fn prop_value_to_lua(lua: &Lua, v: PropValue) -> mlua::Result<Value> {
    Ok(match v {
        PropValue::I8(x) => Value::Integer(x as i64),
        PropValue::I16(x) => Value::Integer(x as i64),
        PropValue::I32(x) => Value::Integer(x as i64),
        PropValue::I64(x) => Value::Integer(x),
        PropValue::U8(x) => Value::Integer(x as i64),
        PropValue::U16(x) => Value::Integer(x as i64),
        PropValue::U32(x) => Value::Integer(x as i64),
        PropValue::U64(x) => Value::Integer(x as i64),
        PropValue::F32(x) => Value::Number(x as f64),
        PropValue::F64(x) => Value::Number(x),
        PropValue::Bool(b) => Value::Boolean(b),
        PropValue::Bytes(b) => Value::String(lua.create_string(&b)?),
    })
}

fn lua_to_prop_value(v: Value, kind: PropKind) -> mlua::Result<PropValue> {
    macro_rules! to_int {
        ($t:ty) => {{
            let i = match v {
                Value::Integer(i) => i,
                Value::Number(n) => n as i64,
                Value::Boolean(b) => b as i64,
                other => {
                    return Err(mlua::Error::external(format!(
                        "expected integer, got {}",
                        other.type_name()
                    )));
                }
            };
            i as $t
        }};
    }
    Ok(match kind {
        PropKind::I8 => PropValue::I8(to_int!(i8)),
        PropKind::I16 => PropValue::I16(to_int!(i16)),
        PropKind::I32 => PropValue::I32(to_int!(i32)),
        PropKind::I64 => PropValue::I64(to_int!(i64)),
        PropKind::U8 => PropValue::U8(to_int!(u8)),
        PropKind::U16 => PropValue::U16(to_int!(u16)),
        PropKind::U32 => PropValue::U32(to_int!(u32)),
        PropKind::U64 => PropValue::U64(to_int!(u64)),
        PropKind::F32 => PropValue::F32(match v {
            Value::Number(n) => n as f32,
            Value::Integer(i) => i as f32,
            other => {
                return Err(mlua::Error::external(format!(
                    "expected number, got {}",
                    other.type_name()
                )));
            }
        }),
        PropKind::F64 => PropValue::F64(match v {
            Value::Number(n) => n,
            Value::Integer(i) => i as f64,
            other => {
                return Err(mlua::Error::external(format!(
                    "expected number, got {}",
                    other.type_name()
                )));
            }
        }),
        PropKind::Bool => PropValue::Bool(match v {
            Value::Boolean(b) => b,
            Value::Integer(i) => i != 0,
            other => {
                return Err(mlua::Error::external(format!(
                    "expected boolean, got {}",
                    other.type_name()
                )));
            }
        }),
        PropKind::Bytes(_) => match v {
            Value::String(s) => PropValue::Bytes(s.as_bytes().to_vec()),
            other => {
                return Err(mlua::Error::external(format!(
                    "expected string for bytes property, got {}",
                    other.type_name()
                )));
            }
        },
    })
}

/// Look up (or lazily build) the [`UFunctionPlan`] for `(class_addr,
/// fn_name)`. Returns `Ok(None)` if the class has been enumerated and the
/// name is genuinely not a UFunction. Returns `Err` only on host-side
/// failures (engine reflection errors).
///
/// The first call against a class triggers a full `list_class_functions`
/// host round-trip; subsequent calls hit the in-process cache. Per-name
/// lookups (the typical hot path) only pay the `ufunction_params` round
/// trip on the very first miss in the per-name slot — after that the
/// plan is shared via `Arc<UFunctionPlan>`.
fn resolve_ufunction_plan(
    ctx: &BindingsCtx,
    class_addr: u64,
    fn_name: &str,
) -> mlua::Result<Option<Arc<UFunctionPlan>>> {
    let key = fn_name.to_ascii_lowercase();
    // Fast path: already cached (hit or negative).
    {
        let metas = ctx.class_metas.lock();
        if let Some(class) = metas.get(&class_addr)
            && let Some(slot) = class.functions.get(&key)
        {
            return Ok(slot.clone());
        }
    }
    // Slow path: enumerate the class's function chain once, then resolve
    // params for the requested name. We don't pre-resolve params for every
    // listed function — most scripts only call a handful of methods per
    // class, so we'd waste a round-trip on the rest.
    let sigs = ctx
        .host
        .list_class_functions(class_addr)
        .map_err(host_err)?;
    let sig = sigs.iter().find(|s| s.name.eq_ignore_ascii_case(fn_name));
    let plan = match sig {
        Some(s) => {
            let params = ctx.host.ufunction_params(s.addr).map_err(host_err)?;
            let parm_buf_size = params
                .iter()
                .map(|p| (p.offset as usize).saturating_add(p.size as usize))
                .max()
                .unwrap_or(0)
                .max(1);
            Some(Arc::new(UFunctionPlan {
                name: s.name.clone(),
                ufunction_addr: s.addr,
                params,
                parm_buf_size,
            }))
        }
        None => None,
    };
    let mut metas = ctx.class_metas.lock();
    let entry = metas.entry(class_addr).or_default();
    entry.functions.insert(key, plan.clone());
    entry.functions_enumerated = true;
    Ok(plan)
}

/// Marshal `args` -> param buffer per `plan`, fire `call_ufunction`,
/// unmarshal OUT / RETURN slots back to a Lua `Variadic` result.
fn invoke_generated(
    lua: &Lua,
    ctx: &BindingsCtx,
    obj_addr: u64,
    class_addr: u64,
    plan: &UFunctionPlan,
    args: Variadic<mlua::Value>,
) -> mlua::Result<Variadic<mlua::Value>> {
    let mut buf = vec![0u8; plan.parm_buf_size];

    // Walk param chain. Each IN / IN-OUT slot consumes one positional Lua
    // arg (skipping RETURN slots, which UE5 owns). OUT-only params get
    // zeroed in the input buffer — UE5 writes them on the way out.
    let mut arg_iter = args.into_iter();
    for p in &plan.params {
        if p.flags.returns {
            // RETURN slot — UE5 owns. Leave zeroed.
            continue;
        }
        if p.flags.out && !is_in_out(p) {
            // OUT-only — caller doesn't supply, UE5 writes back. Leave
            // zeroed.
            continue;
        }
        let lua_arg = arg_iter.next().unwrap_or(mlua::Value::Nil);
        encode_param_into_buf(&mut buf, p, lua_arg).map_err(|e| {
            mlua::Error::external(format!(
                "marshalling arg `{}` for `{}`: {e}",
                p.name, plan.name
            ))
        })?;
    }

    // Dispatch.
    let ret_buf = ctx
        .host
        .call_ufunction(obj_addr, class_addr, &plan.name, buf)
        .map_err(host_err)?;

    // Unmarshal: collect every RETURN + OUT slot in declaration order.
    // Typical case is a single RETURN (e.g. `K2_GetPawn -> APawn*`), but
    // UE5 supports multi-out via `out` params — we surface every one as
    // an extra Lua return value.
    let mut returns: Variadic<mlua::Value> = Variadic::new();
    for p in &plan.params {
        if !(p.flags.returns || p.flags.out) {
            continue;
        }
        let v = decode_param_from_buf(lua, &ret_buf, p).map_err(|e| {
            mlua::Error::external(format!(
                "unmarshalling `{}` from `{}`: {e}",
                p.name, plan.name
            ))
        })?;
        returns.push(v);
    }
    Ok(returns)
}

/// UE5 `CPF_OutParm` without `CPF_ReturnParm` covers both "OUT" and
/// "IN-OUT" cases — Blueprint compilers emit OutParm for any pass-by-ref
/// argument, but only some of those are *also* read on entry. We err on
/// the safe side: treat every non-RETURN flag-set as IN-OUT for
/// marshalling (caller can pass a value; we encode it; UE5 may overwrite).
/// A future v3 can introspect `CPF_ConstParm` to refine.
fn is_in_out(_p: &UFunctionParam) -> bool {
    // v2: always treat OUT params as IN-OUT so the caller can supply a
    // value (which is the common case for FVector / FRotator outs etc.).
    // For pure-OUT cases the caller can pass nil and we'll zero the slot.
    true
}

/// Write `value` into `buf` at `param.offset` per `param.kind`.
fn encode_param_into_buf(
    buf: &mut [u8],
    param: &UFunctionParam,
    value: mlua::Value,
) -> Result<(), String> {
    let off = param.offset as usize;
    let size = param.size as usize;
    if off.saturating_add(size) > buf.len() {
        return Err(format!(
            "param `{}` at offset {off}+{size} overflows {}-byte buffer",
            param.name,
            buf.len()
        ));
    }
    // Nil for an OUT-only slot is benign — leave it zeroed.
    if matches!(value, mlua::Value::Nil) {
        return Ok(());
    }
    macro_rules! to_int {
        ($t:ty) => {{
            let i = match &value {
                mlua::Value::Integer(i) => *i,
                mlua::Value::Number(n) => *n as i64,
                mlua::Value::Boolean(b) => *b as i64,
                other => {
                    return Err(format!("expected integer, got {}", other.type_name()));
                }
            };
            i as $t
        }};
    }
    let bytes: Vec<u8> = match param.kind {
        PropKind::I8 => (to_int!(i8)).to_le_bytes().to_vec(),
        PropKind::I16 => (to_int!(i16)).to_le_bytes().to_vec(),
        PropKind::I32 => (to_int!(i32)).to_le_bytes().to_vec(),
        PropKind::I64 => (to_int!(i64)).to_le_bytes().to_vec(),
        PropKind::U8 => (to_int!(u8)).to_le_bytes().to_vec(),
        PropKind::U16 => (to_int!(u16)).to_le_bytes().to_vec(),
        PropKind::U32 => (to_int!(u32)).to_le_bytes().to_vec(),
        PropKind::U64 => (to_int!(u64)).to_le_bytes().to_vec(),
        PropKind::F32 => {
            let f = match &value {
                mlua::Value::Number(n) => *n as f32,
                mlua::Value::Integer(i) => *i as f32,
                other => {
                    return Err(format!("expected number, got {}", other.type_name()));
                }
            };
            f.to_le_bytes().to_vec()
        }
        PropKind::F64 => {
            let f = match &value {
                mlua::Value::Number(n) => *n,
                mlua::Value::Integer(i) => *i as f64,
                other => {
                    return Err(format!("expected number, got {}", other.type_name()));
                }
            };
            f.to_le_bytes().to_vec()
        }
        PropKind::Bool => {
            let b = match &value {
                mlua::Value::Boolean(b) => *b,
                mlua::Value::Integer(i) => *i != 0,
                other => {
                    return Err(format!("expected boolean, got {}", other.type_name()));
                }
            };
            vec![b as u8]
        }
        PropKind::Bytes(n) => {
            let s = match &value {
                mlua::Value::String(s) => s.as_bytes().to_vec(),
                other => {
                    return Err(format!(
                        "expected string of {n} bytes, got {}",
                        other.type_name()
                    ));
                }
            };
            if s.len() as u32 != n {
                return Err(format!("expected exactly {n} bytes, got {} bytes", s.len()));
            }
            s
        }
    };
    let copy_len = bytes.len().min(size);
    buf[off..off + copy_len].copy_from_slice(&bytes[..copy_len]);
    Ok(())
}

/// Read one OUT / RETURN slot out of `buf` per `param.kind`.
fn decode_param_from_buf(
    lua: &Lua,
    buf: &[u8],
    param: &UFunctionParam,
) -> Result<mlua::Value, String> {
    let off = param.offset as usize;
    let size = param.size as usize;
    if off.saturating_add(size) > buf.len() {
        return Err(format!(
            "param `{}` at offset {off}+{size} overflows {}-byte response",
            param.name,
            buf.len()
        ));
    }
    let slice = &buf[off..off + size];
    let v: mlua::Value = match param.kind {
        PropKind::I8 => mlua::Value::Integer(i8::from_le_bytes(slice.try_into().unwrap()) as i64),
        PropKind::I16 => mlua::Value::Integer(i16::from_le_bytes(slice.try_into().unwrap()) as i64),
        PropKind::I32 => mlua::Value::Integer(i32::from_le_bytes(slice.try_into().unwrap()) as i64),
        PropKind::I64 => mlua::Value::Integer(i64::from_le_bytes(slice.try_into().unwrap())),
        PropKind::U8 => mlua::Value::Integer(slice[0] as i64),
        PropKind::U16 => mlua::Value::Integer(u16::from_le_bytes(slice.try_into().unwrap()) as i64),
        PropKind::U32 => mlua::Value::Integer(u32::from_le_bytes(slice.try_into().unwrap()) as i64),
        PropKind::U64 => mlua::Value::Integer(u64::from_le_bytes(slice.try_into().unwrap()) as i64),
        PropKind::F32 => mlua::Value::Number(f32::from_le_bytes(slice.try_into().unwrap()) as f64),
        PropKind::F64 => mlua::Value::Number(f64::from_le_bytes(slice.try_into().unwrap())),
        PropKind::Bool => mlua::Value::Boolean(slice[0] != 0),
        PropKind::Bytes(n) => {
            // Object returns are surfaced as a fresh UObjectHandle: UE5
            // ObjectProperty is 8 bytes pointing at the UObject in game
            // memory. The handle's class_addr is unknown until the next
            // reflection hop — chaining through GetClass() rediscovers it.
            if n == 8 {
                let ptr = u64::from_le_bytes(slice.try_into().unwrap());
                let ud = lua
                    .create_userdata(UObjectHandle {
                        addr: ptr,
                        class_addr: 0,
                    })
                    .map_err(|e| e.to_string())?;
                mlua::Value::UserData(ud)
            } else {
                let s = lua.create_string(slice).map_err(|e| e.to_string())?;
                mlua::Value::String(s)
            }
        }
    };
    Ok(v)
}

/// Format a Lua MultiValue the way stock `print` does: tab-separated,
/// using `tostring` on each value.
fn format_print_args(lua: &Lua, args: MultiValue) -> mlua::Result<String> {
    let tostring: Function = lua.globals().get("tostring")?;
    let mut out = String::new();
    for (i, v) in args.into_iter().enumerate() {
        if i > 0 {
            out.push('\t');
        }
        let s: mlua::String = tostring.call(v)?;
        out.push_str(&s.to_str()?);
    }
    Ok(out)
}

/// Install every UE4SS-compat binding onto `lua`. Call this on the worker
/// thread after [`Lua::new`]; pair with [`crate::keys::load_key_table`].
pub fn install(lua: &Lua, ctx: Arc<BindingsCtx>) -> mlua::Result<()> {
    lua.set_app_data(ctx.clone());

    let g = lua.globals();

    // -- print -----------------------------------------------------------
    let output = ctx.output.clone();
    let print = lua.create_function(move |lua, args: MultiValue| {
        let line = format_print_args(lua, args)?;
        output.push_info(line);
        Ok(())
    })?;
    g.set("print", print)?;

    // -- StaticFindObject ------------------------------------------------
    let ctx_static = ctx.clone();
    let static_find = lua.create_function(move |_lua, path: String| {
        // UE4SS's StaticFindObject takes a full object path. We map that
        // onto FqnPrefix — the engine returns the first object whose FQN
        // starts with `path` (case-insensitive). For exact-match
        // semantics callers can use `FindAllOf` + their own filter.
        let res = ctx_static
            .host
            .find_uobject("", &NamePredicate::FqnPrefix(path))
            .map_err(host_err)?;
        Ok(res.map(|(obj, cls)| UObjectHandle::with_class(obj, cls)))
    })?;
    g.set("StaticFindObject", static_find)?;

    // -- FindAllOf -------------------------------------------------------
    let ctx_all = ctx.clone();
    let find_all = lua.create_function(move |lua, class: String| {
        let res = ctx_all
            .host
            .find_all_uobjects(&class, &NamePredicate::Any, 4096)
            .map_err(host_err)?;
        let tbl = lua.create_table()?;
        for (i, fo) in res.into_iter().enumerate() {
            tbl.set(i + 1, UObjectHandle::with_class(fo.obj_addr, fo.class_addr))?;
        }
        Ok(tbl)
    })?;
    g.set("FindAllOf", find_all)?;

    // -- ExecuteInGameThread --------------------------------------------
    // Semantics shift vs. v1: the closure is now QUEUED for the next
    // engine-thread tick rather than invoked immediately. The DLL's
    // `ProcessEvent` detour (`game_thread_hook.rs`) drains this queue
    // from the engine's own thread, so closures observe the same thread
    // context as a vanilla UFunction handler. ProcessEvent fires dozens
    // of times per frame, so typical latency is sub-millisecond.
    //
    // The closure is stored as a `RegistryKey` (the only way to keep a
    // `Function` alive across the foreign-thread boundary). The drain
    // code in `LuaRuntime::drain_game_thread` materializes the `Function`
    // via `registry_value`, calls it, then removes the registry slot —
    // closures are one-shot. For a polling loop, schedule with
    // `LoopAsync` or re-enqueue from inside the closure.
    let workers_eit = ctx.workers.clone();
    let exec_in_game = lua.create_function(move |lua, f: Function| {
        let key: RegistryKey = lua.create_registry_value(f)?;
        workers_eit.game_thread.lock().push_back(key);
        Ok(())
    })?;
    g.set("ExecuteInGameThread", exec_in_game)?;

    // -- ExecuteWithDelay ------------------------------------------------
    let workers_ewd = ctx.workers.clone();
    let exec_with_delay = lua.create_function(move |lua, (ms, f): (u64, Function)| {
        let id = workers_ewd.alloc_delayed_id();
        let key: RegistryKey = lua.create_registry_value(f)?;
        workers_ewd.delayed_callbacks.lock().insert(id, key);
        workers_ewd.delayed.lock().push(DelayedTask {
            deadline: Instant::now() + Duration::from_millis(ms),
            id,
        });
        Ok(id)
    })?;
    g.set("ExecuteWithDelay", exec_with_delay)?;

    // -- LoopAsync (UE4SS-compat) ---------------------------------------
    // Implement in Lua on top of ExecuteWithDelay. The closure should
    // return `true` to stop the loop; any other (including no) return
    // schedules the next tick.
    lua.load(
        r#"
        function LoopAsync(ms, fn)
            local function step()
                local ok, stop = pcall(fn)
                if not ok then
                    -- fn errored; surface via print so the user sees it
                    print("LoopAsync error: " .. tostring(stop))
                    return
                end
                if stop ~= true then
                    ExecuteWithDelay(ms, step)
                end
            end
            ExecuteWithDelay(ms, step)
        end
        "#,
    )
    .set_name("openforge-lua-bootstrap")
    .exec()?;

    // -- RegisterKeyBind -------------------------------------------------
    let workers_rkb = ctx.workers.clone();
    let register_keybind = lua.create_function(move |lua, (vk, f): (u32, Function)| {
        let key: RegistryKey = lua.create_registry_value(f)?;
        workers_rkb.keybinds.lock().insert(vk, key);
        Ok(())
    })?;
    g.set("RegisterKeyBind", register_keybind)?;

    // -- IsShuttingDown helper ------------------------------------------
    // Not part of UE4SS but useful for long loops in scripts. Returns
    // true once the runtime is on its way out so a `while` loop can bail.
    let shutdown_flag = ctx.shutdown.clone();
    let is_shutdown =
        lua.create_function(move |_, ()| Ok(shutdown_flag.load(AOrdering::Relaxed)))?;
    g.set("IsShuttingDown", is_shutdown)?;

    Ok(())
}

/// Invoke a delayed callback by id from the worker thread. Returns an
/// `mlua::Error` if the registry slot is missing (shouldn't happen unless
/// the script was stopped between scheduling + dispatch).
pub fn dispatch_delayed(lua: &Lua, workers: &Workers, id: u64) -> mlua::Result<()> {
    let key = workers
        .delayed_callbacks
        .lock()
        .remove(&id)
        .ok_or_else(|| mlua::Error::external(format!("no delayed callback with id {id}")))?;
    let f: Function = lua.registry_value(&key)?;
    f.call::<MultiValue>(())?;
    // Drop the registry slot — `RegistryKey::drop` does not release the
    // slot back to Lua automatically in mlua 0.10; we must explicitly
    // remove it to avoid an unbounded registry.
    lua.remove_registry_value(key)?;
    Ok(())
}

/// Invoke a keybind callback by vk from the worker thread.
pub fn dispatch_key(lua: &Lua, workers: &Workers, vk: u32) -> mlua::Result<()> {
    // The keybind stays armed across dispatches — we only borrow the
    // registry key to call the function, never remove it.
    let key_ref: Option<Function> = {
        let map = workers.keybinds.lock();
        match map.get(&vk) {
            Some(k) => Some(lua.registry_value::<Function>(k)?),
            None => None,
        }
    };
    if let Some(f) = key_ref {
        f.call::<MultiValue>(())?;
    }
    Ok(())
}

/// Helper for upstream code that wants to test "did this Lua value come
/// from one of our UObject userdata wrappers". Currently unused by the
/// runtime but useful in tests and downstream scripts.
#[allow(dead_code)]
pub(crate) fn is_uobject(ud: &AnyUserData) -> bool {
    ud.is::<UObjectHandle>()
}
