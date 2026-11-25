//! The weekly report reads a workspace's usage from its Airhouse once the
//! workspace's history has been copied there, and from Postgres until then —
//! and the report is the same either way.
//!
//! The Airhouse is `usage_fixtures::FakeAirhouse`: an in-memory DuckDB per
//! workspace that runs the SQL the sink and the reads send. The apps are spread
//! over two workspaces so that, half way through, one workspace answers from
//! Airhouse and the other from Postgres. An org has one workspace
//! (`uq_workspaces_org_id`), so those two are two orgs' — and the people count
//! stays the per-org claim it is in the report, over the three apps of the
//! first.

use std::time::Duration;

use chrono::Duration as Hours;
use oxy_custom_apps::usage_backfill::{self, Pass};
use oxy_custom_apps::usage_fixtures::{Cast, FakeAirhouse, history};
use oxy_usage_report::collect::collect;
use sea_orm::ActiveModelTrait;
use uuid::Uuid;

use super::world::World;

async fn copy(w: &World, fake: &FakeAirhouse, workspace: Uuid) {
    let pass = usage_backfill::run_with(&w.db, fake, workspace, Duration::from_secs(60), 7)
        .await
        .expect("a pass");
    assert!(matches!(pass, Pass::Finished { .. }), "{pass:?}");
}

/// The numbers [`history`] seeds, as the report must read them: the org's three
/// people this week and three the week before (Ana, Ben and Cy; Ana, Ben and
/// Dev), and `app`'s own two weeks.
fn assert_fixture_numbers(org: &oxy_usage_report::model::OrgUsage, app: Uuid) {
    assert_eq!(
        (org.people, org.prev_people),
        (3, 3),
        "Ana, Ben and Cy this week; Ana, Ben and Dev the week before"
    );
    let usage = org.apps.iter().find(|a| a.app_id == app).unwrap();
    assert_eq!(
        (
            usage.current.views,
            usage.current.people,
            usage.current.sessions,
            usage.current.error_sessions
        ),
        (7, 3, 4, 2)
    );
    assert_eq!(
        (
            usage.previous.views,
            usage.previous.people,
            usage.previous.sessions,
            usage.previous.error_sessions
        ),
        (4, 3, 3, 1)
    );
}

/// The same history in both stores gives the report the same numbers, for a
/// workspace read from Postgres, from Airhouse, and for a mix of the two.
#[tokio::test]
async fn usage_report_is_identical_in_postgres_and_airhouse() {
    let w = World::new().await;
    let org = w.org("Poke House").await;
    let west = w.org("Poke House West").await;
    let (first, second) = (w.workspace(org).await, w.workspace(west).await);
    let store_ops = w.app_in(org, first, "Store Ops", true).await;
    let checklists = w.app_in(west, second, "Checklists", true).await;
    let old_tool = w.app_in(org, first, "Old Tool", true).await;
    let crew_board = w.app_in(org, first, "Crew Board", true).await;
    let users = [
        (w.user("ana@poke.test").await, "ana@poke.test".to_string()),
        (w.user("ben@poke.test").await, "ben@poke.test".to_string()),
        (w.user("cy@poke.test").await, "cy@poke.test".to_string()),
        (w.user("dev@poke.test").await, "dev@poke.test".to_string()),
    ];

    // Two apps in two workspaces of two orgs, opened by the same four people:
    // each org's people are counted once across its apps, in whichever store
    // it is read from.
    let anchor = w.period.end - Hours::hours(1);
    for app_id in [store_ops, checklists] {
        let rows = history(
            &Cast {
                app_id,
                users: users.clone(),
            },
            anchor,
        );
        for view in rows.views {
            view.insert(&w.db).await.expect("seed view");
        }
        for event in rows.events {
            event.insert(&w.db).await.expect("seed event");
        }
    }
    let ana = users[0].0;
    // An app opened this week and three weeks ago is not in its first week;
    // one opened this week and never before is.
    w.view(old_tool, ana, w.this_week(6), "production").await;
    w.view(old_tool, ana, w.week_before(-24 * 14), "production")
        .await;
    w.view(crew_board, ana, w.this_week(7), "production").await;

    let fake = FakeAirhouse::new();
    fake.with_airhouse(first).with_airhouse(second);
    fake.install();

    // Nothing copied: Postgres answers, and its numbers are the fixture's.
    let from_postgres = collect(&w.db, w.period).await.unwrap();
    assert_eq!(fake.queries(), 0, "no workspace is finished");
    assert_eq!(from_postgres.orgs.len(), 2);
    let org_usage = |id: Uuid| {
        from_postgres
            .orgs
            .iter()
            .find(|o| o.org_id == id)
            .expect("both orgs are in the report")
    };
    for (id, app) in [(org, store_ops), (west, checklists)] {
        assert_fixture_numbers(org_usage(id), app);
    }
    // Old Tool and Crew Board are the first org's, and only Ana opened them.
    let first_week = |name: &str| {
        org_usage(org)
            .apps
            .iter()
            .find(|a| a.name == name)
            .unwrap()
            .first_week
    };
    assert!(first_week("Crew Board"));
    assert!(!first_week("Old Tool"));

    // One workspace copied: its org's apps are read from Airhouse, the other
    // org's from Postgres, and the report does not change.
    copy(&w, &fake, first).await;
    let mixed = collect(&w.db, w.period).await.unwrap();
    assert!(fake.queries() > 0, "the copied workspace answered");
    assert_eq!(mixed, from_postgres);

    // Both copied: every app is read from Airhouse.
    copy(&w, &fake, second).await;
    let queries = fake.queries();
    let from_airhouse = collect(&w.db, w.period).await.unwrap();
    assert!(fake.queries() > queries);
    assert_eq!(from_airhouse, from_postgres);
}

/// A workspace with no Airhouse is never copied and the report reads it from
/// Postgres, without asking any Airhouse.
#[tokio::test]
async fn usage_report_keeps_reading_postgres_for_a_workspace_with_no_airhouse() {
    let w = World::new().await;
    let org = w.org("Rivermark").await;
    let workspace = w.workspace(org).await;
    let app = w.app_in(org, workspace, "Crew Board", true).await;
    let kai = w.user("kai@rivermark.test").await;
    w.view(app, kai, w.this_week(5), "production").await;

    // An Airhouse exists, for some other workspace.
    let fake = FakeAirhouse::new();
    fake.with_airhouse(Uuid::new_v4());
    fake.install();

    let pass =
        usage_backfill::run_with(&w.db, fake.as_ref(), workspace, Duration::from_secs(60), 7)
            .await
            .unwrap();
    assert_eq!(pass, Pass::NoAirhouse);
    let report = collect(&w.db, w.period).await.unwrap();
    let usage = &report.orgs[0].apps[0];
    assert_eq!((usage.current.views, usage.current.people), (1, 1));
    assert_eq!(fake.queries(), 0);
}
