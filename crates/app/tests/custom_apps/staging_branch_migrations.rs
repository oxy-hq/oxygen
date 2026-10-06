//! Previews P4b: a publish migrates the org's OLTP **staging branch** too —
//! under the branch's own ledger target, `branch:<provider id>` — and never
//! production's ledger or database; a file that fails on the branch fails its
//! queued task, never the publish. The branch's ledger starts, and restarts on
//! a reset, as a copy of production's, because its data is a copy of
//! production's.
//!
//! The branch step is a task the publish queues (`staging_task`);
//! [`publish_files`] runs it through the registered executor after the
//! publish, as the worker fleet would.
//!
//! Real databases throughout: the org's tenant on the test cluster and a
//! `LocalProvider` branch copied from it (`staging_functions_oltp::OltpApp`).

use std::collections::BTreeSet;

use agentic_connector::{DatabaseConnector as _, PostgresConnector};
use agentic_core::result::CellValue;
use entity::apps;
use oxy_app::server::api::custom_apps_functions::env_policy::OltpHome;
use oxy_app::server::api::custom_apps_functions::host::writer_connection;
use oxy_app::server::api::custom_apps_migrations::{MigrationTarget, read_ledger};
use oxy_app::server::api::custom_apps_publish::{OrgRef, PublishInput, PublishResult, publish};
use oxy_auth::user::LOCAL_GUEST_EMAIL;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde_json::json;

use crate::custom_apps_publish_function_artifacts::tar_gz;
use crate::staging_functions_oltp::{OltpApp, run_then_cleanup};
use crate::staging_migration_tasks::drain;

pub(crate) const INIT: (&str, &str) = (
    "0001_init.sql",
    "create table widgets (id int primary key);",
);
pub(crate) const NOTE: (&str, &str) =
    ("0002_note.sql", "alter table widgets add column note text;");
const BROKEN: (&str, &str) = ("0003_broken.sql", "create table broken (;");

/// What a failed branch apply's task records.
pub(crate) const BRANCH_WARNING: &str = "OLTP staging branch was not migrated";

/// A bundle declaring `files` as its OLTP migrations, and no functions.
fn bundle(slug: &str, files: &[(&str, &str)]) -> Vec<u8> {
    let manifest = json!({
        "schemaVersion": 2,
        "slug": slug,
        "functions": {},
        "migrations": { "dir": "migrations" },
    })
    .to_string();
    let paths: Vec<String> = files
        .iter()
        .map(|(n, _)| format!("migrations/{n}"))
        .collect();
    let mut entries: Vec<(&str, &[u8])> = vec![
        (
            "index.html",
            b"<!doctype html><html><head></head><body></body></html>",
        ),
        ("oxy-app.json", manifest.as_bytes()),
    ];
    entries.extend(
        paths
            .iter()
            .zip(files)
            .map(|(p, (_, sql))| (p.as_str(), sql.as_bytes())),
    );
    tar_gz(&entries)
}

/// A publish, and then what the worker fleet does after it: the staging
/// migration tasks it queued, run through the registered executor.
pub(crate) struct Published {
    /// The publish response's warnings.
    pub(crate) warnings: Vec<String>,
    /// Why each staging migration task failed — recorded on its run, never on
    /// the publish.
    pub(crate) staging: Vec<String>,
}

pub(crate) async fn publish_files(
    app: &OltpApp,
    build_id: &str,
    promote: bool,
    files: &[(&str, &str)],
) -> Published {
    let published = publish_only(app, build_id, promote, files).await;
    let staging = drain(&app.t.db, published.app_id).await;
    Published {
        warnings: published.warnings,
        staging,
    }
}

/// Neither the publish nor its staging tasks had anything to say.
fn assert_clean(published: &Published, what: &str) {
    assert!(
        published.warnings.is_empty() && published.staging.is_empty(),
        "{what}: {:?} / {:?}",
        published.warnings,
        published.staging
    );
}

/// The publish alone; its staging migrations stay queued.
async fn publish_only(
    app: &OltpApp,
    build_id: &str,
    promote: bool,
    files: &[(&str, &str)],
) -> PublishResult {
    publish(PublishInput {
        org_ref: Some(OrgRef::Id(app.t.org_id)),
        app_slug: app.slug.clone(),
        project_id: app.workspace,
        branch: None,
        build_id: build_id.to_string(),
        name: None,
        promote,
        tarball: bundle(&app.slug, files),
        manifest: None,
        source_repo: None,
        commit_sha: None,
        published_by: Some(app.t.guest_id),
        published_by_email: Some(LOCAL_GUEST_EMAIL.to_string()),
        publisher: Some(oxy_app::server::authz::Caller::without_credential(
            app.t.guest_id,
            LOCAL_GUEST_EMAIL,
        )),
        machine_app_id: None,
        published_via: None,
        semantic_revision_id: None,
    })
    .await
    .unwrap_or_else(|e| panic!("publish {build_id}: {e}"))
}

/// The files recorded for the app under `target`.
pub(crate) async fn ledger(app: &OltpApp, target: &MigrationTarget) -> BTreeSet<String> {
    let app_id = apps::Entity::find()
        .filter(apps::Column::OrgId.eq(app.t.org_id))
        .filter(apps::Column::Slug.eq(app.slug.clone()))
        .one(&app.t.db)
        .await
        .expect("query app")
        .expect("the app")
        .id;
    read_ledger(&app.t.db, app_id, "oltp", target)
        .await
        .expect("read ledger")
        .into_keys()
        .collect()
}

pub(crate) fn files(names: &[(&str, &str)]) -> BTreeSet<String> {
    names.iter().map(|(n, _)| n.to_string()).collect()
}

/// Whether `widgets.note` exists in the database `home` reaches.
async fn has_note_column(app: &OltpApp, home: &OltpHome) -> bool {
    let writer = oxy_oltp::schema::app_writer_name(&app.slug).expect("writer");
    let conn = writer_connection(&app.t.db, app.t.org_id, &writer, home)
        .await
        .expect("resolve");
    let connector = PostgresConnector::from_dsn(&conn.dsn, conn.verify_tls).expect("connector");
    let got = connector
        .execute_query(
            "select count(*)::bigint as n from information_schema.columns \
             where table_name = 'widgets' and column_name = 'note'",
            1,
        )
        .await
        .expect("probe the column");
    match got.result.rows.first().map(|row| &row.0[0]) {
        Some(CellValue::Number(n)) => *n > 0.0,
        Some(CellValue::Text(s)) => s != "0",
        other => panic!("unexpected count {other:?}"),
    }
}

/// Production at 0001, then the branch cut: the branch starts with
/// production's ledger. Returns the branch's home and ledger target.
pub(crate) async fn production_then_branch(app: &OltpApp) -> (OltpHome, MigrationTarget) {
    let first = publish_files(app, "m1", true, &[INIT]).await;
    assert_clean(&first, "no branch yet");
    let row = app.store.provision_branch().await;
    let target = MigrationTarget::Branch(row.provider_branch_id.clone());
    assert_eq!(
        ledger(app, &target).await,
        files(&[INIT]),
        "a branch cut from production starts with production's ledger"
    );
    (OltpHome::StagingBranch(row.provider_branch_id), target)
}

#[tokio::test]
async fn a_publish_migrates_the_staging_branch_under_its_own_target_and_never_production() {
    let app = OltpApp::provision(&[]).await;
    run_then_cleanup(&app, async {
        let (branch, target) = production_then_branch(&app).await;
        let production = MigrationTarget::Production;

        // Staging only: production's step does not run; the branch's does,
        // and does not re-run 0001, which the copy already holds. The publish
        // answers first: its branch step is queued, not yet applied.
        let staged = publish_only(&app, "m2", false, &[INIT, NOTE]).await;
        assert!(staged.warnings.is_empty(), "{:?}", staged.warnings);
        assert_eq!(
            ledger(&app, &target).await,
            files(&[INIT]),
            "not applied yet"
        );
        let failures = drain(&app.t.db, staged.app_id).await;
        assert!(failures.is_empty(), "{failures:?}");
        assert_eq!(ledger(&app, &target).await, files(&[INIT, NOTE]));
        assert_eq!(ledger(&app, &production).await, files(&[INIT]));
        assert!(has_note_column(&app, &branch).await);
        assert!(!has_note_column(&app, &OltpHome::Production).await);

        // A file that fails on the branch fails its task; the publish went
        // ahead without a word about it.
        let broken = publish_files(&app, "m3", false, &[INIT, NOTE, BROKEN]).await;
        assert!(broken.warnings.is_empty(), "{:?}", broken.warnings);
        assert!(
            broken.staging.iter().any(|w| w.contains(BRANCH_WARNING)),
            "{:?}",
            broken.staging
        );
        assert_eq!(ledger(&app, &target).await, files(&[INIT, NOTE]));
        assert_eq!(ledger(&app, &production).await, files(&[INIT]));

        // Promoted: production applies 0002 itself — its ledger never said
        // the branch's apply was its own.
        let promoted = publish_files(&app, "m4", true, &[INIT, NOTE]).await;
        assert_clean(&promoted, "promoted");
        assert_eq!(ledger(&app, &production).await, files(&[INIT, NOTE]));
        assert!(has_note_column(&app, &OltpHome::Production).await);
    })
    .await;
}

#[tokio::test]
async fn a_branch_reset_sets_its_migration_ledger_to_a_copy_of_productions() {
    let app = OltpApp::provision(&[]).await;
    run_then_cleanup(&app, async {
        let (branch, target) = production_then_branch(&app).await;
        publish_files(&app, "m2", false, &[INIT, NOTE]).await;
        assert_eq!(ledger(&app, &target).await, files(&[INIT, NOTE]));

        let reset = app.store.reset_branch().await;
        assert_eq!(
            MigrationTarget::Branch(reset.provider_branch_id),
            target,
            "a local reset re-copies under the same id"
        );
        assert_eq!(
            ledger(&app, &target).await,
            files(&[INIT]),
            "staging's own file is forgotten with its data; production's is kept"
        );
        assert!(!has_note_column(&app, &branch).await, "a fresh copy");

        // Staging re-migrates cleanly: 0001 is not run on a copy that has it.
        let again = publish_files(&app, "m3", false, &[INIT, NOTE]).await;
        assert_clean(&again, "re-migrated");
        assert_eq!(ledger(&app, &target).await, files(&[INIT, NOTE]));
        assert!(has_note_column(&app, &branch).await);
        assert_eq!(
            ledger(&app, &MigrationTarget::Production).await,
            files(&[INIT])
        );
    })
    .await;
}
