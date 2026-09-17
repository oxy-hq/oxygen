//! `load_history`'s raw SQL, against a real Postgres.
//!
//! ## Why this needs a database
//!
//! The unit tests in `metering::history_tests` cover `daily_series`, which is
//! pure — they hand it a map and check the carry-forward. Nothing executes the
//! `format!`-built `DISTINCT ON` statement, so a typo, a renamed column, or a
//! `FromQueryResult` field that no longer lines up surfaces as a 500 on the
//! Storage tab's default view rather than as a failing test.
//!
//! That is the same class as the `SUM(bigint)` → `numeric` decode this work
//! already shipped once and had to fix: SQL that compiles fine because it is a
//! string, and only fails when a real server parses it.
//!
//! ## What it pins
//!
//! * The statement parses and its columns decode.
//! * A day with several samples collapses to its **closing** reading, not its
//!   first and not a sum.
//! * A sample from before the window still sets the opening day, which is the
//!   whole reason the query reaches back past `period_start`.
//! * Bucketing is **UTC**, independent of the session `TimeZone` — bare
//!   `date_trunc` on a `timestamptz` buckets by the connection's GUC, so on a
//!   non-UTC deployment a day's real closing reading would be silently dropped.

use crate::common::{Schema, fresh_db, test_db};
use chrono::{DateTime, FixedOffset, TimeZone, Utc};
use entity::{app_storage_usage_samples, apps, organizations, workspaces};
use oxy_app::server::api::admin::apps::storage::fleet_rows_scoped;
use oxy_app::server::api::custom_apps_storage::metering::load_history;
use oxy_app::server::api::custom_apps_storage::sweeper::{apps_due, unmeasured_app_count};
use sea_orm::{
    ActiveModelTrait, ActiveValue, ConnectOptions, ConnectionTrait, Database, DatabaseConnection,
    Statement,
};
use uuid::Uuid;

const MB: i64 = 1024 * 1024;

/// `app_storage_usage_samples.app_id` has an `ON DELETE CASCADE` FK, so a real
/// `apps` row (and its org + workspace) has to exist first.
async fn seed_app(conn: &DatabaseConnection) -> Uuid {
    seed_app_with_org(conn).await.0
}

/// [`seed_app`], but hands back the org too — the scope tests need to name it.
/// Every call makes its own org, so two apps are never scope-siblings by accident.
async fn seed_app_with_org(conn: &DatabaseConnection) -> (Uuid, Uuid) {
    let org_id = Uuid::new_v4();
    organizations::ActiveModel {
        id: ActiveValue::Set(org_id),
        name: ActiveValue::Set(format!("hist-{org_id}")),
        // NOT NULL, and unique — derive it from the id so parallel tests on a
        // shared server never collide.
        slug: ActiveValue::Set(format!("hist-{}", org_id.simple())),
        ..Default::default()
    }
    .insert(conn)
    .await
    .expect("seed org");

    let workspace_id = Uuid::new_v4();
    workspaces::ActiveModel {
        id: ActiveValue::Set(workspace_id),
        name: ActiveValue::Set("history".into()),
        // Nullable on `workspaces` (unlike `apps.org_id`, which is not).
        org_id: ActiveValue::Set(Some(org_id)),
        ..Default::default()
    }
    .insert(conn)
    .await
    .expect("seed workspace");

    // Mirrors `custom_app_visibility::seed_app` — `apps` has several NOT NULL
    // columns with no default, and none of them matter to this test.
    let app_id = Uuid::new_v4();
    apps::ActiveModel {
        id: ActiveValue::Set(app_id),
        slug: ActiveValue::Set(format!("hist-{}", app_id.simple())),
        name: ActiveValue::Set("History".into()),
        org_id: ActiveValue::Set(org_id),
        project_id: ActiveValue::Set(workspace_id),
        branch: ActiveValue::Set("main".into()),
        source_repo: ActiveValue::Set("hist/test".into()),
        status: ActiveValue::Set("active".into()),
        source_type: ActiveValue::Set("s3".into()),
        source_config: ActiveValue::Set(serde_json::json!({})),
        visibility: ActiveValue::Set("org".into()),
        ..Default::default()
    }
    .insert(conn)
    .await
    .expect("seed app");
    (app_id, org_id)
}

/// A second app inside an org that already exists. `seed_app_with_org` always
/// mints a fresh org, so it cannot express "two apps, one org" — which is the
/// only shape that distinguishes a correctly narrowed count from a saturated one.
async fn seed_app_into_org(conn: &DatabaseConnection, org_id: Uuid, app_id: Uuid) {
    let workspace_id = Uuid::new_v4();
    workspaces::ActiveModel {
        id: ActiveValue::Set(workspace_id),
        name: ActiveValue::Set("history".into()),
        org_id: ActiveValue::Set(Some(org_id)),
        ..Default::default()
    }
    .insert(conn)
    .await
    .expect("seed workspace");

    apps::ActiveModel {
        id: ActiveValue::Set(app_id),
        slug: ActiveValue::Set(format!("hist-{}", app_id.simple())),
        name: ActiveValue::Set("History".into()),
        org_id: ActiveValue::Set(org_id),
        project_id: ActiveValue::Set(workspace_id),
        branch: ActiveValue::Set("main".into()),
        source_repo: ActiveValue::Set("hist/test".into()),
        status: ActiveValue::Set("active".into()),
        source_type: ActiveValue::Set("s3".into()),
        source_config: ActiveValue::Set(serde_json::json!({})),
        visibility: ActiveValue::Set("org".into()),
        ..Default::default()
    }
    .insert(conn)
    .await
    .expect("seed app");
}

async fn add_sample(
    conn: &DatabaseConnection,
    app_id: Uuid,
    at: DateTime<FixedOffset>,
    bytes: i64,
) {
    app_storage_usage_samples::ActiveModel {
        app_id: ActiveValue::Set(app_id),
        measured_at: ActiveValue::Set(at),
        bytes: ActiveValue::Set(bytes),
        object_count: ActiveValue::Set(bytes / MB),
    }
    .insert(conn)
    .await
    .expect("insert sample");
}

/// A UTC instant `days_ago` days back.
///
/// **Known, accepted flake window:** this reads `Utc::now()` at insert time while
/// `load_history` recomputes it, so a test straddling UTC midnight between the
/// two shifts the window by a day and `points[0]` comes back empty. The gap is
/// milliseconds wide and closing it would mean threading a clock through the
/// production signature for a test's benefit. Noted here so a 00:00 UTC CI
/// failure is diagnosed as this rather than as a real regression.
fn utc(days_ago: i64, hour: u32, minute: u32) -> DateTime<FixedOffset> {
    let day = (Utc::now() - chrono::Duration::days(days_ago)).date_naive();
    Utc.from_utc_datetime(&day.and_hms_opt(hour, minute, 0).expect("valid time"))
        .into()
}

#[tokio::test]
async fn a_days_closing_reading_wins_and_earlier_ones_are_dropped() {
    let conn = test_db().await;
    let app_id = seed_app(&conn).await;

    // Three readings on the same UTC day. The sweeper really does write this
    // often — one per app per tick, every 15 minutes by default.
    add_sample(&conn, app_id, utc(1, 2, 0), 10 * MB).await;
    add_sample(&conn, app_id, utc(1, 11, 30), 20 * MB).await;
    add_sample(&conn, app_id, utc(1, 23, 45), 30 * MB).await;

    let points = load_history(&conn, Some(app_id), None, 2)
        .await
        .expect("history");

    assert_eq!(points.len(), 2, "one point per requested day");
    assert_eq!(
        points[0].bytes,
        30 * MB,
        "expected the 23:45 closing reading, not the first of the day and not a sum"
    );
}

#[tokio::test]
async fn a_pre_window_sample_still_sets_the_opening_day() {
    let conn = test_db().await;
    let app_id = seed_app(&conn).await;

    // Measured well before the window and never since — the app still holds
    // those bytes, so every day in range must report them.
    add_sample(&conn, app_id, utc(20, 6, 0), 7 * MB).await;

    let points = load_history(&conn, Some(app_id), None, 3)
        .await
        .expect("history");

    assert!(
        points.iter().all(|p| p.bytes == 7 * MB),
        "carry-forward lost: {points:?}"
    );
}

#[tokio::test]
async fn bucketing_is_utc_regardless_of_the_session_timezone() {
    // Takes the URL from `fresh_db` rather than reading OXY_DATABASE_URL
    // back out of the process env: that only resolves to THIS database because
    // nextest gives each test its own process, and under plain `cargo test` a
    // concurrent `test_db()` would point it at a different one.
    let (conn, url) = fresh_db(Schema::Central).await;
    let app_id = seed_app(&conn).await;

    // Asia/Bangkok is UTC+7 with no DST, so its day boundary is 17:00 UTC.
    //
    // The bug needs two samples in the same LOCAL bucket but on different UTC
    // days, so the later one displaces the earlier UTC day's close:
    //
    //   A  day-2 16:45 UTC  ->  local day-2 23:45   (local bucket L)
    //   B  day-2 23:45 UTC  ->  local day-1 06:45   (local bucket L+1)
    //   C  day-1 09:00 UTC  ->  local day-1 16:00   (local bucket L+1)
    //
    // Bucketing locally collapses L+1 to its latest row (C) and DROPS B — so UTC
    // day-2 falls back to A and reports 11 MiB. Bucketing in UTC keeps B as
    // day-2's close: 22 MiB.
    //
    // An earlier version of this test used two samples on the same UTC day at
    // 16:45 and 23:45. Those straddle the local boundary, so both survive either
    // bucketing and `daily_series` re-buckets to UTC and picks the close anyway —
    // the assertion held with AND without the fix. It passed for the wrong reason.
    add_sample(&conn, app_id, utc(2, 16, 45), 11 * MB).await;
    add_sample(&conn, app_id, utc(2, 23, 45), 22 * MB).await;
    add_sample(&conn, app_id, utc(1, 9, 0), 33 * MB).await;

    // A single-connection pool, with the timezone SET on it after connect.
    //
    // Three things had to be ruled out to get here, and each failed silently:
    //   * `SET TIME ZONE` on the shared `conn` — `DatabaseConnection` wraps a
    //     pool, so the GUC lands on whichever connection served the statement and
    //     `load_history` may acquire a different one.
    //   * a `?options=-c timezone=…` URL parameter — did nothing.
    //   * `ALTER DATABASE … SET TimeZone` — also nothing, because sqlx pins
    //     `timezone=UTC` in its connection startup parameters and that overrides
    //     the database default.
    //
    // So the SET has to happen *after* connect, and the pool has to be capped at
    // one connection for "after" to mean anything. The sanity check below exists
    // because every one of those attempts reported success while leaving the
    // session on UTC — at which point the whole test passes for the wrong reason.
    let mut opts = ConnectOptions::new(url);
    opts.max_connections(1).min_connections(1);
    let tz_conn = Database::connect(opts)
        .await
        .expect("connect a single-connection pool");
    tz_conn
        .execute_raw(Statement::from_string(
            tz_conn.get_database_backend(),
            "SET TIME ZONE 'Asia/Bangkok'".to_string(),
        ))
        .await
        .expect("set session timezone");

    // Sanity-check the premise: if this connection were UTC after all, the
    // assertion below would pass no matter what the query did.
    let tz: String = tz_conn
        .query_one_raw(Statement::from_string(
            tz_conn.get_database_backend(),
            "SHOW TimeZone".to_string(),
        ))
        .await
        .expect("read session timezone")
        .expect("one row")
        .try_get_by_index(0)
        .expect("decode timezone");
    assert_eq!(tz, "Asia/Bangkok", "session timezone did not take effect");

    let points = load_history(&tz_conn, Some(app_id), None, 3)
        .await
        .expect("history");

    assert_eq!(
        points[0].bytes,
        22 * MB,
        "session timezone leaked into the day bucket: UTC day-2's 23:45 close was \
         dropped and the 16:45 reading reported instead"
    );
    assert_eq!(points[1].bytes, 33 * MB, "UTC day-1 close");
}

#[tokio::test]
async fn a_fleet_query_sums_apps_and_carries_each_forward_independently() {
    let conn = test_db().await;
    let a = seed_app(&conn).await;
    let b = seed_app(&conn).await;

    add_sample(&conn, a, utc(2, 9, 0), 5 * MB).await;
    add_sample(&conn, b, utc(2, 9, 0), 3 * MB).await;
    // Only `a` is re-measured on the later day; `b` must not vanish from the total.
    add_sample(&conn, a, utc(1, 9, 0), 6 * MB).await;

    // Three days, so the window OPENS on the day both apps were measured. With
    // `days = 2` it would start yesterday and those readings would land in the
    // lookback instead — carried forward rather than plotted, which is correct
    // behaviour but not what this test is trying to observe.
    let points = load_history(&conn, None, None, 3).await.expect("history");

    // `test_db` creates a fresh database per test, so these two apps ARE the
    // fleet — assert absolute totals rather than a delta.
    assert_eq!(points[0].bytes, 8 * MB, "two days ago: a=5 + b=3");
    assert_eq!(
        points[1].bytes,
        9 * MB,
        "yesterday: a re-measured to 6, b carried forward at 3 — a dip here would \
         mean an unsampled app dropped out of the total"
    );
    assert_eq!(points[2].bytes, 9 * MB, "today: both carried forward");
}

#[tokio::test]
async fn an_org_scope_excludes_another_orgs_samples_from_the_fleet_total() {
    // The Storage tab is gated on `Action::PlatformApps`, which an App Operator
    // may hold bounded to specific orgs. Before this filter existed the fleet
    // series was every app's bytes regardless of grant, so a bounded operator
    // charted the whole company's storage.
    //
    // This is the assertion that fails if the `app_id IN (SELECT id FROM apps
    // WHERE org_id = ANY(..))` clause is dropped or its placeholder mis-numbered
    // — the handler-level check can't catch that, because the handler's job is
    // only to pass the scope down.
    let conn = test_db().await;
    let (mine, my_org) = seed_app_with_org(&conn).await;
    let theirs = seed_app(&conn).await;

    add_sample(&conn, mine, utc(1, 9, 0), 5 * MB).await;
    add_sample(&conn, theirs, utc(1, 9, 0), 100 * MB).await;

    let unbounded = load_history(&conn, None, None, 2).await.expect("history");
    assert_eq!(
        unbounded[1].bytes,
        105 * MB,
        "an unbounded grant still sees the whole fleet"
    );

    let scoped = load_history(&conn, None, Some(&[my_org]), 2)
        .await
        .expect("history");
    assert_eq!(
        scoped[1].bytes,
        5 * MB,
        "a grant bounded to one org must not see the other org's 100MB"
    );
}

#[tokio::test]
async fn an_empty_org_scope_matches_nothing_rather_than_everything() {
    // `Scope::Orgs(vec![])` is a real state — a grant bounded to no org yet. The
    // dangerous failure is an empty list degrading to "no filter": `IN ()` is a
    // syntax error in Postgres, which is why this uses `= ANY($n)` over an
    // array, and an empty array matches nothing. Getting this wrong hands a
    // zero-scope grant the entire fleet.
    let conn = test_db().await;
    let app_id = seed_app(&conn).await;
    add_sample(&conn, app_id, utc(1, 9, 0), 7 * MB).await;

    let scoped = load_history(&conn, None, Some(&[]), 2)
        .await
        .expect("history");
    assert!(
        scoped.iter().all(|p| p.bytes == 0),
        "a scope naming no orgs must yield an empty series, got {:?}",
        scoped.iter().map(|p| p.bytes).collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn an_org_scope_still_bounds_a_single_app_query() {
    // Both filters are applied, not one or the other — the handler 404s an
    // out-of-scope `?appId=` before reaching here, so this pins the belt to that
    // braces: even if the handler check regressed, the query would not serve
    // another org's series.
    let conn = test_db().await;
    let (_mine, my_org) = seed_app_with_org(&conn).await;
    let theirs = seed_app(&conn).await;
    add_sample(&conn, theirs, utc(1, 9, 0), 42 * MB).await;

    let scoped = load_history(&conn, Some(theirs), Some(&[my_org]), 2)
        .await
        .expect("history");
    assert!(
        scoped.iter().all(|p| p.bytes == 0),
        "an app id outside the scope must yield nothing, got {:?}",
        scoped.iter().map(|p| p.bytes).collect::<Vec<_>>()
    );
}

// ── The rollup-side scope filters ────────────────────────────────────────────
//
// `load_history` above covers the samples SQL. These cover the other three
// queries the scope fix touches, each of which lives in a handler or a sweep and
// is otherwise only reachable through HTTP with a forged principal or a real S3
// walk. Deleting any one of their filters is invisible to every other test in
// the suite while a bounded App Operator gets back the whole fleet.

/// A rollup row for `app_id` in `org_id`, so the fleet queries have something to
/// rank. Values are arbitrary — these tests assert on *which rows come back*.
async fn add_usage_row(conn: &DatabaseConnection, app_id: Uuid, org_id: Uuid, bytes: i64) {
    entity::app_storage_usage::ActiveModel {
        app_id: ActiveValue::Set(app_id),
        org_id: ActiveValue::Set(org_id),
        bytes: ActiveValue::Set(bytes),
        object_count: ActiveValue::Set(1),
        untagged_bytes: ActiveValue::Set(0),
        untagged_object_count: ActiveValue::Set(0),
        prefix_breakdown: ActiveValue::Set(None),
        measured_at: ActiveValue::Set(Utc::now().into()),
        measure_status: ActiveValue::Set("ok".into()),
        measure_detail: ActiveValue::Set(None),
    }
    .insert(conn)
    .await
    .expect("insert usage row");
}

#[tokio::test]
async fn the_fleet_rollup_query_returns_only_the_scoped_orgs_rows() {
    // This is the blocking finding itself: the fleet table listed every org's
    // app names, slugs and byte totals to a grant bounded to one org.
    let conn = test_db().await;
    let (mine, my_org) = seed_app_with_org(&conn).await;
    let (theirs, their_org) = seed_app_with_org(&conn).await;
    add_usage_row(&conn, mine, my_org, 5 * MB).await;
    add_usage_row(&conn, theirs, their_org, 100 * MB).await;

    let unbounded = fleet_rows_scoped(&conn, None).await.expect("fleet rows");
    assert_eq!(unbounded.len(), 2, "an unbounded grant sees both");
    assert_eq!(
        unbounded[0].bytes,
        100 * MB,
        "ordered by bytes descending, so the header's ranking is the query's"
    );

    let scoped = fleet_rows_scoped(&conn, Some(&[my_org]))
        .await
        .expect("fleet rows");
    assert_eq!(
        scoped.iter().map(|r| r.app_id).collect::<Vec<_>>(),
        vec![mine],
        "a grant bounded to one org must not see the other org's row"
    );

    assert!(
        fleet_rows_scoped(&conn, Some(&[]))
            .await
            .expect("fleet rows")
            .is_empty(),
        "a scope naming no orgs must not degrade to no filter"
    );
}

#[tokio::test]
async fn unmeasured_app_count_narrows_both_of_its_counts() {
    // The subtle half. This is `total - measured`, so narrowing only the total
    // subtracts a fleet-wide measured count from a scoped one, saturates to 0,
    // and reports "everything is measured" to the operator who most needs to
    // know it isn't — a wrong answer that looks like good news.
    let conn = test_db().await;
    let (mine_measured, my_org) = seed_app_with_org(&conn).await;
    add_usage_row(&conn, mine_measured, my_org, MB).await;
    // A second app in the SAME org, deliberately unmeasured.
    let mine_unmeasured = Uuid::new_v4();
    seed_app_into_org(&conn, my_org, mine_unmeasured).await;
    // Three more measured apps elsewhere, enough that a fleet-wide `measured`
    // would exceed this org's total and saturate the subtraction to zero.
    for _ in 0..3 {
        let (other, other_org) = seed_app_with_org(&conn).await;
        add_usage_row(&conn, other, other_org, MB).await;
    }

    assert_eq!(
        unmeasured_app_count(&conn, Some(&[my_org]))
            .await
            .expect("count"),
        1,
        "one of this org's two apps is unmeasured; a saturated 0 here means \
         only one of the two counts was narrowed"
    );
    assert_eq!(
        unmeasured_app_count(&conn, None).await.expect("count"),
        1,
        "unbounded: 5 apps, 4 measured"
    );
}

#[tokio::test]
async fn the_sweep_batch_only_contains_apps_the_grant_reaches() {
    // Cost, not disclosure — but a grant bounded to one org kicking off a walk
    // over every app in the fleet is the same missing check.
    let conn = test_db().await;
    let (_mine, my_org) = seed_app_with_org(&conn).await;
    let (_theirs, _their_org) = seed_app_with_org(&conn).await;

    let unbounded = apps_due(&conn, 100, None).await.expect("apps due");
    assert_eq!(unbounded.len(), 2);

    let scoped = apps_due(&conn, 100, Some(&[my_org]))
        .await
        .expect("apps due");
    assert_eq!(
        scoped.iter().map(|a| a.org_id).collect::<Vec<_>>(),
        vec![my_org],
        "the walk must not reach outside the grant"
    );
}
