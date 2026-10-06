//! File discovery + extension routing.
//!
//! Walks the workspace once and classifies each interesting file by
//! kind so the compiler can dispatch to the right per-kind parser.
//! Skips the obvious non-source directories (`.git`, `target`,
//! `node_modules`, `dist`, `.semantics`, `.preagg`) at any depth, the same
//! way the preagg executor's walk does — see [`is_skipped`].

use crate::errors::CompileError;
use glob::glob;
use std::path::{Path, PathBuf};
use tracing::{debug, warn};

/// One classified file the compiler will process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredFile {
    /// Workspace-relative path (forward slashes).
    pub rel_path: String,
    /// Absolute path; what we actually open.
    pub abs_path: PathBuf,
    pub kind: FileKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FileKind {
    /// `config.yml` at the workspace root.
    Config,
    /// `*.agentic.yml`.
    AgenticAgent,
    /// `*.view.yml`.
    SemanticView,
    /// `*.topic.yml`.
    SemanticTopic,
    /// `*.app.yml`.
    App,
    /// `*.procedure.yml` (back-compat), `*.automation.yml` (canonical).
    /// The exact extension is recorded so the row carries `extension`.
    Automation(AutomationKind),
    /// `*.sql` outside `modeling/` and `schemas/` (modeling SQL is dbt-shaped,
    /// handled by Airform later; `schemas/` is DDL, see `SchemaMigration`).
    VerifiedQuery,
    /// `schemas/**/*.sql` — DDL applied to the org's OLTP database, in
    /// `file_path` order. Carved out of `VerifiedQuery` because these are
    /// migrations to run, not queries to serve.
    SchemaMigration,
    /// `*.airway.yml`.
    AirwayPipeline,
    /// `.monitor.yml` at the workspace root — the anomaly-monitor
    /// configuration. Singleton per workspace.
    MonitorConfig,
    /// `reconcile.yml` at the workspace root — reconciliation checks.
    /// Singleton per workspace.
    ReconcileConfig,
    /// `.world-model.yml` at the workspace root — the world-model entity
    /// labels / display fields / allowlist. Singleton per workspace.
    WorldModelConfig,
    /// `*.simulation.yml` — a declared world with a known truth. Many per
    /// workspace: each point on the grid of worlds is its own file, which is
    /// what makes the grid versioned and reviewable rather than a set of CLI
    /// flags nobody can diff.
    Simulation,
    /// A `.md` file some agent's `context:` reaches. The one kind not found by
    /// extension alone: a README nobody references is not one. See
    /// [`crate::context_documents`].
    ContextDocument,
}

/// The kind of automation file. `.procedure.yml` is kept for back-compat;
/// `.automation.yml` is canonical. The `.workflow.yml` extension is no longer
/// recognized as a file kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AutomationKind {
    Procedure,
    Automation,
}

/// Back-compat alias — `AutomationKind` was historically named `ProcedureKind`.
pub type ProcedureKind = AutomationKind;

impl AutomationKind {
    pub fn extension(&self) -> &'static str {
        match self {
            AutomationKind::Procedure => "procedure",
            AutomationKind::Automation => "automation",
        }
    }
}

/// Walk the workspace and return every interesting file, classified.
/// Order is stable (sorted by `rel_path`) so the same workspace
/// always produces the same revision_id-relative order of work — makes
/// progress logs and failure summaries deterministic for replay.
pub fn discover(workspace_root: &Path) -> Result<Vec<DiscoveredFile>, CompileError> {
    if !workspace_root.exists() {
        return Err(CompileError::WorkspaceNotFound(
            workspace_root.display().to_string(),
        ));
    }
    if !workspace_root.is_dir() {
        return Err(CompileError::WorkspaceNotFound(format!(
            "{} is not a directory",
            workspace_root.display()
        )));
    }

    let mut out = Vec::new();

    // config.yml — root-only.
    let config_path = workspace_root.join("config.yml");
    if config_path.is_file() {
        out.push(DiscoveredFile {
            rel_path: "config.yml".to_string(),
            abs_path: config_path,
            kind: FileKind::Config,
        });
    }

    // .monitor.yml — root-only, anomaly monitor configuration. Optional.
    let monitor_path = workspace_root.join(".monitor.yml");
    if monitor_path.is_file() {
        out.push(DiscoveredFile {
            rel_path: ".monitor.yml".to_string(),
            abs_path: monitor_path,
            kind: FileKind::MonitorConfig,
        });
    }

    // reconcile.yml — root-only, reconciliation configuration. Optional.
    let reconcile_path = workspace_root.join("reconcile.yml");
    if reconcile_path.is_file() {
        out.push(DiscoveredFile {
            rel_path: "reconcile.yml".to_string(),
            abs_path: reconcile_path,
            kind: FileKind::ReconcileConfig,
        });
    }

    // .world-model.yml — root-only, world-model entity config. Optional.
    let world_model_path = workspace_root.join(".world-model.yml");
    if world_model_path.is_file() {
        out.push(DiscoveredFile {
            rel_path: ".world-model.yml".to_string(),
            abs_path: world_model_path,
            kind: FileKind::WorldModelConfig,
        });
    }

    // YAML kinds — one glob per extension. Done sequentially because
    // workspaces have ~tens to ~hundreds of files and the per-glob
    // overhead is negligible.
    push_glob(
        workspace_root,
        "**/*.agentic.yml",
        FileKind::AgenticAgent,
        &mut out,
    )?;
    push_glob(
        workspace_root,
        "**/*.view.yml",
        FileKind::SemanticView,
        &mut out,
    )?;
    push_glob(
        workspace_root,
        "**/*.topic.yml",
        FileKind::SemanticTopic,
        &mut out,
    )?;
    push_glob(workspace_root, "**/*.app.yml", FileKind::App, &mut out)?;
    push_glob(
        workspace_root,
        "**/*.procedure.yml",
        FileKind::Automation(AutomationKind::Procedure),
        &mut out,
    )?;
    push_glob(
        workspace_root,
        "**/*.automation.yml",
        FileKind::Automation(AutomationKind::Automation),
        &mut out,
    )?;
    push_glob(
        workspace_root,
        "**/*.airway.yml",
        FileKind::AirwayPipeline,
        &mut out,
    )?;
    push_glob(
        workspace_root,
        "**/*.simulation.yml",
        FileKind::Simulation,
        &mut out,
    )?;
    push_glob(
        workspace_root,
        "**/*.sql",
        FileKind::VerifiedQuery,
        &mut out,
    )?;
    // Must follow the VerifiedQuery glob, which skips `schemas/` so these
    // files are claimed here exactly once.
    push_glob(
        workspace_root,
        "schemas/**/*.sql",
        FileKind::SchemaMigration,
        &mut out,
    )?;
    push_context_documents(workspace_root, &mut out)?;

    out.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    Ok(out)
}

/// Markdown reached by an agent's `context:`. Must follow the agent glob: which
/// documents exist as a kind is decided by the agents already in `out`.
///
/// Not a `push_glob`, because there is no pattern to give it — the patterns are
/// the agents' own. The two skip rules it would have applied are applied by
/// the walk this shares with the working-copy arm, so the enumerations cannot
/// disagree.
fn push_context_documents(root: &Path, out: &mut Vec<DiscoveredFile>) -> Result<(), CompileError> {
    let agents: Vec<DiscoveredFile> = out
        .iter()
        .filter(|file| file.kind == FileKind::AgenticAgent)
        .cloned()
        .collect();
    let documents = crate::context_documents::reachable_from_agents(root, &agents)
        .map_err(|e| CompileError::Walk(format!("context documents: {e}")))?;
    out.extend(documents);
    Ok(())
}

/// Glob expansion + skip filter + classification helper.
fn push_glob(
    root: &Path,
    pattern: &str,
    kind: FileKind,
    out: &mut Vec<DiscoveredFile>,
) -> Result<(), CompileError> {
    let glob_pattern = root.join(pattern).to_string_lossy().into_owned();
    let entries = glob(&glob_pattern).map_err(|e| CompileError::Walk(e.to_string()))?;

    for entry in entries {
        let abs_path = match entry {
            Ok(p) => p,
            Err(e) => {
                warn!(error = %e, pattern = pattern, "glob entry failed");
                continue;
            }
        };

        if !abs_path.is_file() {
            continue;
        }

        let rel = match abs_path.strip_prefix(root) {
            Ok(r) => r.to_string_lossy().replace('\\', "/"),
            Err(_) => continue,
        };

        // Only the parent directories are checked against `is_skipped` — it
        // exists to prune directories (see its doc comment), not to drop a
        // dot-prefixed FILE. A dot-prefixed dir like `.oxy_state/` is still
        // skipped at any depth; `.draft.simulation.yml` at the workspace
        // root, or `legit/.wip.simulation.yml`, is not.
        let (dir_part, file_name) = match rel.rsplit_once('/') {
            Some((dir, name)) => (dir, name),
            None => ("", rel.as_str()),
        };
        if is_skipped(dir_part) {
            dropped(&rel, "under a skipped directory");
            continue;
        }

        // Don't include test files that mirror an extension —
        // `foo.agent.test.yml` is NOT a real `.agentic.yml`.
        // Same idea as `crates/core/src/config/storage.rs:421`.
        //
        // Matched against the FILE NAME only, deliberately. `.test.` names a
        // test file, and the sibling arm above goes out of its way to check
        // `dir_part` so that file names are left to this one; matching the
        // whole `rel` made this arm the only one that pruned on a DIRECTORY
        // name, so a real `orders.view.yml` under `semantics/v1.test.cases/`
        // vanished. That also put the two workspace enumerations back into
        // the disagreement `list_simulations_and_the_walker_drop_the_same_paths`
        // exists to prevent: `storage.rs::list_by_sub_extension` has no
        // `.test.` rule at all, so it listed such a file while this dropped
        // it. The conventional `x.agent.test.yml` and the pinned-divergence
        // `drafts/scratch.test.agentic.yml` both carry `.test.` in the file
        // name, so both still drop.
        if file_name.contains(".test.") {
            dropped(&rel, "test file mirroring an entity extension");
            continue;
        }

        // VerifiedQuery only applies outside `modeling/` — the dbt-shaped
        // SQL files there are handled by Airform's separate compile
        // pipeline (Phase 1.6c will fold those in). `schemas/` is not a drop:
        // the `schemas/**/*.sql` glob claims those as `SchemaMigration`
        // immediately after, so it is not reported as one.
        if matches!(kind, FileKind::VerifiedQuery) && rel.starts_with("schemas/") {
            continue;
        }
        if matches!(kind, FileKind::VerifiedQuery) && rel.starts_with("modeling/") {
            dropped(&rel, "dbt-shaped SQL under modeling/, owned by Airform");
            continue;
        }

        out.push(DiscoveredFile {
            rel_path: rel,
            abs_path,
            kind,
        });
    }

    Ok(())
}

/// Report a file that matched a glob but will not be compiled.
///
/// Discovery is the only thing between a file on disk and a
/// `*_definitions` row, so a drop with no record reads to the user as "my
/// view disappeared" with nothing to grep for — and the skip set matches
/// whole path components at any depth, so a workspace that happens to keep
/// models under `dist/` (distribution) or `build/` loses them with no error
/// anywhere. DEBUG rather than WARN on purpose: the common case is a
/// `node_modules/` with thousands of matches, and a warn per file would bury
/// every other line in the compile log.
fn dropped(rel_path: &str, reason: &str) {
    debug!(rel_path, reason, "discovery dropped a file");
}

/// Directories that should never be walked: build outputs, vendored
/// dependencies, VCS internals, derived artifact directories.
///
/// Checked against every path component, at any depth — a `sub/target/` or
/// `sub/build/` is a build directory wherever it sits, not just at the
/// workspace root. This is the single definition of "a path the workspace
/// does not enumerate": `crates/core/src/config/storage.rs` (the working-copy
/// arm) calls this same function rather than keeping its own skip list, so a
/// world (or any other kind) does not exist on one arm and not the other
/// depending on which instance answered. Matches the any-depth skip
/// documented in `product-context.md` -> "Semantic file discovery".
///
/// Missing a name here matters more than for plain discovery: a stray
/// .view.yml / .topic.yml / .agentic.yml copy under .oxy_state/ or build/
/// produces a Duplicate failure, which flips the whole revision to Failed
/// and prevents promotion -- so a single intermediate artifact could break
/// every compile on the workspace.
pub fn is_skipped(rel_path: &str) -> bool {
    rel_path.split('/').any(is_skipped_component)
}

/// One path component's skip decision: hidden (dot-prefixed) at any depth, or
/// one of the named build/vendor directories.
fn is_skipped_component(component: &str) -> bool {
    const SKIPPED_NAMES: &[&str] = &["target", "node_modules", "dist", "build"];
    component.starts_with('.') || SKIPPED_NAMES.contains(&component)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn write(root: &Path, rel: &str, content: &str) {
        let path = root.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, content).unwrap();
    }

    #[test]
    fn discover_finds_all_kinds_and_skips_noise() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();

        write(root, "config.yml", "models: []");
        write(root, "agents/foo.agentic.yml", "name: foo");
        write(root, "semantics/views/orders.view.yml", "name: orders");
        write(root, "semantics/topics/sales.topic.yml", "name: sales");
        write(root, "apps/dash.app.yml", "name: dash");
        write(root, "workflows/x.procedure.yml", "name: x");
        write(root, "workflows/cron.automation.yml", "name: cron");
        write(root, "pipelines/data.airway.yml", "name: data");
        write(
            root,
            "simulations/confounded.simulation.yml",
            "name: confounded",
        );
        write(root, "queries/top.sql", "SELECT 1");

        // noise we should skip
        write(root, ".git/config", "[core]");
        write(root, "target/debug/something", "binary");
        write(root, "node_modules/foo/index.js", "");
        write(root, ".semantics/.manifest.json", "{}");
        write(root, "tests/sales.agent.test.yml", "name: t");
        write(root, "modeling/proj/models/foo.sql", "SELECT 1");
        // noise nested below the root — must be skipped at any depth, not
        // just as a root prefix (the mismatch this skip predicate exists to
        // avoid: `crates/core/src/config/storage.rs` calls the same function).
        write(root, "build/nested.simulation.yml", "name: nested");
        write(root, "sub/target/deep.simulation.yml", "name: deep");
        // The any-depth rule reaches EVERY kind, not just Simulation, and that
        // is the half of the change with the wider blast radius: under the old
        // root-prefix rule these two compiled. A view or an agent stranded in a
        // build dir is the "duplicate view name" failure this skip exists to
        // prevent, so the kinds that can actually collide are pinned here too.
        write(root, "sub/build/stray.view.yml", "name: orders");
        write(root, "foo/.hidden/stray.agentic.yml", "name: foo");

        let found = discover(root).unwrap();
        let by_kind: Vec<_> = found.iter().map(|f| (f.rel_path.clone(), f.kind)).collect();

        // config.yml present
        assert!(
            by_kind
                .iter()
                .any(|(p, k)| p == "config.yml" && matches!(k, FileKind::Config))
        );

        // each kind present exactly once (no test/modeling noise)
        assert_eq!(
            by_kind
                .iter()
                .filter(|(_, k)| matches!(k, FileKind::AgenticAgent))
                .count(),
            1,
            "exactly one .agentic.yml (test file should be filtered)"
        );
        assert_eq!(
            by_kind
                .iter()
                .filter(|(_, k)| matches!(k, FileKind::VerifiedQuery))
                .count(),
            1,
            "exactly one .sql outside modeling/"
        );
        assert_eq!(
            by_kind
                .iter()
                .filter(|(_, k)| matches!(k, FileKind::Simulation))
                .count(),
            1,
            "exactly one .simulation.yml"
        );

        let paths: Vec<_> = by_kind.iter().map(|(p, _)| p.clone()).collect();

        // The skip rule is any-depth and kind-agnostic, so assert it that way
        // rather than one kind at a time: nothing discovered may sit under a
        // skipped component at any position. The per-kind counts above would
        // pass a regression that only leaked a kind they do not count.
        for path in &paths {
            assert!(
                !is_skipped(path.rsplit_once('/').map(|(dir, _)| dir).unwrap_or("")),
                "{path} was discovered under a skipped directory"
            );
        }
        // …and the converse, so the loop above cannot pass by discovering
        // nothing: a dot-prefixed FILE is NOT a skipped path. `is_skipped`
        // prunes directories; `push_glob` deliberately leaves file names to
        // the `.test.` filter, which is what keeps both arms agreeing.
        assert!(
            !is_skipped(""),
            "a root-level file has no parent components and must never be skipped"
        );

        let mut sorted = paths.clone();
        sorted.sort();
        assert_eq!(paths, sorted, "discover output must be sorted by rel_path");
    }

    /// A writer that keeps every rendered log line so a test can assert on it.
    #[derive(Clone, Default)]
    struct SharedBuf(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

    impl std::io::Write for SharedBuf {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for SharedBuf {
        type Writer = SharedBuf;
        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    /// Run `f` with a DEBUG-level subscriber installed on this thread and
    /// return what it logged. `with_default` is thread-local, so this is safe
    /// under nextest's parallelism.
    fn capture_logs<T>(f: impl FnOnce() -> T) -> (T, String) {
        let buf = SharedBuf::default();
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::DEBUG)
            .with_writer(buf.clone())
            .with_ansi(false)
            .finish();
        let out = tracing::subscriber::with_default(subscriber, f);
        let logs = String::from_utf8(buf.0.lock().unwrap().clone()).unwrap();
        (out, logs)
    }

    /// `.test.` names a test FILE (`sales.agentic.test.yml` — the convention
    /// `ConfigStorage::load_test_config` infers a target from). Matched
    /// against the whole relative path it also swallows every real entity
    /// under a DIRECTORY whose name happens to contain `.test.`, which no
    /// sibling arm does: the arm immediately above it goes out of its way to
    /// check only `dir_part` so file names are left alone.
    #[test]
    fn a_directory_named_with_dot_test_does_not_drop_the_files_under_it() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();

        write(
            root,
            "semantics/v1.test.cases/orders.view.yml",
            "name: orders",
        );
        write(root, "queries/v1.test.cases/top.sql", "SELECT 1");
        write(root, "worlds/q3.test.grid/w.simulation.yml", "name: w");

        let found = discover(root).unwrap();
        let paths: Vec<_> = found.iter().map(|f| f.rel_path.as_str()).collect();

        for expected in [
            "semantics/v1.test.cases/orders.view.yml",
            "queries/v1.test.cases/top.sql",
            "worlds/q3.test.grid/w.simulation.yml",
        ] {
            assert!(
                paths.contains(&expected),
                "{expected} was dropped because a DIRECTORY component contains \
                 `.test.`; got {paths:?}"
            );
        }
    }

    /// The regression risk of narrowing the `.test.` arm: the drops it is
    /// actually for must survive. A file NAME carrying `.test.` is a test
    /// fixture mirroring an entity extension, and every directory in the
    /// documented skip set (`product-context.md` -> "Semantic file
    /// discovery") must still prune at ANY depth — a stray `.view.yml` copy
    /// under one of them is the "duplicate view name" failure that flips the
    /// whole revision to Failed.
    #[test]
    fn test_file_names_and_the_documented_skip_dirs_still_drop() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();

        write(root, "semantics/orders.view.yml", "name: orders");

        // File names that mirror an entity extension but are test fixtures.
        write(root, "semantics/orders.test.view.yml", "name: orders");
        write(root, "worlds/w.test.simulation.yml", "name: w");

        // The documented any-depth directory skips, one stray copy each.
        for skipped in [
            ".worktrees/wt/orders.view.yml",
            ".git/orders.view.yml",
            ".oxy_state/orders.view.yml",
            ".semantics/orders.view.yml",
            "node_modules/pkg/orders.view.yml",
            "sub/node_modules/pkg/orders.view.yml",
            "target/debug/orders.view.yml",
            "sub/target/orders.view.yml",
            "dist/orders.view.yml",
            "sub/build/orders.view.yml",
            "foo/.hidden/orders.view.yml",
        ] {
            write(root, skipped, "name: orders");
        }

        let found = discover(root).unwrap();
        let paths: Vec<_> = found.iter().map(|f| f.rel_path.as_str()).collect();

        assert_eq!(
            paths,
            vec!["semantics/orders.view.yml"],
            "exactly one view survives: every stray copy is pruned and both \
             `.test.`-named fixtures are dropped"
        );
    }

    /// A dropped file must leave a trace. Discovery is the only thing between
    /// a file on disk and a row in Postgres, so a silent drop reads to the
    /// user as "my view disappeared" with nothing to grep for. DEBUG rather
    /// than WARN because the common case is a `node_modules/` with thousands
    /// of matches — a warn per file would be unreadable.
    #[test]
    fn a_dropped_file_is_reported_at_debug_rather_than_vanishing() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();

        write(root, "sub/build/stray.view.yml", "name: orders");
        write(root, "semantics/orders.test.view.yml", "name: orders");

        let (found, logs) = capture_logs(|| discover(root).unwrap());
        assert!(found.is_empty(), "both files are dropped: {found:?}");

        assert!(
            logs.contains("sub/build/stray.view.yml"),
            "a file pruned by a skipped directory must be greppable; got:\n{logs}"
        );
        assert!(
            logs.contains("semantics/orders.test.view.yml"),
            "a file pruned as a test fixture must be greppable; got:\n{logs}"
        );
    }

    #[test]
    fn discover_returns_workspace_not_found_when_dir_missing() {
        let result = discover(Path::new("/nonexistent/path/oxy-test"));
        assert!(matches!(result, Err(CompileError::WorkspaceNotFound(_))));
    }
}
