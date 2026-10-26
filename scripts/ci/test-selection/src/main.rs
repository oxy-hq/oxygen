//! `test-selection --old-metadata <json> --new-metadata <json> --changed <file>
//! --config <toml> --root-manifest <Cargo.toml>`
//!
//! Prints the selection as JSON on stdout (`{"mode":"all"}`,
//! `{"mode":"packages","packages":[...]}` or `{"mode":"none"}`) and, on stderr,
//! what each changed path matched. `--changed` is NUL-separated, as
//! `git diff --name-only -z` writes it. `select.sh` produces every input.

use std::collections::HashMap;
use std::process::ExitCode;

use guppy::graph::PackageGraph;
use test_selection::{Change, Config, explain, select, workspace_resolver};

const FLAGS: [&str; 5] = [
    "--old-metadata",
    "--new-metadata",
    "--changed",
    "--config",
    "--root-manifest",
];

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("test-selection: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let args = parse_args(std::env::args().skip(1))?;
    let read = |flag: &str| {
        let path = &args[flag];
        std::fs::read_to_string(path).map_err(|e| format!("{flag} {path}: {e}"))
    };
    let old = graph(&read("--old-metadata")?, "--old-metadata")?;
    let new = graph(&read("--new-metadata")?, "--new-metadata")?;
    let changed_paths: Vec<String> = read("--changed")?
        .split('\0')
        .filter(|p| !p.is_empty())
        .map(str::to_owned)
        .collect();
    let config = Config::parse(&read("--config")?)?;
    let change = Change {
        old: &old,
        new: &new,
        changed_paths: &changed_paths,
        resolver: workspace_resolver(&read("--root-manifest")?)?,
    };

    for line in explain(&change, &config)? {
        eprintln!("{line}");
    }
    let selection = select(&change, &config)?;
    println!(
        "{}",
        serde_json::to_string(&selection).map_err(|e| e.to_string())?
    );
    Ok(())
}

fn parse_args(mut args: impl Iterator<Item = String>) -> Result<HashMap<String, String>, String> {
    let mut parsed = HashMap::new();
    while let Some(flag) = args.next() {
        if !FLAGS.contains(&flag.as_str()) {
            return Err(format!(
                "unknown argument {flag}; expected {}",
                FLAGS.join(", ")
            ));
        }
        let value = args.next().ok_or_else(|| format!("{flag} needs a value"))?;
        parsed.insert(flag, value);
    }
    match FLAGS.iter().find(|flag| !parsed.contains_key(**flag)) {
        Some(missing) => Err(format!("missing {missing}")),
        None => Ok(parsed),
    }
}

fn graph(json: &str, flag: &str) -> Result<PackageGraph, String> {
    PackageGraph::from_json(json).map_err(|e| format!("{flag}: {e}"))
}
