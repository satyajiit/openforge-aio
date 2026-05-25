//! Parse-only Lua validation.
//!
//! We `load` the source into a chunk via `mlua` and ask for it as a
//! function — that runs the full Lua 5.4 parser without executing a single
//! statement. The throwaway `Lua` instance is dropped immediately; we never
//! evaluate user code on the host (the in-game runtime in Phase B does).

use super::{LuaParseError, LuaValidation};

/// Parse `code` and return a validation result. Always returns `Ok` — a
/// syntax error is encoded as `LuaValidation { ok: false, errors: [...] }`
/// rather than `Err`, so the FE renders inline rather than as a toast.
pub fn parse_validate(code: &str, name: &str) -> LuaValidation {
    let lua = mlua::Lua::new();
    match lua.load(code).set_name(name).into_function() {
        Ok(_) => LuaValidation {
            ok: true,
            errors: Vec::new(),
        },
        Err(err) => LuaValidation {
            ok: false,
            errors: vec![extract_error(&err)],
        },
    }
}

fn extract_error(err: &mlua::Error) -> LuaParseError {
    let msg = err.to_string();
    let line = parse_line_number(&msg).unwrap_or(0);
    LuaParseError { line, message: msg }
}

/// mlua's syntax errors look like:
///   `syntax error: [string "user_script"]:12: '<eof>' expected near 'end'`
/// We pull out the integer between the last `]:` and `:`. Best-effort —
/// returns `None` if the format ever shifts.
fn parse_line_number(msg: &str) -> Option<u32> {
    let after_bracket = msg.rfind("]:")?;
    let rest = &msg[after_bracket + 2..];
    let end = rest.find(':')?;
    rest[..end].parse::<u32>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ok_for_valid_script() {
        let v = parse_validate("print('hi')\n", "t");
        assert!(v.ok, "expected ok=true, got {v:?}");
        assert!(v.errors.is_empty());
    }

    #[test]
    fn error_with_line_for_invalid_script() {
        let v = parse_validate("print('hi'\n", "t");
        assert!(!v.ok);
        assert_eq!(v.errors.len(), 1);
        assert!(v.errors[0].line >= 1, "got {:?}", v.errors[0]);
    }
}
