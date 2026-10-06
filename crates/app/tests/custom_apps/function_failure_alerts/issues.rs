//! The Issues view over the same invocation rows the pager decides from.
//!
//! Like the pager's verdicts, what an issue *is* lives in SQL — the grouping
//! key, which rows count, which row is "the latest" — so it is tested against
//! Postgres, with rows inserted the way finalization writes them.

use chrono::{Duration, Utc};
use entity::{app_builds, app_function_invocations, apps};
use oxy_app::server::api::admin::apps::issues::{Issue, MAX_ISSUES, issues_of};
use sea_orm::{ActiveModelTrait, ActiveValue::Set, DatabaseConnection, EntityTrait};
use uuid::Uuid;

use super::{FINGERPRINT, FUNCTION, Seeded, failed, failed_as, failed_in, seed};
use crate::common::test_db;

async fn app(db: &DatabaseConnection, s: &Seeded) -> apps::Model {
    apps::Entity::find_by_id(s.app_id)
        .one(db)
        .await
        .expect("load app")
        .expect("the seeded app")
}

/// Make `build` the one production serves.
async fn publish(db: &DatabaseConnection, s: &Seeded, build: Uuid) {
    let mut app: apps::ActiveModel = app(db, s).await.into();
    app.published_build_id = Set(Some(build));
    app.update(db).await.expect("publish");
}

/// A second build of the seeded app, labelled `b2`.
async fn second_build(db: &DatabaseConnection, s: &Seeded) -> Uuid {
    let id = Uuid::new_v4();
    app_builds::ActiveModel {
        id: Set(id),
        app_id: Set(s.app_id),
        build_id: Set("b2".into()),
        s3_prefix: Set(format!("customer-apps/{}/builds/b2/", s.app_id)),
        created_at: Set(Utc::now().into()),
        validation_status: Set("passed".into()),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed second build");
    id
}

/// One failed production invocation on `build`, with its own error text.
async fn failed_on(db: &DatabaseConnection, s: &Seeded, build: Uuid, error: &str, ago: Duration) {
    app_function_invocations::ActiveModel {
        id: Set(Uuid::new_v4()),
        app_id: Set(s.app_id),
        build_id: Set(build),
        function_name: Set(FUNCTION.into()),
        mode: Set("route".into()),
        user_id: Set(None),
        status: Set("error".into()),
        duration_ms: Set(Some(12)),
        error: Set(Some(error.into())),
        cancel_requested_at: Set(None),
        created_at: Set((Utc::now() - ago).into()),
        idempotency_key: Set(None),
        result_body: Set(None),
        result_status: Set(None),
        request_hash: Set(None),
        failure_fingerprint: Set(Some(FINGERPRINT.into())),
        environment: Set("production".into()),
    }
    .insert(db)
    .await
    .expect("seed invocation");
}

async fn issues(db: &DatabaseConnection, s: &Seeded, days: i64) -> Vec<Issue> {
    issues_of(db, &app(db, s).await, days, Utc::now())
        .await
        .expect("issues")
        .issues
}

fn keys(issues: &[Issue]) -> Vec<(&str, &str)> {
    issues
        .iter()
        .map(|i| (i.function_name.as_str(), i.fingerprint.as_str()))
        .collect()
}

#[tokio::test]
async fn repeated_failures_of_one_kind_are_one_issue() {
    let db = test_db().await;
    let s = seed(&db).await;
    for minutes in [50, 20, 5] {
        failed(&db, &s, Duration::minutes(minutes)).await;
    }

    let issues = issues(&db, &s, 7).await;

    assert_eq!(keys(&issues), [(FUNCTION, FINGERPRINT)]);
    assert_eq!(issues[0].occurrences, 3);
    assert!(issues[0].first_seen < issues[0].last_seen);
    assert_eq!(issues[0].last.created_at, issues[0].last_seen);
}

/// The pager's key is `(function, fingerprint)`. Every timeout hashes the
/// same, so a fingerprint shared by two functions is two failures — the pager
/// counts them apart, and a view that merged them would disagree with the
/// page that sent the reader here.
#[tokio::test]
async fn one_fingerprint_on_two_functions_is_two_issues() {
    let db = test_db().await;
    let s = seed(&db).await;
    failed_as(
        &db,
        &s,
        "upload-report",
        Some("timeout0000000000"),
        Duration::minutes(9),
    )
    .await;
    failed_as(
        &db,
        &s,
        "sync-orders",
        Some("timeout0000000000"),
        Duration::minutes(3),
    )
    .await;

    let issues = issues(&db, &s, 7).await;

    // Most recently seen first.
    assert_eq!(
        keys(&issues),
        [
            ("sync-orders", "timeout0000000000"),
            ("upload-report", "timeout0000000000")
        ]
    );
}

#[tokio::test]
async fn only_productions_fingerprinted_failures_inside_the_window_count() {
    let db = test_db().await;
    let s = seed(&db).await;
    failed(&db, &s, Duration::hours(2)).await;
    // Staging is somebody testing; the pager ignores it and so does this.
    failed_in(&db, &s, "staging", Duration::minutes(1)).await;
    // A row from before fingerprints existed has nothing to be grouped by.
    failed_as(&db, &s, FUNCTION, None, Duration::minutes(1)).await;
    // Outside a one-day window, inside a week.
    failed_as(
        &db,
        &s,
        "nightly",
        Some("aaaaaaaaaaaaaaaa"),
        Duration::days(3),
    )
    .await;

    let day = issues(&db, &s, 1).await;
    assert_eq!(keys(&day), [(FUNCTION, FINGERPRINT)]);
    assert_eq!(day[0].occurrences, 1);

    let week = issues(&db, &s, 7).await;
    assert_eq!(
        keys(&week),
        [(FUNCTION, FINGERPRINT), ("nightly", "aaaaaaaaaaaaaaaa")]
    );
}

/// "Is it still wrong" without a stored status: an issue the live build has
/// had is still happening on what users are served; one it has not had was
/// last seen on a build that has since been replaced.
#[tokio::test]
async fn an_issue_knows_whether_the_live_build_has_had_it() {
    let db = test_db().await;
    let s = seed(&db).await;
    let b2 = second_build(&db, &s).await;
    failed_on(
        &db,
        &s,
        s.build_id,
        "function threw: old",
        Duration::hours(5),
    )
    .await;

    // Nothing published: no build is live, so no issue is on it.
    assert!(!issues(&db, &s, 7).await[0].on_live_build);

    // b2 goes live and has not failed this way.
    publish(&db, &s, b2).await;
    let fixed = issues(&db, &s, 7).await;
    assert!(!fixed[0].on_live_build);
    assert_eq!(fixed[0].last.build_id.as_deref(), Some("b1"));

    // The fix did not hold: it happens on b2 as well.
    failed_on(&db, &s, b2, "function threw: new", Duration::minutes(1)).await;
    let back = issues(&db, &s, 7).await;
    assert_eq!(back.len(), 1);
    assert!(back[0].on_live_build);
    assert_eq!(back[0].builds, 2);
    assert_eq!(back[0].occurrences, 2);
    // The latest occurrence is the one shown, not the first or an arbitrary one.
    assert_eq!(back[0].last.error.as_deref(), Some("function threw: new"));
    assert_eq!(back[0].last.build_id.as_deref(), Some("b2"));
}

/// A cut list says it is one.
#[tokio::test]
async fn more_issues_than_the_cap_are_reported_as_truncated() {
    let db = test_db().await;
    let s = seed(&db).await;
    for n in 0..=MAX_ISSUES {
        let fingerprint = format!("{n:016x}");
        failed_as(
            &db,
            &s,
            FUNCTION,
            Some(&fingerprint),
            Duration::minutes(n as i64 + 1),
        )
        .await;
    }

    let list = issues_of(&db, &app(&db, &s).await, 7, Utc::now())
        .await
        .expect("issues");

    assert_eq!(list.issues.len(), MAX_ISSUES);
    assert!(list.truncated);
    // The newest are the ones kept.
    assert_eq!(list.issues[0].fingerprint, format!("{:016x}", 0));
}

#[tokio::test]
async fn an_app_that_has_not_failed_has_no_issues() {
    let db = test_db().await;
    let s = seed(&db).await;

    let list = issues_of(&db, &app(&db, &s).await, 7, Utc::now())
        .await
        .expect("issues");

    assert!(list.issues.is_empty());
    assert!(!list.truncated);
    assert_eq!(list.window_days, 7);
}
