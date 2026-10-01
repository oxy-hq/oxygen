//! Nothing staging may fail or delay a publish it did not block before
//! staging existed (review fix round 1, B4 and S2):
//!
//! - a staging-only (`--no-promote`) publish **queues** its sibling Airhouse
//!   migration and answers without waiting for it, once per build; the task,
//!   run by the executor production registers, fails — here, no Airhouse on
//!   the deployment — and that failure is its run's, never the publish's;
//! - a promoting publish whose `nonProduction` mapping cannot be checked (the
//!   workspace has no compiled config) succeeds with a warning, while a
//!   staging-only one answers 409 to retry;
//! - production's Airhouse apply lock and the sibling's are different locks, so
//!   neither waits on nor fails `Busy` against the other.

use agentic_core::delegation::TaskOutcome;
use entity::workspaces;
use oxy_app::server::api::custom_apps_migrations::{MigrationTarget, airhouse_lock_key};
use oxy_app::server::api::custom_apps_nonproduction::MappingRefusal;
use oxy_app::server::api::custom_apps_nonproduction::staging_task::STAGING_MIGRATIONS_KIND;
use oxy_app::server::api::custom_apps_nonproduction::staging_task_executor::bounded;
use oxy_app::server::api::custom_apps_publish::{
    OrgRef, PublishError, PublishInput, PublishResult, publish,
};
use oxy_auth::user::LOCAL_GUEST_EMAIL;
use sea_orm::{ActiveModelTrait, ActiveValue};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::custom_app_functions_fixture::{Tenant, seeded_tenant};
use crate::custom_apps_publish_function_artifacts::tar_gz;
use crate::staging_migration_tasks::{assert_queued_once, queued, run_one, run_row};

const INDEX: &[u8] = b"<!doctype html><html><head><title>s</title></head><body></body></html>";

/// Publish app `slug` whose `oxy-app.json` adds `extra` to a one-function
/// manifest, with `files` beside it.
async fn publish_with(
    t: &Tenant,
    workspace: Uuid,
    slug: &str,
    promote: bool,
    extra: Value,
    files: &[(&str, &[u8])],
) -> Result<PublishResult, PublishError> {
    let mut manifest = json!({
        "schemaVersion": 2,
        "slug": slug,
        "functions": { "noop": { "route": true } },
    });
    for (key, value) in extra.as_object().expect("an object") {
        manifest[key] = value.clone();
    }
    let manifest = manifest.to_string();
    let mut all: Vec<(&str, &[u8])> = vec![
        ("index.html", INDEX),
        ("oxy-app.json", manifest.as_bytes()),
        (
            "functions/noop.js",
            b"export default async () => Response.json({});",
        ),
    ];
    all.extend_from_slice(files);
    publish(PublishInput {
        org_ref: Some(OrgRef::Id(t.org_id)),
        app_slug: slug.to_string(),
        project_id: workspace,
        branch: None,
        build_id: format!("nb-{}", &Uuid::new_v4().simple().to_string()[..8]),
        name: None,
        promote,
        tarball: tar_gz(&all),
        manifest: None,
        source_repo: None,
        commit_sha: None,
        published_by: Some(t.guest_id),
        published_by_email: Some(LOCAL_GUEST_EMAIL.to_string()),
        machine_app_id: None,
        published_via: None,
        semantic_revision_id: None,
    })
    .await
}

/// A workspace row in the tenant's org that was never compiled.
async fn uncompiled_workspace(t: &Tenant) -> Uuid {
    let id = Uuid::new_v4();
    workspaces::ActiveModel {
        id: ActiveValue::Set(id),
        name: ActiveValue::Set("Never compiled".into()),
        org_id: ActiveValue::Set(Some(t.org_id)),
        ..Default::default()
    }
    .insert(&t.db)
    .await
    .expect("seed workspace");
    id
}

#[tokio::test]
async fn a_staging_only_publish_queues_its_sibling_migration_whose_failure_is_the_tasks() {
    let t = seeded_tenant().await;
    let workspace = uncompiled_workspace(&t).await;
    let published = publish_with(
        &t,
        workspace,
        "nb-sibling",
        false,
        json!({ "airhouseMigrations": { "dir": "airhouse-migrations" } }),
        &[(
            "airhouse-migrations/0001_visits.sql",
            b"CREATE TABLE app_nb_sibling.visits (visit_id VARCHAR NOT NULL);",
        )],
    )
    .await
    .expect("a failing sibling migration never fails the publish");
    assert!(
        published.warnings.is_empty(),
        "the publish answered before the apply ran: {:?}",
        published.warnings
    );

    // Queued, once, as a platform run filed under the app's workspace.
    let tasks = queued(&t.db, published.app_id).await;
    assert_eq!(tasks.len(), 1, "one store declares files: {tasks:?}");
    let (task_id, spec) = tasks.into_iter().next().unwrap();
    let (source_type, metadata) = run_row(&t.db, &task_id).await;
    assert_eq!(source_type, STAGING_MIGRATIONS_KIND);
    assert_eq!(metadata["store"], "airhouse");
    assert_queued_once(&t.db, &spec).await;

    // The worker's half: the apply fails, and the failure is the task's.
    match run_one(&t.db, &task_id, spec).await {
        TaskOutcome::Failed(why) => assert!(
            why.contains("staging's Airhouse schema was not migrated"),
            "{why}"
        ),
        other => panic!("no Airhouse here, so the apply fails: {other:?}"),
    }
}

#[tokio::test]
async fn an_unchecked_mapping_warns_on_a_promote_and_refuses_a_staging_only_publish() {
    let t = seeded_tenant().await;
    let workspace = uncompiled_workspace(&t).await;
    let mapping = json!({ "nonProduction": { "destinations": { "ch": "ch_staging" } } });
    let promoted = publish_with(&t, workspace, "nb-map", true, mapping.clone(), &[])
        .await
        .expect("a promote is not blocked by a mapping it could not check");
    assert!(
        promoted
            .warnings
            .iter()
            .any(|w| w.contains("could not be checked")),
        "{:?}",
        promoted.warnings
    );
    match publish_with(&t, workspace, "nb-map", false, mapping, &[]).await {
        Err(PublishError::DestinationMapping(MappingRefusal::Unchecked(_))) => {}
        other => panic!("a staging-only publish answers 409 to retry, got {other:?}"),
    }
}

/// Production's apply lock and the sibling's are different Postgres advisory
/// locks: one held does not stop the other, and production's still stops
/// production's (the control).
#[tokio::test]
async fn production_and_sibling_apply_locks_do_not_contend() {
    let t = seeded_tenant().await;
    let app = Uuid::new_v4();
    let production = airhouse_lock_key(app, &MigrationTarget::Production);
    let sibling = airhouse_lock_key(app, &MigrationTarget::Schema("app_x__staging".into()));
    let pool = t.db.get_postgres_connection_pool();
    let mut holder = pool.begin().await.expect("holder");
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(production)
        .execute(&mut *holder)
        .await
        .expect("take production's lock");
    let try_lock = |key: i64| {
        let pool = pool.clone();
        async move {
            let mut other = pool.begin().await.expect("other");
            let got: bool = sqlx::query_scalar("SELECT pg_try_advisory_xact_lock($1)")
                .bind(key)
                .fetch_one(&mut *other)
                .await
                .expect("try");
            let _ = other.rollback().await;
            got
        }
    };
    assert!(
        try_lock(sibling).await,
        "the sibling's apply waits on production's"
    );
    assert!(
        !try_lock(production).await,
        "control: production's lock is held"
    );
    let _ = holder.rollback().await;
}

/// Fix round 3, ruling 3: a sibling apply past its outer deadline is dropped,
/// and dropping it releases the advisory lock it held — the next publish's
/// apply is not left `Busy` behind a stuck one.
#[tokio::test]
async fn a_sibling_apply_past_its_deadline_releases_its_lock() {
    let t = seeded_tenant().await;
    let key = airhouse_lock_key(
        Uuid::new_v4(),
        &MigrationTarget::Schema("app_x__staging".into()),
    );
    let pool = t.db.get_postgres_connection_pool().clone();
    let (locked_tx, locked_rx) = tokio::sync::oneshot::channel();
    let stuck = tokio::spawn(bounded(std::time::Duration::from_millis(500), {
        let pool = pool.clone();
        async move {
            // What `apply_airhouse` holds while it applies: a transaction with
            // the target's advisory lock.
            let mut lock = pool.begin().await.expect("lock transaction");
            sqlx::query("SELECT pg_advisory_xact_lock($1)")
                .bind(key)
                .execute(&mut *lock)
                .await
                .expect("take the lock");
            let _ = locked_tx.send(());
            std::future::pending::<()>().await;
            drop(lock);
        }
    }));
    locked_rx.await.expect("the apply took its lock");
    let try_lock = || {
        let pool = pool.clone();
        async move {
            let mut other = pool.begin().await.expect("other");
            let got: bool = sqlx::query_scalar("SELECT pg_try_advisory_xact_lock($1)")
                .bind(key)
                .fetch_one(&mut *other)
                .await
                .expect("try");
            let _ = other.rollback().await;
            got
        }
    };
    assert!(!try_lock().await, "control: the stuck apply holds the lock");
    assert_eq!(
        stuck.await.expect("the task ends"),
        None,
        "dropped at its deadline"
    );
    let mut released = false;
    for _ in 0..50 {
        if try_lock().await {
            released = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(released, "the lock outlived the dropped apply");
}
