//! Idempotent edit of the workspace `Cargo.toml`'s `[workspace].members` list.

use anyhow::{Context, Result, anyhow};
use toml_edit::{Array, DocumentMut, Item, Value};

pub fn add_member(text: &str, member: &str) -> Result<String> {
    let mut doc: DocumentMut = text.parse().context("parse workspace Cargo.toml")?;
    let workspace = doc
        .get_mut("workspace")
        .ok_or_else(|| anyhow!("[workspace] table missing"))?;
    let workspace_table = workspace
        .as_table_like_mut()
        .ok_or_else(|| anyhow!("[workspace] is not a table"))?;
    let members_item = workspace_table
        .get_mut("members")
        .ok_or_else(|| anyhow!("[workspace].members missing"))?;

    let arr: &mut Array = match members_item {
        Item::Value(Value::Array(a)) => a,
        _ => return Err(anyhow!("[workspace].members is not an array")),
    };

    let already = arr.iter().any(|v| v.as_str() == Some(member));
    if !already {
        arr.push(member);
        // Sort alphabetically.
        let mut owned: Vec<String> = arr
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect();
        owned.sort();
        arr.clear();
        for s in owned {
            arr.push(s);
        }
    }

    Ok(doc.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const BEFORE: &str = "[workspace]
resolver = \"2\"
members = [
    \"crates/core\",
    \"crates/runtime\",
    \"crates/games/batman\",
]
";

    #[test]
    fn inserts_and_sorts() {
        let after = add_member(BEFORE, "crates/games/arkham").unwrap();
        let a_idx = after.find("crates/games/arkham").unwrap();
        let b_idx = after.find("crates/games/batman").unwrap();
        assert!(a_idx < b_idx);
    }

    #[test]
    fn idempotent() {
        let once = add_member(BEFORE, "crates/games/batman").unwrap();
        let twice = add_member(&once, "crates/games/batman").unwrap();
        assert_eq!(once, twice);
    }
}
