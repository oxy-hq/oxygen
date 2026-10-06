//! Pins where the two workspace-file enumerations disagree.
//!
//! `oxy_compile::walker::discover` (compile boundary) and `ConfigManager<WorkingCopy>`'s
//! `list_*` methods (working copy) walk the same tree for the same kinds by
//! different rules. Folding them behind one `Kind` abstraction would silently
//! pick a winner, and the loser's behaviour would change without a diff.
//!
//! Every assertion below is a divergence that exists today, or a former one
//! now pinned as resolved. Changing one is allowed; changing one without
//! noticing is not.
//!
//! The `.test.` file-name rule was a divergence and is now shared: the walker
//! and `storage.rs`'s `list_entity_files` both drop it. The winner was picked
//! deliberately and one-way — the working copy dropping a fixture costs an
//! author nothing, while the walker keeping one would compile a test file into
//! a `*_definitions` row and serve it to the fleet as a real entity.

use std::fs;
use std::path::Path;

use oxy::config::ConfigBuilder;
use oxy_compile::walker::{FileKind, discover};
use tempfile::TempDir;

fn write(root: &Path, rel: &str, body: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
}

/// A workspace holding one file of every shape the two sides disagree about.
fn fixture() -> TempDir {
    let dir = TempDir::new().unwrap();
    let root = dir.path();

    write(root, "config.yml", "models: []\ndatabases: []\n");

    write(root, "orders.app.yml", "name: orders\n");
    write(root, "generated/scratch.app.yml", "name: scratch\n");

    write(root, "analyst.agentic.yml", "name: analyst\n");
    write(root, "analyst.agent.test.yml", "name: analyst-test\n");

    write(root, "nightly.automation.yml", "name: nightly\n");
    write(root, "legacy.procedure.yml", "name: legacy\n");
    write(root, "ancient.workflow.yml", "name: ancient\n");

    write(root, "ingest.airway.yml", "name: ingest\n");
    write(root, "semantics/orders.view.yml", "name: orders\n");
    write(root, "semantics/sales.topic.yml", "name: sales\n");

    write(root, "revenue.sql", "SELECT 1\n");
    write(root, "modeling/proj/models/dbt.sql", "SELECT 2\n");

    write(root, ".monitor.yml", "monitors: []\n");
    write(root, "reconcile.yml", "checks: []\n");
    write(root, ".world-model.yml", "entities: []\n");

    dir
}

fn walked(root: &Path, kind: FileKind) -> Vec<String> {
    let mut found: Vec<String> = discover(root)
        .expect("walker discovers the fixture")
        .into_iter()
        .filter(|f| f.kind == kind)
        .map(|f| f.rel_path)
        .collect();
    found.sort();
    found
}

fn walked_any_automation(root: &Path) -> Vec<String> {
    let mut found: Vec<String> = discover(root)
        .expect("walker discovers the fixture")
        .into_iter()
        .filter(|f| matches!(f.kind, FileKind::Automation(_)))
        .map(|f| f.rel_path)
        .collect();
    found.sort();
    found
}

async fn manager(root: &Path) -> oxy::config::ConfigManager<oxy::config::WorkingCopy> {
    ConfigBuilder::new()
        .with_workspace_path(root)
        .unwrap()
        .build_with_working_copy(oxy::config::Origin::Disk, oxy::config::OnMissing::Empty)
        .await
        .unwrap()
}

/// The combination a single type parameter cannot express: the ide owns a disk
/// AND serves a config that came off the compile boundary. Capability and origin
/// are separate axes, and this is the case that proves it.
#[tokio::test]
async fn the_ide_holds_a_working_copy_and_a_compiled_origin_at_once() {
    use oxy::adapters::workspace::builder::WorkspaceBuilder;
    use oxy::config::Origin;

    let dir = fixture();
    let root = dir.path();
    let workspace_id = uuid::Uuid::new_v4();
    let revision_id = uuid::Uuid::new_v4();

    // `Config` deliberately does not derive `Default` — an empty config is a
    // fallback, never something you construct on purpose.
    let compiled: oxy::config::model::Config =
        serde_yaml::from_str("models: []\ndatabases: []\n").unwrap();

    let wm = WorkspaceBuilder::new(workspace_id)
        .with_working_copy_and_provided_config(root, compiled, revision_id)
        .unwrap()
        .build()
        .await
        .unwrap();

    assert_eq!(
        wm.config_manager.origin(),
        Origin::Compiled {
            workspace_id,
            revision_id
        },
        "the config came from Postgres"
    );
    assert_eq!(wm.config_manager.revision_id(), Some(revision_id));
    assert_eq!(
        wm.config_manager.workspace_path(),
        root,
        "and the disk methods are still there, because the ide has a disk"
    );

    let detached = wm.config_manager.clone().without_working_copy();
    assert!(
        detached.working_copy().is_none(),
        "dropping the capability leaves nothing to fall through to"
    );
    assert_eq!(
        detached.origin(),
        Origin::Compiled {
            workspace_id,
            revision_id
        },
        "but where the bytes came from is unchanged — the axes are independent"
    );
}

/// Closed deliberately. The working copy used to filter `generated/` (the
/// `save-from-run` output directory) while the walker compiled it, so the same
/// workspace listed different apps depending on whether it had been compiled.
/// The compiled arm won: generated apps are listed on both sides.
#[tokio::test]
async fn apps_agree_including_the_generated_directory() {
    let dir = fixture();
    let root = dir.path();

    let mut from_disk: Vec<String> = manager(root)
        .await
        .list_apps(false)
        .await
        .unwrap()
        .into_iter()
        .map(|entry| entry.file_path)
        .collect();
    from_disk.sort();

    assert_eq!(
        from_disk,
        vec!["generated/scratch.app.yml", "orders.app.yml"],
        "the working copy no longer hides apps under `generated/`"
    );
    assert_eq!(
        walked(root, FileKind::App),
        from_disk,
        "and the walker enumerates exactly the same set"
    );
}

/// `name` is the field the two arms disagreed on for longer than the directory
/// filter did: the walker reads the YAML `name:`, the working copy used to
/// derive one from the filename. Both now go through the same rule.
#[tokio::test]
async fn app_names_come_from_the_yaml_on_both_sides() {
    let dir = fixture();
    let root = dir.path();

    let entries = manager(root).await.list_apps(false).await.unwrap();
    let orders = entries
        .iter()
        .find(|e| e.file_path == "orders.app.yml")
        .expect("the fixture writes orders.app.yml with `name: orders`");

    assert_eq!(orders.name, "orders", "`name:` from the YAML, not the stem");
    assert!(!orders.published, "the fixture declares no `published:`");
}

/// The `.test.` rule now costs nothing at any spelling, because both arms
/// apply it.
///
/// The convention is `x.agent.test.yml` — it ends `.test.yml`, not
/// `.agentic.yml`, so no extension match ever claimed it on either side.
/// `x.test.agentic.yml` is the shape that used to diverge: it DOES end
/// `.agentic.yml`, so the working copy listed it while the walker dropped it,
/// and the same workspace showed a different agent set once compiled.
///
/// **The winner picked is the walker**, and the reason is one-way: the working
/// copy dropping a fixture costs an author nothing, while the walker keeping
/// one would compile a test file into `agent_definitions` and serve it to the
/// fleet as a real agent. `storage.rs`'s `list_entity_files` is the
/// working-copy half of that one rule; `list_by_sub_extension` underneath it
/// is untouched, because `list_tests` needs the names this drops.
#[tokio::test]
async fn agents_agree_on_every_test_mirror_spelling() {
    let dir = fixture();
    let root = dir.path();

    let listed = |root: &Path| {
        let root = root.to_path_buf();
        async move {
            let mut v: Vec<String> = manager(&root)
                .await
                .list_analytics_agents()
                .await
                .unwrap()
                .into_iter()
                .map(|entry| entry.file_path)
                .collect();
            v.sort();
            v
        }
    };

    // The fixture already holds `analyst.agent.test.yml`, the conventional name.
    assert_eq!(
        listed(root).await,
        walked(root, FileKind::AgenticAgent),
        "at the names the codebase actually uses, the two enumerations agree"
    );

    // The shape that used to diverge, now agreed on.
    write(root, "drafts/scratch.test.agentic.yml", "name: scratch\n");
    assert_eq!(
        listed(root).await,
        vec!["analyst.agentic.yml"],
        "the working copy drops it too, because the rule matches `.test.` \
         anywhere in the file NAME — `scratch.test.agentic.yml` carries it \
         regardless of which directory holds it"
    );
    assert_eq!(
        walked(root, FileKind::AgenticAgent),
        vec!["analyst.agentic.yml"],
        "and the walker still drops it"
    );
    assert_eq!(
        listed(root).await,
        walked(root, FileKind::AgenticAgent),
        "at every spelling, the two enumerations agree"
    );

    // A fixtures DIRECTORY is not a file name: the real agent under it stays,
    // on both arms. This is the half of the rule that must NOT widen.
    write(
        root,
        "drafts/v1.test.cases/real.agentic.yml",
        "name: real\n",
    );
    assert_eq!(
        listed(root).await,
        vec![
            "analyst.agentic.yml",
            "drafts/v1.test.cases/real.agentic.yml"
        ],
        "a directory component carrying `.test.` does not drop the files under it"
    );
    assert_eq!(
        listed(root).await,
        walked(root, FileKind::AgenticAgent),
        "and both arms agree about that too"
    );
}

#[tokio::test]
async fn automations_diverge_on_the_legacy_workflow_extension() {
    let dir = fixture();
    let root = dir.path();

    assert_eq!(
        {
            let mut v: Vec<String> = manager(root)
                .await
                .list_automations()
                .await
                .unwrap()
                .into_iter()
                .map(|a| a.file_path)
                .collect();
            v.sort();
            v
        },
        vec![
            "ancient.workflow.yml",
            "legacy.procedure.yml",
            "nightly.automation.yml",
        ],
        "the working copy still unions three extensions"
    );
    assert_eq!(
        walked_any_automation(root),
        vec!["legacy.procedure.yml", "nightly.automation.yml"],
        "the walker dropped `.workflow.yml` as a file kind, so one of these \
         cannot be compiled and only ever resolves from disk"
    );
}

/// What survives of the old divergence: `list_tests` itself.
///
/// Both enumerations now drop a FILE NAME containing `.test.`, for every
/// entity kind, unconditionally — but only the file name; a directory
/// component carrying `.test.` is `is_skipped`'s question, not this rule's.
/// The remaining asymmetry is that tests are a working-copy-only concept: the
/// walker has no test `FileKind` at all, so `.test.yml` files reach
/// `list_tests` and never reach a compiled revision.
#[tokio::test]
async fn test_mirrors_drop_on_both_arms_but_list_tests_is_working_copy_only() {
    let dir = fixture();
    let root = dir.path();

    // `analyst.agent.test.yml` does not end in `.agentic.yml`, so no extension
    // match claims it as an agent on either side.
    assert_eq!(
        walked(root, FileKind::AgenticAgent),
        vec!["analyst.agentic.yml"]
    );
    assert!(
        !discover(root)
            .unwrap()
            .iter()
            .any(|f| f.rel_path.contains(".test.")),
        "the walker drops every path containing `.test.`"
    );
    assert!(
        !manager(root)
            .await
            .list_analytics_agents()
            .await
            .unwrap()
            .iter()
            .any(|e| e.file_path.contains(".test.")),
        "and so does the working copy, now that both share the rule"
    );

    assert!(
        manager(root)
            .await
            .list_tests()
            .await
            .unwrap()
            .iter()
            .all(|p| p.to_string_lossy().contains(".test.")),
        "tests are a working-copy-only concept — the walker has no test kind, so \
         `.test.yml` files exist on one side and not the other"
    );
}

#[tokio::test]
async fn verified_queries_and_root_singletons_exist_only_on_the_boundary() {
    let dir = fixture();
    let root = dir.path();

    assert_eq!(
        walked(root, FileKind::VerifiedQuery),
        vec!["revenue.sql"],
        "`modeling/` SQL is excluded — Airform owns it"
    );

    for kind in [
        FileKind::Config,
        FileKind::MonitorConfig,
        FileKind::ReconcileConfig,
        FileKind::WorldModelConfig,
    ] {
        assert_eq!(
            walked(root, kind).len(),
            1,
            "{kind:?} is a root singleton the walker knows about"
        );
    }

    // None of the five has a `list_*` counterpart on ConfigManager. A unified
    // `Kind` would have to invent one, or keep them off the shared surface.
}

#[tokio::test]
async fn pipelines_and_semantic_files_agree() {
    let dir = fixture();
    let root = dir.path();

    assert_eq!(
        manager(root)
            .await
            .list_pipelines()
            .await
            .unwrap()
            .into_iter()
            .map(|p| p.file_path)
            .collect::<Vec<_>>(),
        walked(root, FileKind::AirwayPipeline),
        "airway pipelines are the one kind both sides enumerate identically"
    );

    assert_eq!(
        walked(root, FileKind::SemanticView),
        vec!["semantics/orders.view.yml"]
    );
    assert_eq!(
        walked(root, FileKind::SemanticTopic),
        vec!["semantics/sales.topic.yml"]
    );
    // Semantic views/topics have no ConfigManager listing at all — they are read
    // through SemanticManager on one side and compiled_reader on the other.
}

/// Markdown context documents are enumerated by ONE walk
/// (`oxy_compile::context_documents::discover`), which the compile walker and
/// the working copy both call. So where every other kind here pins an
/// agreement between two implementations, this pins that there is still only
/// one — and that it carries both skip rules, which the walker's own
/// `push_glob` would otherwise have applied for it.
#[tokio::test]
async fn context_documents_agree_because_both_arms_share_one_walk() {
    let dir = fixture();
    let root = dir.path();
    write(
        root,
        "analyst.agentic.yml",
        "name: analyst\ncontext:\n  - ./**/*.md\n",
    );
    write(root, "docs/glossary.md", "# Glossary\n");
    write(
        root,
        "docs/v1.test.cases/real.md",
        "# a fixtures DIRECTORY\n",
    );
    write(root, "docs/draft.test.md", "# a fixture FILE\n");
    write(root, "sub/build/stray.md", "# build output\n");
    write(root, ".hidden/secret.md", "# hidden\n");
    write(root, "node_modules/pkg/README.md", "# vendored\n");

    let mut from_disk: Vec<String> = manager(root)
        .await
        .context_documents(&["./**/*.md".to_string()])
        .await
        .unwrap()
        .read()
        .expect("the working copy answers")
        .into_iter()
        .map(|document| document.file_path)
        .collect();
    from_disk.sort();

    assert_eq!(
        from_disk,
        vec!["docs/glossary.md", "docs/v1.test.cases/real.md"],
        "the working copy prunes skipped directories and drops the `.test.` \
         file name, and keeps a real file under a `.test.` directory"
    );
    assert_eq!(
        walked(root, FileKind::ContextDocument),
        from_disk,
        "and the walker compiles exactly the same set"
    );

    // The kind exists only by reference. Narrow the one agent's `context:` and
    // the README-shaped files stop being documents on both arms at once.
    write(
        root,
        "analyst.agentic.yml",
        "name: analyst\ncontext:\n  - ./docs/*.md\n",
    );
    assert_eq!(
        walked(root, FileKind::ContextDocument),
        vec!["docs/glossary.md"]
    );
}

/// The incident shape, stated directly: a workspace root that is not on this
/// disk must not read as a workspace that has nothing in it.
///
/// Every lister used to answer `[]` here, so "this replica holds no working
/// copy" and "the customer configured nothing" arrived at the handler as the
/// same value. Both shipped outages are that sentence.
#[tokio::test]
async fn an_absent_workspace_root_is_an_error_not_an_empty_workspace() {
    let dir = TempDir::new().unwrap();
    let root = dir.path().join("never-cloned-here");
    let manager = ConfigBuilder::new()
        .with_workspace_path(&root)
        .unwrap()
        .build_with_working_copy(oxy::config::Origin::Disk, oxy::config::OnMissing::Empty)
        .await
        .unwrap();

    for (kind, result) in [
        ("agents", manager.list_analytics_agents().await.is_err()),
        ("automations", manager.list_automations().await.is_err()),
        ("pipelines", manager.list_pipelines().await.is_err()),
        ("apps", manager.list_apps(false).await.is_err()),
        ("tests", manager.list_tests().await.is_err()),
        (
            "context documents",
            manager
                .context_documents(&["./docs/*.md".to_string()])
                .await
                .is_err(),
        ),
    ] {
        assert!(
            result,
            "listing {kind} on an absent root must fail, not return []"
        );
    }
}

/// The counter-assertion, so the guard above cannot be satisfied by refusing
/// every listing: a root that exists and holds nothing is still empty.
#[tokio::test]
async fn an_empty_workspace_root_still_lists_nothing() {
    let dir = TempDir::new().unwrap();
    let manager = manager(dir.path()).await;

    assert!(manager.list_analytics_agents().await.unwrap().is_empty());
    assert!(manager.list_apps(false).await.unwrap().is_empty());
    assert!(
        manager
            .context_documents(&["./docs/*.md".to_string()])
            .await
            .unwrap()
            .read()
            .expect("the working copy answers")
            .is_empty()
    );
}

/// Building a manager must not bring a workspace root into existence.
///
/// `WorkingCopy::new` resolved the fallback state dir eagerly, and that resolver created
/// it — `<root>/.oxy_state`, which creates `<root>`. So a node that had never
/// cloned a workspace manufactured an empty one the moment it built a manager
/// for it. That is why an absent root was undetectable downstream: by the time
/// anything looked, it existed and was empty.
#[tokio::test]
async fn building_a_manager_creates_no_workspace_on_disk() {
    let dir = TempDir::new().unwrap();
    let root = dir.path().join("never-cloned-here");

    let _manager = ConfigBuilder::new()
        .with_workspace_path(&root)
        .unwrap()
        .build_with_working_copy(oxy::config::Origin::Disk, oxy::config::OnMissing::Empty)
        .await
        .unwrap();

    assert!(
        !root.exists(),
        "constructing a manager created the workspace root at {}",
        root.display()
    );
}

/// The back door onto the same hole.
///
/// `WorkingCopy::new` was stopped from creating `<root>/.oxy_state`, but the sibling
/// accessor still resolved it through the *creating* variant — so any handler
/// calling `resolve_state_dir()` re-manufactured the workspace root the
/// constructor no longer did. Worse, that resolver calls `std::process::exit(1)`
/// when creation fails, so the fallout would not be a catchable error.
#[tokio::test]
async fn resolving_the_state_dir_creates_no_workspace_on_disk() {
    let dir = TempDir::new().unwrap();
    let root = dir.path().join("never-cloned-here");
    let manager = ConfigBuilder::new()
        .with_workspace_path(&root)
        .unwrap()
        .build_with_working_copy(oxy::config::Origin::Disk, oxy::config::OnMissing::Empty)
        .await
        .unwrap();

    let resolved = manager.resolve_state_dir().await;
    if std::env::var("OXY_STATE_DIR").is_err() {
        assert!(
            resolved.is_err(),
            "resolving the fallback state dir under an absent root must fail"
        );
    }
    // Holds either way, which is the invariant that matters: with
    // `OXY_STATE_DIR` set the state dir lives outside the workspace and the
    // root is never touched; without it, the guard refuses before creating one.
    assert!(
        !root.exists(),
        "resolving the state dir created the workspace root at {}",
        root.display()
    );
}

/// The counter-guard: a workspace that IS here still gets its state dir made.
#[tokio::test]
async fn resolving_the_state_dir_still_creates_it_for_a_real_workspace() {
    let dir = TempDir::new().unwrap();
    let manager = manager(dir.path()).await;

    let state_dir = manager.resolve_state_dir().await.unwrap();
    assert!(
        state_dir.is_dir(),
        "{} was not created",
        state_dir.display()
    );
}

/// `working_copy()` and `can_read_disk()` are not the same question, and the
/// difference is where the fleet bugs live.
///
/// The request middleware builds `with_working_copy_and_compiled_config`
/// whenever a compiled config exists — on a stateless replica too. So the
/// manager holds an `WorkingCopy` over a root that is not there, and `working_copy()`
/// answers `Some`. Any gate written on it passes on exactly the pod it was
/// meant to stop.
#[tokio::test]
async fn holding_an_fs_is_not_the_same_as_having_a_disk() {
    use oxy::workspace_fs_probe::{process_owns_workspace_files, set_process_owns_workspace_files};

    let dir = fixture();
    let manager = manager(dir.path()).await;
    let restore = process_owns_workspace_files();

    set_process_owns_workspace_files(true);
    assert!(manager.working_copy().is_some());
    assert!(
        manager.can_read_disk(),
        "a node that owns its files has one by both readings"
    );

    set_process_owns_workspace_files(false);
    assert!(
        manager.working_copy().is_some(),
        "the value still carries an WorkingCopy — this is the trap"
    );
    assert!(
        !manager.can_read_disk(),
        "but a pod that owns no files cannot reach a disk, whatever the value says"
    );

    set_process_owns_workspace_files(restore);
}
