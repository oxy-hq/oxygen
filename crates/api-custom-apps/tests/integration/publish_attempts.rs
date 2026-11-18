//! Every publish that reaches the pipeline leaves one `app_publish_attempts`
//! row (`custom_apps_publish_attempts`), landed or refused, with its stage
//! timings; a refusal names the stage it came from and keeps its error. Driven
//! through the real `publish()` against a per-test database, and read back
//! through the console's handler (`GET /customer-apps/{id}/publishes`).
//!
//! The row a failure at **each** stage writes is pinned by the unit tests in
//! `custom_apps_publish_attempts::record`; here the refusals reachable without
//! an OLTP or object-store fault are driven end to end: `admit` (a reused
//! build id) and `inflate` (a bundle that is not a gzip).

use axum::extract::{Path, Query};
use chrono::{Duration, Utc};
use entity::{app_builds, app_publish_attempts};
use oxy_api_custom_apps::admin::apps::publishes::{PublishesQuery, list_publishes};
use oxy_api_custom_apps::custom_apps_publish::{
    PublishOptions, PublishTarget, Publisher, publish, publish_staged,
};
use oxy_api_custom_apps::custom_apps_publish_attempts::{KEEP_ATTEMPTS, Stage};
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, DatabaseConnection, EntityTrait, PaginatorTrait,
    QueryFilter, QueryOrder,
};
use serde_json::json;
use uuid::Uuid;

use crate::common::test_db;
use crate::custom_apps_publish_machine::{Tenant, input, seed_tenant};

/// The app's attempts, newest first.
async fn attempts(db: &DatabaseConnection, app_id: Uuid) -> Vec<app_publish_attempts::Model> {
    app_publish_attempts::Entity::find()
        .filter(app_publish_attempts::Column::AppId.eq(app_id))
        .order_by_desc(app_publish_attempts::Column::StartedAt)
        .all(db)
        .await
        .expect("read the attempts")
}

fn stage_keys(row: &app_publish_attempts::Model) -> Vec<String> {
    let mut keys: Vec<String> = row
        .stage_ms
        .as_object()
        .expect("{stage: ms}")
        .keys()
        .cloned()
        .collect();
    keys.sort();
    keys
}

fn sorted(names: &[&str]) -> Vec<String> {
    let mut names: Vec<String> = names.iter().map(|n| n.to_string()).collect();
    names.sort();
    names
}

/// A person's publish of `slug`, landed.
async fn landed(t: &Tenant, slug: &str, build_id: &str) -> Uuid {
    let who = Publisher::from_request(&t.admin, None);
    publish(input(t, slug, build_id, false, who))
        .await
        .expect("a person's publish")
        .app_id
}

#[tokio::test]
async fn a_publish_that_lands_answers_and_records_every_stage() {
    let db = test_db().await;
    let t = seed_tenant(&db).await;
    let who = Publisher::from_request(&t.admin, None);
    let result = publish(input(&t, "timed", "b-1", false, who))
        .await
        .expect("publish");

    let answered: Vec<&str> = result.stages.iter().map(|s| s.stage.as_str()).collect();
    assert_eq!(
        answered,
        Stage::ALL.map(Stage::name),
        "the answer carries every stage's time, in pipeline order"
    );

    let rows = attempts(&db, result.app_id).await;
    assert_eq!(rows.len(), 1, "one attempt per publish");
    let row = &rows[0];
    let build = app_builds::Entity::find()
        .filter(app_builds::Column::AppId.eq(result.app_id))
        .filter(app_builds::Column::BuildId.eq("b-1"))
        .one(&db)
        .await
        .expect("read the build")
        .expect("the build row");
    assert_eq!(row.outcome, "success");
    assert_eq!(
        row.app_build_id,
        Some(build.id),
        "names the build it landed"
    );
    assert_eq!(row.build_id, "b-1");
    assert_eq!(row.environment, "staging");
    assert_eq!(
        row.published_by,
        Some(t.admin.id),
        "as the build row names it"
    );
    assert_eq!(
        (row.failed_stage.as_deref(), row.error.as_deref()),
        (None, None)
    );
    assert_eq!(stage_keys(row), sorted(&Stage::ALL.map(Stage::name)));
    assert!(row.finished_at >= row.started_at);
}

#[tokio::test]
async fn a_refused_publish_is_recorded_at_the_stage_that_refused_it() {
    let db = test_db().await;
    let t = seed_tenant(&db).await;
    let app_id = landed(&t, "refused", "b-1").await;
    let who = || Publisher::from_request(&t.admin, None);

    // admit: the build id is taken.
    let reused = publish_staged(
        input(&t, "refused", "b-1", false, who()),
        PublishTarget::Channels,
        PublishOptions::default(),
    )
    .await
    .expect_err("a reused build id is refused");
    assert_eq!(reused.stage, Some(Stage::Admit));

    // inflate: the bundle is not a gzip.
    let mut garbled = input(&t, "refused", "b-2", false, who());
    garbled.tarball = b"not a tarball".to_vec();
    let bad = publish_staged(garbled, PublishTarget::Channels, PublishOptions::default())
        .await
        .expect_err("a bundle that is not a gzip is refused");
    assert_eq!(bad.stage, Some(Stage::Inflate));

    let rows = attempts(&db, app_id).await;
    let failed: Vec<_> = rows.iter().filter(|r| r.outcome == "failed").collect();
    assert_eq!(failed.len(), 2, "both refusals left a row");

    let (inflate, admit) = (failed[0], failed[1]);
    assert_eq!(inflate.failed_stage.as_deref(), Some("inflate"));
    assert_eq!(
        inflate.error.as_deref(),
        Some(bad.error.to_string().as_str())
    );
    assert_eq!(inflate.app_build_id, None);
    assert_eq!(inflate.build_id, "b-2");
    assert_eq!(stage_keys(inflate), sorted(&["admit", "inflate"]));

    assert_eq!(admit.failed_stage.as_deref(), Some("admit"));
    assert_eq!(
        admit.error.as_deref(),
        Some(reused.error.to_string().as_str())
    );
    assert_eq!(stage_keys(admit), sorted(&["admit"]));

    // The ledger holds the build that landed, and nothing for the refusals.
    let builds = app_builds::Entity::find()
        .filter(app_builds::Column::AppId.eq(app_id))
        .count(&db)
        .await
        .expect("count builds");
    assert_eq!(builds, 1);
}

/// A new app whose first publish fails is undone with it: there is no app for
/// the console to show the attempt under, and none is written.
#[tokio::test]
async fn a_first_publish_that_fails_leaves_no_attempt() {
    let db = test_db().await;
    let t = seed_tenant(&db).await;
    let who = Publisher::from_request(&t.admin, None);
    let mut garbled = input(&t, "never-born", "b-1", false, who);
    garbled.tarball = b"not a tarball".to_vec();
    publish(garbled).await.expect_err("refused");

    let any = app_publish_attempts::Entity::find()
        .count(&db)
        .await
        .expect("count attempts");
    assert_eq!(any, 0);
}

/// A refusal, the way a CI loop makes them.
async fn refused(t: &Tenant, slug: &str, build_id: &str) {
    let who = Publisher::from_request(&t.admin, None);
    let mut garbled = input(t, slug, build_id, false, who);
    garbled.tarball = b"not a tarball".to_vec();
    publish(garbled).await.expect_err("refused");
}

/// The console's read: newest first, `?limit=` attempts **of each outcome**,
/// as retention keeps them — so a run of failures after the last landed build
/// does not push the landed builds' timings out of the answer. Each attempt
/// has its stages in pipeline order and its publisher by email.
#[tokio::test]
async fn the_console_reads_the_newest_of_each_outcome() {
    let db = test_db().await;
    let t = seed_tenant(&db).await;
    let app_id = landed(&t, "listed", "ok-1").await;
    landed(&t, "listed", "ok-2").await;
    for build_id in ["bad-3", "bad-4", "bad-5"] {
        refused(&t, "listed", build_id).await;
    }

    let limit = PublishesQuery { limit: Some(2) };
    let listed = list_publishes(Path(app_id), Query(limit))
        .await
        .expect("200")
        .0
        .attempts;
    let seen: Vec<(&str, &str)> = listed
        .iter()
        .map(|a| (a.build_id.as_str(), a.outcome.as_str()))
        .collect();
    assert_eq!(
        seen,
        [
            ("bad-5", "failed"),
            ("bad-4", "failed"),
            ("ok-2", "success"),
            ("ok-1", "success"),
        ]
    );
    assert_eq!(listed[0].failed_stage.as_deref(), Some("inflate"));
    let stages: Vec<&str> = listed[2].stages.iter().map(|s| s.stage.as_str()).collect();
    assert_eq!(stages, Stage::ALL.map(Stage::name));
    assert_eq!(listed[2].published_by_email, t.admin.email);

    let missing = list_publishes(Path(Uuid::new_v4()), Query(PublishesQuery::default())).await;
    assert_eq!(missing.err(), Some(axum::http::StatusCode::NOT_FOUND));
}

/// A caller refused authorization writes nothing into the app it named: not
/// a row, so not a pruned-away failure either. Another org's admin names this
/// org, its workspace and its app's slug; and names its own org with this
/// org's workspace.
#[tokio::test]
async fn an_unauthorized_publish_writes_no_attempt_into_another_orgs_app() {
    let db = test_db().await;
    let victim = seed_tenant(&db).await;
    let app_id = landed(&victim, "victim-app", "ok-1").await;
    let intruder = seed_tenant(&db).await;
    let as_intruder = || Publisher::from_request(&intruder.admin, None);

    let mut named_victim = input(&victim, "victim-app", "evil-1", false, as_intruder());
    named_victim.build_id = "x".repeat(10_000);
    let refused = publish_staged(
        named_victim,
        PublishTarget::Channels,
        PublishOptions::default(),
    )
    .await
    .expect_err("an outsider may not publish into another org");
    assert_eq!(refused.stage, Some(Stage::Admit));

    let mut own_org_their_workspace =
        input(&intruder, "victim-app", "evil-2", false, as_intruder());
    own_org_their_workspace.project_id = victim.workspace;
    publish(own_org_their_workspace)
        .await
        .expect_err("a workspace outside the named org is refused");

    let rows = attempts(&db, app_id).await;
    let outcomes: Vec<&str> = rows.iter().map(|r| r.outcome.as_str()).collect();
    assert_eq!(
        outcomes,
        ["success"],
        "only the app's own publish is recorded"
    );
    let anywhere = app_publish_attempts::Entity::find()
        .count(&db)
        .await
        .expect("count attempts");
    assert_eq!(anywhere, 1, "and nothing was recorded anywhere else");
}

/// Retention: the newest `KEEP_ATTEMPTS` of each outcome, pruned after each
/// attempt, so a run of failures never pushes out the landed builds' timings.
#[tokio::test]
async fn an_apps_attempts_are_kept_to_the_newest_of_each_outcome() {
    let db = test_db().await;
    let t = seed_tenant(&db).await;
    let app_id = landed(&t, "kept", "b-0").await;
    let long_ago = Utc::now() - Duration::days(30);
    for i in 0..KEEP_ATTEMPTS {
        let at = (long_ago + Duration::minutes(i as i64)).fixed_offset();
        app_publish_attempts::ActiveModel {
            id: ActiveValue::Set(Uuid::new_v4()),
            app_id: ActiveValue::Set(app_id),
            app_build_id: ActiveValue::Set(None),
            build_id: ActiveValue::Set(format!("old-{i}")),
            environment: ActiveValue::Set("staging".into()),
            published_by: ActiveValue::Set(None),
            published_via: ActiveValue::Set(None),
            published_token_id: ActiveValue::Set(None),
            started_at: ActiveValue::Set(at),
            finished_at: ActiveValue::Set(at),
            outcome: ActiveValue::Set("failed".into()),
            failed_stage: ActiveValue::Set(Some("inflate".into())),
            error: ActiveValue::Set(Some("old".into())),
            stage_ms: ActiveValue::Set(json!({ "admit": 1 })),
        }
        .insert(&db)
        .await
        .expect("an old failure");
    }

    refused(&t, "kept", "b-new").await;

    let rows = attempts(&db, app_id).await;
    let failed: Vec<&str> = rows
        .iter()
        .filter(|r| r.outcome == "failed")
        .map(|r| r.build_id.as_str())
        .collect();
    assert_eq!(failed.len() as u64, KEEP_ATTEMPTS, "held to the window");
    assert_eq!(failed[0], "b-new", "the newest is kept");
    assert!(!failed.contains(&"old-0"), "the oldest is pruned");
    assert_eq!(
        rows.iter().filter(|r| r.outcome == "success").count(),
        1,
        "a run of failures leaves the landed attempt alone"
    );
}
