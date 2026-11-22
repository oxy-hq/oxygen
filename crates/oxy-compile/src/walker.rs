//! File discovery + extension routing.
//!
//! Walks the workspace once and classifies each interesting file by
//! kind so the compiler can dispatch to the right per-kind parser.
//! Skips every dot-prefixed directory (`.git`, `.worktrees`, `.claude` — whose
//! `worktrees/` holds agent worktrees, each a full copy of the project —
//! `.semantics`, `.oxy_state`, ...) and the build/vendor directories
//! (`target`, `node_modules`, `dist`, `build`) at any depth — see
//! [`is_skipped`]. A skipped directory is pruned, not walked and filtered.
//!
//! "Once" is literal. Discovery used to run one recursive `glob` per kind, ten
//! in all, and each descended into every skipped directory before the skip
//! rule threw the matches away: ten passes over `node_modules` and `.git` to
//! find nothing. Measured on a working copy of 24,701 entries (an app's
//! `node_modules`, `.git`, one agent worktree) that was 323 ms per discovery
//! against 3.6 ms for this walk, with the same files found;
//! `internal-docs/compile-performance.md` has the numbers for smaller trees.

use crate::errors::CompileError;
use std::ffi::OsStr;
use std::fs;
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
    /// `*.agent.yml` — a classic agent. Retired: nothing runs it and it compiles
    /// to no row. Discovered only so the compile can say so
    /// ([`crate::classic_agents`]). Test mirrors (`x.agent.test.yml`,
    /// `x.test.agent.yml`) are not this kind.
    RetiredAgent,
    /// `*.test.yml` — an eval suite (`oxyc evals run`). The one kind whose name
    /// carries `.test.`: the fixture rule below drops a test file MIRRORING an
    /// entity extension, and this is the test file itself. A file that does
    /// not parse as a mapping compiles to no row rather than failing the
    /// revision, as the working-copy lister skips it.
    EvalTest,
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

    // Every kind claimed by its file name, in one pass over the tree.
    walk_named_kinds(workspace_root, &mut out)?;
    push_context_documents(workspace_root, &mut out)?;

    out.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    Ok(out)
}

/// The kinds a file below the root is claimed by from its name alone. No
/// suffix here ends another, so the order never decides between two.
///
/// A new `*.foo.yml` kind is one line here. `.sql` is not in the table because
/// its kind depends on where the file sits ([`classify`]).
const NAMED_KINDS: &[(&str, FileKind)] = &[
    (".agentic.yml", FileKind::AgenticAgent),
    (".agent.yml", FileKind::RetiredAgent),
    (".view.yml", FileKind::SemanticView),
    (".topic.yml", FileKind::SemanticTopic),
    (
        ".procedure.yml",
        FileKind::Automation(AutomationKind::Procedure),
    ),
    (
        ".automation.yml",
        FileKind::Automation(AutomationKind::Automation),
    ),
    (".airway.yml", FileKind::AirwayPipeline),
    (".simulation.yml", FileKind::Simulation),
    (".test.yml", FileKind::EvalTest),
];

/// The kind `file_name`, at workspace-relative `rel_path`, would compile as;
/// `None` for a file no kind claims. `.sql` under `schemas/` (at the root) is
/// DDL; anywhere else it is a verified query, including under `modeling/`,
/// which [`Walk::file`] then drops by name so the drop is reported.
fn classify(rel_path: &str, file_name: &str) -> Option<FileKind> {
    if let Some((_, kind)) = NAMED_KINDS
        .iter()
        .find(|(suffix, _)| file_name.ends_with(suffix))
    {
        return Some(*kind);
    }
    if !file_name.ends_with(".sql") {
        return None;
    }
    if rel_path.starts_with("schemas/") {
        Some(FileKind::SchemaMigration)
    } else {
        Some(FileKind::VerifiedQuery)
    }
}

/// Walk the tree below `root` once, pruning skipped directories, and push
/// every file a [`NAMED_KINDS`] suffix or `.sql` claims.
///
/// An unreadable root is an error: "could not look" must not come back as
/// "found nothing" (`internal-docs/compile-boundary.md` § "Absent is not
/// empty"). An unreadable directory below it is skipped with a warning.
fn walk_named_kinds(root: &Path, out: &mut Vec<DiscoveredFile>) -> Result<(), CompileError> {
    let walk_err = |e: std::io::Error| CompileError::Walk(format!("{}: {e}", root.display()));
    let entries = fs::read_dir(root).map_err(walk_err)?;
    let mut walk = Walk {
        out,
        ancestors: vec![fs::canonicalize(root).map_err(walk_err)?],
    };
    walk.directory(root, "", entries);
    Ok(())
}

/// One pass over the tree.
///
/// Directory symlinks are followed, as the per-kind globs this replaced
/// followed them, so a workspace that links `semantics/` in from elsewhere
/// keeps compiling. `ancestors` holds the real path of every directory being
/// walked, root first: a link back to one of them is a loop, which the globs
/// followed until the path grew too long, and which this does not follow.
struct Walk<'a> {
    out: &'a mut Vec<DiscoveredFile>,
    ancestors: Vec<PathBuf>,
}

impl Walk<'_> {
    fn directory(&mut self, dir: &Path, rel_dir: &str, entries: fs::ReadDir) {
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    warn!(rel_dir, %error, "discovery: a directory entry is not readable");
                    continue;
                }
            };
            let name = entry.file_name();
            let path = dir.join(&name);
            let rel_path = match rel_dir {
                "" => name.to_string_lossy().into_owned(),
                parent => format!("{parent}/{}", name.to_string_lossy()),
            };
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            let is_link = file_type.is_symlink();
            let is_dir = if is_link {
                fs::metadata(&path).is_ok_and(|meta| meta.is_dir())
            } else {
                file_type.is_dir()
            };
            if is_dir {
                self.descend(&path, &rel_path, &name, is_link);
            } else if let Some(file_name) = name.to_str() {
                // A name that is not UTF-8 matched no glob before, and matches
                // no suffix now.
                self.file(path, rel_path, file_name);
            }
        }
    }

    fn descend(&mut self, path: &Path, rel_path: &str, name: &OsStr, is_link: bool) {
        if is_skipped_component(&name.to_string_lossy()) {
            // One line per directory, not per file: this prunes before it
            // descends, which is what keeps it out of `node_modules`.
            dropped(
                rel_path,
                "a skipped directory; nothing below it is discovered",
            );
            return;
        }
        let real = if is_link {
            match fs::canonicalize(path) {
                Ok(real) if self.ancestors.contains(&real) => {
                    warn!(
                        rel_path,
                        "discovery: a directory symlink leads back to a directory it is inside; \
                         not followed"
                    );
                    return;
                }
                Ok(real) => real,
                Err(error) => {
                    warn!(rel_path, %error, "discovery: a directory symlink cannot be resolved");
                    return;
                }
            }
        } else {
            // Not a link, so its real path is its real parent's plus its name.
            match self.ancestors.last() {
                Some(parent) => parent.join(name),
                None => path.to_path_buf(),
            }
        };
        match fs::read_dir(path) {
            Ok(entries) => {
                self.ancestors.push(real);
                self.directory(path, rel_path, entries);
                self.ancestors.pop();
            }
            Err(error) => warn!(rel_path, %error, "discovery: a directory is not readable"),
        }
    }

    fn file(&mut self, abs_path: PathBuf, rel_path: String, file_name: &str) {
        let Some(kind) = classify(&rel_path, file_name) else {
            return;
        };
        // Follows a symlink, as the globs did: a link to a file is that file,
        // and a dangling link is nothing.
        if !abs_path.is_file() {
            return;
        }
        // Don't include test files that mirror an extension —
        // `foo.agent.test.yml` is NOT a real `.agentic.yml`.
        // Same idea as `crates/core/src/config/storage.rs:421`.
        //
        // Matched against the FILE NAME only, deliberately: a real
        // `orders.view.yml` under `semantics/v1.test.cases/` is a view. The
        // conventional `x.agent.test.yml` and the pinned-divergence
        // `drafts/scratch.test.agentic.yml` both carry `.test.` in the file
        // name, so both drop, and the working-copy lister
        // (`storage.rs::list_entity_files`) applies the same rule.
        if file_name.contains(".test.") && kind != FileKind::EvalTest {
            dropped(&rel_path, "test file mirroring an entity extension");
            return;
        }
        // VerifiedQuery only applies outside `modeling/` — the dbt-shaped
        // SQL files there are handled by Airform's separate compile pipeline.
        if kind == FileKind::VerifiedQuery && rel_path.starts_with("modeling/") {
            dropped(
                &rel_path,
                "dbt-shaped SQL under modeling/, owned by Airform",
            );
            return;
        }
        self.out.push(DiscoveredFile {
            rel_path,
            abs_path,
            kind,
        });
    }
}

/// Markdown reached by an agent's `context:`. Must follow the agent walk: which
/// documents exist as a kind is decided by the agents already in `out`.
///
/// Not a [`NAMED_KINDS`] suffix, because no name claims a document — the
/// patterns are the agents' own. The two skip rules apply all the same, in
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

/// Report a path discovery will not compile: a file an entity kind would
/// have claimed, or a skipped directory it did not walk.
///
/// Discovery is the only thing between a file on disk and a
/// `*_definitions` row, so a drop with no record reads to the user as "my
/// view disappeared" with nothing to grep for — and the skip set matches
/// whole path components at any depth, so a workspace that happens to keep
/// models under `dist/` (distribution) or `build/` loses them with no error
/// anywhere. A skipped directory is reported once, by its own path, because
/// it is pruned rather than walked: naming each file under it would mean
/// walking `node_modules`, which is the cost pruning exists to avoid. DEBUG
/// rather than WARN on purpose: every `.git` and `node_modules` is one of
/// these, and a warn each would bury every other line in the compile log.
fn dropped(rel_path: &str, reason: &str) {
    debug!(rel_path, reason, "discovery dropped a path");
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
        // A Data App is no longer a compiled kind: it must be left alone.
        write(root, "apps/dash.app.yml", "name: dash");
        // Nor is `.workflow.yml` (product-context.md: "no longer recognized");
        // only `oxyc migrate-automations` reads one, to rename it.
        write(root, "workflows/ancient.workflow.yml", "name: ancient");
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
        assert!(
            !paths.iter().any(|p| p.ends_with(".app.yml")),
            "a .app.yml was discovered: {paths:?}"
        );
        assert!(
            !paths.iter().any(|p| p.ends_with(".workflow.yml")),
            "a .workflow.yml was discovered: {paths:?}"
        );
        assert_eq!(
            by_kind
                .iter()
                .filter(|(_, k)| matches!(k, FileKind::Automation(_)))
                .count(),
            2,
            "exactly the .procedure.yml and the .automation.yml; retired spellings stay retired"
        );

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
        // prunes directories; the walk deliberately leaves file names to
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
        // …while the test file itself, an eval suite, is discovered as one.
        write(root, "tests/sales.agentic.test.yml", "cases: []");

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
            vec!["semantics/orders.view.yml", "tests/sales.agentic.test.yml"],
            "exactly one view survives: every stray copy is pruned and both \
             `.test.`-named fixtures are dropped; the eval suite is its own kind"
        );
    }

    /// Claude Code keeps agent worktrees under `.claude/worktrees/<name>/`, each
    /// a full checkout of the project. If discovery walked into them, every
    /// view, topic and agent would compile twice and each copy would be a
    /// Duplicate failure, so one agent session left behind fails every view
    /// in the workspace. A dossier that carried such worktrees reported 99
    /// duplicate errors and none of them were real. This test covers the
    /// shape it had: real files at the root and stale copies nested under
    /// the worktree.
    #[test]
    fn claude_code_agent_worktrees_are_not_discovered() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();

        write(root, "config.yml", "databases: []");
        write(root, "semantics/views/orders.view.yml", "name: orders");
        write(root, "semantics/topics/sales.topic.yml", "name: sales");
        for wt in [".claude/worktrees/x", ".claude/worktrees/fix-y"] {
            write(root, &format!("{wt}/config.yml"), "databases: []");
            write(
                root,
                &format!("{wt}/semantics/views/orders.view.yml"),
                "name: orders",
            );
            write(
                root,
                &format!("{wt}/semantics/topics/sales.topic.yml"),
                "name: sales",
            );
        }

        let found = discover(root).unwrap();
        let paths: Vec<_> = found.iter().map(|f| f.rel_path.as_str()).collect();

        assert_eq!(
            paths.iter().filter(|p| p.ends_with(".view.yml")).count(),
            1,
            "only the root view is discovered, so compile sees no duplicate; got {paths:?}"
        );
        assert!(
            paths.iter().all(|p| !p.starts_with(".claude/")),
            "nothing under .claude/worktrees/ is discovered; got {paths:?}"
        );
    }

    /// A dropped file must leave a trace. Discovery is the only thing between
    /// a file on disk and a row in Postgres, so a silent drop reads to the
    /// user as "my view disappeared" with nothing to grep for. DEBUG rather
    /// than WARN because every `.git` and `node_modules` is a drop. A skipped
    /// directory is pruned, not walked, so its trace names the directory: the
    /// files below it are never listed, which is the point of pruning.
    #[test]
    fn a_dropped_file_is_reported_at_debug_rather_than_vanishing() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();

        write(root, "sub/build/stray.view.yml", "name: orders");
        write(root, "semantics/orders.test.view.yml", "name: orders");

        let (found, logs) = capture_logs(|| discover(root).unwrap());
        assert!(found.is_empty(), "both files are dropped: {found:?}");

        assert!(
            logs.contains("sub/build"),
            "a skipped directory must be greppable by its path; got:\n{logs}"
        );
        assert!(
            logs.contains("semantics/orders.test.view.yml"),
            "a file pruned as a test fixture must be greppable; got:\n{logs}"
        );
    }

    /// `.sql` is DDL only under the ROOT `schemas/`, the way the
    /// `schemas/**/*.sql` glob anchored it; a `schemas/` deeper down is an
    /// ordinary directory of verified queries. And a kind is claimed by its
    /// name wherever it sits, root-level and dot-prefixed file names included.
    #[test]
    fn sql_is_ddl_only_under_the_root_schemas_directory() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();

        write(root, "schemas/001_init.sql", "CREATE TABLE t (id int)");
        write(root, "schemas/v2/002_more.sql", "ALTER TABLE t ADD c int");
        write(root, "reports/schemas/top.sql", "SELECT 1");
        write(root, "top.sql", "SELECT 1");
        write(root, ".draft.simulation.yml", "name: draft");

        let found = discover(root).unwrap();
        let by_path: Vec<_> = found
            .iter()
            .map(|f| (f.rel_path.as_str(), f.kind))
            .collect();

        assert_eq!(
            by_path,
            vec![
                (".draft.simulation.yml", FileKind::Simulation),
                ("reports/schemas/top.sql", FileKind::VerifiedQuery),
                ("schemas/001_init.sql", FileKind::SchemaMigration),
                ("schemas/v2/002_more.sql", FileKind::SchemaMigration),
                ("top.sql", FileKind::VerifiedQuery),
            ]
        );
    }

    /// The globs this walk replaced followed directory symlinks, so a
    /// workspace that links a directory in still compiles it, under the
    /// link's path. A link back to a directory already being walked is a
    /// loop: the globs followed it until the path grew too long; the walk
    /// stops at it, and still finds every real file once.
    #[cfg(unix)]
    #[test]
    fn directory_symlinks_are_followed_but_a_loop_is_not() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        let shared = TempDir::new().unwrap();

        write(shared.path(), "orders.view.yml", "name: orders");
        write(root, "semantics/topics/sales.topic.yml", "name: sales");
        std::os::unix::fs::symlink(shared.path(), root.join("semantics/views")).unwrap();
        // A loop: `semantics/topics/up` -> `semantics`.
        std::os::unix::fs::symlink(root.join("semantics"), root.join("semantics/topics/up"))
            .unwrap();
        // A dangling link named like a view is nothing.
        std::os::unix::fs::symlink(root.join("gone"), root.join("ghost.view.yml")).unwrap();

        let found = discover(root).unwrap();
        let paths: Vec<_> = found.iter().map(|f| f.rel_path.as_str()).collect();

        assert_eq!(
            paths,
            vec![
                "semantics/topics/sales.topic.yml",
                "semantics/views/orders.view.yml",
            ],
            "the linked view is found under the link's path, the loop is cut, and the \
             dangling link is skipped"
        );
    }

    #[test]
    fn discover_returns_workspace_not_found_when_dir_missing() {
        let result = discover(Path::new("/nonexistent/path/oxy-test"));
        assert!(matches!(result, Err(CompileError::WorkspaceNotFound(_))));
    }
}
