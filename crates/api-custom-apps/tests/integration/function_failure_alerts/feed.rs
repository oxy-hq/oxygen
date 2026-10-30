//! The cross-app failure feed (`GET /api/customer-apps/function-failures`)
//! over the rows finalization writes.
//!
//! Which rows count, the production-only filter, the grant's org scope and the
//! keyset page are all SQL, so they are tested against Postgres.

use chrono::{DateTime, Duration, Utc};
use entity::{app_builds, app_function_invocations, apps, org_billing, organizations, workspaces};
use oxy_api_custom_apps::admin::apps::function_failures::{
    Cursor, FailuresPage, Request, failed_rows_query, failures,
};
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ConnectionTrait, DatabaseBackend, DatabaseConnection,
    QueryTrait, Statement, TransactionTrait,
};
use uuid::Uuid;

use crate::common::test_db;

const POST_JE_ERROR: &str = "function threw: HostError: ctx.warehouse: failed to convert query \
     results: row stream: driver error: connection closed";

struct App {
    org_id: Uuid,
    app_id: Uuid,
    build_id: Uuid,
}

/// An org (with a billing row in `billing`, when given) and one app in it.
async fn app_in_new_org(
    db: &DatabaseConnection,
    slug: &str,
    billing: Option<org_billing::BillingStatus>,
) -> App {
    let org_id = Uuid::new_v4();
    organizations::ActiveModel {
        id: Set(org_id),
        name: Set(format!("{slug} org")),
        slug: Set(format!("{slug}-{org_id}")),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed org");
    if let Some(status) = billing {
        org_billing::ActiveModel {
            id: Set(Uuid::new_v4()),
            org_id: Set(org_id),
            status: Set(status),
            seats_paid: Set(0),
            created_at: Set(Utc::now().into()),
            updated_at: Set(Utc::now().into()),
            ..Default::default()
        }
        .insert(db)
        .await
        .expect("seed billing");
    }
    let workspace_id = Uuid::new_v4();
    workspaces::ActiveModel {
        id: Set(workspace_id),
        name: Set(format!("{slug} workspace")),
        org_id: Set(Some(org_id)),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed workspace");
    let app_id = Uuid::new_v4();
    apps::ActiveModel {
        id: Set(app_id),
        slug: Set(slug.into()),
        name: Set(slug.into()),
        org_id: Set(org_id),
        project_id: Set(workspace_id),
        branch: Set("main".into()),
        source_repo: Set("feed/test".into()),
        status: Set("active".into()),
        source_type: Set("s3".into()),
        source_config: Set(serde_json::json!({})),
        published_at: Set(Some(Utc::now().into())),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed app");
    let build_id = Uuid::new_v4();
    app_builds::ActiveModel {
        id: Set(build_id),
        app_id: Set(app_id),
        build_id: Set("b1".into()),
        s3_prefix: Set(format!("customer-apps/{app_id}/builds/b1/")),
        created_at: Set(Utc::now().into()),
        validation_status: Set("passed".into()),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed build");
    App {
        org_id,
        app_id,
        build_id,
    }
}

/// One invocation row. `fingerprint` and `status` decide whether it counts.
struct Row<'a> {
    status: &'a str,
    fingerprint: Option<&'a str>,
    environment: &'a str,
    at: DateTime<Utc>,
}

async fn invoked(db: &DatabaseConnection, app: &App, row: Row<'_>) -> Uuid {
    let id = Uuid::new_v4();
    app_function_invocations::ActiveModel {
        id: Set(id),
        app_id: Set(app.app_id),
        build_id: Set(app.build_id),
        function_name: Set("post-je".into()),
        mode: Set("route".into()),
        user_id: Set(None),
        status: Set(row.status.into()),
        duration_ms: Set(Some(40)),
        error: Set((row.status != "success").then(|| POST_JE_ERROR.to_string())),
        cancel_requested_at: Set(None),
        created_at: Set(row.at.into()),
        idempotency_key: Set(None),
        result_body: Set(None),
        result_status: Set(None),
        request_hash: Set(None),
        failure_fingerprint: Set(row.fingerprint.map(str::to_string)),
        environment: Set(row.environment.into()),
        credential_token_id: Set(None),
        error_kind: Set(row.fingerprint.map(|_| "threw".to_string())),
    }
    .insert(db)
    .await
    .expect("seed invocation");
    id
}

/// A production failure at `at`.
async fn failed_at(db: &DatabaseConnection, app: &App, at: DateTime<Utc>) -> Uuid {
    let row = Row {
        status: "error",
        fingerprint: Some("bace6ddec50847c8"),
        environment: "production",
        at,
    };
    invoked(db, app, row).await
}

/// Whole seconds: a Postgres round trip of `Utc::now()` is not exact on Linux.
fn base() -> DateTime<Utc> {
    DateTime::from_timestamp(Utc::now().timestamp() - 3600, 0).unwrap()
}

fn last_hours(hours: i64) -> Request {
    let until = base() + Duration::minutes(30);
    Request {
        since: until - Duration::hours(hours),
        until,
        limit: 100,
        after: None,
        org_id: None,
        app_id: None,
    }
}

fn ids(page: &FailuresPage) -> Vec<Uuid> {
    page.failures.iter().map(|f| f.invocation_id).collect()
}

#[tokio::test]
async fn only_production_failures_in_the_window_are_listed_newest_first() {
    let db = test_db().await;
    let app = app_in_new_org(&db, "bookkeeping", Some(org_billing::BillingStatus::Active)).await;
    let t = base();
    let older = failed_at(&db, &app, t - Duration::minutes(10)).await;
    let newer = failed_at(&db, &app, t).await;
    // A success that caught a failed host call carries a fingerprint: it counts.
    let caught = Row {
        status: "success",
        fingerprint: Some("f00d"),
        environment: "production",
        at: t - Duration::minutes(5),
    };
    let caught = invoked(&db, &app, caught).await;
    // None of these count: a plain success, a staging failure, a cancelled
    // run, and a production failure before the window.
    let success = Row {
        status: "success",
        fingerprint: None,
        environment: "production",
        at: t,
    };
    invoked(&db, &app, success).await;
    let staging = Row {
        status: "error",
        fingerprint: Some("bace6ddec50847c8"),
        environment: "staging",
        at: t,
    };
    invoked(&db, &app, staging).await;
    let cancelled = Row {
        status: "cancelled",
        fingerprint: None,
        environment: "production",
        at: t,
    };
    invoked(&db, &app, cancelled).await;
    failed_at(&db, &app, t - Duration::hours(30)).await;

    let page = failures(&db, None, &last_hours(24)).await.expect("feed");
    assert_eq!(ids(&page), vec![newer, caught, older]);
    assert_eq!(page.next_cursor, None);

    let first = &page.failures[0];
    assert_eq!(first.app_id, app.app_id);
    assert_eq!(first.app_slug, "bookkeeping");
    assert_eq!(first.org_id, app.org_id);
    assert_eq!(first.org_name.as_deref(), Some("bookkeeping org"));
    assert_eq!(first.org_billing_status.as_deref(), Some("active"));
    assert_eq!(first.function_name, "post-je");
    assert_eq!(first.kind.as_deref(), Some("threw"));
    assert_eq!(first.fingerprint.as_deref(), Some("bace6ddec50847c8"));
    assert_eq!(first.error.as_deref(), Some(POST_JE_ERROR));
}

#[tokio::test]
async fn a_bounded_grant_sees_only_its_orgs_and_filters_narrow_within_it() {
    let db = test_db().await;
    let mine = app_in_new_org(&db, "mine", None).await;
    let theirs = app_in_new_org(&db, "theirs", Some(org_billing::BillingStatus::Incomplete)).await;
    let t = base();
    let my_failure = failed_at(&db, &mine, t).await;
    let their_failure = failed_at(&db, &theirs, t).await;

    // Unbounded: both, and an org with no billing row reads as `None`.
    let all = failures(&db, None, &last_hours(2)).await.expect("feed");
    assert!(ids(&all).contains(&my_failure) && ids(&all).contains(&their_failure));
    let mine_row = all
        .failures
        .iter()
        .find(|f| f.invocation_id == my_failure)
        .unwrap();
    assert_eq!(mine_row.org_billing_status, None);

    // Bounded to my org: theirs is absent, not refused.
    let scope = [mine.org_id];
    let bounded = failures(&db, Some(&scope), &last_hours(2))
        .await
        .expect("feed");
    assert_eq!(ids(&bounded), vec![my_failure]);

    // Naming their org or app from inside that grant is an empty page.
    let mut by_org = last_hours(2);
    by_org.org_id = Some(theirs.org_id);
    assert!(
        failures(&db, Some(&scope), &by_org)
            .await
            .unwrap()
            .failures
            .is_empty()
    );
    let mut by_app = last_hours(2);
    by_app.app_id = Some(theirs.app_id);
    assert!(
        failures(&db, Some(&scope), &by_app)
            .await
            .unwrap()
            .failures
            .is_empty()
    );

    // Unbounded, the same filter narrows to that app.
    assert_eq!(
        ids(&failures(&db, None, &by_app).await.unwrap()),
        vec![their_failure]
    );

    // A grant bounded to nothing sees nothing.
    assert!(
        failures(&db, Some(&[]), &last_hours(2))
            .await
            .unwrap()
            .failures
            .is_empty()
    );
}

#[tokio::test]
async fn pages_walk_every_row_once_including_rows_that_share_a_timestamp() {
    let db = test_db().await;
    let app = app_in_new_org(&db, "paging", None).await;
    let t = base();
    let mut seeded = Vec::new();
    // Five rows; three share one instant, so the id breaks the tie.
    for minutes in [0, 0, 0, 1, 2] {
        seeded.push(failed_at(&db, &app, t - Duration::minutes(minutes)).await);
    }
    let mut request = last_hours(2);
    request.app_id = Some(app.app_id);
    request.limit = 2;
    let mut seen = Vec::new();
    loop {
        let page = failures(&db, None, &request).await.expect("feed");
        assert!(page.failures.len() <= 2);
        seen.extend(ids(&page));
        match page.next_cursor {
            Some(raw) => request.after = Some(Cursor::decode(&raw).expect("a cursor it minted")),
            None => break,
        }
    }
    let mut expected = seeded.clone();
    expected.sort();
    let mut got = seen.clone();
    got.sort();
    assert_eq!(got, expected, "every row exactly once");
    assert_eq!(seen.len(), 5);
}

/// The feed's query, unbounded and on a later page, can be answered from the
/// failure-feed partial index. With sequential scans off, Postgres picks the
/// index only if it can prove the query's WHERE implies the index predicate —
/// which is what stating the migration's predicate verbatim is for. Without
/// the failure filter, the plan falls back to another index.
#[tokio::test]
async fn the_unbounded_feed_query_can_use_the_failure_index() {
    let db = test_db().await;
    let mut request = last_hours(24);
    for after in [
        None,
        Some(Cursor {
            created_at: base(),
            id: Uuid::new_v4(),
        }),
    ] {
        request.after = after;
        let built = failed_rows_query(None, &request).build(DatabaseBackend::Postgres);
        let txn = db.begin().await.expect("begin");
        txn.execute_unprepared("SET LOCAL enable_seqscan = off")
            .await
            .expect("set");
        let plan: Vec<String> = txn
            .query_all_raw(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                format!("EXPLAIN {}", built.sql),
                built.values.map(|v| v.0).unwrap_or_default(),
            ))
            .await
            .expect("explain")
            .into_iter()
            .map(|row| row.try_get_by_index::<String>(0).expect("plan line"))
            .collect();
        txn.rollback().await.expect("rollback");
        let plan = plan.join("\n");
        assert!(
            plan.contains("idx_app_function_invocations_failure_feed"),
            "the feed query did not use the failure index (cursor: {after:?}):\n{plan}"
        );
    }
}
