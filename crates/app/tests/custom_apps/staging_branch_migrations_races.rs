//! Previews P4b, review fix round 1: the staging-branch migration step under
//! contention.
//!
//! - **A locked branch never holds the publish's pointers.** The step is a
//!   task the publish queues after the pointers move, with a lock timeout on
//!   its session: a table another session holds makes it give up and fail the
//!   task (never the publish), and the pointer had already moved while it
//!   waited.
//! - **A file applied across a reset is not recorded.** The record and the
//!   check that the branch is still the cut the apply connected to share one
//!   transaction; a reset in between leaves the file unrecorded, so staging
//!   re-runs it on the new copy rather than skipping it forever.

use entity::apps;
use oxy_app::server::api::custom_apps_functions::env_policy::OltpHome;
use oxy_app::server::api::custom_apps_functions::host::writer_connection;
use oxy_app::server::api::custom_apps_migrations::{
    DeclaredMigration, MigrationTarget, apply_to_resolved_branch,
};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use uuid::Uuid;

use crate::staging_branch_migrations::{
    BRANCH_WARNING, INIT, NOTE, files, ledger, production_then_branch, publish_files,
};
use crate::staging_functions_oltp::{OltpApp, run_then_cleanup};

/// The app's row: its id, and the build its staging pointer names.
async fn app_row(app: &OltpApp) -> apps::Model {
    apps::Entity::find()
        .filter(apps::Column::OrgId.eq(app.t.org_id))
        .filter(apps::Column::Slug.eq(app.slug.clone()))
        .one(&app.t.db)
        .await
        .expect("query app")
        .expect("the app")
}

/// A session on the branch holding `widgets` exclusively until dropped.
async fn lock_widgets_on(app: &OltpApp, home: &OltpHome) -> (tokio_postgres::Client, String) {
    let writer = oxy_oltp::schema::app_writer_name(&app.slug).expect("writer");
    let conn = writer_connection(&app.t.db, app.t.org_id, &writer, home)
        .await
        .expect("resolve the branch writer");
    let config: tokio_postgres::Config = conn.dsn.parse().expect("a DSN");
    let database = config.get_dbname().expect("a database").to_string();
    let client = oxy_oltp::connect::connect(&conn.dsn, "branch lock holder")
        .await
        .expect("connect to the branch");
    client
        .batch_execute("BEGIN; LOCK TABLE widgets IN ACCESS EXCLUSIVE MODE")
        .await
        .expect("hold widgets");
    (client, database)
}

/// Waits until a session in `database` is blocked on a lock — the branch
/// step's `ALTER TABLE` — and answers the staging pointer at that moment.
async fn pointer_once_blocked(app: &OltpApp, database: &str) -> Option<Uuid> {
    let admin = crate::common::admin_url().await;
    let client = oxy_oltp::connect::connect(&admin, "branch lock watcher")
        .await
        .expect("connect admin");
    for _ in 0..1200 {
        let waiting: i64 = client
            .query_one(
                "SELECT count(*) FROM pg_stat_activity \
                 WHERE datname = $1 AND wait_event_type = 'Lock'",
                &[&database],
            )
            .await
            .expect("read pg_stat_activity")
            .get(0);
        if waiting > 0 {
            return app_row(app).await.draft_build_id;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("the branch step never blocked on the held lock");
}

#[tokio::test]
async fn a_locked_branch_warns_and_never_holds_the_pointer_move() {
    let app = OltpApp::provision(&[]).await;
    run_then_cleanup(&app, async {
        let (home, target) = production_then_branch(&app).await;
        let before = app_row(&app).await.draft_build_id;
        let (holder, database) = lock_widgets_on(&app, &home).await;

        let (published, pointer_while_blocked) = tokio::join!(
            publish_files(&app, "m2", false, &[INIT, NOTE]),
            pointer_once_blocked(&app, &database),
        );
        drop(holder);

        assert!(pointer_while_blocked.is_some());
        assert_ne!(
            pointer_while_blocked, before,
            "the staging pointer moved before the branch step waited on its lock"
        );
        assert!(published.warnings.is_empty(), "{:?}", published.warnings);
        let warning = published
            .staging
            .iter()
            .find(|w| w.contains(BRANCH_WARNING))
            .unwrap_or_else(|| panic!("a failed task: {:?}", published.staging));
        assert!(warning.contains("lock timeout"), "{warning}");
        assert_eq!(ledger(&app, &target).await, files(&[INIT]));
    })
    .await;
}

#[tokio::test]
async fn a_file_applied_across_a_reset_is_not_recorded() {
    let app = OltpApp::provision(&[]).await;
    run_then_cleanup(&app, async {
        let (_, target) = production_then_branch(&app).await;
        let row = app_row(&app).await;
        let writer = oxy_oltp::schema::WriterRef::app(
            oxy_oltp::schema::app_writer_name(&app.slug).expect("writer"),
        )
        .expect("writer ref");
        // Resolved, then the branch is reset: the race the record's check is
        // for, with the reset landed inside it.
        let resolved = oxy_oltp::resolver::resolve_branch_writer_for_org(
            &app.t.db,
            app.t.org_id,
            oxy_oltp::OltpBranch::Staging,
            &writer,
        )
        .await
        .expect("resolve")
        .expect("the org's branch");
        app.store.reset_branch().await;

        let note = DeclaredMigration {
            filename: NOTE.0.to_string(),
            checksum: "note-checksum".to_string(),
            sql: NOTE.1.to_string(),
        };
        let build = row.published_build_id.expect("the promoted build");
        let err = apply_to_resolved_branch(&app.t.db, row.id, build, &[note], &resolved)
            .await
            .expect_err("a file applied across a reset is not recorded");

        assert!(err.to_string().contains("reset or re-cut"), "{err}");
        assert_eq!(
            ledger(&app, &target).await,
            files(&[INIT]),
            "the fresh copy's ledger is production's; the file is re-run next publish"
        );
        assert_eq!(
            ledger(&app, &MigrationTarget::Production).await,
            files(&[INIT])
        );
    })
    .await;
}
