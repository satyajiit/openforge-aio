use std::fs;
use std::process::ExitCode;

use anyhow::{Context, Result, anyhow};
use openforge_runtime::{ConfigFormat, GameManifest};

use crate::cli::NewFeatureArgs;
use crate::sigemit::{StubArgs, signature_stub, signature_stub_ron};
use crate::term;
use crate::workspace::Workspace;

pub fn run(ws: &Workspace, args: &NewFeatureArgs) -> Result<ExitCode> {
    validate_slug(&args.game, "game id")?;
    validate_slug(&args.feature, "feature id")?;

    let game_dir = ws.game_dir(&args.game);
    if !game_dir.exists() {
        return Err(anyhow!(
            "game crate {} does not exist; run `new-game` first",
            game_dir.display()
        ));
    }

    // Honor the game's declared signature format. Read its manifest with the
    // REAL runtime parser so the scaffolder writes the same dialect the build
    // emits and the runtime parses. The per-file extension still wins at build
    // time; this seeds the right one.
    let manifest_path = game_dir.join("manifest.toml");
    let manifest_text = fs::read_to_string(&manifest_path)
        .with_context(|| format!("read {}", manifest_path.display()))?;
    let manifest = GameManifest::parse(&manifest_text)
        .map_err(|e| anyhow!("parse {}: {e}", manifest_path.display()))?;
    let format = manifest.engine.config_format;
    let ext = match format {
        ConfigFormat::Toml => "toml",
        ConfigFormat::Ron => "ron",
    };

    let sigs_dir = game_dir.join("signatures");
    fs::create_dir_all(&sigs_dir)?;
    let path = sigs_dir.join(format!("{}.{ext}", args.feature));
    if path.exists() {
        return Err(anyhow!(
            "{} already exists; refusing to overwrite",
            path.display()
        ));
    }
    let description = args
        .description
        .as_deref()
        .unwrap_or("<describe this cheat>");
    let tier = args.tier.as_deref().unwrap_or("general");
    let stub_args = StubArgs {
        feature: &args.feature,
        display_name: &args.name,
        description,
        tier,
        kind: args.kind,
        strategy: args.strategy,
    };
    let stub = match format {
        ConfigFormat::Toml => signature_stub(&stub_args),
        ConfigFormat::Ron => signature_stub_ron(&stub_args),
    };

    let tmp = path.with_extension(format!("{ext}.tmp-cli"));
    fs::write(&tmp, stub)?;
    fs::rename(&tmp, &path)?;
    term::ok(format!("created {}", path.display()));
    term::next_step(format!(
        "openforge-discover --game {} scan --feature {} --type {} --value <known> --name \"{}\"",
        args.game,
        args.feature,
        kind_label(args.kind),
        args.name
    ));
    Ok(ExitCode::SUCCESS)
}

fn validate_slug(s: &str, label: &str) -> Result<()> {
    let mut chars = s.chars();
    let Some(first) = chars.next() else {
        return Err(anyhow!("{label} cannot be empty"));
    };
    if !first.is_ascii_lowercase() {
        return Err(anyhow!(
            "{label} must start with a-z lowercase letter; got {first:?}"
        ));
    }
    for c in chars {
        if !(c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_') {
            return Err(anyhow!(
                "{label} must match [a-z][a-z0-9_]*; got invalid character {c:?}"
            ));
        }
    }
    Ok(())
}

fn kind_label(k: crate::cli::FeatureKind) -> &'static str {
    use crate::cli::FeatureKind;
    match k {
        FeatureKind::I8 => "i8",
        FeatureKind::I16 => "i16",
        FeatureKind::I32 => "i32",
        FeatureKind::I64 => "i64",
        FeatureKind::U8 => "u8",
        FeatureKind::U16 => "u16",
        FeatureKind::U32 => "u32",
        FeatureKind::U64 => "u64",
        FeatureKind::F32 => "f32",
        FeatureKind::F64 => "f64",
        FeatureKind::Bool => "bool",
    }
}
