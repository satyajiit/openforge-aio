//! Idempotent edit of `crates/bundle/src/lib.rs`. Inserts `pub use openforge_game_<id>;`
//! in alphabetical order with the other re-exports, and appends `openforge_game_<id>::GAME_ID`
//! to the `FORCE_LINK` static slice (also alpha-ordered).

use anyhow::{Result, anyhow};

const REEXPORT_PREFIX: &str = "pub use openforge_game_";
const LINK_PREFIX: &str = "        openforge_game_";
const LINK_SLICE_OPEN: &str = "static FORCE_LINK: &[&'static str] = &[";
const LINK_SLICE_CLOSE: &str = "    ];";

pub fn add_game(text: &str, game_id: &str) -> Result<String> {
    let crate_name = format!("openforge_game_{game_id}");
    let reexport = format!("{REEXPORT_PREFIX}{game_id};");
    let link = format!("{LINK_PREFIX}{game_id}::GAME_ID,");

    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();

    // Insert `pub use` line in alpha order, but only if absent.
    if !lines.iter().any(|l| l.trim() == reexport.trim()) {
        let existing: Vec<usize> = lines
            .iter()
            .enumerate()
            .filter_map(|(i, l)| {
                if l.trim_start().starts_with(REEXPORT_PREFIX) {
                    Some(i)
                } else {
                    None
                }
            })
            .collect();
        let insert_at = if existing.is_empty() {
            // Fall back: insert after the first blank line below `//!` comments.
            let first_blank = lines
                .iter()
                .position(|l| l.trim().is_empty())
                .unwrap_or(lines.len());
            first_blank + 1
        } else {
            let mut chosen = *existing.last().unwrap() + 1;
            for &i in &existing {
                let existing_name = lines[i]
                    .trim_start()
                    .strip_prefix(REEXPORT_PREFIX)
                    .and_then(|s| s.split([';', ' ']).next())
                    .unwrap_or("");
                if game_id < existing_name {
                    chosen = i;
                    break;
                }
            }
            chosen
        };
        lines.insert(insert_at, reexport);
    }

    // Update FORCE_LINK slice.
    let open_idx = lines
        .iter()
        .position(|l| l.contains(LINK_SLICE_OPEN))
        .ok_or_else(|| anyhow!("FORCE_LINK slice not found in bundle lib"))?;
    let close_idx = lines
        .iter()
        .enumerate()
        .skip(open_idx + 1)
        .find(|(_, l)| l.trim() == LINK_SLICE_CLOSE.trim())
        .map(|(i, _)| i)
        .ok_or_else(|| anyhow!("FORCE_LINK closing bracket not found"))?;

    let already_present = lines[(open_idx + 1)..close_idx]
        .iter()
        .any(|l| l.contains(&format!("{crate_name}::GAME_ID")));

    if !already_present {
        let inner: Vec<usize> = ((open_idx + 1)..close_idx)
            .filter(|i| lines[*i].trim().starts_with("openforge_game_"))
            .collect();
        let insert_at = if inner.is_empty() {
            open_idx + 1
        } else {
            let mut chosen = *inner.last().unwrap() + 1;
            for &i in &inner {
                let existing_name = lines[i]
                    .trim_start()
                    .strip_prefix("openforge_game_")
                    .and_then(|s| s.split("::").next())
                    .unwrap_or("");
                if game_id < existing_name {
                    chosen = i;
                    break;
                }
            }
            chosen
        };
        lines.insert(insert_at, link);
    }

    let mut joined = lines.join("\n");
    if text.ends_with('\n') && !joined.ends_with('\n') {
        joined.push('\n');
    }
    Ok(joined)
}

#[cfg(test)]
mod tests {
    use super::*;

    const BEFORE: &str = r#"//! comment

pub use openforge_game_batman;

pub fn ensure_linked() {
    #[used]
    static FORCE_LINK: &[&'static str] = &[
        openforge_game_batman::GAME_ID,
    ];
    let _ = FORCE_LINK;
}
"#;

    #[test]
    fn inserts_before_batman() {
        let out = add_game(BEFORE, "arkham").unwrap();
        let a_idx = out.find("pub use openforge_game_arkham;").unwrap();
        let b_idx = out.find("pub use openforge_game_batman;").unwrap();
        assert!(a_idx < b_idx, "arkham must come before batman");
        let force_a = out.find("openforge_game_arkham::GAME_ID,").unwrap();
        let force_b = out.find("openforge_game_batman::GAME_ID,").unwrap();
        assert!(force_a < force_b);
    }

    #[test]
    fn idempotent() {
        let once = add_game(BEFORE, "batman").unwrap();
        let twice = add_game(&once, "batman").unwrap();
        assert_eq!(once, twice);
    }
}
