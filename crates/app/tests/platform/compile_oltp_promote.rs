//! DB-backed tests for apply-then-promote
//! ([`oxy_app::server::compile_oltp::settle_deferred_promotion`]).
//!
//! The property worth a real database is the negative one: **a revision whose
//! DDL fails must not become the one the workspace serves.** That is the whole
//! reason promotion was moved out of the compile transaction, and it is not
//! observable without a tenant Postgres that can actually reject a statement.
//!
//! Shares [`super::oltp_provisioner`]'s fixture — control plane on a per-test
//! database, tenant plane on the same cluster through `LocalProvider`, so
//! `CREATE DATABASE` / `CREATE ROLE` / the platform DDL all really run.
//!
//! Run with:
//! `cargo nextest run -p oxy-app --test platform -E 'test(compile_oltp_promote)'`

use chrono::Utc;
use entity::{revisions, schema_migration_definitions, workspaces};
use oxy_app::server::compile_oltp::{Settled, settle_deferred_promotion};
use oxy_compile::{CompileOutcome, Promotion, RevisionStatus};
use sea_orm::{ActiveModelTrait, ActiveValue, DatabaseConnection, EntityTrait};
use uuid::Uuid;

use super::oltp_provisioner::{Fx, seed_org, tenant_superuser_conn, with_fx};

/// A workspace belonging to `org_id`, serving nothing yet.
async fn seed_workspace(db: &DatabaseConnection, org_id: Uuid) -> Uuid {
    let now = Utc::now().fixed_offset();
    let ws_id = Uuid::new_v4();
    workspaces::ActiveModel {
        id: ActiveValue::Set(ws_id),
        name: ActiveValue::Set("promote-ws".into()),
        git_namespace_id: ActiveValue::Set(None),
        git_remote_url: ActiveValue::Set(None),
        created_at: ActiveValue::Set(now),
        updated_at: ActiveValue::Set(now),
        path: ActiveValue::Set(None),
        last_opened_at: ActiveValue::Set(None),
        created_by: ActiveValue::Set(None),
        org_id: ActiveValue::Set(Some(org_id)),
        status: ActiveValue::Set(entity::workspaces::WorkspaceStatus::Ready),
        error: ActiveValue::Set(None),
        monthly_vlm_budget_micros: ActiveValue::Set(None),
        current_revision_id: ActiveValue::Set(None),
        default_branch: ActiveValue::Set(None),
        repo_subdir: ActiveValue::Set(None),
    }
    .insert(db)
    .await
    .expect("seed workspace");
    ws_id
}

/// A `ready` / `main` revision carrying `ddl` as its single
/// `schemas/0001_*.sql` file. `None` means a revision with no DDL at all.
async fn seed_revision(db: &DatabaseConnection, ws_id: Uuid, ddl: Option<&str>) -> Uuid {
    let now = Utc::now().fixed_offset();
    let rev_id = Uuid::new_v4();
    revisions::ActiveModel {
        revision_id: ActiveValue::Set(rev_id),
        workspace_id: ActiveValue::Set(ws_id),
        git_sha: ActiveValue::Set(format!("sha-{}", rev_id.simple())),
        branch: ActiveValue::Set(Some("main".into())),
        schema_version: ActiveValue::Set(1),
        status: ActiveValue::Set("ready".into()),
        kind: ActiveValue::Set("main".into()),
        owner_user_id: ActiveValue::Set(None),
        compiler_version: ActiveValue::Set("test".into()),
        started_at: ActiveValue::Set(now),
        finished_at: ActiveValue::Set(Some(now)),
        file_count_seen: ActiveValue::Set(1),
        file_count_compiled: ActiveValue::Set(1),
        file_count_failed: ActiveValue::Set(0),
        error_summary: ActiveValue::Set(None),
    }
    .insert(db)
    .await
    .expect("seed revision");

    if let Some(sql) = ddl {
        schema_migration_definitions::ActiveModel {
            revision_id: ActiveValue::Set(rev_id),
            file_path: ActiveValue::Set("schemas/0001_store_ops.sql".into()),
            content_sha256: ActiveValue::Set(format!("{:x}", md5_ish(sql))),
            content: ActiveValue::Set(sql.to_string()),
        }
        .insert(db)
        .await
        .expect("seed schema migration");
    }
    rev_id
}

/// Any stable per-content value will do — the migrator only compares this
/// against what it previously recorded, never recomputes it.
fn md5_ish(s: &str) -> u64 {
    s.bytes().fold(1469598103934665603u64, |h, b| {
        (h ^ b as u64).wrapping_mul(1099511628211)
    })
}

/// A `CompileOutcome` shaped exactly as `oxy-compile` returns one for a
/// revision it deliberately left unpromoted.
fn deferred_outcome(revision_id: Uuid, count: u32) -> CompileOutcome {
    let now = Utc::now();
    CompileOutcome {
        revision_id,
        status: RevisionStatus::Ready,
        git_sha: "sha".into(),
        branch: Some("main".into()),
        started_at: now,
        finished_at: now,
        file_count_seen: 1,
        file_count_compiled: 1,
        file_count_failed: 0,
        failures: Vec::new(),
        promotion: Promotion::Deferred {
            schema_migration_count: count,
        },
    }
}

async fn current_revision(db: &DatabaseConnection, ws_id: Uuid) -> Option<Uuid> {
    workspaces::Entity::find_by_id(ws_id)
        .one(db)
        .await
        .expect("load workspace")
        .expect("workspace exists")
        .current_revision_id
}

#[tokio::test]
async fn ddl_lands_in_the_tenant_database_and_then_the_revision_goes_live() {
    with_fx(|fx: std::sync::Arc<Fx>| async move {
        fx.provisioner
            .provision(fx.org_id)
            .await
            .expect("provision tenant");

        let ws = seed_workspace(&fx.db, fx.org_id).await;
        let rev = seed_revision(
            &fx.db,
            ws,
            Some("CREATE TABLE store_ops_submissions (id uuid PRIMARY KEY);"),
        )
        .await;

        let settled = settle_deferred_promotion(&fx.db, ws, &deferred_outcome(rev, 1)).await;

        match &settled {
            Settled::Applied {
                applied,
                promotion,
                already_applied,
            } => {
                assert_eq!(*applied, 1, "one migration should have run");
                assert_eq!(*already_applied, 0);
                assert_eq!(*promotion, Promotion::Promoted);
            }
            other => panic!("expected Applied, got {other:?}"),
        }

        assert_eq!(
            current_revision(&fx.db, ws).await,
            Some(rev),
            "the workspace must serve the revision whose tables now exist"
        );

        // The table is really there — not merely a ledger row claiming it.
        let client = tenant_superuser_conn(fx.org_id).await;
        let n: i64 = client
            .query_one(
                "SELECT count(*) FROM information_schema.tables \
                 WHERE table_name = 'store_ops_submissions'",
                &[],
            )
            .await
            .expect("query information_schema")
            .get(0);
        assert_eq!(n, 1, "the DDL should have created the table");
    })
    .await;
}

#[tokio::test]
async fn a_second_settle_is_idempotent_and_reports_nothing_new_applied() {
    with_fx(|fx: std::sync::Arc<Fx>| async move {
        fx.provisioner
            .provision(fx.org_id)
            .await
            .expect("provision tenant");

        let ws = seed_workspace(&fx.db, fx.org_id).await;
        let rev = seed_revision(
            &fx.db,
            ws,
            Some("CREATE TABLE idempotent_check (id uuid PRIMARY KEY);"),
        )
        .await;

        let first = settle_deferred_promotion(&fx.db, ws, &deferred_outcome(rev, 1)).await;
        assert!(matches!(first, Settled::Applied { applied: 1, .. }));

        // The idempotency short-circuit re-runs this for an unchanged SHA, so
        // it happens on every repeat compile — it has to be a ledger read,
        // not a second `CREATE TABLE` that fails.
        let second = settle_deferred_promotion(&fx.db, ws, &deferred_outcome(rev, 1)).await;
        match &second {
            Settled::Applied {
                applied,
                already_applied,
                ..
            } => {
                assert_eq!(*applied, 0, "nothing new to apply");
                assert_eq!(*already_applied, 1, "the ledger should recognise the file");
            }
            other => panic!("expected an idempotent Applied, got {other:?}"),
        }
    })
    .await;
}

#[tokio::test]
async fn a_failed_migration_leaves_the_previous_revision_serving() {
    with_fx(|fx: std::sync::Arc<Fx>| async move {
        fx.provisioner
            .provision(fx.org_id)
            .await
            .expect("provision tenant");

        let ws = seed_workspace(&fx.db, fx.org_id).await;

        // A good revision, live.
        let good = seed_revision(&fx.db, ws, None).await;
        let mut m: workspaces::ActiveModel = workspaces::Entity::find_by_id(ws)
            .one(&fx.db)
            .await
            .unwrap()
            .unwrap()
            .into();
        m.current_revision_id = ActiveValue::Set(Some(good));
        m.update(&fx.db).await.expect("promote the good revision");

        // A newer revision whose DDL Postgres will refuse.
        let bad = seed_revision(&fx.db, ws, Some("CREAT TABLE oops (id uuid);")).await;
        let settled = settle_deferred_promotion(&fx.db, ws, &deferred_outcome(bad, 1)).await;

        assert!(
            matches!(settled, Settled::Failed { .. }),
            "expected Failed, got {settled:?}"
        );
        assert!(
            !settled.compile_succeeded(),
            "a broken migration has to fail the task, or CI reports a deploy that never went live"
        );
        assert_eq!(
            current_revision(&fx.db, ws).await,
            Some(good),
            "THE property: the workspace must still serve the revision whose tables exist"
        );
    })
    .await;
}

#[tokio::test]
async fn an_org_with_no_oltp_database_still_promotes() {
    with_fx(|fx: std::sync::Arc<Fx>| async move {
        // Deliberately NOT provisioned. A workspace can carry `schemas/*.sql`
        // long before anyone buys it a database, and blocking promotion would
        // wedge every other thing the revision compiles — agents, views, apps
        // — over a feature nobody on this workspace can reach yet.
        let org = seed_org(&fx.db).await;
        let ws = seed_workspace(&fx.db, org).await;
        let rev = seed_revision(&fx.db, ws, Some("CREATE TABLE later (id uuid);")).await;

        let settled = settle_deferred_promotion(&fx.db, ws, &deferred_outcome(rev, 1)).await;

        assert!(
            matches!(settled, Settled::NoTenant { .. }),
            "expected NoTenant, got {settled:?}"
        );
        assert_eq!(
            current_revision(&fx.db, ws).await,
            Some(rev),
            "the rest of the revision must still go live"
        );
    })
    .await;
}

#[tokio::test]
async fn a_compile_that_did_not_defer_is_left_alone() {
    with_fx(|fx: std::sync::Arc<Fx>| async move {
        let org = seed_org(&fx.db).await;
        let ws = seed_workspace(&fx.db, org).await;
        let rev = seed_revision(&fx.db, ws, None).await;

        let mut outcome = deferred_outcome(rev, 0);
        outcome.promotion = Promotion::Promoted;

        let settled = settle_deferred_promotion(&fx.db, ws, &outcome).await;

        assert!(
            matches!(settled, Settled::NotDeferred(Promotion::Promoted)),
            "expected NotDeferred, got {settled:?}"
        );
        // The compiler already promoted inside its transaction; this path must
        // not re-promote, and above all must not reach for a tenant database
        // that a workspace without DDL has no reason to have.
        assert_eq!(current_revision(&fx.db, ws).await, None);
    })
    .await;
}
