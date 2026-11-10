//! `oxy migrate-automations` — migrate a customer project from the legacy
//! Procedures/Workflows naming to the canonical **Automations** naming.
//!
//! What it does, rooted at the given project directory:
//!   1. Renames every `*.procedure.yml` / `*.workflow.yml` file to the new
//!      canonical `*.automation.yml` extension.
//!   2. Rewrites references to the old extensions in every `.yml` / `.yaml` /
//!      `.sql` file — covering both exact path refs (`src:`, `workflow_ref:`,
//!      `agent_ref:` targets) and glob includes (`procedures/*.procedure.yml`).
//!
//! Back-compat note: the runtime still parses `.procedure.yml` /
//! `.workflow.yml`, so this migration is opt-in and safe to run incrementally.
//! `--dry-run` previews the plan without touching disk.
//!
//! Naming collisions (e.g. a directory holding both `x.procedure.yml` and
//! `x.workflow.yml`, which would both map to `x.automation.yml`, or a target
//! `x.automation.yml` that already exists) are reported and skipped — the tool
//! never overwrites a file. Those few cases are resolved by hand.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use clap::Parser;
use oxy_shared::errors::OxyError;

use ::oxy::theme::StyledText;

/// Directories never walked — hidden/build dirs that may hold stray copies.
/// Mirrors the semantic file-discovery skip list.
const SKIP_DIRS: &[&str] = &[
    ".git",
    ".worktrees",
    ".oxy_state",
    "target",
    "node_modules",
    "dist",
    "build",
    ".venv",
];

/// Legacy extensions that become the canonical one.
const OLD_EXTENSIONS: &[&str] = &[".procedure.yml", ".workflow.yml"];
const NEW_EXTENSION: &str = ".automation.yml";

/// File kinds whose textual references we rewrite.
const REWRITE_EXTENSIONS: &[&str] = &[".yml", ".yaml", ".sql"];

#[derive(Parser, Debug)]
pub struct MigrateAutomationsArgs {
    /// Project directory to migrate (defaults to the current directory).
    #[clap(default_value = ".")]
    pub path: String,

    /// Preview the changes without modifying any files.
    #[clap(long, default_value_t = false)]
    pub dry_run: bool,
}

struct Rename {
    from: PathBuf,
    to: PathBuf,
}

/// Entry point for `oxy migrate-automations`.
pub fn migrate_automations(args: MigrateAutomationsArgs) -> Result<(), OxyError> {
    let root = PathBuf::from(&args.path);
    if !root.is_dir() {
        return Err(OxyError::ArgumentError(format!(
            "{} is not a directory",
            root.display()
        )));
    }

    let mut files = Vec::new();
    collect_files(&root, &mut files)?;

    let (renames, collisions) = plan_renames(&files);
    report_collisions(&root, &collisions);

    if renames.is_empty() {
        if collisions.is_empty() {
            println!(
                "{}",
                "No .procedure.yml / .workflow.yml files found — nothing to migrate.".text()
            );
        }
        return Ok(());
    }

    if args.dry_run {
        print_plan(&root, &renames);
        println!(
            "{}",
            format!(
                "\nDry run: {} file(s) would be renamed. Re-run without --dry-run to apply.",
                renames.len()
            )
            .tertiary()
        );
        return Ok(());
    }

    apply_renames(&root, &renames)?;
    let rewritten = rewrite_references(&root)?;

    println!(
        "{}",
        format!(
            "\n✓ Renamed {} file(s); updated references in {} file(s).",
            renames.len(),
            rewritten
        )
        .success()
    );
    if !collisions.is_empty() {
        println!(
            "{}",
            format!(
                "⚠ Skipped {} file(s) due to naming collisions (listed above) — resolve them by hand.",
                collisions.len()
            )
            .warning()
        );
    }
    Ok(())
}

/// Recursively collect every file under `dir`, skipping hidden/build dirs.
fn collect_files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), OxyError> {
    let entries = std::fs::read_dir(dir)
        .map_err(|e| OxyError::IOError(format!("read dir {}: {e}", dir.display())))?;
    for entry in entries {
        let entry = entry.map_err(|e| OxyError::IOError(format!("read entry: {e}")))?;
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if path.is_dir() {
            if SKIP_DIRS.contains(&name.as_ref()) || name.starts_with('.') {
                continue;
            }
            collect_files(&path, out)?;
        } else {
            out.push(path);
        }
    }
    Ok(())
}

/// Compute the rename plan and the set of collision sources to skip.
///
/// A source is a collision when its target already exists on disk, or when
/// more than one source maps to the same target (e.g. both `x.procedure.yml`
/// and `x.workflow.yml`).
fn plan_renames(files: &[PathBuf]) -> (Vec<Rename>, Vec<PathBuf>) {
    let existing: HashSet<&Path> = files.iter().map(|p| p.as_path()).collect();
    let sources: Vec<(&PathBuf, PathBuf)> = files
        .iter()
        .filter_map(|p| target_path(p).map(|t| (p, t)))
        .collect();

    // Targets claimed by more than one source.
    let mut seen: HashSet<PathBuf> = HashSet::new();
    let mut duplicated: HashSet<PathBuf> = HashSet::new();
    for (_, target) in &sources {
        if !seen.insert(target.clone()) {
            duplicated.insert(target.clone());
        }
    }

    let mut renames = Vec::new();
    let mut collisions = Vec::new();
    for (src, target) in sources {
        if duplicated.contains(&target) || existing.contains(target.as_path()) {
            collisions.push(src.clone());
        } else {
            renames.push(Rename {
                from: src.clone(),
                to: target,
            });
        }
    }
    (renames, collisions)
}

/// The `.automation.yml` target for a legacy file, or `None` if not legacy.
fn target_path(path: &Path) -> Option<PathBuf> {
    let name = path.file_name()?.to_str()?;
    for ext in OLD_EXTENSIONS {
        if let Some(stem) = name.strip_suffix(ext) {
            return Some(path.with_file_name(format!("{stem}{NEW_EXTENSION}")));
        }
    }
    None
}

fn apply_renames(root: &Path, renames: &[Rename]) -> Result<(), OxyError> {
    for r in renames {
        std::fs::rename(&r.from, &r.to).map_err(|e| {
            OxyError::IOError(format!(
                "rename {} -> {}: {e}",
                r.from.display(),
                r.to.display()
            ))
        })?;
        println!(
            "{}",
            format!("renamed {} -> {}", rel(root, &r.from), rel(root, &r.to)).text()
        );
    }
    Ok(())
}

/// Replace legacy extension references in every rewritable file under `root`.
/// Returns the number of files actually modified.
fn rewrite_references(root: &Path) -> Result<usize, OxyError> {
    let mut files = Vec::new();
    collect_files(root, &mut files)?;
    let mut modified = 0;
    for path in files {
        if !is_rewritable(&path) {
            continue;
        }
        let original = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(_) => continue, // skip non-UTF8 / unreadable files
        };
        let mut updated = original.clone();
        for ext in OLD_EXTENSIONS {
            updated = updated.replace(ext, NEW_EXTENSION);
        }
        if updated != original {
            std::fs::write(&path, updated)
                .map_err(|e| OxyError::IOError(format!("write {}: {e}", path.display())))?;
            println!("{}", format!("updated refs in {}", rel(root, &path)).text());
            modified += 1;
        }
    }
    Ok(modified)
}

fn is_rewritable(path: &Path) -> bool {
    let name = match path.file_name().and_then(|n| n.to_str()) {
        Some(n) => n,
        None => return false,
    };
    REWRITE_EXTENSIONS.iter().any(|ext| name.ends_with(ext))
}

fn report_collisions(root: &Path, collisions: &[PathBuf]) {
    if collisions.is_empty() {
        return;
    }
    println!(
        "{}",
        "Naming collisions (skipped — target already exists or two files map to one name):"
            .warning()
    );
    for c in collisions {
        println!("{}", format!("  {}", rel(root, c)).warning());
    }
    println!();
}

fn print_plan(root: &Path, renames: &[Rename]) {
    println!("{}", "Planned renames:".tertiary());
    for r in renames {
        println!(
            "{}",
            format!("  {} -> {}", rel(root, &r.from), rel(root, &r.to)).text()
        );
    }
}

fn rel(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .display()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write(dir: &Path, name: &str, content: &str) {
        let p = dir.join(name);
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(p, content).unwrap();
    }

    #[test]
    fn renames_and_rewrites_references() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(root, "procedures/report.procedure.yml", "tasks: []\n");
        write(root, "workflows/sync.workflow.yml", "tasks: []\n");
        write(
            root,
            "agents/default.agentic.yml",
            "workflow_ref: procedures/report.procedure.yml\nincludes:\n  - workflows/*.workflow.yml\n",
        );

        migrate_automations(MigrateAutomationsArgs {
            path: root.to_string_lossy().to_string(),
            dry_run: false,
        })
        .unwrap();

        assert!(root.join("procedures/report.automation.yml").exists());
        assert!(root.join("workflows/sync.automation.yml").exists());
        assert!(!root.join("procedures/report.procedure.yml").exists());

        let agent = fs::read_to_string(root.join("agents/default.agentic.yml")).unwrap();
        assert!(agent.contains("procedures/report.automation.yml"));
        assert!(agent.contains("workflows/*.automation.yml"));
        assert!(!agent.contains(".procedure.yml"));
        assert!(!agent.contains(".workflow.yml"));
    }

    #[test]
    fn skips_collisions_without_overwriting() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        // Both map to corr.automation.yml -> collision, both skipped.
        write(root, "corr.procedure.yml", "a: 1\n");
        write(root, "corr.workflow.yml", "b: 2\n");
        // Clean one renames fine.
        write(root, "solo.procedure.yml", "c: 3\n");

        migrate_automations(MigrateAutomationsArgs {
            path: root.to_string_lossy().to_string(),
            dry_run: false,
        })
        .unwrap();

        assert!(root.join("corr.procedure.yml").exists());
        assert!(root.join("corr.workflow.yml").exists());
        assert!(!root.join("corr.automation.yml").exists());
        assert!(root.join("solo.automation.yml").exists());
    }

    #[test]
    fn dry_run_changes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(root, "x.procedure.yml", "tasks: []\n");

        migrate_automations(MigrateAutomationsArgs {
            path: root.to_string_lossy().to_string(),
            dry_run: true,
        })
        .unwrap();

        assert!(root.join("x.procedure.yml").exists());
        assert!(!root.join("x.automation.yml").exists());
    }
}
