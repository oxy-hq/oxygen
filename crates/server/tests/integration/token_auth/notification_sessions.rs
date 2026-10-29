//! The bell inside a custom app reads `/api/notifications*` on the app's own
//! host, and both people it serves can: a member on their browser session,
//! and a frontline worker on the PIN session a kiosk minted.
//!
//! What the app host adds, and is checked here through the outer stack it
//! actually runs (`environment_guard_middleware` and
//! `subdomain_rewrite_middleware` around `/api`):
//!
//! - `/api/*` passes through to the normal router, so the session cookie
//!   authenticates and every query is still `user_id = me`;
//! - `app_id` narrows the list, the badge and `read-all` to the app;
//! - an `href` goes absolute to the admin host, since a relative one would be
//!   rewritten into the current app; on the product host it stays relative;
//! - a staging host answers the reads and refuses the read-state writes
//!   (`EnvironmentRefused`), as it refuses every `/api` write.

use axum::Router;
use axum::http::StatusCode;
use chrono::{Duration, Utc};
use entity::{notifications, org_frontline_members, users};
use sea_orm::{ActiveModelTrait, ActiveValue, ColumnTrait, EntityTrait, QueryFilter};
use serde_json::Value;
use uuid::Uuid;

use super::stack::flat_api;
use super::{call, fixture};

const APP_HOST: &str = "acme--store.customer-apps.oxygen-hq.com";
const STAGING_HOST: &str = "staging--acme--store.customer-apps.oxygen-hq.com";
const ADMIN: &str = "https://app.oxygen-hq.com";

/// `/api` behind the two app-host layers, outermost first, as `oxy serve`
/// wraps it (`wrap_outer_service`).
fn app_host_stack() -> Router {
    Router::new()
        .nest("/api", flat_api())
        .layer(axum::middleware::from_fn(
            oxy_app_core::custom_apps_host_dispatch::subdomain_rewrite_middleware,
        ))
        .layer(axum::middleware::from_fn(
            oxy_app_core::custom_app_env_request::environment_guard_middleware,
        ))
}

async fn on(host: &str, cookie: &str, method: &str, uri: &str) -> (StatusCode, Value) {
    call(
        app_host_stack(),
        method,
        uri,
        &[("host", host), ("cookie", cookie)],
        None,
    )
    .await
}

async fn signal(
    db: &sea_orm::DatabaseConnection,
    org: Uuid,
    user: Uuid,
    app: Option<Uuid>,
    kind: &str,
    subject: Option<(&str, String)>,
) -> Uuid {
    let id = Uuid::new_v4();
    notifications::ActiveModel {
        id: ActiveValue::Set(id),
        org_id: ActiveValue::Set(org),
        user_id: ActiveValue::Set(user),
        kind: ActiveValue::Set(kind.into()),
        title: ActiveValue::Set(String::new()),
        body: ActiveValue::Set(None),
        link: ActiveValue::Set(None),
        subject_kind: ActiveValue::Set(subject.as_ref().map(|s| s.0.to_string())),
        subject_id: ActiveValue::Set(subject.map(|s| s.1)),
        created_at: ActiveValue::Set(Utc::now().fixed_offset()),
        read_at: ActiveValue::Set(None),
        message_id: ActiveValue::Set(None),
        app_id: ActiveValue::Set(app),
        thread_key: ActiveValue::Set(None),
    }
    .insert(db)
    .await
    .expect("seed a notification");
    id
}

/// A frontline worker: a `users` row with no mailbox, enrolled in the org,
/// holding no membership — and the session the kiosk sign-in mints for them
/// (`api-frontline`'s `login`: `create_auth_token_with_ttl`, 12 hours).
async fn crew(db: &sea_orm::DatabaseConnection, org: Uuid) -> (Uuid, String) {
    let id = Uuid::new_v4();
    let user = users::ActiveModel {
        id: ActiveValue::Set(id),
        email: ActiveValue::Set(None),
        name: ActiveValue::Set("Shift Lead".into()),
        picture: ActiveValue::Set(None),
        email_verified: ActiveValue::Set(false),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed a worker");
    org_frontline_members::ActiveModel {
        org_id: ActiveValue::Set(org),
        user_id: ActiveValue::Set(id),
        status: ActiveValue::Set("active".into()),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("enrol the worker");
    let jwt = oxy_app::surface::session::create_auth_token_with_ttl(user, Duration::hours(12))
        .await
        .expect("mint a shift session");
    (id, format!("oxy_session={jwt}"))
}

fn ids(inbox: &Value) -> Vec<String> {
    inbox["items"]
        .as_array()
        .unwrap_or_else(|| panic!("items in {inbox}"))
        .iter()
        .map(|i| i["id"].as_str().expect("an id").to_string())
        .collect()
}

#[tokio::test]
async fn a_member_and_a_crew_member_read_their_own_inbox_on_the_app_host() {
    // SAFETY: nextest runs each test in its own process.
    unsafe { std::env::set_var("OXY_API_URL", ADMIN) };
    let fx = fixture().await;
    let (app, other_app) = (Uuid::new_v4(), Uuid::new_v4());
    let mine = signal(&fx.db, fx.org_id, fx.user.id, Some(app), "late", None).await;
    signal(&fx.db, fx.org_id, fx.user.id, Some(other_app), "late", None).await;
    let notice = signal(
        &fx.db,
        fx.org_id,
        fx.user.id,
        None,
        "oxy.workspace_ready",
        Some(("workspace", fx.workspace_id.to_string())),
    )
    .await;
    let (worker, shift) = crew(&fx.db, fx.org_id).await;
    let theirs = signal(&fx.db, fx.org_id, worker, Some(app), "late", None).await;

    // The member, on the app host, narrowed to the app.
    let scoped = format!("/api/notifications?app_id={app}");
    let (status, inbox) = on(APP_HOST, &fx.cookie, "GET", &scoped).await;
    assert_eq!(status, StatusCode::OK, "{inbox}");
    assert_eq!(ids(&inbox), [mine.to_string()]);
    assert_eq!(inbox["unread"], 1);
    assert!(
        inbox["items"][0].get("link").is_some_and(Value::is_null),
        "{inbox}"
    );
    let badge = format!("/api/notifications/unread?app_id={app}");
    assert_eq!(on(APP_HOST, &fx.cookie, "GET", &badge).await.1["unread"], 1);
    let (_, all) = on(APP_HOST, &fx.cookie, "GET", "/api/notifications/unread").await;
    assert_eq!(all["unread"], 3, "without app_id, everything of theirs");
    // Per app: the two apps, one each; the platform notice is in the total only.
    let mut per_app: Vec<(String, u64)> = all["apps"]
        .as_array()
        .unwrap_or_else(|| panic!("apps in {all}"))
        .iter()
        .map(|a| {
            let id = a["app_id"].as_str().expect("an app id").to_string();
            (id, a["unread"].as_u64().expect("a count"))
        })
        .collect();
    per_app.sort();
    let mut want = vec![(app.to_string(), 1), (other_app.to_string(), 1)];
    want.sort();
    assert_eq!(per_app, want, "{all}");

    // A platform notice's web-app path goes absolute on the app host, and
    // stays relative on the product host.
    let (_, everything) = on(APP_HOST, &fx.cookie, "GET", "/api/notifications").await;
    let item = everything["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["id"] == notice.to_string())
        .expect("the notice is listed");
    let href = item["href"].as_str().expect("an href");
    assert!(href.starts_with(&format!("{ADMIN}/acme-")), "{href}");
    assert!(
        href.ends_with(&format!("/workspaces/{}", fx.workspace_id)),
        "{href}"
    );
    let (_, product) = on("app.oxygen-hq.com", &fx.cookie, "GET", "/api/notifications").await;
    let relative = product["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["id"] == notice.to_string())
        .unwrap()["href"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(relative.starts_with("/acme-"), "{relative}");

    // The worker, on the same host with the kiosk's session: their own only.
    let (status, inbox) = on(APP_HOST, &shift, "GET", &scoped).await;
    assert_eq!(status, StatusCode::OK, "{inbox}");
    assert_eq!(ids(&inbox), [theirs.to_string()]);

    // `read-all` for the app clears the worker's, and nobody else's.
    let clear = format!("/api/notifications/read-all?app_id={app}");
    assert_eq!(
        on(APP_HOST, &shift, "POST", &clear).await.0,
        StatusCode::NO_CONTENT
    );
    let unread: Vec<Uuid> = notifications::Entity::find()
        .filter(notifications::Column::ReadAt.is_null())
        .all(&fx.db)
        .await
        .expect("unread")
        .into_iter()
        .map(|n| n.id)
        .collect();
    assert!(!unread.contains(&theirs));
    assert!(unread.contains(&mine), "the member's stays unread");

    // A staging host reads, and refuses the write like every `/api` write.
    assert_eq!(
        on(STAGING_HOST, &fx.cookie, "GET", &scoped).await.0,
        StatusCode::OK
    );
    assert_eq!(
        on(STAGING_HOST, &fx.cookie, "POST", &clear).await.0,
        StatusCode::FORBIDDEN
    );
}
