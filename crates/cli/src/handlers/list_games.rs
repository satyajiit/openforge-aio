use std::fs;
use std::process::ExitCode;

use anyhow::Result;
use openforge_runtime::GameManifest;

use crate::cli::ListGamesArgs;
use crate::workspace::Workspace;

pub fn run(ws: &Workspace, args: &ListGamesArgs) -> Result<ExitCode> {
    let mut rows: Vec<Row> = Vec::new();
    for entry in fs::read_dir(ws.games_dir())? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .map(str::to_string)
            .unwrap_or_default();
        if name == "_template" || name.starts_with('.') {
            continue;
        }
        let manifest_path = path.join("manifest.toml");
        let Ok(text) = fs::read_to_string(&manifest_path) else {
            continue;
        };
        let Ok(m) = GameManifest::parse(&text) else {
            continue;
        };
        rows.push(Row {
            id: m.game.id.clone(),
            display_name: m.game.display_name.clone(),
            process_names: m.game.process_names.clone(),
            supported_versions: m.game.supported_versions.clone(),
        });
    }
    rows.sort_by(|a, b| a.id.cmp(&b.id));

    if args.markdown {
        println!("| id | display_name | process names | versions |");
        println!("|----|--------------|---------------|----------|");
        for row in &rows {
            println!(
                "| `{}` | {} | {} | {} |",
                row.id,
                row.display_name,
                row.process_names.join(", "),
                row.supported_versions.join(", ")
            );
        }
    } else {
        for row in &rows {
            println!("- {} ({})", row.display_name, row.id);
            println!("    processes : {}", row.process_names.join(", "));
            println!("    versions  : {}", row.supported_versions.join(", "));
        }
    }
    Ok(ExitCode::SUCCESS)
}

struct Row {
    id: String,
    display_name: String,
    process_names: Vec<String>,
    supported_versions: Vec<String>,
}
