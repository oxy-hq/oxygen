//! The lease routes against Postgres: the listing, both deletes, the zero-rows
//! triage, workspace isolation, and the `WorkspaceAdmin` guard.
//!
//! Ported from `oxy airway leases | release-lease` (deleted 2026-11-21 for
//! `oxyc airway`), whose lease handling had no test of its own: the delete was
//! run against a database by hand. The two bugs its comments record are pinned
//! here — a `--workspace-id` honoured for the listing but discarded for the
//! DELETE (it could only clear the nil workspace's lease), and a confirmed
//! release that could free whichever run the next cron tick started.

use axum::Router;
use axum::body::Body;
use axum::extract::Request;
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::Response;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{Duration, Utc};
use entity::workspace_members::WorkspaceRole;
use oxy_api_airway::leases::{self, LeaseView, ReleaseMode, ReleaseOutcome};
use oxy_app::surface::EffectiveWorkspaceRole;
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement};
use tower::ServiceExt;
use uuid::Uuid;

use crate::common::{Schema, test_db_with};

async fn hold(db: &DatabaseConnection, ws: Uuid, pipeline: &str, run_id: &str, ttl: Duration) {
    let now = Utc::now();
    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO airway_pipeline_leases (workspace_id, pipeline_name, run_id, acquired_at, expires_at) \
         VALUES ($1, $2, $3, $4, $5)",
        [
            ws.into(),
            pipeline.into(),
            run_id.into(),
            now.into(),
            (now + ttl).into(),
        ],
    ))
    .await
    .expect("insert lease");
}

async fn holders(db: &DatabaseConnection, ws: Uuid) -> Vec<(String, String)> {
    leases::list(db, ws, Utc::now())
        .await
        .expect("list")
        .into_iter()
        .map(|l| (l.pipeline_name, l.run_id))
        .collect()
}

fn scoped(run_id: &str) -> ReleaseMode {
    ReleaseMode::Scoped {
        run_id: run_id.into(),
    }
}

#[tokio::test]
async fn the_listing_is_the_workspaces_own_and_flags_lapsed_rows() {
    let db = test_db_with(Schema::All).await;
    let (ws, other) = (Uuid::new_v4(), Uuid::new_v4());
    hold(&db, ws, "live", "r-live", Duration::hours(6)).await;
    hold(&db, ws, "stale", "r-stale", Duration::hours(-1)).await;
    hold(&db, other, "theirs", "r-theirs", Duration::hours(6)).await;

    let listed: Vec<LeaseView> = leases::list(&db, ws, Utc::now()).await.expect("list");
    let by_name = |n: &str| listed.iter().find(|l| l.pipeline_name == n);
    assert_eq!(listed.len(), 2, "only this workspace's: {listed:?}");
    assert!(!by_name("live").expect("live").lapsed);
    assert!(by_name("stale").expect("stale").lapsed, "past expires_at");
    assert!(by_name("theirs").is_none(), "another workspace's lease");
}

/// The confirmed path deletes only the run that was reviewed.
#[tokio::test]
async fn a_scoped_release_frees_the_reviewed_run() {
    let db = test_db_with(Schema::All).await;
    let ws = Uuid::new_v4();
    hold(&db, ws, "toast", "r1", Duration::hours(6)).await;

    let outcome = leases::release(&db, ws, "toast", &scoped("r1"), Utc::now())
        .await
        .expect("release");
    assert_eq!(outcome, ReleaseOutcome::Released { removed: 1 });
    assert!(holders(&db, ws).await.is_empty());
}

/// The cron tick started a new run between review and confirm: the scoped
/// delete leaves it, and the answer names it.
#[tokio::test]
async fn a_scoped_release_never_frees_a_successor() {
    let db = test_db_with(Schema::All).await;
    let ws = Uuid::new_v4();
    hold(&db, ws, "toast", "r2", Duration::hours(6)).await;

    let outcome = leases::release(&db, ws, "toast", &scoped("r1"), Utc::now())
        .await
        .expect("release");
    match outcome {
        ReleaseOutcome::HeldByAnotherRun(holder) => assert_eq!(holder.run_id, "r2"),
        other => panic!("expected the successor to be reported, got {other:?}"),
    }
    assert_eq!(holders(&db, ws).await, vec![("toast".into(), "r2".into())]);
}

/// Retried under its own id: still live, so left alone, and said so.
#[tokio::test]
async fn the_reviewed_run_reacquiring_is_reported_as_reheld() {
    let db = test_db_with(Schema::All).await;
    let ws = Uuid::new_v4();
    hold(&db, ws, "toast", "r1", Duration::hours(6)).await;
    // The run released and was retried between review and confirm, so the row
    // is a new one under the same id — which a scoped delete would remove. The
    // triage is for the window where the release found nothing; drive it by
    // deleting first, then re-acquiring before the classification reads.
    let removed = agentic_pipeline::airway_run::release_airway_lease_scoped(&db, ws, "toast", "r1")
        .await
        .expect("first release");
    assert_eq!(removed, 1);
    hold(&db, ws, "toast", "r1", Duration::hours(6)).await;
    let holder_now = leases::list(&db, ws, Utc::now())
        .await
        .expect("list")
        .into_iter()
        .find(|l| l.pipeline_name == "toast");
    assert_eq!(
        leases::classify_release(&scoped("r1"), 0, holder_now),
        ReleaseOutcome::ReheldBySameRun
    );
}

#[tokio::test]
async fn nothing_to_release_is_already_free_on_both_paths() {
    let db = test_db_with(Schema::All).await;
    let ws = Uuid::new_v4();
    for mode in [scoped("r1"), ReleaseMode::Force] {
        let outcome = leases::release(&db, ws, "toast", &mode, Utc::now())
            .await
            .expect("release");
        assert_eq!(outcome, ReleaseOutcome::AlreadyFree, "{mode:?}");
    }
}

/// Force removes whatever holds it — in the named workspace only.
#[tokio::test]
async fn force_releases_the_named_workspaces_lease_and_no_other() {
    let db = test_db_with(Schema::All).await;
    let (ws, other) = (Uuid::new_v4(), Uuid::new_v4());
    hold(&db, ws, "toast", "r1", Duration::hours(6)).await;
    hold(&db, other, "toast", "r9", Duration::hours(6)).await;
    hold(&db, Uuid::nil(), "toast", "r0", Duration::hours(6)).await;

    let outcome = leases::release(&db, ws, "toast", &ReleaseMode::Force, Utc::now())
        .await
        .expect("release");
    assert_eq!(outcome, ReleaseOutcome::Released { removed: 1 });
    assert!(holders(&db, ws).await.is_empty());
    assert_eq!(holders(&db, other).await.len(), 1, "another tenant's lease");
    assert_eq!(
        holders(&db, Uuid::nil()).await.len(),
        1,
        "the nil workspace is never substituted for the named one"
    );
    // Cleanup: the nil workspace's row is shared by every test in the database.
    agentic_pipeline::airway_run::force_release_airway_lease(&db, Uuid::nil(), "toast")
        .await
        .expect("cleanup");
}

fn with_role(role: WorkspaceRole) -> Router {
    let inject = move |mut req: Request, next: Next| {
        let role = role.clone();
        async move {
            req.extensions_mut().insert(EffectiveWorkspaceRole(role));
            next.run(req).await
        }
    };
    Router::new()
        .nest("/{workspace_id}", oxy_api_airway::routes())
        .layer(middleware::from_fn(inject))
        .with_state(oxy_app::surface::bare_app_state())
}

async fn status(router: Router, method: &str, path: &str) -> StatusCode {
    let req = axum::http::Request::builder()
        .method(method)
        .uri(path)
        .body(Body::empty())
        .unwrap();
    let response: Response = router.oneshot(req).await.expect("oneshot");
    response.status()
}

/// `WorkspaceAdmin` is the first extractor, so a member is refused before the
/// handler opens a connection — no database needed.
#[tokio::test]
async fn a_workspace_member_may_neither_list_nor_release() {
    let ws = Uuid::new_v4();
    let name = URL_SAFE_NO_PAD.encode(b"toast");
    for role in [WorkspaceRole::Member, WorkspaceRole::Viewer] {
        assert_eq!(
            status(
                with_role(role.clone()),
                "GET",
                &format!("/{ws}/agentic-airway/leases")
            )
            .await,
            StatusCode::FORBIDDEN,
            "{role:?} listing"
        );
        assert_eq!(
            status(
                with_role(role.clone()),
                "DELETE",
                &format!("/{ws}/agentic-airway/leases/{name}?force=true")
            )
            .await,
            StatusCode::FORBIDDEN,
            "{role:?} releasing"
        );
    }
}
