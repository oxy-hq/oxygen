//! "Leave kiosk mode" and "Revoke" as routes, against a real database: who may
//! call them from where, what a database error reads as, and which cookies the
//! answer clears.
//!
//! The org middleware is stood in for (it would need the whole org router), so
//! every request here is an org admin's that has already passed `OrgAdmin`. The
//! doors under test are the ones inside the handlers:
//!
//! - **Same origin.** Leaving has no body, so a cross-origin POST is a simple
//!   request the browser sends without a preflight, and the session and kiosk
//!   cookies are `SameSite=Lax` — a sibling subdomain (another org's
//!   custom-app host) is same-site and gets both attached. The CORS layer only
//!   withholds the response. So the handler refuses an `Origin` that is not
//!   this host, with the check the custom-app gate already uses.
//! - **Unknown is not "not a kiosk".** A database error answers 503, never the
//!   404 whose copy tells the admin the tablet was already revoked.
//! - **Both cookies go.** `oxy_kiosk` and the page-readable `oxy_kiosk_hint`
//!   are cleared together, by leaving and by revoking this browser's own kiosk.

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::Response;
use axum::routing::{delete, post};
use entity::org_members::OrgRole;
use entity::{org_kiosk_devices, org_members, organizations, users};
use oxy_app::server::api::frontline_devices::{DeviceError, revoke_device};
use oxy_app::server::api::frontline_kiosk_mode::{leave, leave_kiosk};
use oxy_app::server::api::middlewares::org_context::OrgContext;
use oxy_auth::types::AuthenticatedUser;
use sea_orm::{ActiveModelTrait, ActiveValue, ConnectionTrait, DatabaseConnection, EntityTrait};
use tower::ServiceExt;
use uuid::Uuid;

use crate::frontline_kiosk_mode::{bound_kiosk, kiosk_cookie, seed_org, seed_user, wired_db};

/// The host the SPA is served from in production, and a sibling custom-app
/// host on the same registrable domain.
const APP_HOST: &str = "app.oxygen-hq.com";
const SIBLING_ORIGIN: &str = "https://other-org--store-ops.customer-apps.oxygen-hq.com";

/// A real Admin membership: `OrgAdmin` asks the authz loader, which reads it
/// back from the database, so a context-only one would be refused.
async fn seed_admin_membership(
    db: &DatabaseConnection,
    org_id: Uuid,
    user_id: Uuid,
) -> org_members::Model {
    let now = chrono::Utc::now().fixed_offset();
    org_members::ActiveModel {
        id: ActiveValue::Set(Uuid::new_v4()),
        org_id: ActiveValue::Set(org_id),
        user_id: ActiveValue::Set(user_id),
        role: ActiveValue::Set(OrgRole::Admin),
        created_at: ActiveValue::Set(now),
        updated_at: ActiveValue::Set(now),
    }
    .insert(db)
    .await
    .expect("seed admin membership")
}

fn as_authenticated(user: &users::Model) -> AuthenticatedUser {
    AuthenticatedUser {
        id: user.id,
        email: user.email.clone(),
        name: user.name.clone(),
        picture: None,
        status: user.status.clone(),
    }
}

/// The leave and revoke routes, as `org_middleware` and the auth layer would
/// hand them an org admin's request.
pub(crate) fn admin_routes(ctx: OrgContext, admin: AuthenticatedUser) -> Router {
    let inject = move |mut req: Request<Body>, next: Next| {
        let ctx = ctx.clone();
        let user = admin.clone();
        Box::pin(async move {
            req.extensions_mut().insert(ctx);
            req.extensions_mut().insert(user);
            next.run(req).await
        }) as futures::future::BoxFuture<'static, Response>
    };
    Router::new()
        .route("/orgs/{org_id}/frontline/device/leave", post(leave_kiosk))
        .route(
            "/orgs/{org_id}/frontline/devices/{id}",
            delete(revoke_device),
        )
        .layer(middleware::from_fn(inject))
}

/// An org, an admin of it, and the routes as that admin.
async fn admin_of_new_org(db: &DatabaseConnection) -> (Uuid, Router, String) {
    let org_id = seed_org(db).await;
    let org = organizations::Entity::find_by_id(org_id)
        .one(db)
        .await
        .unwrap()
        .expect("seeded org");
    let email = format!("maya-{}@acme.test", Uuid::new_v4().simple());
    let admin = seed_user(db, Some(email)).await;
    let membership = seed_admin_membership(db, org_id, admin.id).await;
    let jwt = oxy_app::server::api::auth::create_auth_token(admin.clone())
        .await
        .expect("mint a session");
    let ctx = OrgContext {
        org,
        membership,
        is_global_override: false,
    };
    (org_id, admin_routes(ctx, as_authenticated(&admin)), jwt)
}

/// A POST to leave carrying the cookies a kiosk browser holds, arriving at
/// `host` from `origin`; `bearer` is what the SPA adds from `localStorage`.
fn leave_request(
    org: Uuid,
    host: &str,
    origin: &str,
    cookies: &str,
    bearer: Option<&str>,
) -> Request<Body> {
    let mut req = Request::builder()
        .method("POST")
        .uri(format!("/orgs/{org}/frontline/device/leave"))
        .header(header::HOST, host)
        .header(header::ORIGIN, origin)
        .header(header::COOKIE, cookies);
    if let Some(token) = bearer {
        req = req.header(header::AUTHORIZATION, token);
    }
    req.body(Body::empty()).unwrap()
}

async fn is_revoked(db: &DatabaseConnection, id: Uuid) -> bool {
    org_kiosk_devices::Entity::find_by_id(id)
        .one(db)
        .await
        .unwrap()
        .expect("the row stays")
        .revoked_at
        .is_some()
}

fn set_cookies(resp: &Response) -> Vec<String> {
    resp.headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .map(|v| v.to_str().unwrap().to_string())
        .collect()
}

/// True when `set` expires the cookie `name` (an empty value, `Max-Age=0`).
fn clears(set: &[String], name: &str) -> bool {
    let prefix = format!("{name}=;");
    set.iter()
        .any(|c| c.starts_with(&prefix) && c.contains("Max-Age=0"))
}

#[tokio::test]
async fn a_cookie_only_call_from_a_sibling_subdomain_is_refused_and_the_spas_call_still_works() {
    let db = wired_db().await;
    let (org, routes, jwt) = admin_of_new_org(&db).await;
    let (kiosk, cookie) = bound_kiosk(&db, org, "Front counter").await;
    let jar = format!("oxy_session={jwt}; oxy_kiosk={cookie}");

    // Another org's custom app, same site: the browser attaches both cookies
    // to a simple POST and the page never needs to read the answer.
    let forged = routes
        .clone()
        .oneshot(leave_request(org, APP_HOST, SIBLING_ORIGIN, &jar, None))
        .await
        .unwrap();
    assert_eq!(forged.status(), StatusCode::FORBIDDEN);
    assert!(
        set_cookies(&forged).is_empty(),
        "a refused call clears nothing"
    );
    assert!(
        !is_revoked(&db, kiosk).await,
        "the tablet must still be a kiosk after a cross-site call"
    );

    // The SPA on the same host: same-origin, with the bearer token it always
    // attaches.
    let spa = routes
        .clone()
        .oneshot(leave_request(
            org,
            APP_HOST,
            &format!("https://{APP_HOST}"),
            &jar,
            Some(&jwt),
        ))
        .await
        .unwrap();
    assert_eq!(spa.status(), StatusCode::NO_CONTENT);
    assert!(is_revoked(&db, kiosk).await, "the SPA's leave revokes");

    // Local development: Vite's page on 127.0.0.1 reaching the API on
    // localhost is the loopback pair the CORS layer already accepts.
    let (dev_kiosk, dev_cookie) = bound_kiosk(&db, org, "Dev tablet").await;
    let dev = routes
        .oneshot(leave_request(
            org,
            "localhost:3000",
            "http://127.0.0.1:5173",
            &format!("oxy_session={jwt}; oxy_kiosk={dev_cookie}"),
            Some(&jwt),
        ))
        .await
        .unwrap();
    assert_eq!(dev.status(), StatusCode::NO_CONTENT);
    assert!(is_revoked(&db, dev_kiosk).await);
}

/// The table the kiosk lookup reads, made unreadable — a query error, which is
/// what `bound_device` used to fold into "not a kiosk".
pub(crate) async fn break_kiosk_lookups(db: &DatabaseConnection) {
    db.execute_unprepared("ALTER TABLE org_kiosk_devices RENAME TO org_kiosk_devices_away")
        .await
        .expect("rename the kiosk table");
}

#[tokio::test]
async fn leaving_while_the_database_errors_is_503_not_already_revoked() {
    let db = wired_db().await;
    let (org, routes, jwt) = admin_of_new_org(&db).await;
    let (_, cookie) = bound_kiosk(&db, org, "Front counter").await;
    break_kiosk_lookups(&db).await;

    let resp = routes
        .oneshot(leave_request(
            org,
            APP_HOST,
            &format!("https://{APP_HOST}"),
            &format!("oxy_session={jwt}; oxy_kiosk={cookie}"),
            Some(&jwt),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::SERVICE_UNAVAILABLE,
        "an unknown answer must not read as 'this browser isn't a kiosk'"
    );
    assert!(
        set_cookies(&resp).is_empty(),
        "nothing was left, so nothing is cleared"
    );
    assert!(matches!(
        leave(&db, org, &kiosk_cookie(&cookie)).await,
        Err(DeviceError::Db(_))
    ));
}

#[tokio::test]
async fn leaving_and_revoking_this_browsers_own_kiosk_clear_both_cookies() {
    let db = wired_db().await;
    let (org, routes, jwt) = admin_of_new_org(&db).await;

    let (_, cookie) = bound_kiosk(&db, org, "Front counter").await;
    let left = routes
        .clone()
        .oneshot(leave_request(
            org,
            APP_HOST,
            &format!("https://{APP_HOST}"),
            &format!("oxy_session={jwt}; oxy_kiosk={cookie}; oxy_kiosk_hint=1"),
            Some(&jwt),
        ))
        .await
        .unwrap();
    assert_eq!(left.status(), StatusCode::NO_CONTENT);
    let set = set_cookies(&left);
    assert!(clears(&set, "oxy_kiosk"), "{set:?}");
    assert!(clears(&set, "oxy_kiosk_hint"), "{set:?}");

    // Settings → Crew → Revoke, opened on the tablet itself.
    let (this_kiosk, this_cookie) = bound_kiosk(&db, org, "Drive-thru").await;
    let (other_kiosk, _) = bound_kiosk(&db, org, "Back office").await;
    let revoke = |id: Uuid| {
        Request::builder()
            .method("DELETE")
            .uri(format!("/orgs/{org}/frontline/devices/{id}"))
            .header(header::AUTHORIZATION, jwt.as_str())
            .header(
                header::COOKIE,
                format!("oxy_kiosk={this_cookie}; oxy_kiosk_hint=1"),
            )
            .body(Body::empty())
            .unwrap()
    };

    // Another kiosk, revoked from this tablet: this browser is still a kiosk.
    let other = routes.clone().oneshot(revoke(other_kiosk)).await.unwrap();
    assert_eq!(other.status(), StatusCode::NO_CONTENT);
    assert!(set_cookies(&other).is_empty(), "{:?}", set_cookies(&other));
    assert!(is_revoked(&db, other_kiosk).await);

    let own = routes.oneshot(revoke(this_kiosk)).await.unwrap();
    assert_eq!(own.status(), StatusCode::NO_CONTENT);
    let set = set_cookies(&own);
    assert!(clears(&set, "oxy_kiosk"), "{set:?}");
    assert!(clears(&set, "oxy_kiosk_hint"), "{set:?}");
    assert!(is_revoked(&db, this_kiosk).await);
}
