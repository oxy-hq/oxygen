//! The Activity tab reads a workspace's Airhouse once its usage history has been
//! copied there, and Postgres until then — and both stores give the same
//! numbers.
//!
//! The database is a real Postgres (`common::test_db`); the workspace's
//! Airhouse is `usage_fixtures::FakeAirhouse`, one in-memory DuckDB per
//! workspace that runs the SQL the sink and the reads send unchanged. What the
//! fake cannot say is whether DuckLake accepts that SQL, so the DDL and the
//! read relation are also checked for keys and indexes in `oxy-custom-apps`.
//!
//! | Test | Proves |
//! | ---- | ------ |
//! | `usage_backfill_copies_history_and_a_rerun_writes_nothing` | one pass copies every row; a second pass reads nothing; a forced second pass over the whole history writes no row |
//! | `usage_backfill_does_not_duplicate_a_row_the_drain_already_wrote` | a fact the drain wrote with nanoseconds is the same row as its Postgres copy |
//! | `usage_backfill_resumes_where_a_failed_pass_stopped` | a failed insert leaves the position after the last good batch; the next pass finishes with no duplicate |
//! | `usage_backfill_pauses_on_its_budget_and_finishes_later` | a pass out of time saves its place and the workspace keeps reading Postgres |
//! | `usage_backfill_skips_a_workspace_with_no_airhouse` | no control row; the Activity tab reads Postgres and never asks Airhouse |
//! | `usage_reads_activity_is_identical_in_postgres_and_airhouse` | summary, visitors, event counts and drill-down, before and after the copy |
//! | `usage_reads_lists_and_vitals_are_identical_in_postgres_and_airhouse` | the app list's last-active, each app's last customer view, who viewed each app per window (the platform facts' inputs) and the release vitals, before and after the copy |
//! | `usage_reads_vitals_fall_back_when_airhouse_is_slow` | a vitals read whose Airhouse is slower than its deadline answers from Postgres in time, and the workspace is not asked again for a while |
//! | `usage_reads_fall_back_to_postgres_when_airhouse_cannot_answer` | a finished workspace whose Airhouse is down still answers, from Postgres, and is not asked again for a while |
//! | `usage_sdk_events_are_mirrored_to_airhouse_by_the_drain` | `record_event` writes Postgres and the drain appends the event to Airhouse |
//! | `usage_backfill_sweep_tries_the_longest_unattempted_workspace_first` | a workspace whose passes keep failing sorts behind the others |
//! | `usage_backfill_sweep_queues_one_task_per_unfinished_workspace` | the sweep queues a task for a workspace with an Airhouse and none for one that finished |
//! | `usage_counting_sql_in_the_docs_is_the_sql_that_runs` | the two counts `data-placement.md` quotes are the code's text and count apps without an Airhouse and apps not yet copied |
//! | `usage_backfill_kind_is_handed_to_the_fleet_and_is_a_platform_daemon` | the kind is registered by `oxy-server` and listed as a daemon where the run feed and workspace health look |

use std::time::Duration;

use axum::Json;
use axum::extract::{Path, Query};
use chrono::Utc;
use entity::{apps, custom_app_event, custom_app_view_event, organizations, users, workspaces};
use oxy_api_custom_apps::custom_apps_activity::{
    EventsQuery, VisitorsQuery, get_events, get_summary, get_visitors,
};
use oxy_api_custom_apps::custom_apps_usage_backfill::{
    USAGE_BACKFILL_KIND, is_pending, queue_unfinished,
};
use oxy_custom_apps::usage_airhouse::{UsageFact, UsageSink};
use oxy_custom_apps::usage_backfill::{self, BackfillError, Pass};
use oxy_custom_apps::usage_fixtures::{Cast, FakeAirhouse, History, history};
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ConnectionTrait, DatabaseBackend, DatabaseConnection,
    EntityTrait, QueryOrder, Statement,
};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::common::{Schema, test_db, test_db_with};

const BUDGET: Duration = Duration::from_secs(60);

struct World {
    db: DatabaseConnection,
    workspace_id: Uuid,
    org_id: Uuid,
    app_id: Uuid,
    cast: Cast,
    history: History,
}

impl World {
    /// Every row of the fixture history, staging and sandbox included.
    fn rows(&self) -> i64 {
        (self.history.views.len() + self.history.events.len()) as i64
    }
}

/// An org, a workspace, an app and four users, with the fixture history in
/// Postgres, ending now.
async fn world(db: DatabaseConnection) -> World {
    let org_id = Uuid::new_v4();
    organizations::ActiveModel {
        id: Set(org_id),
        name: Set("Usage Org".into()),
        slug: Set(format!("usage-org-{org_id}")),
        ..Default::default()
    }
    .insert(&db)
    .await
    .expect("seed org");
    let workspace_id = Uuid::new_v4();
    workspaces::ActiveModel {
        id: Set(workspace_id),
        name: Set("Usage Workspace".into()),
        org_id: Set(Some(org_id)),
        ..Default::default()
    }
    .insert(&db)
    .await
    .expect("seed workspace");
    let app_id = Uuid::new_v4();
    apps::ActiveModel {
        id: Set(app_id),
        slug: Set(format!("usage-app-{app_id}")),
        name: Set("Usage App".into()),
        org_id: Set(org_id),
        project_id: Set(workspace_id),
        branch: Set("main".into()),
        source_repo: Set("usage/test".into()),
        status: Set("active".into()),
        source_type: Set("s3".into()),
        source_config: Set(json!({})),
        published_at: Set(Some(Utc::now().into())),
        ..Default::default()
    }
    .insert(&db)
    .await
    .expect("seed app");
    let cast = Cast::new(app_id);
    for (id, email) in &cast.users {
        users::ActiveModel {
            id: Set(*id),
            email: Set(Some(email.clone())),
            name: Set("Usage Visitor".into()),
            picture: Set(None),
            email_verified: Set(true),
            ..Default::default()
        }
        .insert(&db)
        .await
        .expect("seed user");
    }
    let history = history(&cast, Utc::now());
    for view in history.views.clone() {
        view.insert(&db).await.expect("seed view");
    }
    for event in history.events.clone() {
        event.insert(&db).await.expect("seed event");
    }
    World {
        db,
        workspace_id,
        org_id,
        app_id,
        cast,
        history,
    }
}

/// A fake Airhouse for the world's workspace, installed as the process's sink.
fn airhouse(world: &World) -> std::sync::Arc<FakeAirhouse> {
    let fake = FakeAirhouse::new();
    fake.with_airhouse(world.workspace_id);
    fake.install();
    fake
}

async fn backfill(world: &World, fake: &FakeAirhouse) -> Pass {
    usage_backfill::run_with(&world.db, fake, world.workspace_id, BUDGET, 5)
        .await
        .expect("a pass")
}

/// Everything the Activity tab shows for the app, as the API serves it.
async fn tab(app_id: Uuid) -> Value {
    let Json(summary) = get_summary(Path(app_id)).await.expect("summary");
    let Json(visitors) = get_visitors(Path(app_id), Query(VisitorsQuery { days: 7, limit: 50 }))
        .await
        .expect("visitors");
    let events = |name: Option<&str>| EventsQuery {
        days: 7,
        event_name: name.map(str::to_string),
        limit: 50,
    };
    let Json(groups) = get_events(Path(app_id), Query(events(None)))
        .await
        .expect("event counts");
    let Json(occurrences) = get_events(Path(app_id), Query(events(Some("export-clicked"))))
        .await
        .expect("occurrences");
    json!({
        "summary": serde_json::to_value(summary).unwrap(),
        "visitors": serde_json::to_value(visitors).unwrap(),
        "groups": serde_json::to_value(groups).unwrap(),
        "occurrences": serde_json::to_value(occurrences).unwrap(),
    })
}

/// What the fixture history is worth to the tab, written out rather than read
/// back from either store: a reader wrong in the same way in both would
/// otherwise pass.
fn assert_the_fixtures_numbers(tab: &Value, world: &World) {
    let summary = &tab["summary"];
    assert_eq!(
        summary["total_views_7d"], 7,
        "production views since 7 days"
    );
    assert_eq!(summary["unique_users_7d"], 3);
    assert_eq!(summary["total_events_7d"], 7);
    let newest_production_view = world
        .history
        .views
        .iter()
        .filter(|v| v.environment.as_ref() == "production")
        .map(|v| *v.viewed_at.as_ref())
        .max()
        .unwrap();
    assert_eq!(
        summary["last_viewed_at"],
        serde_json::to_value(newest_production_view).unwrap(),
        "the last view, at the microsecond"
    );

    let visitors = tab["visitors"]["rows"].as_array().unwrap();
    let by_email: Vec<(&str, i64, i64)> = visitors
        .iter()
        .map(|v| {
            (
                v["user_email"].as_str().unwrap(),
                v["views"].as_i64().unwrap(),
                v["sessions"].as_i64().unwrap(),
            )
        })
        .collect();
    let [(_, u1), (_, u2), (_, u3), _] = &world.cast.users;
    assert_eq!(
        by_email,
        vec![
            (u1.as_str(), 3, 2),
            (u2.as_str(), 3, 1),
            (u3.as_str(), 1, 1)
        ],
        "most recent first; staging, sandbox and older views are not visits"
    );
    // The latest *recorded* role: the second visitor's newest view recorded
    // none, the third's never did.
    assert_eq!(visitors[0]["app_role"], "admin");
    assert_eq!(visitors[0]["org_role"], "owner");
    assert_eq!(visitors[1]["app_role"], "member");
    assert_eq!(visitors[1]["org_role"], "member");
    assert_eq!(visitors[2]["app_role"], Value::Null);

    let groups: Vec<(&str, i64)> = tab["groups"]["groups"]
        .as_array()
        .unwrap()
        .iter()
        .map(|g| {
            (
                g["event_name"].as_str().unwrap(),
                g["count"].as_i64().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        groups,
        vec![
            ("export-clicked", 3),
            ("oxy-error", 3),
            ("filter-applied", 1)
        ]
    );
    let payloads: Vec<&Value> = tab["occurrences"]["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| &o["payload"])
        .collect();
    assert_eq!(
        payloads,
        vec![
            &json!({ "format": "csv" }),
            &json!({ "format": "xlsx", "rows": 12 }),
            &json!({}),
        ],
        "newest first"
    );
}

/// One pass copies every row of both tables, whatever its environment, and a
/// workspace that finished is not read again. Resetting the row by hand and
/// walking the whole history a second time writes nothing: the insert finds
/// every id.
#[tokio::test]
async fn usage_backfill_copies_history_and_a_rerun_writes_nothing() {
    let world = world(test_db().await).await;
    let fake = airhouse(&world);

    assert_eq!(
        backfill(&world, &fake).await,
        Pass::Finished {
            rows: world.rows() as u64
        }
    );
    assert_eq!(fake.row_count(world.workspace_id), world.rows());
    assert_eq!(fake.distinct_ids(world.workspace_id), world.rows());
    let row = usage_backfill::state(&world.db, world.workspace_id)
        .await
        .unwrap()
        .expect("the control row");
    assert!(row.finished_at.is_some());
    assert_eq!(row.rows_copied, world.rows());
    let appends = fake.appends();

    // Finished: nothing is read, nothing is written.
    assert_eq!(backfill(&world, &fake).await, Pass::AlreadyFinished);
    assert_eq!(fake.appends(), appends);
    assert_eq!(fake.row_count(world.workspace_id), world.rows());

    // Forced over the whole history again: every batch is handed to the insert,
    // none of it lands.
    world
        .db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "UPDATE custom_app_usage_backfill \
             SET finished_at = NULL, cursor_kind = NULL, cursor_app_id = NULL, \
                 cursor_at = NULL, cursor_id = NULL \
             WHERE workspace_id = $1",
            [world.workspace_id.into()],
        ))
        .await
        .unwrap();
    assert_eq!(
        backfill(&world, &fake).await,
        Pass::Finished {
            rows: world.rows() as u64
        }
    );
    assert_eq!(
        fake.row_count(world.workspace_id),
        world.rows(),
        "a second walk over the history writes zero rows"
    );
    assert_eq!(fake.distinct_ids(world.workspace_id), world.rows());
}

/// The drain writes a fact with the clock's nanoseconds; Postgres keeps the
/// microseconds. They are one row, not two.
#[tokio::test]
async fn usage_backfill_does_not_duplicate_a_row_the_drain_already_wrote() {
    let world = world(test_db().await).await;
    let fake = airhouse(&world);
    fake.prepare(&world.db, world.workspace_id).await.unwrap();

    let view = custom_app_view_event::Entity::find()
        .order_by_asc(custom_app_view_event::Column::ViewedAt)
        .one(&world.db)
        .await
        .unwrap()
        .unwrap();
    let mut written = UsageFact::from_view_model(&view, world.org_id, Utc::now());
    written.occurred_at += chrono::Duration::nanoseconds(789);
    let event = custom_app_event::Entity::find()
        .order_by_desc(custom_app_event::Column::OccurredAt)
        .one(&world.db)
        .await
        .unwrap()
        .unwrap();
    let mut written_event = UsageFact::from_event_model(&event, world.org_id, Utc::now());
    written_event.occurred_at += chrono::Duration::nanoseconds(999);
    fake.append(&world.db, world.workspace_id, &[written, written_event])
        .await
        .unwrap();
    assert_eq!(fake.row_count(world.workspace_id), 2);

    backfill(&world, &fake).await;
    assert_eq!(fake.row_count(world.workspace_id), world.rows());
    assert_eq!(fake.distinct_ids(world.workspace_id), world.rows());
}

/// An insert that fails mid-history fails the pass, and the saved position is
/// after the last batch that landed. The next pass carries on from it.
#[tokio::test]
async fn usage_backfill_resumes_where_a_failed_pass_stopped() {
    let world = world(test_db().await).await;
    let fake = airhouse(&world);

    fake.fail_appends_after(1);
    let failed = usage_backfill::run_with(&world.db, fake.as_ref(), world.workspace_id, BUDGET, 5)
        .await
        .expect_err("the second insert fails");
    assert!(matches!(failed, BackfillError::Sink(_)), "{failed}");
    let row = usage_backfill::state(&world.db, world.workspace_id)
        .await
        .unwrap()
        .unwrap();
    assert!(row.finished_at.is_none());
    assert_eq!(row.rows_copied, 5, "one batch of five landed");
    assert_eq!(row.cursor_kind.as_deref(), Some("view"));
    assert_eq!(fake.row_count(world.workspace_id), 5);

    fake.stop_failing_appends();
    assert_eq!(
        backfill(&world, &fake).await,
        Pass::Finished {
            rows: world.rows() as u64 - 5
        },
        "the second pass hands over only what the first did not"
    );
    assert_eq!(fake.row_count(world.workspace_id), world.rows());
    assert_eq!(fake.distinct_ids(world.workspace_id), world.rows());
    let row = usage_backfill::state(&world.db, world.workspace_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.rows_copied, world.rows());
    assert!(row.finished_at.is_some());
}

/// A pass whose time is up saves its place; until a pass finishes, the tab
/// keeps reading Postgres.
#[tokio::test]
async fn usage_backfill_pauses_on_its_budget_and_finishes_later() {
    let world = world(test_db().await).await;
    let fake = airhouse(&world);

    let paused = usage_backfill::run_with(
        &world.db,
        fake.as_ref(),
        world.workspace_id,
        Duration::ZERO,
        5,
    )
    .await
    .unwrap();
    assert_eq!(paused, Pass::Paused { rows: 0 });
    let row = usage_backfill::state(&world.db, world.workspace_id)
        .await
        .unwrap()
        .expect("the pass was begun");
    assert!(row.finished_at.is_none());
    assert_eq!(fake.row_count(world.workspace_id), 0);

    assert_the_fixtures_numbers(&tab(world.app_id).await, &world);
    assert_eq!(fake.queries(), 0, "unfinished: Postgres answers");

    backfill(&world, &fake).await;
    assert_the_fixtures_numbers(&tab(world.app_id).await, &world);
    assert!(fake.queries() > 0, "finished: Airhouse answers");
}

/// A workspace without an Airhouse is never marked, never asked, and keeps
/// reading Postgres.
#[tokio::test]
async fn usage_backfill_skips_a_workspace_with_no_airhouse() {
    let world = world(test_db().await).await;
    // An Airhouse exists, for some other workspace.
    let fake = FakeAirhouse::new();
    fake.with_airhouse(Uuid::new_v4());
    fake.install();

    assert_eq!(backfill(&world, &fake).await, Pass::NoAirhouse);
    assert!(
        usage_backfill::state(&world.db, world.workspace_id)
            .await
            .unwrap()
            .is_none(),
        "nothing is recorded for a workspace that cannot be copied"
    );
    assert_the_fixtures_numbers(&tab(world.app_id).await, &world);
    assert_eq!(
        fake.queries(),
        0,
        "a workspace with no Airhouse reads Postgres"
    );
}

/// The same history in both stores gives the tab the same answer, to the
/// microsecond, for all four reads — and the answer is the one the fixture is
/// worth.
#[tokio::test]
async fn usage_reads_activity_is_identical_in_postgres_and_airhouse() {
    let world = world(test_db().await).await;
    let fake = airhouse(&world);

    let from_postgres = tab(world.app_id).await;
    assert_the_fixtures_numbers(&from_postgres, &world);
    assert_eq!(fake.queries(), 0);

    backfill(&world, &fake).await;
    let from_airhouse = tab(world.app_id).await;
    assert!(fake.queries() >= 4, "Airhouse answered all four reads");
    assert_the_fixtures_numbers(&from_airhouse, &world);
    assert_eq!(from_airhouse, from_postgres);
}

/// Production web-vitals events for `build`: two `/home` loads with every
/// vital in bounds, one `/orders` load whose LCP is over a minute and whose INP
/// is not a number, and four rows no release counts: another build, a path
/// that is not a string, staging, and a load after the release ended.
async fn seed_vitals(world: &World, build: Uuid, other: Uuid) {
    let [(u1, e1), (u2, e2), (u3, e3), _] = world.cast.users.clone();
    let now = Utc::now();
    let rows = [
        (
            (u1, &e1),
            json!({ "path": "/home", "build": build.to_string(), "lcp": 1200, "inp": 80, "cls": 50 }),
            1,
            "production",
        ),
        (
            (u2, &e2),
            json!({ "path": "/home", "build": build.to_string(), "lcp": 2500, "inp": 200, "cls": 100 }),
            2,
            "production",
        ),
        (
            (u3, &e3),
            json!({ "path": "/orders", "build": build.to_string(), "lcp": 61000, "inp": "fast", "cls": 10 }),
            3,
            "production",
        ),
        (
            (u1, &e1),
            json!({ "path": "/home", "build": other.to_string(), "lcp": 9000 }),
            4,
            "production",
        ),
        (
            (u1, &e1),
            json!({ "path": 7, "build": build.to_string(), "lcp": 9000 }),
            5,
            "production",
        ),
        (
            (u2, &e2),
            json!({ "path": "/home", "build": build.to_string(), "lcp": 9000 }),
            6,
            "staging",
        ),
    ];
    for ((user_id, email), payload, hours, environment) in rows {
        custom_app_event::ActiveModel {
            id: Set(Uuid::new_v4()),
            app_id: Set(world.app_id),
            user_id: Set(user_id),
            user_email: Set(email.clone()),
            session_id: Set(Uuid::new_v4()),
            event_name: Set("oxy-web-vitals".into()),
            payload: Set(payload),
            occurred_at: Set((now - chrono::Duration::hours(hours)
                + chrono::Duration::microseconds(123_457))
            .fixed_offset()),
            environment: Set(environment.into()),
        }
        .insert(&world.db)
        .await
        .expect("seed a vitals event");
    }
}

/// What the app list, the platform facts and the release timeline read about
/// the world's app, as JSON. The release is `[now - 10h, now - 30min)`.
async fn lists_and_vitals(world: &World, build: Uuid) -> Value {
    use oxy_api_custom_apps::custom_apps_activity::last_active_at_by_app;
    use oxy_custom_apps::usage_reads::{
        self, Customers, ReportApp, VitalsStretch, last_views, viewers_by_window,
    };
    let now = Utc::now();
    let app = [ReportApp {
        app_id: world.app_id,
        workspace_id: world.workspace_id,
    }];
    let last_active = last_active_at_by_app(&world.db, &app)
        .await
        .expect("last active");
    let staff = [world.cast.users[0].0];
    let customers = Customers {
        staff: &staff,
        since: now - chrono::Duration::days(7),
    };
    let last = last_views(&world.db, &app, Some(customers))
        .await
        .expect("last views");
    let windows = [
        (now - chrono::Duration::days(7), now),
        (
            now - chrono::Duration::days(14),
            now - chrono::Duration::days(7),
        ),
    ];
    let viewers = viewers_by_window(&world.db, &app, &windows)
        .await
        .expect("viewers");
    let vitals = usage_reads::release_vitals(
        &world.db,
        world.app_id,
        &[VitalsStretch {
            idx: 0,
            build_id: build,
            starts_at: now - chrono::Duration::hours(10),
            ends_at: now - chrono::Duration::minutes(30),
        }],
        std::time::Duration::from_secs(10),
        4_000,
    )
    .await
    .expect("vitals");
    let user = |id: Uuid| {
        world
            .cast
            .users
            .iter()
            .position(|(u, _)| *u == id)
            .map(|i| i + 1)
    };
    let mut viewers: Vec<(usize, Option<usize>, i64)> = viewers
        .iter()
        .map(|v| (v.window, user(v.user_id), v.views))
        .collect();
    viewers.sort();
    json!({
        "last_active": last_active.get(&world.app_id).map(|t| t.to_rfc3339()),
        "anyone": last.get(&world.app_id).map(|l| l.anyone.to_rfc3339()),
        "customer": last.get(&world.app_id).and_then(|l| l.customer).map(|t| t.to_rfc3339()),
        "viewers": viewers,
        "vitals": vitals
            .iter()
            .map(|r| json!([r.idx, r.path, r.samples, r.lcp_p75, r.inp_p75, r.cls_p75, r.rank]))
            .collect::<Vec<_>>(),
    })
}

/// The reads the app list, the platform facts and the release timeline make
/// give the same answer from both stores, and the answer is the one the
/// fixture is worth, written out rather than read back.
#[tokio::test]
async fn usage_reads_lists_and_vitals_are_identical_in_postgres_and_airhouse() {
    let world = world(test_db().await).await;
    let fake = airhouse(&world);
    let build = Uuid::new_v4();
    seed_vitals(&world, build, Uuid::new_v4()).await;

    let from_postgres = lists_and_vitals(&world, build).await;
    assert_eq!(fake.queries(), 0);
    let newest = |environment: &str, skip: Option<Uuid>| {
        world
            .history
            .views
            .iter()
            .filter(|v| v.environment.as_ref() == environment)
            .filter(|v| Some(*v.user_id.as_ref()) != skip)
            .map(|v| v.viewed_at.as_ref().with_timezone(&Utc).to_rfc3339())
            .max()
    };
    assert_eq!(
        from_postgres["last_active"],
        json!(newest("production", None))
    );
    assert_eq!(from_postgres["anyone"], from_postgres["last_active"]);
    assert_eq!(
        from_postgres["customer"],
        json!(newest("production", Some(world.cast.users[0].0))),
        "the newest view by someone who is not staff"
    );
    assert_eq!(
        from_postgres["viewers"],
        json!([
            // This week: user 1 three times, user 2 three, user 3 once.
            [0, 1, 3],
            [0, 2, 3],
            [0, 3, 1],
            // The week before: users 1 and 2 once, user 4 twice.
            [1, 1, 1],
            [1, 2, 1],
            [1, 4, 2],
        ])
    );
    assert_eq!(
        from_postgres["vitals"],
        json!([
            // p75 of two: lower + 0.75 * (upper - lower); 87.5 rounds to 88.
            [0, "/home", 2, 2175, 170, 88, 1],
            // LCP over a minute and an INP that is not a number are no samples.
            [0, "/orders", 1, null, null, 10, 2],
        ])
    );

    backfill(&world, &fake).await;
    let from_airhouse = lists_and_vitals(&world, build).await;
    assert!(fake.queries() >= 4, "Airhouse answered every read");
    assert_eq!(from_airhouse, from_postgres);
}

/// A slow Airhouse runs out of the vitals read's own deadline, well inside the
/// caller's, and the read falls back to Postgres; the workspace is then left
/// alone for a while rather than costing every load the same wait.
#[tokio::test]
async fn usage_reads_vitals_fall_back_when_airhouse_is_slow() {
    use oxy_custom_apps::usage_reads::{VitalsStretch, release_vitals};
    let world = world(test_db().await).await;
    let fake = airhouse(&world);
    let build = Uuid::new_v4();
    seed_vitals(&world, build, Uuid::new_v4()).await;
    backfill(&world, &fake).await;
    let now = Utc::now();
    let stretch = [VitalsStretch {
        idx: 0,
        build_id: build,
        starts_at: now - chrono::Duration::hours(10),
        ends_at: now - chrono::Duration::minutes(30),
    }];
    let read = || {
        release_vitals(
            &world.db,
            world.app_id,
            &stretch,
            Duration::from_millis(200),
            4_000,
        )
    };
    let from_airhouse = read().await.expect("vitals");
    let asked = fake.queries();

    fake.delay_queries(Duration::from_secs(5));
    let started = std::time::Instant::now();
    let fallen_back = read().await.expect("vitals");
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "did not wait out Airhouse"
    );
    assert_eq!(fallen_back, from_airhouse, "Postgres gives the same answer");
    assert_eq!(fake.queries(), asked + 1);
    read().await.expect("vitals");
    assert_eq!(fake.queries(), asked + 1, "left alone after the timeout");
}

/// Postgres still has every row, so a finished workspace whose Airhouse cannot
/// be read answers from there rather than failing the tab.
#[tokio::test]
async fn usage_reads_fall_back_to_postgres_when_airhouse_cannot_answer() {
    let world = world(test_db().await).await;
    let fake = airhouse(&world);
    backfill(&world, &fake).await;

    fake.fail_queries(true);
    let first = tab(world.app_id).await;
    assert_eq!(fake.queries(), 1, "Airhouse was asked once, and failed");
    assert_the_fixtures_numbers(&first, &world);
    // Then it is left alone for a while: the other reads, and the next load of
    // the tab, do not wait out its timeout.
    let again = tab(world.app_id).await;
    assert_eq!(fake.queries(), 1);
    assert_eq!(again, first);
}

/// An SDK event is written to Postgres by `record_event` and reaches Airhouse
/// through the drain, in a batch with the beacon's events.
#[tokio::test]
async fn usage_sdk_events_are_mirrored_to_airhouse_by_the_drain() {
    let world = world(test_db().await).await;
    let fake = airhouse(&world);
    fake.prepare(&world.db, world.workspace_id).await.unwrap();
    let (user, email) = world.cast.users[0].clone();

    let id = oxy_api_custom_apps::custom_apps_tracking::record_event(
        &world.db,
        world.app_id,
        user,
        email,
        Uuid::new_v4(),
        "export-clicked".into(),
        json!({ "format": "pdf" }),
        "production",
    )
    .await
    .expect("recorded");
    assert_eq!(fake.row_count(world.workspace_id), 0, "not yet drained");
    oxy_api_custom_apps::custom_apps_usage_queue::flush().await;

    assert_eq!(fake.row_count(world.workspace_id), 1);
    let rows = fake
        .query(
            &world.db,
            world.workspace_id,
            "SELECT event_id, kind FROM oxy_custom_app_usage",
        )
        .await
        .unwrap();
    let row = rows.iter().next().unwrap();
    assert_eq!(row.uuid("event_id").unwrap(), id);
    assert_eq!(row.text("kind").unwrap(), "event");
}

/// The sweep queues one task for a workspace that has an Airhouse and is not
/// finished, never a second while it waits, and none once it has finished.
#[tokio::test]
async fn usage_backfill_sweep_queues_one_task_per_unfinished_workspace() {
    let world = world(test_db_with(Schema::All).await).await;
    let fake = airhouse(&world);
    // The tenant row is what says this workspace has an Airhouse.
    world
        .db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "INSERT INTO airhouse_tenants \
               (id, workspace_id, airhouse_tenant_id, bucket, status, created_at) \
             VALUES ($1, $2, $3, 'bucket', 'active', now())",
            [
                Uuid::new_v4().into(),
                world.workspace_id.into(),
                format!("tenant-{}", world.workspace_id).into(),
            ],
        ))
        .await
        .expect("seed tenant");

    assert_eq!(queue_unfinished(&world.db).await.unwrap(), 1);
    assert!(is_pending(&world.db, world.workspace_id).await.unwrap());
    assert_eq!(
        queue_unfinished(&world.db).await.unwrap(),
        0,
        "a task already waiting is not queued again"
    );
    let queued = world
        .db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT count(*) AS n FROM agentic_task_queue \
             WHERE spec->>'kind' = $1 AND spec->'payload'->>'workspace_id' = $2",
            [
                USAGE_BACKFILL_KIND.into(),
                world.workspace_id.to_string().into(),
            ],
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get::<i64>("", "n")
        .unwrap();
    assert_eq!(queued, 1);

    // Once finished, the workspace needs no task.
    world
        .db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "DELETE FROM agentic_task_queue WHERE spec->>'kind' = $1",
            [USAGE_BACKFILL_KIND.into()],
        ))
        .await
        .unwrap();
    backfill(&world, &fake).await;
    assert_eq!(queue_unfinished(&world.db).await.unwrap(), 0);
}

/// The sweep tries the workspace attempted longest ago first, so one whose
/// passes keep failing is retried behind the others, not instead of them.
#[tokio::test]
async fn usage_backfill_sweep_tries_the_longest_unattempted_workspace_first() {
    let db = test_db_with(Schema::All).await;
    let older = world(db.clone()).await;
    let newer = world(db.clone()).await;
    for (i, w) in [&older, &newer].into_iter().enumerate() {
        db.execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "INSERT INTO airhouse_tenants \
               (id, workspace_id, airhouse_tenant_id, bucket, status, created_at) \
             VALUES ($1, $2, $3, 'bucket', 'active', now() + make_interval(secs => $4))",
            [
                Uuid::new_v4().into(),
                w.workspace_id.into(),
                format!("tenant-{}", w.workspace_id).into(),
                (i as f64).into(),
            ],
        ))
        .await
        .expect("seed tenant");
    }
    let order = || async {
        usage_backfill::unfinished_workspaces(&db, 10)
            .await
            .unwrap()
    };
    assert_eq!(order().await, vec![older.workspace_id, newer.workspace_id]);

    // A pass queued (and failed) for the older one: the other goes first.
    usage_backfill::note_attempt(&db, older.workspace_id)
        .await
        .unwrap();
    assert_eq!(order().await, vec![newer.workspace_id, older.workspace_id]);
    usage_backfill::note_attempt(&db, newer.workspace_id)
        .await
        .unwrap();
    assert_eq!(order().await, vec![older.workspace_id, newer.workspace_id]);
}

/// The worker fleet runs the copy only if `oxy-server` hands in its kind, and a
/// failed copy must not mark a customer's workspace unhealthy or fill the run
/// feed: the kind is a platform daemon in every list that tells one from the
/// tenant's own work. Source scans — the registry and two of the lists are
/// private to their crates.
#[test]
fn usage_backfill_kind_is_handed_to_the_fleet_and_is_a_platform_daemon() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let read = |file: &str| std::fs::read_to_string(root.join(file)).expect(file);

    let seams = read("../server/src/custom_apps_seams.rs");
    let from = seams.find("fn tasks(").expect("`tasks` is gone");
    let tasks = &seams[from..];
    let tasks: String = tasks[..tasks.find("fn background(").unwrap()]
        .split_whitespace()
        .collect();
    assert!(
        tasks.contains(
            "TaskSeam{kind:USAGE_BACKFILL_KIND,\
             build:|deps|{Arc::new(custom_apps_usage_backfill::UsageBackfillExecutor{db:deps.db.clone(),})},}"
        ),
        "`custom_apps_seams::tasks` no longer hands in the usage backfill's executor"
    );

    for (file, list) in [
        (
            "../agentic/runtime/src/lifecycle/crud/queries.rs",
            "pub const SYSTEM_SOURCE_TYPES: &[&str] = &[",
        ),
        (
            "../app-core/src/customer_runs.rs",
            "const NON_WORKSPACE_RUN_SOURCES: &[&str] = &[",
        ),
        (
            "../../web-app/src/pages/ide/coordinator/components/constants.ts",
            "const SYSTEM_SOURCE_TYPES: readonly string[] = [",
        ),
    ] {
        let source = read(file);
        let start = source
            .find(list)
            .unwrap_or_else(|| panic!("{file} no longer declares `{list}`"));
        let body = &source[start..];
        let body = &body[..body.find("];").expect("the list ends")];
        assert!(
            body.contains(&format!("\"{USAGE_BACKFILL_KIND}\"")),
            "{file}: `{USAGE_BACKFILL_KIND}` is missing from the daemon list"
        );
    }
}

/// The two counts that decide when Postgres may stop receiving usage are
/// quoted in `data-placement.md`; they are the text the code runs, and they
/// count what they say.
#[tokio::test]
async fn usage_counting_sql_in_the_docs_is_the_sql_that_runs() {
    use oxy_custom_apps::usage_backfill::{APPS_NOT_YET_COPIED_SQL, APPS_WITHOUT_AIRHOUSE_SQL};

    let doc = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../internal-docs/data-placement.md"),
    )
    .expect("read data-placement.md");
    let flat = |text: &str| text.split_whitespace().collect::<Vec<_>>().join(" ");
    for sql in [APPS_WITHOUT_AIRHOUSE_SQL, APPS_NOT_YET_COPIED_SQL] {
        assert!(
            flat(&doc).contains(&flat(sql)),
            "data-placement.md no longer quotes:\n{sql}"
        );
    }

    let world = world(test_db_with(Schema::All).await).await;
    let count = |sql: &'static str| {
        let db = world.db.clone();
        async move {
            db.query_one_raw(Statement::from_string(DatabaseBackend::Postgres, sql))
                .await
                .unwrap()
                .unwrap()
                .try_get_by_index::<i64>(0)
                .unwrap()
        }
    };
    // One app, no Airhouse tenant.
    assert_eq!(count(APPS_WITHOUT_AIRHOUSE_SQL).await, 1);
    assert_eq!(count(APPS_NOT_YET_COPIED_SQL).await, 0);

    // The workspace gets an Airhouse: no longer without one, not yet copied.
    world
        .db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "INSERT INTO airhouse_tenants \
               (id, workspace_id, airhouse_tenant_id, bucket, status, created_at) \
             VALUES ($1, $2, $3, 'bucket', 'active', now())",
            [
                Uuid::new_v4().into(),
                world.workspace_id.into(),
                format!("tenant-{}", world.workspace_id).into(),
            ],
        ))
        .await
        .expect("seed tenant");
    assert_eq!(count(APPS_WITHOUT_AIRHOUSE_SQL).await, 0);
    assert_eq!(count(APPS_NOT_YET_COPIED_SQL).await, 1);

    // Copied: both are zero.
    let fake = airhouse(&world);
    backfill(&world, &fake).await;
    assert_eq!(count(APPS_WITHOUT_AIRHOUSE_SQL).await, 0);
    assert_eq!(count(APPS_NOT_YET_COPIED_SQL).await, 0);
}
