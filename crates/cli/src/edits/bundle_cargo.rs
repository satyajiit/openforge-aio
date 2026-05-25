//! Idempotent edit of `crates/bundle/Cargo.toml`: insert a path-dep for a new
//! game crate in alphabetical order. Preserves formatting via `toml_edit`.

use anyhow::{Context, Result};
use toml_edit::{DocumentMut, InlineTable, Item, Value};

pub fn add_game_dep(text: &str, game_id: &str) -> Result<String> {
    let mut doc: DocumentMut = text.parse().context("parse bundle Cargo.toml")?;
    let deps = doc["dependencies"].or_insert(Item::Table(Default::default()));
    let table = deps
        .as_table_mut()
        .context("[dependencies] is not a table")?;

    let dep_name = format!("openforge-game-{game_id}");
    if table.contains_key(&dep_name) {
        return Ok(doc.to_string());
    }

    let mut value = InlineTable::new();
    value.insert("path", Value::from(format!("../games/{game_id}")));
    table.insert(&dep_name, Item::Value(Value::InlineTable(value)));

    // Re-sort dependencies alphabetically (preserves comments via toml_edit).
    table.sort_values();

    Ok(doc.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const BEFORE: &str = "[package]
name = \"openforge-bundle\"

[dependencies]
openforge-game-batman = { path = \"../games/batman\" }
";

    #[test]
    fn inserts_alphabetically() {
        let after = add_game_dep(BEFORE, "arkham").unwrap();
        assert!(after.contains("openforge-game-arkham = { path = \"../games/arkham\" }"));
        let arkham_idx = after.find("openforge-game-arkham").unwrap();
        let batman_idx = after.find("openforge-game-batman").unwrap();
        assert!(arkham_idx < batman_idx);
    }

    #[test]
    fn idempotent() {
        let once = add_game_dep(BEFORE, "batman").unwrap();
        let twice = add_game_dep(&once, "batman").unwrap();
        assert_eq!(once, twice);
    }
}
