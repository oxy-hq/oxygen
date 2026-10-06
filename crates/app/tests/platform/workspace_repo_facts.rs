//! `workspaces.default_branch` and `workspaces.repo_subdir`: who may write
//! them, and who reads them.
//!
//! Both are facts about a checkout, and until they are columns only the node
//! holding that checkout can answer either — which is one of the things
//! keeping compile on it (`internal-docs/factory-retirement.md`, phase 0).
//! The rules pinned here:
//!
//! * the node with the checkout records both, once, and corrects them when
//!   they drift — local git stays the authority there;
//! * a process with no checkout reads the stored branch and writes nothing;
//! * nothing is recorded that is not a fact about the repository: not the git
//!   client's `"main"` guess, not anything for a workspace with no remote, not
//!   anything before the clone has landed.
//!
//! The repositories are built on disk with no network: `origin/HEAD` is a
//! symbolic ref, and writing it directly is all `git symbolic-ref` reads.
//!
//! Run with:
//! `cargo nextest run -p oxy-app --test platform -E 'test(workspace_repo_facts)'`

use std::path::{Path, PathBuf};
use std::process::Command;

use entity::workspaces::{self, WorkspaceStatus};
use migration::{Migrator, MigratorTrait, SchemaManager};
use oxy::workspace_fs_probe::set_process_owns_workspace_files;
use oxy_app::server::default_branch::resolve_default_branch;
use oxy_app::server::workspace_repo_facts::record_from_checkout;
use sea_orm::{
    ActiveModelTrait, ActiveValue, ConnectionTrait, DatabaseConnection, EntityTrait, Statement,
};
use tempfile::TempDir;
use uuid::Uuid;

use crate::common::{Schema, fresh_db};

const REMOTE: &str = "https://github.com/acme/analytics.git";
const MIGRATION: &str = "m20261005_000001_workspace_default_branch_and_subdir";

/// A repository on disk with the workspace root in `data/oxy`.
struct Checkout {
    _dir: TempDir,
    workspace: PathBuf,
}

fn git(cwd: &Path, args: &[&str]) {
    let out = Command::new("git")
        .current_dir(cwd)
        // The developer's own config (signing, hooks, templates) is not part
        // of what is being tested and can fail a commit here.
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .args(["-c", "user.name=test", "-c", "user.email=test@example.com"])
        .args(args)
        .output()
        .expect("run git");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Drop a developer's `GIT_DEFAULT_BRANCH`, which would otherwise answer for
/// every repository in this file.
fn no_branch_override() {
    // SAFETY: nextest runs each test in its own process (`fresh_db` asserts
    // it), and every test calls this before anything reads the environment.
    unsafe { std::env::remove_var("GIT_DEFAULT_BRANCH") };
}

/// `origin_head` is the branch `origin/HEAD` names; `None` leaves it unset,
/// which is the case where git can only guess.
fn checkout(origin_head: Option<&str>) -> Checkout {
    no_branch_override();
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    git(root, &["init", "-q", "-b", "trunk"]);
    git(root, &["commit", "-q", "--allow-empty", "-m", "init"]);
    if let Some(branch) = origin_head {
        let tracking = format!("refs/remotes/origin/{branch}");
        git(root, &["update-ref", &tracking, "HEAD"]);
        git(
            root,
            &["symbolic-ref", "refs/remotes/origin/HEAD", &tracking],
        );
    }
    let workspace = root.join("data").join("oxy");
    std::fs::create_dir_all(&workspace).expect("workspace dir");
    Checkout {
        _dir: dir,
        workspace,
    }
}

struct Seed<'a> {
    path: &'a Path,
    remote: Option<&'a str>,
    status: WorkspaceStatus,
    default_branch: Option<&'a str>,
    repo_subdir: Option<&'a str>,
}

impl<'a> Seed<'a> {
    /// A remote-backed, Ready workspace with neither fact recorded yet.
    fn at(path: &'a Path) -> Self {
        Self {
            path,
            remote: Some(REMOTE),
            status: WorkspaceStatus::Ready,
            default_branch: None,
            repo_subdir: None,
        }
    }

    async fn insert(self, db: &DatabaseConnection) -> workspaces::Model {
        workspaces::ActiveModel {
            id: ActiveValue::Set(Uuid::new_v4()),
            name: ActiveValue::Set("repo-facts".into()),
            path: ActiveValue::Set(Some(self.path.to_string_lossy().into_owned())),
            git_remote_url: ActiveValue::Set(self.remote.map(str::to_string)),
            status: ActiveValue::Set(self.status),
            default_branch: ActiveValue::Set(self.default_branch.map(str::to_string)),
            repo_subdir: ActiveValue::Set(self.repo_subdir.map(str::to_string)),
            ..Default::default()
        }
        .insert(db)
        .await
        .expect("seed workspace")
    }
}

/// `(default_branch, repo_subdir)` as the row holds them now.
async fn stored(db: &DatabaseConnection, id: Uuid) -> (Option<String>, Option<String>) {
    let row = workspaces::Entity::find_by_id(id)
        .one(db)
        .await
        .expect("load workspace")
        .expect("workspace exists");
    (row.default_branch, row.repo_subdir)
}

fn both(branch: &str, subdir: &str) -> (Option<String>, Option<String>) {
    (Some(branch.to_string()), Some(subdir.to_string()))
}

/// The process as a serve or worker pod: it owns no working copy. Restored on
/// drop; nextest gives each test its own process, so no other test sees it.
pub(crate) struct AsDisklessReplica;

impl AsDisklessReplica {
    pub(crate) fn enter() -> Self {
        set_process_owns_workspace_files(false);
        oxy::workspace_fs_probe::reset_leaks();
        AsDisklessReplica
    }
}

impl Drop for AsDisklessReplica {
    fn drop(&mut self) {
        set_process_owns_workspace_files(true);
        oxy::workspace_fs_probe::reset_leaks();
    }
}

#[tokio::test]
async fn the_backfill_writes_once() {
    let (db, _url) = fresh_db(Schema::Central).await;
    let repo = checkout(Some("trunk"));
    let before = Seed::at(&repo.workspace).insert(&db).await;

    assert!(
        record_from_checkout(&db, &before, &repo.workspace, "trunk").await,
        "a row with neither fact recorded is written"
    );
    assert_eq!(stored(&db, before.id).await, both("trunk", "data/oxy"));

    let after = workspaces::Entity::find_by_id(before.id)
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    assert!(
        !record_from_checkout(&db, &after, &repo.workspace, "trunk").await,
        "a row that already agrees with the checkout is not written again"
    );
    assert_eq!(stored(&db, before.id).await, both("trunk", "data/oxy"));
}

#[tokio::test]
async fn resolving_on_the_node_with_the_checkout_backfills_the_row() {
    let (db, _url) = fresh_db(Schema::Central).await;
    let repo = checkout(Some("trunk"));
    let row = Seed::at(&repo.workspace).insert(&db).await;

    assert_eq!(
        resolve_default_branch(&db, row.id).await.as_deref(),
        Some("trunk")
    );
    assert_eq!(stored(&db, row.id).await, both("trunk", "data/oxy"));
}

/// Local git is the authority where there is a checkout: a stored value that
/// disagrees is neither returned nor left standing.
#[tokio::test]
async fn the_checkout_wins_over_a_stale_row_and_corrects_it() {
    let (db, _url) = fresh_db(Schema::Central).await;
    let repo = checkout(Some("trunk"));
    let row = Seed {
        default_branch: Some("renamed-away"),
        repo_subdir: Some("old/place"),
        ..Seed::at(&repo.workspace)
    }
    .insert(&db)
    .await;

    assert_eq!(
        resolve_default_branch(&db, row.id).await.as_deref(),
        Some("trunk")
    );
    assert_eq!(stored(&db, row.id).await, both("trunk", "data/oxy"));
}

#[tokio::test]
async fn a_workspace_with_no_remote_keeps_nulls() {
    let (db, _url) = fresh_db(Schema::Central).await;
    // The checkout could answer both questions; the workspace has no remote,
    // so neither answer names a repository anything could fetch.
    let repo = checkout(Some("trunk"));
    let row = Seed {
        remote: None,
        ..Seed::at(&repo.workspace)
    }
    .insert(&db)
    .await;

    assert_eq!(
        resolve_default_branch(&db, row.id).await.as_deref(),
        Some("trunk"),
        "the answer itself is unchanged"
    );
    assert_eq!(stored(&db, row.id).await, (None, None));
}

/// With `origin/HEAD` unset the git client answers `"main"` as a guess. The
/// subdirectory is still a fact and is recorded; the guess is not.
#[tokio::test]
async fn a_guessed_branch_is_never_recorded() {
    let (db, _url) = fresh_db(Schema::Central).await;
    let repo = checkout(None);
    let row = Seed::at(&repo.workspace).insert(&db).await;

    assert_eq!(
        resolve_default_branch(&db, row.id).await.as_deref(),
        Some("main")
    );
    assert_eq!(
        stored(&db, row.id).await,
        (None, Some("data/oxy".to_string()))
    );
}

/// While a clone is in flight the directory exists with no `.git` of its own,
/// so git discovery would answer for whatever repository encloses it.
#[tokio::test]
async fn nothing_is_recorded_before_the_clone_has_landed() {
    let (db, _url) = fresh_db(Schema::Central).await;
    let repo = checkout(Some("trunk"));
    let row = Seed {
        status: WorkspaceStatus::Cloning,
        ..Seed::at(&repo.workspace)
    }
    .insert(&db)
    .await;

    resolve_default_branch(&db, row.id).await;
    assert_eq!(stored(&db, row.id).await, (None, None));
}

#[tokio::test]
async fn a_process_with_no_checkout_reads_the_stored_branch() {
    let (db, _url) = fresh_db(Schema::Central).await;
    no_branch_override();
    let _replica = AsDisklessReplica::enter();
    let absent = PathBuf::from(format!("/nonexistent/workspaces/{}", Uuid::new_v4()));

    let recorded = Seed {
        default_branch: Some("trunk"),
        ..Seed::at(&absent)
    }
    .insert(&db)
    .await;
    assert_eq!(
        resolve_default_branch(&db, recorded.id).await.as_deref(),
        Some("trunk")
    );

    // Nothing recorded: the answer is what it was before the column existed.
    let unrecorded = Seed::at(&absent).insert(&db).await;
    assert_eq!(
        resolve_default_branch(&db, unrecorded.id).await.as_deref(),
        Some("main")
    );
}

/// The override is process-wide and the node with the checkout obeys it, so a
/// pod without one must too, or the two answer differently for one workspace.
#[tokio::test]
async fn the_branch_override_wins_over_the_stored_branch() {
    let (db, _url) = fresh_db(Schema::Central).await;
    // SAFETY: process-per-test, and nothing has read the environment yet.
    unsafe { std::env::set_var("GIT_DEFAULT_BRANCH", "release") };
    let _replica = AsDisklessReplica::enter();
    let absent = PathBuf::from(format!("/nonexistent/workspaces/{}", Uuid::new_v4()));
    let row = Seed {
        default_branch: Some("trunk"),
        ..Seed::at(&absent)
    }
    .insert(&db)
    .await;

    assert_eq!(
        resolve_default_branch(&db, row.id).await.as_deref(),
        Some("release")
    );
}

/// The checkout is on this machine's disk, so only the process's own standing
/// stops the write. A pod that does not own files must not record facts about
/// a directory it happens to be able to see.
#[tokio::test]
async fn a_process_with_no_checkout_never_writes() {
    let (db, _url) = fresh_db(Schema::Central).await;
    let repo = checkout(Some("trunk"));
    let row = Seed::at(&repo.workspace).insert(&db).await;
    let _replica = AsDisklessReplica::enter();

    resolve_default_branch(&db, row.id).await;
    assert!(!record_from_checkout(&db, &row, &repo.workspace, "trunk").await);
    assert_eq!(stored(&db, row.id).await, (None, None));
}

/// `(column, is_nullable, has_default)` for the two columns, by name.
async fn columns(db: &DatabaseConnection) -> Vec<(String, bool, bool)> {
    let rows = db
        .query_all_raw(Statement::from_string(
            db.get_database_backend(),
            "SELECT column_name, is_nullable = 'YES', column_default IS NOT NULL \
             FROM information_schema.columns \
             WHERE table_name = 'workspaces' \
               AND column_name IN ('default_branch', 'repo_subdir') \
             ORDER BY column_name",
        ))
        .await
        .expect("read columns");
    rows.iter()
        .map(|r| {
            (
                r.try_get_by_index(0).unwrap(),
                r.try_get_by_index(1).unwrap(),
                r.try_get_by_index(2).unwrap(),
            )
        })
        .collect()
}

/// Up and down are both re-runnable, and what `up` leaves is something the
/// previous deploy can live with: nullable, no default, so a binary that
/// names its columns keeps inserting.
#[tokio::test]
async fn the_migration_adds_both_columns_and_reverts_cleanly() {
    let (db, _url) = fresh_db(Schema::Central).await;
    let migration = Migrator::migrations()
        .into_iter()
        .find(|m| m.name() == MIGRATION)
        .expect("the migration is registered in the chain");
    let manager = SchemaManager::new(&db);
    let added = vec![
        ("default_branch".to_string(), true, false),
        ("repo_subdir".to_string(), true, false),
    ];
    assert_eq!(columns(&db).await, added, "the chain applied it");

    migration.down(&manager).await.expect("down");
    assert!(columns(&db).await.is_empty());
    migration
        .down(&manager)
        .await
        .expect("down again is a no-op");

    migration.up(&manager).await.expect("up");
    migration.up(&manager).await.expect("up again is a no-op");
    assert_eq!(columns(&db).await, added);
}
