//! Getting a browser out of kiosk mode, and warning before one goes in.
//!
//! Enrolling a browser as a store kiosk sets `oxy_kiosk` for a year
//! ([`super::frontline_devices`]). From then on that browser's `/login` is the
//! crew's name board, for every Oxygen app it opens. Until 2026-09-28 the only
//! way back was an org admin revoking the device in Settings → Crew — from
//! some other browser, because "Sign in as an admin" on the kiosk forwarded the
//! login URL's `return_to`, which on a kiosk is almost always the kiosk's app.
//! A manager who enrolled their own phone during a demo could not undo it from
//! the phone; the #p0 thread of 2026-09-23 ended on "use a private window".
//!
//! Two pieces live here:
//!
//! - [`leave_kiosk`] — `POST /api/orgs/{org_id}/frontline/device/leave`. An org
//!   admin standing at the tablet (the web app's `/kiosk` page) revokes the
//!   kiosk that the request's OWN cookie names, and the response clears the
//!   cookie and its hint. It revokes rather than only clearing: the row is the
//!   audit trail of which tablet a shift was signed in on, and a cleared
//!   cookie alone would leave a live secret on a row that still reads "bound".
//! - [`signed_in_account`] — what the enrol confirm page reads to warn a
//!   browser that is somebody's own before it becomes the store's.
//!
//! Who may leave is `OrgAdmin`, the ring that revokes from Settings. Crew
//! cannot: a frontline worker holds no `org_members` row, so the org
//! middleware turns them away before the guard runs — the tablet belongs to
//! the store, not to whoever is signed in on it.
//!
//! From where, too: only this host. The route takes no body, so a
//! cross-origin POST to it is a "simple request" the browser sends without a
//! preflight, and the session and kiosk cookies are `SameSite=Lax` — a sibling
//! subdomain (another org's custom-app host or org subdomain) is the same
//! site, so it gets both attached. The CORS layer only withholds the answer;
//! the revoke would already have happened. So the handler refuses an `Origin`
//! (or `Referer`) that is not this host, with `is_allowed_origin` — the check
//! the custom-app data gate uses for its own cookie-authenticated routes. The
//! web app calls same-origin (`${origin}/api`; in development the loopback
//! pair that check allows), so it is unaffected.

use axum::extract::Path;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use entity::users::UserStatus;
use oxy::database::client::establish_connection;
use oxy_app_core::audit;
use oxy_auth::authenticator::Authenticator;
use oxy_auth::built_in::{BuiltInAuthenticator, extract_session_cookie};
use oxy_auth::types::AuthenticatedUser;
use oxy_auth::user::{LOCAL_GUEST_EMAIL, UserService};
use sea_orm::DatabaseConnection;
use tracing::{info, instrument, warn};
use uuid::Uuid;

use super::frontline_devices::{
    BoundDevice, DeviceError, bound_device, json_error, no_store, revoke,
};
use super::frontline_kiosk_cookie::clear_kiosk_cookies_on;
use oxy_app::surface::is_allowed_origin;
use oxy_app::surface::role_guards::OrgAdmin;
use oxy_app::surface::session::is_request_secure;

/// What leaving kiosk mode did.
#[derive(Debug)]
pub struct LeftKiosk {
    /// The kiosk the request's cookie named.
    pub device: BoundDevice,
    /// `false` when another admin revoked it between the lookup and the
    /// write. The browser still leaves; only the audit entry is skipped, since
    /// that other revoke already wrote one.
    pub changed: bool,
}

/// Revoke the kiosk the request's own `oxy_kiosk` cookie names, when it is
/// one of `org_id`'s.
///
/// `NotFound` for a request with no live kiosk cookie **and** for one whose
/// cookie names another org's kiosk — one answer, so an admin of org A holding
/// org B's tablet learns nothing about B's device through A's route, and
/// cannot revoke it there (`revoke`'s own org filter would refuse it too).
///
/// `Db` when the lookup itself failed — unknown, which is not `NotFound`.
pub async fn leave(
    db: &DatabaseConnection,
    org_id: Uuid,
    headers: &HeaderMap,
) -> Result<LeftKiosk, DeviceError> {
    let device = bound_device(db, headers)
        .await?
        .filter(|d| d.org_id == org_id)
        .ok_or(DeviceError::NotFound)?;
    let changed = revoke(db, org_id, device.id).await?;
    Ok(LeftKiosk { device, changed })
}

/// `POST /api/orgs/{org_id}/frontline/device/leave` — org admin, from the
/// kiosk itself. Revokes the kiosk this browser's `oxy_kiosk` cookie names and
/// answers 204 with `Set-Cookie`s that expire the cookie and its hint under
/// exactly the attributes they were set with. 403 for a request from another
/// origin (see the module docs); 404 when this browser is not a live kiosk of
/// this org; 503 when the database could not say, and nothing changed. Audited
/// as `frontline.device.revoked`, like a revoke from Settings → Crew, with
/// `metadata.via = "device"`.
#[instrument(skip_all, fields(org = %org_id))]
pub async fn leave_kiosk(
    OrgAdmin(_ctx): OrgAdmin,
    actor: oxy_app_core::audit::RequestActor,
    Path(org_id): Path<Uuid>,
    headers: HeaderMap,
) -> Response {
    if !is_allowed_origin(&headers) {
        warn!("leave kiosk mode refused: the request came from another origin");
        return json_error(StatusCode::FORBIDDEN, "origin not allowed");
    }
    let Ok(db) = establish_connection().await else {
        return json_error(StatusCode::SERVICE_UNAVAILABLE, "database unavailable");
    };
    match leave(&db, org_id, &headers).await {
        Ok(left) => {
            if left.changed {
                record_left(&db, &actor, org_id, &left.device).await;
            }
            info!(device = %left.device.id, "kiosk mode left from the device itself");
            left_response(is_request_secure(&headers))
        }
        Err(DeviceError::NotFound) => json_error(
            StatusCode::NOT_FOUND,
            "this browser is not a kiosk of this organization",
        ),
        Err(DeviceError::Db(e)) => {
            warn!(error = %e, "leaving kiosk mode: the kiosk could not be looked up");
            json_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "could not check this kiosk right now; nothing changed",
            )
        }
        Err(e) => {
            warn!(error = %e, "leaving kiosk mode failed");
            json_error(StatusCode::INTERNAL_SERVER_ERROR, "leave failed")
        }
    }
}

/// The same trail entry a revoke from Settings files, so "who switched this
/// tablet off" has one answer whichever screen they used; `via` tells the two
/// apart. Best-effort, like every audit write here.
async fn record_left(
    db: &DatabaseConnection,
    actor: &oxy_app_core::audit::RequestActor,
    org_id: Uuid,
    device: &BoundDevice,
) {
    audit::record_best_effort(
        db,
        audit::AuditEntry::for_request(actor, "frontline.device.revoked")
            .org(org_id)
            .target(
                "frontline_device",
                device.id.to_string(),
                device.name.clone(),
            )
            .metadata(serde_json::json!({ "via": "device" })),
    )
    .await;
}

/// 204, the clearing cookies, and no caching. A rejected header is logged,
/// not failed — see [`clear_kiosk_cookies_on`].
fn left_response(secure: bool) -> Response {
    let mut resp = StatusCode::NO_CONTENT.into_response();
    clear_kiosk_cookies_on(&mut resp, secure);
    no_store(&mut resp);
    resp
}

/// The Oxygen account this browser is signed in as — its email address — or
/// `None`.
///
/// `None` for no session cookie (checked first, so a store tablet with none
/// costs no lookup), an invalid or expired token, a user who no longer
/// resolves, an inactive user, a frontline worker's shift session (no address
/// by construction — `internal-docs/frontline-identity.md`), the legacy local
/// guest, and a database that is away. The enrol page renders whatever this
/// answers, so every failure is the plain page, never an error page.
///
/// Only the cookie: the enrol page is reached by navigation, and a navigation
/// carries no `Authorization` header. Reads only.
pub(crate) async fn signed_in_account(headers: &HeaderMap) -> Option<String> {
    extract_session_cookie(headers)?;
    let identity = BuiltInAuthenticator::new(oxy_auth::token::SandboxAgent::Refuse)
        .authenticate(headers)
        .await
        .ok()?;
    let user = UserService::find_user_by_identity(&identity)
        .await
        .ok()
        .flatten()?;
    account_label(&user)
}

/// An account is an active user with a real address. A frontline worker has
/// none, and the local guest's is a placeholder.
fn account_label(user: &AuthenticatedUser) -> Option<String> {
    if user.status != UserStatus::Active {
        return None;
    }
    user.email
        .clone()
        .filter(|e| !e.trim().is_empty() && e != LOCAL_GUEST_EMAIL)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::{HeaderValue, header};

    fn user(email: Option<&str>, status: UserStatus) -> AuthenticatedUser {
        AuthenticatedUser {
            id: Uuid::new_v4(),
            email: email.map(str::to_string),
            name: "Robert".into(),
            picture: None,
            status,
            credential: None,
        }
    }

    #[test]
    fn only_an_active_user_with_an_address_counts_as_an_account() {
        assert_eq!(
            account_label(&user(Some("robert@oxy.tech"), UserStatus::Active)).as_deref(),
            Some("robert@oxy.tech")
        );
        // A frontline worker's shift session: no address, by construction.
        assert_eq!(account_label(&user(None, UserStatus::Active)), None);
        assert_eq!(
            account_label(&user(Some("robert@oxy.tech"), UserStatus::Deleted)),
            None
        );
        assert_eq!(
            account_label(&user(Some(LOCAL_GUEST_EMAIL), UserStatus::Active)),
            None
        );
        assert_eq!(account_label(&user(Some("  "), UserStatus::Active)), None);
    }

    /// The web app sends a kiosk's admin sign-in to `<origin>/kiosk`, absolute
    /// (`kioskAdminReturnTo` in `web-app/src/pages/login/signInDestination.ts`),
    /// and every provider's post-login redirect asks this allowlist first. In
    /// production shape — a session-cookie zone, no localhost opt-in — the
    /// app host and an org subdomain pass and loopback does not; local dev
    /// passes loopback only with the opt-in.
    ///
    /// Sets process environment: sound because nextest runs each test in its
    /// own process.
    #[test]
    fn the_kiosk_admin_destination_passes_the_return_to_allowlist_in_production_shape() {
        use oxy_app::surface::session::validate_return_to_url;
        // SAFETY: nextest isolates each test in its own process, and nothing
        // else in this one reads the environment concurrently.
        unsafe {
            std::env::set_var("OXY_SESSION_COOKIE_DOMAIN", ".oxygen-hq.com");
            std::env::remove_var("OXY_AUTH_ALLOW_LOCALHOST_RETURN");
        }
        assert!(validate_return_to_url("https://app.oxygen-hq.com/kiosk"));
        assert!(validate_return_to_url(
            "https://pokehouse.oxygen-hq.com/kiosk"
        ));
        assert!(!validate_return_to_url("http://127.0.0.1:5173/kiosk"));
        unsafe { std::env::set_var("OXY_AUTH_ALLOW_LOCALHOST_RETURN", "1") };
        assert!(validate_return_to_url("http://127.0.0.1:5173/kiosk"));
    }

    #[test]
    fn leaving_answers_204_and_clears_both_cookies_it_was_bound_with() {
        use crate::frontline_kiosk_cookie::clear_kiosk_cookies;
        for secure in [true, false] {
            let resp = left_response(secure);
            assert_eq!(resp.status(), StatusCode::NO_CONTENT);
            let set: Vec<_> = resp
                .headers()
                .get_all(header::SET_COOKIE)
                .iter()
                .map(|v| v.to_str().unwrap().to_string())
                .collect();
            assert_eq!(set, clear_kiosk_cookies(secure).to_vec());
            assert!(set.iter().all(|c| c.contains("Max-Age=0")), "{set:?}");
            assert!(set[1].starts_with("oxy_kiosk_hint=;"), "{set:?}");
            assert_eq!(resp.headers()[header::CACHE_CONTROL], "no-store, private");
        }
    }

    fn with(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (name, value) in pairs {
            h.insert(
                axum::http::HeaderName::from_bytes(name.as_bytes()).unwrap(),
                HeaderValue::from_str(value).unwrap(),
            );
        }
        h
    }

    /// The same-origin door, as `leave_kiosk` calls it: the web app's own
    /// calls pass, a sibling subdomain's does not.
    #[test]
    fn only_this_hosts_pages_may_ask_to_leave() {
        let host = ("host", "app.oxygen-hq.com");
        let cookie = ("cookie", "oxy_session=jwt; oxy_kiosk=id.secret");
        // The SPA, same-origin in production and on an org subdomain.
        assert!(is_allowed_origin(&with(&[
            host,
            cookie,
            ("origin", "https://app.oxygen-hq.com")
        ])));
        assert!(is_allowed_origin(&with(&[
            ("host", "pokehouse.oxygen-hq.com"),
            ("origin", "https://pokehouse.oxygen-hq.com")
        ])));
        // Local development: Vite's page reaching the API on loopback.
        assert!(is_allowed_origin(&with(&[
            ("host", "localhost:3000"),
            ("origin", "http://127.0.0.1:5173")
        ])));
        // Same site, other origin: a custom-app host, another org subdomain,
        // an opaque origin, a Referer-only form post.
        for origin in [
            "https://other-org--store-ops.customer-apps.oxygen-hq.com",
            "https://other-org.oxygen-hq.com",
            "null",
        ] {
            assert!(
                !is_allowed_origin(&with(&[host, cookie, ("origin", origin)])),
                "{origin}"
            );
        }
        assert!(!is_allowed_origin(&with(&[
            host,
            cookie,
            ("referer", "https://other-org.oxygen-hq.com/page")
        ])));
    }
}
