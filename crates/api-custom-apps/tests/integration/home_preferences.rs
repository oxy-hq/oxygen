//! A person's own home against a real database: the preferences row
//! (`home_user_preferences`) and the org teams a layout's `for` is read against.

use super::custom_app_visibility::{seed_member, seed_org, seed_user};
use crate::common::test_db;
use entity::org_members::OrgRole;
use oxy_api_custom_apps::home_widgets::preferences::Preferences;
use oxy_api_custom_apps::home_widgets::preferences::org_section_kinds;
use oxy_api_custom_apps::home_widgets::{layout_store, preferences_store, viewer};
use oxy_app_core::widgets::layout::Viewer;
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement};
use uuid::Uuid;

async fn seed_team(conn: &DatabaseConnection, org: Uuid, name: &str, members: &[Uuid]) {
    let team = Uuid::new_v4();
    conn.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO org_teams (id, org_id, name, created_at, updated_at) \
         VALUES ($1, $2, $3, now(), now())",
        [team.into(), org.into(), name.into()],
    ))
    .await
    .expect("seed team");
    for user in members {
        conn.execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO org_team_members (id, team_id, user_id, created_at) \
             VALUES ($1, $2, $3, now())",
            [Uuid::new_v4().into(), team.into(), (*user).into()],
        ))
        .await
        .expect("seed team member");
    }
}

#[tokio::test]
async fn no_row_is_nothing_hidden_and_today_shown() {
    let conn = test_db().await;
    let (user, _) = seed_user(&conn).await;
    let org = seed_org(&conn).await;
    let got = preferences_store::load(&conn, user, org).await.unwrap();
    assert_eq!(got, Preferences::default());
    assert_eq!(
        got.to_json(),
        serde_json::json!({ "hidden": [], "today": "shown" })
    );
}

#[tokio::test]
async fn a_save_is_read_back_and_replaced_by_the_next_one() {
    let conn = test_db().await;
    let (user, _) = seed_user(&conn).await;
    let (other, _) = seed_user(&conn).await;
    let org = seed_org(&conn).await;
    let first = Preferences {
        hidden: vec!["bookkeeping/close".into(), "store-ops/today#mission".into()],
        today_hidden: true,
    };
    preferences_store::save(&conn, user, org, &first)
        .await
        .unwrap();
    assert_eq!(
        preferences_store::load(&conn, user, org).await.unwrap(),
        first
    );
    // Replaced, not merged.
    let second = Preferences {
        hidden: vec![],
        today_hidden: false,
    };
    preferences_store::save(&conn, user, org, &second)
        .await
        .unwrap();
    assert_eq!(
        preferences_store::load(&conn, user, org).await.unwrap(),
        second
    );
    // One person's choices are not another's, nor another org's.
    assert_eq!(
        preferences_store::load(&conn, other, org).await.unwrap(),
        Preferences::default()
    );
    let elsewhere = seed_org(&conn).await;
    assert_eq!(
        preferences_store::load(&conn, user, elsewhere)
            .await
            .unwrap(),
        Preferences::default()
    );
}

#[tokio::test]
async fn a_viewer_carries_the_teams_of_that_org_only() {
    let conn = test_db().await;
    let (user, email) = seed_user(&conn).await;
    let (amy, _) = seed_user(&conn).await;
    let (org, other_org) = (seed_org(&conn).await, seed_org(&conn).await);
    seed_member(&conn, org, user, OrgRole::Member).await;
    seed_team(&conn, org, "Office", &[user, amy]).await;
    seed_team(&conn, org, "Stores", &[amy]).await;
    seed_team(&conn, other_org, "Elsewhere", &[user]).await;

    let got = viewer::viewer(&conn, user, Some(&email), org)
        .await
        .unwrap();
    assert_eq!(got, Viewer::new(Some(&email), ["office".to_string()]));
    let nobody = viewer::viewer(&conn, Uuid::new_v4(), None, org)
        .await
        .unwrap();
    assert_eq!(nobody, Viewer::default());
}

#[tokio::test]
async fn the_answer_names_the_orgs_layout_sections_in_order() {
    let conn = test_db().await;
    let (user, _) = seed_user(&conn).await;
    let org = seed_org(&conn).await;
    // No stored layout: the default home.
    assert_eq!(
        org_section_kinds(&conn, org).await.unwrap(),
        ["widgets", "apps", "insights"]
    );
    // A layout that dropped Insights and put Apps first.
    let layout = serde_json::json!({ "version": 1, "sections": [
        { "kind": "apps" },
        { "kind": "widgets", "title": "Back office", "items": [] }
    ] });
    layout_store::save(&conn, org, &layout, None, user)
        .await
        .unwrap();
    let kinds = org_section_kinds(&conn, org).await.unwrap();
    assert_eq!(kinds, ["apps", "widgets"]);
    let prefs = Preferences {
        hidden: vec![],
        today_hidden: true,
    };
    assert_eq!(
        prefs.answer(&kinds),
        serde_json::json!({ "hidden": [], "today": "hidden", "sections": ["apps", "widgets"] })
    );
    // Another org's layout is not this one's.
    let elsewhere = seed_org(&conn).await;
    assert_eq!(
        org_section_kinds(&conn, elsewhere).await.unwrap(),
        ["widgets", "apps", "insights"]
    );
}

/// What sits in the `layout` column, as a server older than `for` reads it.
async fn raw_layout(conn: &DatabaseConnection, org: Uuid) -> serde_json::Value {
    let row = conn
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT layout FROM org_home_layouts WHERE org_id = $1",
            [org.into()],
        ))
        .await
        .unwrap()
        .expect("a layout row");
    row.try_get("", "layout").unwrap()
}

#[tokio::test]
async fn a_layouts_for_is_kept_beside_the_document_an_older_server_reads() {
    let conn = test_db().await;
    let (user, _) = seed_user(&conn).await;
    let org = seed_org(&conn).await;
    let layout = serde_json::json!({ "version": 1, "sections": [
        { "kind": "widgets", "title": "Back office",
          "for": { "people": ["scott@poke.house"] },
          "items": [{ "widget": "bookkeeping/close", "for": { "teams": ["office"] } }] },
        { "kind": "apps" }
    ] });
    layout_store::save(&conn, org, &layout, None, user)
        .await
        .unwrap();

    // The column an older server reads carries no `for`: it parses there in full.
    let stored = raw_layout(&conn, org).await;
    assert!(!stored.to_string().contains("\"for\""), "{stored}");
    assert_eq!(stored["sections"][0]["title"], "Back office");
    // This server reads it back whole.
    let loaded = layout_store::load(&conn, org).await.unwrap().unwrap();
    assert_eq!(loaded.layout, layout);

    // An older server saves a layout of its own: `layout` and `revision` only.
    let theirs = serde_json::json!({ "version": 1, "sections": [
        { "kind": "widgets", "items": [{ "widget": "bookkeeping/close" }] }
    ] });
    conn.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE org_home_layouts SET layout = $2, revision = revision + 1 WHERE org_id = $1",
        [org.into(), theirs.clone().into()],
    ))
    .await
    .unwrap();
    // The `for`s it left behind describe another document, so they are not pinned on this one.
    let loaded = layout_store::load(&conn, org).await.unwrap().unwrap();
    assert_eq!(loaded.layout, theirs);
}
