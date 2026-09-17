//! Usage tracking captures **who** and **in what capacity**, with no help from
//! the app's own code.
//!
//! `custom_app_access_control.rs` covers who is *allowed* in. This covers what
//! gets written down when they arrive. Two properties are worth a database:
//!
//! - **Roles are snapshotted at view time**, and the app and org roles are
//!   recorded separately because they routinely disagree. A log that resolved
//!   roles at read time would rewrite its own history every time somebody was
//!   promoted.
//! - **The roll-up reports the latest *recorded* role.** `NULL` in these columns
//!   means "not recorded" — a row predating the columns, or a lookup that failed
//!   — and must not blank out a visitor whose role is perfectly well known.
//!
//! Both go through the real recorder and the real query rather than asserting on
//! hand-written SQL, because the thing that would actually break is the wiring:
//! `record_view` resolving no role, or the roll-up picking the wrong one.

use crate::common::test_db;
use chrono::Utc;
use entity::{
    app_members, apps, custom_app_view_event, org_members, org_members::OrgRole, organizations,
    users, workspaces,
};
use oxy_app::server::api::custom_apps_activity::{VisitorsQuery, get_visitors};
use oxy_app::server::api::custom_apps_tracking::record_view;
use sea_orm::{ActiveModelTrait, ActiveValue, ColumnTrait, DatabaseConnection, QueryFilter};
use sea_orm::{EntityTrait, QueryOrder};
use uuid::Uuid;

// ── Seeding ─────────────────────────────────────────────────────────────────

async fn seed_user(conn: &DatabaseConnection) -> (Uuid, String) {
    let id = Uuid::new_v4();
    let email = format!("activity-{id}@example.com");
    users::ActiveModel {
        id: ActiveValue::Set(id),
        email: ActiveValue::Set(Some(email.clone())),
        name: ActiveValue::Set("Activity Test".into()),
        picture: ActiveValue::Set(None),
        email_verified: ActiveValue::Set(true),
        ..Default::default()
    }
    .insert(conn)
    .await
    .expect("seed user");
    (id, email)
}

async fn seed_org(conn: &DatabaseConnection) -> Uuid {
    let id = Uuid::new_v4();
    organizations::ActiveModel {
        id: ActiveValue::Set(id),
        name: ActiveValue::Set("Activity Org".into()),
        slug: ActiveValue::Set(format!("activity-org-{id}")),
        ..Default::default()
    }
    .insert(conn)
    .await
    .expect("seed org");
    id
}

async fn seed_app(conn: &DatabaseConnection, org_id: Uuid) -> apps::Model {
    let workspace_id = Uuid::new_v4();
    workspaces::ActiveModel {
        id: ActiveValue::Set(workspace_id),
        name: ActiveValue::Set("Activity Workspace".into()),
        org_id: ActiveValue::Set(Some(org_id)),
        ..Default::default()
    }
    .insert(conn)
    .await
    .expect("seed workspace");

    let id = Uuid::new_v4();
    apps::ActiveModel {
        id: ActiveValue::Set(id),
        slug: ActiveValue::Set(format!("activity-app-{id}")),
        name: ActiveValue::Set("Activity App".into()),
        org_id: ActiveValue::Set(org_id),
        project_id: ActiveValue::Set(workspace_id),
        branch: ActiveValue::Set("main".into()),
        source_repo: ActiveValue::Set("activity/test".into()),
        status: ActiveValue::Set("active".into()),
        source_type: ActiveValue::Set("s3".into()),
        source_config: ActiveValue::Set(serde_json::json!({})),
        published_at: ActiveValue::Set(Some(Utc::now().into())),
        ..Default::default()
    }
    .insert(conn)
    .await
    .expect("seed app")
}

/// Write a view row straight to the table, bypassing the recorder — for
/// asserting on how the roll-up *reads* history, including history the recorder
/// can no longer produce (pre-column `NULL`s).
#[allow(clippy::too_many_arguments)]
async fn insert_view(
    conn: &DatabaseConnection,
    app_id: Uuid,
    user_id: Uuid,
    user_email: &str,
    minutes_ago: i64,
    app_role: Option<&str>,
    org_role: Option<&str>,
) {
    custom_app_view_event::ActiveModel {
        id: ActiveValue::Set(Uuid::new_v4()),
        app_id: ActiveValue::Set(app_id),
        user_id: ActiveValue::Set(user_id),
        user_email: ActiveValue::Set(user_email.to_string()),
        session_id: ActiveValue::Set(Uuid::new_v4()),
        viewed_at: ActiveValue::Set(
            (Utc::now() - chrono::Duration::minutes(minutes_ago)).fixed_offset(),
        ),
        referrer: ActiveValue::Set(None),
        user_agent_class: ActiveValue::Set("browser".into()),
        source: ActiveValue::Set("subpath".into()),
        app_role: ActiveValue::Set(app_role.map(str::to_string)),
        org_role: ActiveValue::Set(org_role.map(str::to_string)),
    }
    .insert(conn)
    .await
    .expect("insert view");
}

// ── Tests ───────────────────────────────────────────────────────────────────

/// The recorder resolves both roles itself. Nothing in the app's bundle asked
/// for this, and nothing could have supplied it — that is the point: an
/// uninstrumented app still produces a complete usage trail.
///
/// The seeded principal is a plain **org member** holding an **app admin** row,
/// so the two columns must disagree. That combination is not exotic — it is the
/// whole reason per-app admin exists (name an app's administrator without
/// handing them org-Admin, which also carries billing and member management) —
/// and it is exactly what a single collapsed "role" column would destroy.
#[tokio::test]
async fn recording_a_view_snapshots_app_and_org_role_separately() {
    let conn = test_db().await;
    let org = seed_org(&conn).await;
    let app = seed_app(&conn, org).await;
    let (user, email) = seed_user(&conn).await;
    seed_member(&conn, org, user, OrgRole::Member).await;

    app_members::ActiveModel {
        id: ActiveValue::Set(Uuid::new_v4()),
        app_id: ActiveValue::Set(app.id),
        user_id: ActiveValue::Set(user),
        role: ActiveValue::Set(app_members::ROLE_ADMIN.into()),
        created_by: ActiveValue::Set(None),
        ..Default::default()
    }
    .insert(&conn)
    .await
    .expect("seed app admin grant");

    record_view(
        app.clone(),
        user,
        Some(email.clone()),
        Uuid::new_v4(),
        None,
        "browser".into(),
        "subpath".into(),
    )
    .await;

    let row = custom_app_view_event::Entity::find()
        .filter(custom_app_view_event::Column::AppId.eq(app.id))
        .one(&conn)
        .await
        .expect("query view")
        .expect("the recorder must have written a row");

    assert_eq!(row.user_email, email);
    assert_eq!(
        row.app_role.as_deref(),
        Some("admin"),
        "an app_members admin row is app-admin, and the log must say so"
    );
    assert_eq!(
        row.org_role.as_deref(),
        Some("member"),
        "org standing is NOT app standing — collapsing these would lose the distinction"
    );
}

/// A view by someone with no app grant and no org membership records neither
/// role rather than inventing one.
///
/// This is the fail-closed direction that matters. The recorder runs
/// best-effort inside a spawn, and every failure path returns `None`; a bug that
/// defaulted to `"admin"` instead would put a false privilege claim in what an
/// operator reads as an audit trail.
#[tokio::test]
async fn a_view_with_no_standing_records_no_role_rather_than_a_default() {
    let conn = test_db().await;
    let org = seed_org(&conn).await;
    let app = seed_app(&conn, org).await;
    let (stranger, email) = seed_user(&conn).await;

    record_view(
        app.clone(),
        stranger,
        Some(email),
        Uuid::new_v4(),
        None,
        "browser".into(),
        "subpath".into(),
    )
    .await;

    let row = custom_app_view_event::Entity::find()
        .filter(custom_app_view_event::Column::AppId.eq(app.id))
        .one(&conn)
        .await
        .expect("query view")
        .expect("a view is still recorded — tracking is not an access gate");

    assert_eq!(row.app_role, None);
    assert_eq!(row.org_role, None);
}

/// The visitors roll-up reports the latest role it actually has, not whatever
/// sits on the newest row.
///
/// The newest view here carries `NULL` — the shape produced by a failed lookup,
/// and by every row written before the columns existed. Reading that as the
/// answer would show "—" for a visitor whose role is recorded twice over, and
/// would make the whole column read as broken for the first 90 days after
/// rollout, while pre-existing rows aged out.
#[tokio::test]
async fn the_visitor_rollup_reports_the_latest_recorded_role_not_the_latest_row() {
    let conn = test_db().await;
    let org = seed_org(&conn).await;
    let app = seed_app(&conn, org).await;
    let (user, email) = seed_user(&conn).await;
    seed_member(&conn, org, user, OrgRole::Admin).await;

    insert_view(
        &conn,
        app.id,
        user,
        &email,
        180,
        Some("member"),
        Some("member"),
    )
    .await;
    insert_view(
        &conn,
        app.id,
        user,
        &email,
        60,
        Some("admin"),
        Some("admin"),
    )
    .await;
    insert_view(&conn, app.id, user, &email, 1, None, None).await;

    let visitors = get_visitors(
        axum::extract::Path(app.id),
        axum::extract::Query(VisitorsQuery { days: 7, limit: 50 }),
    )
    .await
    .expect("visitors query")
    .0;

    let row = visitors
        .rows
        .iter()
        .find(|r| r.user_id == user)
        .expect("the visitor must appear");
    assert_eq!(row.views, 3);
    assert_eq!(
        row.app_role.as_deref(),
        Some("admin"),
        "the most recent RECORDED role, not the NULL on the newest row"
    );
    assert_eq!(row.org_role.as_deref(), Some("admin"));
}

/// A visitor whose every view predates role capture reads as unknown, and the
/// roll-up still counts them.
///
/// Retention is 90 days, so for one quarter after rollout the table holds both
/// shapes. The failure to avoid is the migration boundary swallowing visitors
/// entirely — an inner join, a `WHERE app_role IS NOT NULL`, a filter that looked
/// harmless.
#[tokio::test]
async fn visitors_from_before_role_capture_still_appear_with_no_role() {
    let conn = test_db().await;
    let org = seed_org(&conn).await;
    let app = seed_app(&conn, org).await;
    let (user, email) = seed_user(&conn).await;
    seed_member(&conn, org, user, OrgRole::Member).await;

    insert_view(&conn, app.id, user, &email, 120, None, None).await;
    insert_view(&conn, app.id, user, &email, 30, None, None).await;

    let visitors = get_visitors(
        axum::extract::Path(app.id),
        axum::extract::Query(VisitorsQuery { days: 7, limit: 50 }),
    )
    .await
    .expect("visitors query")
    .0;

    let row = visitors
        .rows
        .iter()
        .find(|r| r.user_id == user)
        .expect("a pre-capture visitor must still be listed");
    assert_eq!(row.views, 2);
    assert_eq!(row.app_role, None);
    assert_eq!(row.org_role, None);
}

async fn seed_member(conn: &DatabaseConnection, org_id: Uuid, user_id: Uuid, role: OrgRole) {
    org_members::ActiveModel {
        id: ActiveValue::Set(Uuid::new_v4()),
        org_id: ActiveValue::Set(org_id),
        user_id: ActiveValue::Set(user_id),
        role: ActiveValue::Set(role),
        ..Default::default()
    }
    .insert(conn)
    .await
    .expect("seed org member");
}

/// Ordering helper kept honest: `insert_view` stamps `viewed_at` relative to
/// now, so "latest" in the assertions above is a real timestamp ordering rather
/// than insertion order.
#[tokio::test]
async fn inserted_views_are_ordered_by_their_stamped_time() {
    let conn = test_db().await;
    let org = seed_org(&conn).await;
    let app = seed_app(&conn, org).await;
    let (user, email) = seed_user(&conn).await;

    insert_view(&conn, app.id, user, &email, 180, Some("member"), None).await;
    insert_view(&conn, app.id, user, &email, 1, Some("admin"), None).await;

    let newest = custom_app_view_event::Entity::find()
        .filter(custom_app_view_event::Column::AppId.eq(app.id))
        .order_by_desc(custom_app_view_event::Column::ViewedAt)
        .one(&conn)
        .await
        .expect("query")
        .expect("row");
    assert_eq!(newest.app_role.as_deref(), Some("admin"));
}

/// An event names only an app its sender can open.
///
/// The route is keyed by workspace; the body may name the app. A name is
/// honoured when the app is published from this workspace AND the sender passes
/// the same per-app gate the shell and every function invoke run — so a
/// restricted app cannot be written to, or probed for, by naming it from a
/// workspace it shares. Without a name, the fallback is deterministic.
#[tokio::test]
async fn an_event_names_only_an_app_its_sender_can_open() {
    use oxy_app::server::api::custom_apps_activity::{EventAppRefusal, resolve_event_app};

    let conn = test_db().await;
    let org = seed_org(&conn).await;
    let (member, member_email) = seed_user(&conn).await;
    seed_member(&conn, org, member, OrgRole::Member).await;

    // `open` and `restricted` share a workspace; `elsewhere` has its own.
    let open = seed_app(&conn, org).await;
    let restricted_id = Uuid::new_v4();
    let restricted = apps::ActiveModel {
        id: ActiveValue::Set(restricted_id),
        slug: ActiveValue::Set(format!("restricted-{restricted_id}")),
        name: ActiveValue::Set("Restricted App".into()),
        org_id: ActiveValue::Set(org),
        project_id: ActiveValue::Set(open.project_id),
        branch: ActiveValue::Set("main".into()),
        source_repo: ActiveValue::Set("activity/test".into()),
        status: ActiveValue::Set("active".into()),
        source_type: ActiveValue::Set("s3".into()),
        source_config: ActiveValue::Set(serde_json::json!({})),
        visibility: ActiveValue::Set("members".into()),
        published_at: ActiveValue::Set(Some(Utc::now().into())),
        ..Default::default()
    }
    .insert(&conn)
    .await
    .expect("seed restricted app");
    let elsewhere = seed_app(&conn, org).await;

    let resolve = |app_id: Option<Uuid>| {
        resolve_event_app(&conn, open.project_id, member, &member_email, app_id)
    };

    // Named, published here, org-visible: the member can open it, so it is named.
    assert_eq!(resolve(Some(open.id)).await.map(|a| a.id), Ok(open.id));
    // Named, published here, restricted, no grant: not theirs — and not a
    // different answer from "not here", so the app is not probeable by name.
    assert_eq!(
        resolve(Some(restricted.id)).await.map(|a| a.id),
        Err(EventAppRefusal::NotYours)
    );
    // Named, another workspace's app: not theirs either.
    assert_eq!(
        resolve(Some(elsewhere.id)).await.map(|a| a.id),
        Err(EventAppRefusal::NotYours)
    );
    // Unnamed: an app in the workspace, first by id — deterministic, not "one of them".
    assert_eq!(
        resolve(None).await.map(|a| a.id),
        Ok(std::cmp::min(open.id, restricted.id))
    );
    // Unnamed, in a workspace that published nothing.
    assert_eq!(
        resolve_event_app(&conn, Uuid::new_v4(), member, &member_email, None)
            .await
            .map(|a| a.id),
        Err(EventAppRefusal::NoneInWorkspace)
    );
}
