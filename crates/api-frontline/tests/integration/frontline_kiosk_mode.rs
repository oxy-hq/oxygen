//! Getting a browser out of kiosk mode, and warning before one goes in —
//! `oxy_api_frontline::frontline_kiosk_mode`, against a real database.
//!
//! Two promises. **Leaving** revokes exactly the kiosk the request's own cookie
//! names, and only when that kiosk is the org the route is for: no cookie,
//! a forged one and another org's kiosk are one `NotFound`, and the other
//! org's tablet stays bound. **The enrol page** names the account a browser is
//! signed in to before it becomes the store's tablet — and falls back to the
//! plain page for a crew session, a token that does not verify and a user who
//! is gone, while binding nothing on its GET.
//!
//! The enrol-page cases drive the real handler, which opens its own
//! connection, so they point `OXY_DATABASE_URL` at this test's database — sound
//! only under nextest's process-per-test, which `fresh_db` asserts.

use axum::body::to_bytes;
use axum::extract::Query;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use entity::{org_kiosk_devices, organizations, users};
use sea_orm::{ActiveModelTrait, ActiveValue, DatabaseConnection, EntityTrait, ModelTrait};
use uuid::Uuid;

use crate::common::{Schema, fresh_db};
use oxy_api_frontline::frontline_devices::{
    BindQuery, DeviceError, KIOSK_COOKIE_NAME, NewDevice, bind_page, bind_with_token, bound_device,
    create, peek_token,
};
use oxy_api_frontline::frontline_kiosk_mode::leave;

pub(crate) async fn seed_org(db: &DatabaseConnection) -> Uuid {
    let org = Uuid::new_v4();
    organizations::ActiveModel {
        id: ActiveValue::Set(org),
        name: ActiveValue::Set("Poke".into()),
        slug: ActiveValue::Set(format!("poke-{}", &org.simple().to_string()[..8])),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed org");
    org
}

pub(crate) async fn seed_user(db: &DatabaseConnection, email: Option<String>) -> users::Model {
    let id = Uuid::new_v4();
    users::ActiveModel {
        id: ActiveValue::Set(id),
        email: ActiveValue::Set(email),
        name: ActiveValue::Set("Robert".into()),
        picture: ActiveValue::Set(None),
        email_verified: ActiveValue::Set(true),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed user")
}

/// A kiosk enrolled in `org` and bound; answers the cookie value the tablet holds.
pub(crate) async fn bound_kiosk(db: &DatabaseConnection, org: Uuid, name: &str) -> (Uuid, String) {
    let (row, token) = create(
        db,
        org,
        NewDevice {
            name,
            ..Default::default()
        },
    )
    .await
    .expect("create kiosk");
    let (_, cookie) = bind_with_token(db, &token).await.expect("bind kiosk");
    (row.id, cookie)
}

pub(crate) fn cookies(value: &str) -> HeaderMap {
    let mut h = HeaderMap::new();
    h.insert(header::COOKIE, HeaderValue::from_str(value).unwrap());
    h
}

pub(crate) fn kiosk_cookie(value: &str) -> HeaderMap {
    cookies(&format!("{KIOSK_COOKIE_NAME}={value}"))
}

#[tokio::test]
async fn leaving_revokes_the_kiosk_this_browser_is_and_nothing_else() {
    let (db, _url) = fresh_db(Schema::Central).await;
    let org = seed_org(&db).await;
    let other_org = seed_org(&db).await;
    let (ours, our_cookie) = bound_kiosk(&db, org, "Front counter").await;
    let (_, their_cookie) = bound_kiosk(&db, other_org, "Their counter").await;

    // A browser that is no kiosk at all has nothing to leave.
    assert!(matches!(
        leave(&db, org, &HeaderMap::new()).await,
        Err(DeviceError::NotFound)
    ));
    // A forged secret for our own kiosk is no kiosk either.
    assert!(matches!(
        leave(&db, org, &kiosk_cookie(&format!("{ours}.forged"))).await,
        Err(DeviceError::NotFound)
    ));
    // Another org's tablet, through this org's route: the same answer, and
    // that tablet is untouched.
    assert!(matches!(
        leave(&db, org, &kiosk_cookie(&their_cookie)).await,
        Err(DeviceError::NotFound)
    ));
    assert!(
        bound_device(&db, &kiosk_cookie(&their_cookie))
            .await
            .expect("the kiosk lookup itself succeeds")
            .is_some(),
        "another org's kiosk must still be bound"
    );

    let left = leave(&db, org, &kiosk_cookie(&our_cookie))
        .await
        .expect("our own kiosk leaves");
    assert_eq!(left.device.id, ours);
    assert!(left.changed, "this call did the revoke");

    // Revoked, not deleted: the row is the record of which tablet a shift was
    // signed in on.
    let row = org_kiosk_devices::Entity::find_by_id(ours)
        .one(&db)
        .await
        .unwrap()
        .expect("the row stays");
    assert!(row.revoked_at.is_some(), "leaving is a revoke");
    // And the cookie the browser held is inert from here on — which is also
    // what makes a clearing header that never arrived harmless.
    assert!(
        bound_device(&db, &kiosk_cookie(&our_cookie))
            .await
            .expect("the kiosk lookup itself succeeds")
            .is_none()
    );
    assert!(matches!(
        leave(&db, org, &kiosk_cookie(&our_cookie)).await,
        Err(DeviceError::NotFound)
    ));
}

/// The handler opens its own connection and verifies the session itself, so
/// point both at this test.
pub(crate) async fn wired_db() -> DatabaseConnection {
    let (db, url) = fresh_db(Schema::Central).await;
    // SAFETY: single-threaded setup before any other env access; nextest runs
    // each test in its own process (asserted by `fresh_db`).
    unsafe {
        std::env::set_var("OXY_DATABASE_URL", &url);
        std::env::remove_var("OXY_DATABASE_AUTH_MODE");
    }
    // Without this the authenticator answers every request as the zero-config
    // guest and never looks at the cookie.
    oxy_auth::built_in::set_auth_configured(true);
    db
}

async fn enrol_page(token: &str, headers: HeaderMap) -> (StatusCode, String) {
    let resp = bind_page(
        headers,
        Query(BindQuery {
            token: token.to_string(),
        }),
    )
    .await;
    let status = resp.status();
    let body = to_bytes(resp.into_body(), usize::MAX).await.expect("body");
    (status, String::from_utf8(body.to_vec()).expect("utf-8"))
}

pub(crate) async fn session_for(user: users::Model) -> HeaderMap {
    let jwt = oxy_app::server::api::auth::create_auth_token(user)
        .await
        .expect("mint a session");
    cookies(&format!("oxy_session={jwt}"))
}

#[tokio::test]
async fn the_enrol_page_names_the_account_a_browser_is_signed_in_to() {
    let db = wired_db().await;
    let org = seed_org(&db).await;
    let (row, token) = create(
        &db,
        org,
        NewDevice {
            name: "Front counter",
            ..Default::default()
        },
    )
    .await
    .expect("create kiosk");
    let email = format!("robert-{}@oxy.tech", Uuid::new_v4().simple());
    let manager = seed_user(&db, Some(email.clone())).await;

    let (status, page) = enrol_page(&token, session_for(manager).await).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        page.contains(&format!("signed in to Oxygen as {email} in this browser")),
        "the warning names the account: {page}"
    );
    assert!(page.contains(">Enroll anyway</button>"), "{page}");

    // Still a GET that binds nothing: the link is live and the row unbound.
    let still = peek_token(&db, &token).await.expect("the link is unspent");
    assert_eq!(still.id, row.id);
    assert!(still.bound_at.is_none() && still.secret_hash.is_none());
}

#[tokio::test]
async fn the_enrol_page_falls_back_to_the_plain_page_whenever_the_session_is_not_an_account() {
    let db = wired_db().await;
    let org = seed_org(&db).await;
    let (_, token) = create(
        &db,
        org,
        NewDevice {
            name: "Front counter",
            ..Default::default()
        },
    )
    .await
    .expect("create kiosk");

    // A crew member's shift session: a real session, but no account behind it.
    let worker = seed_user(&db, None).await;
    let crew = session_for(worker).await;
    // A token that does not verify.
    let garbage = cookies("oxy_session=not-a-jwt");
    // A token that verifies, for a user who is gone.
    let gone = seed_user(
        &db,
        Some(format!("gone-{}@oxy.tech", Uuid::new_v4().simple())),
    )
    .await;
    let gone_session = session_for(gone.clone()).await;
    gone.delete(&db).await.expect("delete user");

    for (case, headers) in [
        ("no cookie", HeaderMap::new()),
        ("crew session", crew),
        ("unverifiable token", garbage),
        ("deleted user", gone_session),
    ] {
        let (status, page) = enrol_page(&token, headers).await;
        assert_eq!(status, StatusCode::OK, "{case}: never an error page");
        assert!(
            page.contains(">Enroll this tablet</button>"),
            "{case}: the plain page: {page}"
        );
        assert!(!page.contains("signed-in-warning"), "{case}: {page}");
    }
}
