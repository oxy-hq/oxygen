//! Kiosk devices: the binding a frontline PIN is only ever usable inside.
//!
//! # The threat, and the shape of the answer
//!
//! A PIN is four to six digits. `POST /api/frontline/login` verified one for
//! any client that could reach the route, so the whole tenant's crew was a
//! brute-force target with a five-digit keyspace, bounded per identifier by the
//! credential lockout and per org by a rate limit — and by nothing else. The
//! design record names device binding as required before this faces a user.
//!
//! This is that binding. An org admin **creates** a device and gets a one-time
//! enrol link; opening it on the tablet **binds** the device, which sets a
//! long-lived HttpOnly cookie (`oxy_kiosk`) holding a random secret. From then
//! on `login` and `roster` answer only requests carrying a cookie that resolves
//! to an unrevoked device **of the same org**: a PIN typed anywhere else is
//! refused with the same 401 as a wrong PIN, so an attacker learns nothing
//! about whether the identifier exists. **Revoke** is a timestamp; the row is
//! the audit trail of which tablet a shift was signed in on.
//!
//! # What this is not
//!
//! Not a second session. The device cookie says *where* a PIN may be entered;
//! the PIN says *who*. A signed-in worker's session is the ordinary session
//! cookie `login` mints, and everything downstream (`user_can_access_app`,
//! the rings) sees a user, not a device. Not device attestation either: a
//! cookie can be copied off a tablet by someone with the tablet. What it
//! closes is the network — a PIN is no longer a bearer credential.
//!
//! # Where things live
//!
//! Secrets are hashed with SHA-256 — they are 256 random bits, so the slow hash
//! the PIN needs (`oxy_auth::frontline::hash_pin`) would buy nothing here and
//! would put an Argon2 pass on every roster read. The pure functions
//! (`create`, `bind_with_token`, `revoke`, `bound_device`) take a database and
//! are what `tests/platform/frontline_devices.rs` drives; the handlers are thin.

use axum::Json;
use axum::extract::{Path, Query};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use chrono::{Duration, Utc};
use entity::{locations, org_kiosk_devices as devices, organizations};
use oxy::database::client::establish_connection;
use oxy_app_core::audit;
use oxy_auth::extractor::AuthenticatedUserExtractor;
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, DatabaseConnection, DbErr, EntityTrait,
    QueryFilter, QueryOrder, QuerySelect, TransactionTrait,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tracing::{info, instrument, warn};
use uuid::Uuid;

use crate::server::api::auth::{
    extract_base_url_from_headers, is_request_secure, validate_return_to_url,
};
use crate::server::api::middlewares::role_guards::OrgAdmin;

/// The cookie an enrolled kiosk carries: `<device id>.<secret>`.
pub const KIOSK_COOKIE_NAME: &str = "oxy_kiosk";
/// An enrol link is good for a day. Long enough to walk the tablet to the
/// counter; short enough that a link left in a chat thread is dead by the time
/// anyone finds it.
const ENROL_LINK_HOURS: i64 = 24;
/// A bound device stays bound for a year of calendar time; a lost tablet is
/// handled by revocation, not by expiry.
const DEVICE_COOKIE_MAX_AGE_SECS: i64 = 365 * 24 * 60 * 60;
const NAME_MAX_CHARS: usize = 80;

/// How long a kiosk may sit untouched before the app signs the shift session
/// out — thirty minutes, unless the admin said otherwise.
///
/// The shift session's own twelve hours (`frontline::SHIFT_HOURS`) is a
/// ceiling sized for a closing shift, not a working rule: a tablet on a
/// counter stays signed in as whoever last touched it, so everything the next
/// person does is attributed to them. **The platform only carries this
/// number.** Nothing here watches a clock — the custom app in crew mode reads
/// it from `GET /api/frontline/device` and closes its own session, which is
/// the only place that can tell "idle" from "reading the screen".
///
/// Thirty rather than the five it shipped with: five minutes is a number a
/// store notices as a nuisance — a closing shift puts the tablet down between
/// tasks — and the tool these stores are moving off (Jolt) signs out at thirty,
/// so it is the duration their crew already have a habit around. It is a
/// DEFAULT, not a rule: a drive-thru tablet nobody stands at can still be
/// enrolled with a shorter one, and a row that names its own number is
/// untouched by this changing, because NULL — not a frozen copy of today's
/// default — is what a kiosk enrolled without one stores.
pub const DEFAULT_IDLE_TIMEOUT_SECONDS: u32 = 30 * 60;
/// Below this a kiosk is unusable — a worker would be signed out mid-tap —
/// and zero would read as "instantly" rather than "never", so it is refused
/// rather than silently treated as one or the other.
const IDLE_TIMEOUT_MIN_SECONDS: u32 = 30;
/// Above the shift itself the number can never fire: the session is gone
/// first. Pinned to `SHIFT_HOURS` rather than to a second copy of 12, so
/// changing the shift length cannot leave a timeout that is dead on arrival.
const IDLE_TIMEOUT_MAX_SECONDS: u32 = super::frontline::SHIFT_HOURS as u32 * 3600;

fn sha256_hex(s: &str) -> String {
    hex::encode(Sha256::digest(s.as_bytes()))
}

/// 244 bits from the OS, as 64 hex characters. Two v4 UUIDs rather than a
/// `rand` call: the crate is a workspace dependency whose API moved between
/// majors, and `getrandom` underneath `Uuid::new_v4` is the same source.
fn random_secret() -> String {
    format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
}

/// Constant-time equality over two hex digests. The digests are public-shaped
/// but the comparison must not leak how many leading bytes matched.
fn digest_eq(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.bytes()
        .zip(b.bytes())
        .fold(0u8, |acc, (x, y)| acc | (x ^ y))
        == 0
}

/// The `oxy_kiosk` value, if the request carries one. Same RFC 6265 split as
/// `oxy_auth::built_in::extract_session_cookie`, for the same reason it exists
/// there: callers drifted on the empty-value guard.
pub fn extract_kiosk_cookie(headers: &HeaderMap) -> Option<String> {
    let prefix = format!("{KIOSK_COOKIE_NAME}=");
    for value in headers.get_all(header::COOKIE).iter() {
        let Ok(raw) = value.to_str() else { continue };
        for part in raw.split(';') {
            if let Some(v) = part.trim().strip_prefix(prefix.as_str())
                && !v.is_empty()
            {
                return Some(v.to_string());
            }
        }
    }
    None
}

/// An enrolled, unrevoked kiosk the request proved it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoundDevice {
    pub id: Uuid,
    pub org_id: Uuid,
    pub name: String,
    pub return_to: Option<String>,
    /// Where the tablet sits, when the admin said.
    pub location_id: Option<Uuid>,
    /// Seconds of inactivity after which the app closes the shift session.
    /// Always a number: the stored column is nullable and NULL means the
    /// default, so a caller never has to know that.
    pub idle_timeout_seconds: u32,
}

/// The stored column read as the number a kiosk acts on.
///
/// NULL is the default rather than "no timeout" — that is what lets every row
/// enrolled before this column existed acquire the behaviour without a
/// backfill. A negative value cannot be written through `create`, but the
/// column is a plain `INTEGER` and a hand-written row could carry one; it
/// falls back to the default rather than wrapping into an enormous `u32`.
fn effective_idle_timeout(stored: Option<i32>) -> u32 {
    // The same interval the writer validates. A hand-written row above the
    // ceiling would otherwise be served verbatim, arming a timer that can never
    // fire before the shift session itself is gone.
    stored
        .and_then(|v| u32::try_from(v).ok())
        .filter(|v| (IDLE_TIMEOUT_MIN_SECONDS..=IDLE_TIMEOUT_MAX_SECONDS).contains(v))
        .unwrap_or(DEFAULT_IDLE_TIMEOUT_SECONDS)
}

/// The admin's label, trimmed, or the refusal. One copy, because `create` and
/// `update` have to agree on what a name is — a rename that accepted a blank
/// would leave a kiosk nobody can pick out of the list.
fn validate_name(name: &str) -> Result<String, DeviceError> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > NAME_MAX_CHARS {
        return Err(DeviceError::BadName);
    }
    Ok(name.to_string())
}

/// What an admin asked for, as the column stores it. `None` stays `None` —
/// "use the default", not a copy of today's default frozen into the row.
fn validate_idle_timeout(requested: Option<u32>) -> Result<Option<i32>, DeviceError> {
    match requested {
        None => Ok(None),
        Some(secs) if (IDLE_TIMEOUT_MIN_SECONDS..=IDLE_TIMEOUT_MAX_SECONDS).contains(&secs) => {
            Ok(Some(secs as i32))
        }
        Some(_) => Err(DeviceError::BadIdleTimeout),
    }
}

/// Resolve the device a request's kiosk cookie names — bound, unrevoked, and
/// holding the secret the cookie carries. `None` for every other case, and
/// deliberately one `None`: the caller answers the same way whether the cookie
/// is absent, forged, revoked or stale.
pub async fn bound_device(db: &DatabaseConnection, headers: &HeaderMap) -> Option<BoundDevice> {
    let raw = extract_kiosk_cookie(headers)?;
    let (id, secret) = raw.split_once('.')?;
    let id = Uuid::parse_str(id).ok()?;
    let row = devices::Entity::find_by_id(id)
        .one(db)
        .await
        .ok()
        .flatten()?;
    if row.revoked_at.is_some() || row.bound_at.is_none() {
        return None;
    }
    let expected = row.secret_hash.as_deref()?;
    if !digest_eq(expected, &sha256_hex(secret)) {
        return None;
    }
    Some(BoundDevice {
        id: row.id,
        org_id: row.org_id,
        name: row.name,
        return_to: row.return_to,
        location_id: row.location_id,
        idle_timeout_seconds: effective_idle_timeout(row.idle_timeout_seconds),
    })
}

/// Stamp `last_seen_at`. Best-effort and only on a sign-in, not on every roster
/// read: "when was this tablet last used" is a question about shifts.
pub async fn touch(db: &DatabaseConnection, device_id: Uuid) {
    let am = devices::ActiveModel {
        id: Set(device_id),
        last_seen_at: Set(Some(Utc::now().into())),
        ..Default::default()
    };
    if let Err(e) = am.update(db).await {
        warn!(device = %device_id, error = %e, "kiosk last_seen_at not stamped");
    }
}

#[derive(Debug, thiserror::Error)]
pub enum DeviceError {
    #[error("a device needs a name of 1 to {NAME_MAX_CHARS} characters")]
    BadName,
    #[error("return_to is not a destination this deployment allows")]
    BadReturnTo,
    #[error("no such location in this org")]
    BadLocation,
    #[error(
        "idle_timeout_seconds must be between {IDLE_TIMEOUT_MIN_SECONDS} and \
         {IDLE_TIMEOUT_MAX_SECONDS} seconds"
    )]
    BadIdleTimeout,
    /// Covers expired, already used, revoked and never issued — one arm, so the
    /// public bind route cannot be used to tell those apart.
    #[error("that enroll link is not valid")]
    NoSuchToken,
    #[error("no such device")]
    NotFound,
    /// A new enrol link is only for a tablet that never arrived. A bound kiosk
    /// moving to another tablet is revoke-and-enrol, so a link cannot quietly
    /// re-point a device a shift is already signing in on.
    #[error("only a kiosk still waiting for its tablet can get a new link")]
    NotPending,
    /// A revoked row is the record of which tablet a shift was signed in on.
    /// Renaming it, or moving its timeout, would make that record say something
    /// that was never true of any shift — and neither number can ever apply
    /// again, because the cookie is dead. Bringing a tablet back is enrolling
    /// it, which is where a new name belongs.
    #[error("a revoked kiosk cannot be changed — enroll the tablet again")]
    Revoked,
    #[error("database error: {0}")]
    Db(#[from] DbErr),
}

/// What an admin says about a kiosk when they enrol it.
///
/// Grouped rather than passed as five positional arguments — four of them
/// optional, three of them the same shape — which is a swap waiting to happen
/// at a call site reading `(…, None, None, None, None)`.
#[derive(Debug, Default, Clone)]
pub struct NewDevice<'a> {
    /// The admin's label — "Front counter", "Drive-thru iPad".
    pub name: &'a str,
    /// Where the tablet lands after sign-in; held to the return-to allowlist.
    pub return_to: Option<&'a str>,
    /// The place this tablet sits at — one of this org's locations. It is what
    /// narrows the crew roster to this store.
    pub location_id: Option<Uuid>,
    /// Seconds of inactivity before the app closes the session. `None` leaves
    /// the column NULL, which reads as [`DEFAULT_IDLE_TIMEOUT_SECONDS`].
    pub idle_timeout_seconds: Option<u32>,
    pub created_by: Option<Uuid>,
}

/// Create a device and mint its one-time enrol token. The token is returned
/// exactly once, here; only its hash is stored.
pub async fn create(
    db: &DatabaseConnection,
    org_id: Uuid,
    spec: NewDevice<'_>,
) -> Result<(devices::Model, String), DeviceError> {
    let NewDevice {
        name,
        return_to,
        location_id,
        idle_timeout_seconds,
        created_by,
    } = spec;
    let name = validate_name(name)?;
    let idle_timeout_seconds = validate_idle_timeout(idle_timeout_seconds)?;
    let return_to = match return_to.map(str::trim).filter(|s| !s.is_empty()) {
        None => None,
        Some(url) if validate_return_to_url(url) => Some(url.to_string()),
        Some(_) => return Err(DeviceError::BadReturnTo),
    };
    // A place must be this org's. Checked here rather than left to the FK:
    // another org's location id would otherwise bind a tablet to it.
    if let Some(loc) = location_id {
        let present = locations::Entity::find_by_id(loc)
            .filter(locations::Column::OrgId.eq(org_id))
            .one(db)
            .await?
            .is_some();
        if !present {
            return Err(DeviceError::BadLocation);
        }
    }
    let token = random_secret();
    let now = Utc::now();
    let row = devices::ActiveModel {
        id: Set(Uuid::new_v4()),
        org_id: Set(org_id),
        name: Set(name),
        return_to: Set(return_to),
        enrol_token_hash: Set(Some(sha256_hex(&token))),
        enrol_expires_at: Set(Some((now + Duration::hours(ENROL_LINK_HOURS)).into())),
        secret_hash: Set(None),
        created_by: Set(created_by),
        created_at: Set(now.into()),
        bound_at: Set(None),
        last_seen_at: Set(None),
        revoked_at: Set(None),
        location_id: Set(location_id),
        idle_timeout_seconds: Set(idle_timeout_seconds),
    }
    .insert(db)
    .await?;
    Ok((row, token))
}

/// Trade a one-time enrol token for the device secret. Returns the row and
/// the cookie VALUE (`<id>.<secret>`); the secret is never stored, only hashed.
///
/// Single use by construction: the token hash is cleared in the same UPDATE
/// that sets the secret, and the UPDATE is filtered on the hash still being
/// present, so two tablets opening the same link race on one row and exactly
/// one of them binds.
pub async fn bind_with_token(
    db: &DatabaseConnection,
    token: &str,
) -> Result<(devices::Model, String), DeviceError> {
    let hash = sha256_hex(token.trim());
    let now = Utc::now();
    let row = devices::Entity::find()
        .filter(devices::Column::EnrolTokenHash.eq(&hash))
        .filter(devices::Column::RevokedAt.is_null())
        .filter(devices::Column::BoundAt.is_null())
        .one(db)
        .await?
        .ok_or(DeviceError::NoSuchToken)?;
    if row.enrol_expires_at.is_none_or(|t| t < now) {
        return Err(DeviceError::NoSuchToken);
    }
    let secret = random_secret();
    let res = devices::Entity::update_many()
        .col_expr(
            devices::Column::SecretHash,
            sea_orm::sea_query::Expr::value(sha256_hex(&secret)),
        )
        .col_expr(
            devices::Column::BoundAt,
            sea_orm::sea_query::Expr::value(now),
        )
        .col_expr(
            devices::Column::LastSeenAt,
            sea_orm::sea_query::Expr::value(now),
        )
        .col_expr(
            devices::Column::EnrolTokenHash,
            sea_orm::sea_query::Expr::value(Option::<String>::None),
        )
        .col_expr(
            devices::Column::EnrolExpiresAt,
            sea_orm::sea_query::Expr::value(Option::<chrono::DateTime<chrono::FixedOffset>>::None),
        )
        .filter(devices::Column::Id.eq(row.id))
        .filter(devices::Column::EnrolTokenHash.eq(&hash))
        .exec(db)
        .await?;
    if res.rows_affected != 1 {
        return Err(DeviceError::NoSuchToken);
    }
    let bound = devices::Entity::find_by_id(row.id)
        .one(db)
        .await?
        .ok_or(DeviceError::NotFound)?;
    let cookie_value = format!("{}.{secret}", bound.id);
    Ok((bound, cookie_value))
}

/// Replace a lost or expired enrol link. The old token stops working in the
/// same UPDATE that stores the new hash — there is one hash column — and the
/// deadline restarts at a full [`ENROL_LINK_HOURS`].
///
/// Filtered on the device still being unbound and unrevoked, so a tablet that
/// binds (or an admin who revokes) between the read and the write wins, and
/// this answers `NotPending` rather than handing out a link to a spent row.
pub async fn reissue_link(
    db: &DatabaseConnection,
    org_id: Uuid,
    id: Uuid,
) -> Result<(devices::Model, String), DeviceError> {
    let row = devices::Entity::find_by_id(id)
        .filter(devices::Column::OrgId.eq(org_id))
        .one(db)
        .await?
        .ok_or(DeviceError::NotFound)?;
    if row.bound_at.is_some() || row.revoked_at.is_some() {
        return Err(DeviceError::NotPending);
    }
    let token = random_secret();
    let expires = Utc::now() + Duration::hours(ENROL_LINK_HOURS);
    let res = devices::Entity::update_many()
        .col_expr(
            devices::Column::EnrolTokenHash,
            sea_orm::sea_query::Expr::value(sha256_hex(&token)),
        )
        .col_expr(
            devices::Column::EnrolExpiresAt,
            sea_orm::sea_query::Expr::value(expires),
        )
        .filter(devices::Column::Id.eq(id))
        .filter(devices::Column::OrgId.eq(org_id))
        .filter(devices::Column::BoundAt.is_null())
        .filter(devices::Column::RevokedAt.is_null())
        .exec(db)
        .await?;
    if res.rows_affected != 1 {
        return Err(DeviceError::NotPending);
    }
    let fresh = devices::Entity::find_by_id(id)
        .one(db)
        .await?
        .ok_or(DeviceError::NotFound)?;
    Ok((fresh, token))
}

/// What an admin may change about a kiosk that is already on the counter.
///
/// Every field means "leave this alone" when it is `None`, so a caller sending
/// one field cannot blank the others — the shape a PATCH body needs.
///
/// Deliberately narrow. `return_to` and `location_id` are NOT here: both decide
/// what a *bound* tablet does next — which app it opens, whose names its crew
/// picker shows — and moving them under a shift that is signed in is a
/// different act from tuning a number, with its own questions (the roster the
/// tablet is already displaying, the cached access verdict). Those stay
/// revoke-and-enrol until someone asks for them.
#[derive(Debug, Default, Clone)]
pub struct DeviceUpdate<'a> {
    /// The admin's label. Same 1..=[`NAME_MAX_CHARS`] rule as enrolment.
    pub name: Option<&'a str>,
    /// `Some(Some(secs))` sets it, within the same bounds `create` holds;
    /// `Some(None)` clears the column back to NULL, which is how a kiosk goes
    /// back to following [`DEFAULT_IDLE_TIMEOUT_SECONDS`] — including if the
    /// default moves again. `None` leaves whatever is there.
    ///
    /// Three states rather than two, because "set it to 1800" and "follow the
    /// platform" are different rows with the same effect today and different
    /// effects the next time the default changes.
    pub idle_timeout_seconds: Option<Option<u32>>,
}

/// Change a kiosk in place — the alternative to revoking a tablet and walking
/// a new enrol link out to the counter because one number was wrong.
///
/// Answers the row **as it was and as it is**, so the caller can audit the
/// difference without reading it again.
///
/// One transaction, and the first read takes the row lock (`FOR UPDATE`).
/// Another admin's edit to the same kiosk therefore either committed before
/// this read — and is part of "as it was" — or waits until this commits. As
/// three free-standing statements the pair could straddle that other write,
/// and the audit trail signed it with this admin's name. `after` is read inside
/// the same transaction, so it is the row this write produced.
///
/// The org filter is the same fence `revoke` puts up: another tenant's device
/// id is `NotFound`, not a silent no-op. Revoked rows are refused — see
/// [`DeviceError::Revoked`]. An update naming no field at all is not an error;
/// it reads the row back unchanged and writes nothing, so a client that
/// submits an untouched form does not manufacture an audit entry. Every early
/// return drops the transaction, which rolls it back and releases the lock.
pub async fn update(
    db: &DatabaseConnection,
    org_id: Uuid,
    id: Uuid,
    spec: DeviceUpdate<'_>,
) -> Result<(devices::Model, devices::Model), DeviceError> {
    let txn = db.begin().await?;
    let before = devices::Entity::find_by_id(id)
        .filter(devices::Column::OrgId.eq(org_id))
        .lock_exclusive()
        .one(&txn)
        .await?
        .ok_or(DeviceError::NotFound)?;
    if before.revoked_at.is_some() {
        return Err(DeviceError::Revoked);
    }
    let Some(fields) = update_fields(spec)? else {
        return Ok((before.clone(), before));
    };
    // Still filtered on the row being this org's and live. Under the lock a
    // revoke can no longer land between the read and the write; the filter
    // stays as a second fence, and it costs nothing.
    let res = devices::Entity::update_many()
        .set(fields)
        .filter(devices::Column::Id.eq(id))
        .filter(devices::Column::OrgId.eq(org_id))
        .filter(devices::Column::RevokedAt.is_null())
        .exec(&txn)
        .await?;
    if res.rows_affected != 1 {
        return Err(DeviceError::Revoked);
    }
    let after = devices::Entity::find_by_id(id)
        .one(&txn)
        .await?
        .ok_or(DeviceError::NotFound)?;
    txn.commit().await?;
    Ok((before, after))
}

/// The columns an update writes, validated — or `None` when it names none.
///
/// Everything is validated before anything is written: a body carrying a good
/// name and an impossible timeout must leave the row exactly as it was, or the
/// 400 the admin reads would be a lie about half of it.
fn update_fields(spec: DeviceUpdate<'_>) -> Result<Option<devices::ActiveModel>, DeviceError> {
    let mut fields = devices::ActiveModel {
        ..Default::default()
    };
    let mut touched = false;
    if let Some(name) = spec.name {
        fields.name = Set(validate_name(name)?);
        touched = true;
    }
    if let Some(requested) = spec.idle_timeout_seconds {
        fields.idle_timeout_seconds = Set(validate_idle_timeout(requested)?);
        touched = true;
    }
    Ok(touched.then_some(fields))
}

/// Switch a device off. `Ok(true)` when this call did it, `Ok(false)` when it
/// was already off; `NotFound` when the org has no such device — the org
/// filter is what keeps one tenant from revoking another's tablets.
pub async fn revoke(db: &DatabaseConnection, org_id: Uuid, id: Uuid) -> Result<bool, DeviceError> {
    let row = devices::Entity::find_by_id(id)
        .filter(devices::Column::OrgId.eq(org_id))
        .one(db)
        .await?
        .ok_or(DeviceError::NotFound)?;
    if row.revoked_at.is_some() {
        return Ok(false);
    }
    let res = devices::Entity::update_many()
        .col_expr(
            devices::Column::RevokedAt,
            sea_orm::sea_query::Expr::value(Utc::now()),
        )
        .filter(devices::Column::Id.eq(id))
        .filter(devices::Column::RevokedAt.is_null())
        .exec(db)
        .await?;
    Ok(res.rows_affected == 1)
}

fn kiosk_cookie(value: &str, secure: bool) -> String {
    let mut parts = vec![
        format!("{KIOSK_COOKIE_NAME}={value}"),
        "Path=/".to_string(),
        format!("Max-Age={DEVICE_COOKIE_MAX_AGE_SECS}"),
        "HttpOnly".to_string(),
        "SameSite=Lax".to_string(),
    ];
    if secure {
        parts.push("Secure".to_string());
    }
    // Same `Domain` rule as the session cookie, or the two would disagree on
    // which hosts a kiosk is a kiosk for.
    if let Ok(domain) = std::env::var("OXY_SESSION_COOKIE_DOMAIN") {
        let domain = domain.trim();
        if !domain.is_empty() {
            parts.push(format!("Domain={domain}"));
        }
    }
    parts.join("; ")
}

fn json_error(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, Json(serde_json::json!({ "error": msg.into() }))).into_response()
}

/// A response whose body depends on the kiosk cookie must never be served from
/// a cache keyed on the URL alone: a shared cache in front of the fleet would
/// hand one kiosk's roster — or its org and device name — to an unbound
/// browser, or fill from an unbound request and blank the real kiosk. There is
/// no global cache layer on `/api`, so each cookie-gated handler says so itself.
pub fn no_store(resp: &mut Response) {
    let h = resp.headers_mut();
    h.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("no-store, private"),
    );
    h.insert(header::VARY, HeaderValue::from_static("Cookie"));
}

fn html(status: StatusCode, body: String) -> Response {
    let mut resp = (
        status,
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        body,
    )
        .into_response();
    no_store(&mut resp);
    resp
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

const PAGE_CSS: &str = "body{font:16px/1.5 system-ui,sans-serif;max-width:32rem;\
margin:15vh auto;padding:0 1.5rem;color:#15191c}\
h1{font-size:1.4rem;margin:0 0 .5rem}p{color:#4d585f}\
button{font:inherit;font-size:1.1rem;padding:.9rem 1.4rem;border:0;border-radius:6px;\
background:#245a86;color:#fff;width:100%;margin-top:1.2rem}";

/// The tablet reaches the enrol routes by navigation, so even "the database is
/// away" has to be a page and not JSON.
fn unavailable_page() -> Response {
    html(
        StatusCode::SERVICE_UNAVAILABLE,
        format!(
            "<!doctype html><meta name=viewport content=\"width=device-width\">\
             <meta name=\"referrer\" content=\"no-referrer\">\
             <title>Enroll kiosk</title><style>{PAGE_CSS}</style>\
             <h1>Enrollment is not available right now.</h1>\
             <p>Nothing was changed. Try the link again in a moment.</p>"
        ),
    )
}

fn dead_link_page() -> Response {
    html(
        StatusCode::NOT_FOUND,
        format!(
            "<!doctype html><meta name=viewport content=\"width=device-width\">\
             <title>Kiosk link</title><style>{PAGE_CSS}</style>\
             <h1>This kiosk link is not valid any more.</h1>\
             <p>It may have expired, or already been used. \
             Ask your manager for a new one.</p>"
        ),
    )
}

/// Look a token up WITHOUT spending it — what the confirm page needs to name
/// the device. Same filters as the bind, so the page and the bind agree on
/// which links are live.
pub async fn peek_token(
    db: &DatabaseConnection,
    token: &str,
) -> Result<devices::Model, DeviceError> {
    let row = devices::Entity::find()
        .filter(devices::Column::EnrolTokenHash.eq(sha256_hex(token.trim())))
        .filter(devices::Column::RevokedAt.is_null())
        .filter(devices::Column::BoundAt.is_null())
        .one(db)
        .await?
        .ok_or(DeviceError::NoSuchToken)?;
    if row.enrol_expires_at.is_none_or(|t| t < Utc::now()) {
        return Err(DeviceError::NoSuchToken);
    }
    Ok(row)
}

// ── Handlers ────────────────────────────────────────────────────────────────

/// `GET /api/frontline/device` — public. Tells the login page whether it is
/// running on an enrolled kiosk, and for which org, so it can offer crew
/// sign-in. Never an error: an unreadable cookie is "not a kiosk".
pub async fn device_status(headers: HeaderMap) -> Response {
    let mut resp = device_status_body(&headers).await;
    no_store(&mut resp);
    resp
}

async fn device_status_body(headers: &HeaderMap) -> Response {
    let unbound = || Json(serde_json::json!({ "bound": false })).into_response();
    let Ok(db) = establish_connection().await else {
        return unbound();
    };
    let Some(device) = bound_device(&db, headers).await else {
        return unbound();
    };
    let Ok(Some(org)) = organizations::Entity::find_by_id(device.org_id)
        .one(&db)
        .await
    else {
        return unbound();
    };
    // The place, by name, so the login page can say "Front counter · Clovis".
    let location = match device.location_id {
        Some(id) => locations::Entity::find_by_id(id)
            .one(&db)
            .await
            .ok()
            .flatten()
            .map(|l| serde_json::json!({ "id": l.id, "name": l.name })),
        None => None,
    };
    Json(serde_json::json!({
        "bound": true,
        "org": org.slug,
        "orgName": org.name,
        "device": device.name,
        "returnTo": device.return_to,
        "location": location,
        // What the app in crew mode arms its own idle timer with. Always
        // present, so a client never has to carry a copy of the default.
        "idleTimeoutSeconds": device.idle_timeout_seconds,
    }))
    .into_response()
}

#[derive(Debug, Deserialize)]
pub struct BindQuery {
    pub token: String,
}

/// `GET /api/frontline/devices/bind?token=…` — public; the enrol link. Shows
/// a confirm page and binds NOTHING: the link travels through Slack, email or
/// SMS on its way to the tablet, and every one of those unfurls or scans URLs.
/// A GET that spent the single-use token would be consumed by the first bot to
/// look at it and the tablet would find a dead link. The binding happens on the
/// form's POST, which no unfurler submits (and which is what RFC 9110 says a
/// side effect needs anyway).
pub async fn bind_page(Query(q): Query<BindQuery>) -> Response {
    let Ok(db) = establish_connection().await else {
        return unavailable_page();
    };
    match peek_token(&db, &q.token).await {
        Ok(row) => html(
            StatusCode::OK,
            format!(
                "<!doctype html><meta name=viewport content=\"width=device-width\">\
                 <meta name=\"referrer\" content=\"no-referrer\">\
                 <title>Enroll kiosk</title><style>{PAGE_CSS}</style>\
                 <h1>Enroll this tablet as \u{201c}{name}\u{201d}?</h1>\
                 <p>Only do this on the device that will stay at the counter. \
                 It signs the crew in from here on.</p>\
                 <form method=\"post\" action=\"/api/frontline/devices/bind\">\
                 <input type=\"hidden\" name=\"token\" value=\"{token}\">\
                 <button type=\"submit\">Enroll this tablet</button></form>",
                name = escape(&row.name),
                token = escape(q.token.trim()),
            ),
        ),
        Err(DeviceError::NoSuchToken) => dead_link_page(),
        Err(e) => {
            warn!(error = %e, "kiosk enrol page failed");
            unavailable_page()
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct BindForm {
    pub token: String,
}

/// `POST /api/frontline/devices/bind` (form body `token=…`) — public; the
/// confirm page's submit. Binds the device, sets the kiosk cookie, and sends
/// the tablet to the login page (with the app the admin named as `return_to`,
/// when there is one).
///
/// The token is spent by `bind_with_token` BEFORE the cookie is built, so a
/// cookie the header layer rejects — a stray character in
/// `OXY_SESSION_COOKIE_DOMAIN`, say — must not be silently dropped into a
/// redirect that looks like success: the link is dead by then and every retry
/// would look identical. It is logged and answered as the failure it is.
pub async fn bind_submit(headers: HeaderMap, body: String) -> Response {
    let token = match serde_urlencoded::from_str::<BindForm>(&body) {
        Ok(f) => f.token,
        Err(_) => return dead_link_page(),
    };
    let Ok(db) = establish_connection().await else {
        return unavailable_page();
    };
    match bind_with_token(&db, &token).await {
        Ok((row, cookie_value)) => {
            let cookie = kiosk_cookie(&cookie_value, is_request_secure(&headers));
            let Ok(cookie) = HeaderValue::from_str(&cookie) else {
                warn!(
                    device = %row.id,
                    "kiosk bound but its cookie header was rejected — check \
                     OXY_SESSION_COOKIE_DOMAIN; the enrol link is spent, create another device"
                );
                return html(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!(
                        "<!doctype html><meta name=viewport content=\"width=device-width\">\
                         <title>Enroll kiosk</title><style>{PAGE_CSS}</style>\
                         <h1>This tablet could not be enrolled.</h1>\
                         <p>The link has been used but the device could not be remembered on this \
                         browser. Ask your manager to create the kiosk again and to check the \
                         server's cookie settings.</p>"
                    ),
                );
            };
            info!(device = %row.id, org = %row.org_id, "kiosk bound");
            let to = match row.return_to.as_deref() {
                Some(url) => format!("/login?return_to={}", urlencoding::encode(url)),
                None => "/login".to_string(),
            };
            let mut resp = Redirect::to(&to).into_response();
            resp.headers_mut().insert(header::SET_COOKIE, cookie);
            no_store(&mut resp);
            resp
        }
        Err(DeviceError::NoSuchToken) => dead_link_page(),
        Err(e) => {
            warn!(error = %e, "kiosk bind failed");
            unavailable_page()
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct CreateDeviceRequest {
    pub name: String,
    #[serde(default)]
    pub return_to: Option<String>,
    /// Where the tablet sits — one of this org's locations, or none. It also
    /// decides whose names the crew picker shows: a kiosk with a place lists
    /// only the workers assigned there.
    #[serde(default)]
    pub location_id: Option<Uuid>,
    /// Seconds of inactivity after which the app signs the shift out. Omitted
    /// leaves the column NULL, which every reader takes as
    /// [`DEFAULT_IDLE_TIMEOUT_SECONDS`].
    #[serde(default)]
    pub idle_timeout_seconds: Option<u32>,
}

#[derive(Debug, Serialize)]
pub struct CreatedDevice {
    pub id: Uuid,
    pub name: String,
    /// Open this on the tablet. Shown once; the server keeps only a hash.
    ///
    /// Composed on the origin the request arrived on. A browser sends
    /// `Origin`; a CLI (`oxyc`) sends neither that nor `Referer`, and the
    /// shared base-URL helper then answers `http://localhost:3000` — which is
    /// what the dev probe of 2026-09-07 got back. So this route also honours
    /// `Host` (with `X-Forwarded-Proto`), and `bind_path` is beside it for a
    /// client that would rather compose against an origin it knows.
    pub enrol_url: String,
    /// The path half of `enrol_url`, for composing against any origin.
    pub bind_path: String,
    pub expires_at: String,
}

/// The origin to mint the enrol link on: the shared helper's answer when the
/// request carried `Origin` or `Referer`, else `Host` + forwarded scheme, else
/// the helper's localhost fallback.
fn enrol_link_base(headers: &HeaderMap) -> String {
    let from_headers = extract_base_url_from_headers(headers);
    let browser_told_us =
        headers.contains_key(header::ORIGIN) || headers.contains_key(header::REFERER);
    if browser_told_us {
        return from_headers;
    }
    let Some(host) = headers.get(header::HOST).and_then(|h| h.to_str().ok()) else {
        return from_headers;
    };
    let scheme = match headers
        .get("x-forwarded-proto")
        .and_then(|h| h.to_str().ok())
    {
        Some(p) if p.eq_ignore_ascii_case("http") => "http",
        Some(_) => "https",
        None if is_request_secure(headers) => "https",
        None if host.starts_with("localhost") || host.starts_with("127.0.0.1") => "http",
        None => "https",
    };
    format!("{scheme}://{host}")
}

impl CreatedDevice {
    /// The one place a token becomes a link, so create and reissue cannot
    /// drift on the path or the origin.
    fn new(row: devices::Model, token: &str, headers: &HeaderMap) -> Self {
        let bind_path = format!("/api/frontline/devices/bind?token={token}");
        Self {
            id: row.id,
            name: row.name,
            enrol_url: format!("{}{bind_path}", enrol_link_base(headers)),
            bind_path,
            expires_at: row
                .enrol_expires_at
                .map(|t| t.to_rfc3339())
                .unwrap_or_default(),
        }
    }
}

/// `POST /api/orgs/{org_id}/frontline/devices` — org admin. Creates a device
/// and answers with the one-time enrol link.
#[instrument(skip_all, fields(org = %org_id))]
pub async fn create_device(
    OrgAdmin(_ctx): OrgAdmin,
    AuthenticatedUserExtractor(actor): AuthenticatedUserExtractor,
    Path(org_id): Path<Uuid>,
    headers: HeaderMap,
    Json(req): Json<CreateDeviceRequest>,
) -> Response {
    let Ok(db) = establish_connection().await else {
        return json_error(StatusCode::SERVICE_UNAVAILABLE, "database unavailable");
    };
    match create(
        &db,
        org_id,
        NewDevice {
            name: &req.name,
            return_to: req.return_to.as_deref(),
            location_id: req.location_id,
            idle_timeout_seconds: req.idle_timeout_seconds,
            created_by: Some(actor.id),
        },
    )
    .await
    {
        Ok((row, token)) => {
            audit::record_best_effort(
                &db,
                audit::AuditEntry::new(actor.label().to_string(), "frontline.device.created")
                    .actor(actor.id, audit::ActorType::User)
                    .org(org_id)
                    .target("frontline_device", row.id.to_string(), row.name.clone()),
            )
            .await;
            (
                StatusCode::CREATED,
                Json(CreatedDevice::new(row, &token, &headers)),
            )
                .into_response()
        }
        Err(
            e @ (DeviceError::BadName
            | DeviceError::BadReturnTo
            | DeviceError::BadLocation
            | DeviceError::BadIdleTimeout),
        ) => json_error(StatusCode::BAD_REQUEST, e.to_string()),
        Err(e) => {
            warn!(error = %e, "kiosk device create failed");
            json_error(StatusCode::INTERNAL_SERVER_ERROR, "create failed")
        }
    }
}

#[derive(Debug, Serialize)]
pub struct DeviceRow {
    pub id: Uuid,
    pub name: String,
    pub return_to: Option<String>,
    pub created_at: String,
    pub bound_at: Option<String>,
    pub last_seen_at: Option<String>,
    pub revoked_at: Option<String>,
    /// Set while an enrol link is outstanding. The link itself is not
    /// recoverable; `POST …/devices/{id}/enrol-link` replaces a lost one.
    pub enrol_expires_at: Option<String>,
    pub location_id: Option<Uuid>,
    pub location_name: Option<String>,
    /// The effective value — never null, the default filled in — so an admin
    /// can read back what the tablet acts on.
    ///
    /// Because the default is resolved here, a kiosk storing NULL and one
    /// storing exactly [`DEFAULT_IDLE_TIMEOUT_SECONDS`] are indistinguishable
    /// on the wire. That is deliberate (a reader never needs a copy of the
    /// default) and it is why `PATCH …/devices/{id}` takes `null` as "follow
    /// the platform" rather than expecting a client to spell the number.
    pub idle_timeout_seconds: u32,
}

impl DeviceRow {
    /// The one place a stored row becomes the shape the settings list reads,
    /// so the list and the update response cannot drift on a field.
    fn new(r: devices::Model, location_name: Option<String>) -> Self {
        let rfc = |t: Option<chrono::DateTime<chrono::FixedOffset>>| t.map(|t| t.to_rfc3339());
        Self {
            id: r.id,
            name: r.name,
            return_to: r.return_to,
            created_at: r.created_at.to_rfc3339(),
            bound_at: rfc(r.bound_at),
            last_seen_at: rfc(r.last_seen_at),
            revoked_at: rfc(r.revoked_at),
            enrol_expires_at: rfc(r.enrol_expires_at),
            location_id: r.location_id,
            location_name,
            idle_timeout_seconds: effective_idle_timeout(r.idle_timeout_seconds),
        }
    }
}

/// `GET /api/orgs/{org_id}/frontline/devices` — org admin. Newest first; no
/// hash of anything leaves the server.
#[instrument(skip_all, fields(org = %org_id))]
pub async fn list_devices(OrgAdmin(_ctx): OrgAdmin, Path(org_id): Path<Uuid>) -> Response {
    let Ok(db) = establish_connection().await else {
        return json_error(StatusCode::SERVICE_UNAVAILABLE, "database unavailable");
    };
    match devices::Entity::find()
        .filter(devices::Column::OrgId.eq(org_id))
        .order_by_desc(devices::Column::CreatedAt)
        .all(&db)
        .await
    {
        Ok(rows) => {
            let places: std::collections::HashMap<Uuid, String> = match locations::Entity::find()
                .filter(locations::Column::OrgId.eq(org_id))
                .all(&db)
                .await
            {
                Ok(ls) => ls.into_iter().map(|l| (l.id, l.name)).collect(),
                Err(e) => {
                    warn!(error = %e, "kiosk device list: locations not loaded");
                    Default::default()
                }
            };
            let devices: Vec<DeviceRow> = rows
                .into_iter()
                .map(|r| {
                    let place = r.location_id.and_then(|l| places.get(&l).cloned());
                    DeviceRow::new(r, place)
                })
                .collect();
            Json(serde_json::json!({ "devices": devices })).into_response()
        }
        Err(e) => {
            warn!(error = %e, "kiosk device list failed");
            json_error(StatusCode::INTERNAL_SERVER_ERROR, "list failed")
        }
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct UpdateDeviceRequest {
    /// Absent leaves the label alone; a string renames the kiosk.
    #[serde(default)]
    pub name: Option<String>,
    /// **Absent**, `null` and a number are three different requests, which is
    /// why this is a double option and not a plain one: absent leaves the row
    /// alone, `null` clears the column so the kiosk follows the platform
    /// default again, a number sets it (400 outside the same 30 s … `SHIFT_HOURS`
    /// window enrolment holds). A negative or fractional number is not a `u32`,
    /// so it is refused by the `Json` extractor before that check — 422, with
    /// axum's plain-text body rather than `{"error": …}`.
    ///
    /// A plain `Option` would fold `null` into absent, and then "put this
    /// tablet back on the default" would be a request nobody could make —
    /// which is the hole that made changing a kiosk mean re-enrolling it.
    #[serde(default, deserialize_with = "super::operating_graph::dto::patch")]
    pub idle_timeout_seconds: Option<Option<u32>>,
}

/// `PATCH /api/orgs/{org_id}/frontline/devices/{id}` — org admin. Changes a
/// kiosk's label and how long it may sit idle, without touching its binding:
/// the cookie on the tablet keeps working, so nobody walks a new enrol link out
/// to the counter to move one number.
///
/// `PATCH` because the body names only what changes — and because `null` has to
/// stay distinguishable from absent (see [`UpdateDeviceRequest`]), which a PUT
/// of the whole row would not have needed but also would not have offered.
///
/// Answers the updated [`DeviceRow`], the same shape the list serves, so the
/// settings pane can drop it straight into the row it just edited.
#[instrument(skip_all, fields(org = %org_id, device = %id))]
pub async fn update_device(
    OrgAdmin(_ctx): OrgAdmin,
    AuthenticatedUserExtractor(actor): AuthenticatedUserExtractor,
    Path((org_id, id)): Path<(Uuid, Uuid)>,
    Json(req): Json<UpdateDeviceRequest>,
) -> Response {
    let Ok(db) = establish_connection().await else {
        return json_error(StatusCode::SERVICE_UNAVAILABLE, "database unavailable");
    };
    let spec = DeviceUpdate {
        name: req.name.as_deref(),
        idle_timeout_seconds: req.idle_timeout_seconds,
    };
    match update(&db, org_id, id, spec).await {
        Ok((before, after)) => {
            record_device_update(&db, &actor, org_id, &before, &after).await;
            let place = place_name(&db, org_id, after.location_id).await;
            Json(DeviceRow::new(after, place)).into_response()
        }
        Err(DeviceError::NotFound) => json_error(StatusCode::NOT_FOUND, "no such device"),
        Err(e @ DeviceError::Revoked) => json_error(StatusCode::CONFLICT, e.to_string()),
        Err(e @ (DeviceError::BadName | DeviceError::BadIdleTimeout)) => {
            json_error(StatusCode::BAD_REQUEST, e.to_string())
        }
        Err(e) => {
            warn!(error = %e, "kiosk device update failed");
            json_error(StatusCode::INTERNAL_SERVER_ERROR, "update failed")
        }
    }
}

/// "Who changed this tablet's sign-out, and from what."
///
/// Only on a real change, for the reason `set_standing` gives: an audit log
/// that records every idempotent resubmit is one nobody reads. Best-effort, so
/// a trail that cannot be written does not undo a change that has happened.
///
/// The trail carries the **stored** value, null and all — not the effective one
/// — because "cleared it back to the default" and "typed 1800" are the two
/// states this route exists to tell apart, and `DeviceRow` resolves them into
/// the same number.
async fn record_device_update(
    db: &DatabaseConnection,
    actor: &oxy_auth::types::AuthenticatedUser,
    org_id: Uuid,
    before: &devices::Model,
    after: &devices::Model,
) {
    if before.name == after.name && before.idle_timeout_seconds == after.idle_timeout_seconds {
        return;
    }
    let state = |r: &devices::Model| serde_json::json!({ "name": r.name, "idle_timeout_seconds": r.idle_timeout_seconds });
    audit::record_best_effort(
        db,
        audit::AuditEntry::new(actor.label().to_string(), "frontline.device.updated")
            .actor(actor.id, audit::ActorType::User)
            .org(org_id)
            .target("frontline_device", after.id.to_string(), after.name.clone())
            .change(state(before), state(after)),
    )
    .await;
}

/// The name of the place a kiosk sits at, held to this org. `None` for a kiosk
/// with no location, one whose location was deleted, and one whose place
/// belongs to another tenant — the list answers the same way, and none of the
/// three is worth failing a write that already landed.
async fn place_name(db: &DatabaseConnection, org_id: Uuid, id: Option<Uuid>) -> Option<String> {
    let id = id?;
    locations::Entity::find_by_id(id)
        .filter(locations::Column::OrgId.eq(org_id))
        .one(db)
        .await
        .ok()
        .flatten()
        .map(|l| l.name)
}

/// `POST /api/orgs/{org_id}/frontline/devices/{id}/enrol-link` — org admin.
/// A new one-time link for a kiosk whose tablet never bound; the previous link
/// dies. Answers the same body as create. 409 once the kiosk is bound or
/// revoked.
#[instrument(skip_all, fields(org = %org_id, device = %id))]
pub async fn reissue_enrol_link(
    OrgAdmin(_ctx): OrgAdmin,
    AuthenticatedUserExtractor(actor): AuthenticatedUserExtractor,
    Path((org_id, id)): Path<(Uuid, Uuid)>,
    headers: HeaderMap,
) -> Response {
    let Ok(db) = establish_connection().await else {
        return json_error(StatusCode::SERVICE_UNAVAILABLE, "database unavailable");
    };
    match reissue_link(&db, org_id, id).await {
        Ok((row, token)) => {
            audit::record_best_effort(
                &db,
                audit::AuditEntry::new(actor.label().to_string(), "frontline.device.link_reissued")
                    .actor(actor.id, audit::ActorType::User)
                    .org(org_id)
                    .target("frontline_device", row.id.to_string(), row.name.clone()),
            )
            .await;
            Json(CreatedDevice::new(row, &token, &headers)).into_response()
        }
        Err(DeviceError::NotFound) => json_error(StatusCode::NOT_FOUND, "no such device"),
        Err(e @ DeviceError::NotPending) => json_error(StatusCode::CONFLICT, e.to_string()),
        Err(e) => {
            warn!(error = %e, "kiosk enrol link reissue failed");
            json_error(StatusCode::INTERNAL_SERVER_ERROR, "reissue failed")
        }
    }
}

/// `DELETE /api/orgs/{org_id}/frontline/devices/{id}` — org admin. Revokes;
/// the row stays. A kiosk cookie for a revoked device answers like no cookie.
#[instrument(skip_all, fields(org = %org_id, device = %id))]
pub async fn revoke_device(
    OrgAdmin(_ctx): OrgAdmin,
    AuthenticatedUserExtractor(actor): AuthenticatedUserExtractor,
    Path((org_id, id)): Path<(Uuid, Uuid)>,
) -> Response {
    let Ok(db) = establish_connection().await else {
        return json_error(StatusCode::SERVICE_UNAVAILABLE, "database unavailable");
    };
    match revoke(&db, org_id, id).await {
        Ok(changed) => {
            if changed {
                audit::record_best_effort(
                    &db,
                    audit::AuditEntry::new(actor.label().to_string(), "frontline.device.revoked")
                        .actor(actor.id, audit::ActorType::User)
                        .org(org_id)
                        .target("frontline_device", id.to_string(), String::new()),
                )
                .await;
            }
            StatusCode::NO_CONTENT.into_response()
        }
        Err(DeviceError::NotFound) => json_error(StatusCode::NOT_FOUND, "no such device"),
        Err(e) => {
            warn!(error = %e, "kiosk device revoke failed");
            json_error(StatusCode::INTERNAL_SERVER_ERROR, "revoke failed")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(cookie: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(header::COOKIE, HeaderValue::from_str(cookie).unwrap());
        h
    }

    #[test]
    fn the_kiosk_cookie_is_read_beside_the_session_cookie() {
        assert_eq!(
            extract_kiosk_cookie(&headers("oxy_session=jwt; oxy_kiosk=abc.def; x=y")).as_deref(),
            Some("abc.def")
        );
        assert!(extract_kiosk_cookie(&headers("oxy_session=jwt")).is_none());
        // An empty value is no value — the guard callers drifted on before.
        assert!(extract_kiosk_cookie(&headers("oxy_kiosk=; oxy_session=jwt")).is_none());
    }

    #[test]
    fn digest_comparison_rejects_length_and_content_mismatches() {
        let a = sha256_hex("secret");
        assert!(digest_eq(&a, &sha256_hex("secret")));
        assert!(!digest_eq(&a, &sha256_hex("secret2")));
        assert!(!digest_eq(&a, &a[..10]));
    }

    #[test]
    fn a_cookie_gated_response_is_marked_uncacheable() {
        let mut r = Json(serde_json::json!({ "bound": false })).into_response();
        no_store(&mut r);
        assert_eq!(r.headers()[header::CACHE_CONTROL], "no-store, private");
        assert_eq!(r.headers()[header::VARY], "Cookie");
    }

    #[test]
    fn the_confirm_page_escapes_what_it_prints() {
        assert_eq!(
            escape("Front <counter> & \"bar\""),
            "Front &lt;counter&gt; &amp; &quot;bar&quot;"
        );
    }

    #[test]
    fn the_enrol_link_is_minted_on_the_host_a_cli_arrived_on() {
        let mut h = HeaderMap::new();
        h.insert(header::HOST, HeaderValue::from_static("aip.dev.oxy.tech"));
        h.insert("x-forwarded-proto", HeaderValue::from_static("https"));
        assert_eq!(enrol_link_base(&h), "https://aip.dev.oxy.tech");
        // A browser's Origin still wins, as everywhere else.
        h.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://app.oxygen-hq.com"),
        );
        assert_eq!(enrol_link_base(&h), "https://app.oxygen-hq.com");
        // Nothing at all: the helper's fallback, unchanged.
        assert_eq!(enrol_link_base(&HeaderMap::new()), "http://localhost:3000");
    }

    /// The default is thirty minutes, and it has to be a value the writers
    /// would accept — a default outside its own bounds is one an admin can
    /// never type back in, and `effective_idle_timeout` would hand it out while
    /// `validate_idle_timeout` refused it.
    #[test]
    fn the_default_is_thirty_minutes_inside_its_own_bounds() {
        assert_eq!(
            DEFAULT_IDLE_TIMEOUT_SECONDS, 1800,
            "the stores' agreed idle sign-out is 30 minutes"
        );
        assert_eq!(
            validate_idle_timeout(Some(DEFAULT_IDLE_TIMEOUT_SECONDS)).unwrap(),
            Some(DEFAULT_IDLE_TIMEOUT_SECONDS as i32),
            "an admin must be able to ask for the default explicitly"
        );
        assert_eq!(
            effective_idle_timeout(Some(DEFAULT_IDLE_TIMEOUT_SECONDS as i32)),
            DEFAULT_IDLE_TIMEOUT_SECONDS,
            "and a row holding it must read back as itself, not be clamped away"
        );
    }

    /// NULL is the default, not "no timeout". Every kiosk enrolled before the
    /// column existed reads as the default, which is what makes the column
    /// nullable instead of a backfill.
    #[test]
    fn an_unset_idle_timeout_reads_as_the_default() {
        assert_eq!(
            effective_idle_timeout(None),
            DEFAULT_IDLE_TIMEOUT_SECONDS,
            "an unset column must not read as 'never sign out'"
        );
        assert_eq!(effective_idle_timeout(Some(900)), 900);
        // A hand-written row cannot turn the timeout into ~4 billion seconds
        // by wrapping, nor into something that signs a worker out mid-tap.
        assert_eq!(
            effective_idle_timeout(Some(-1)),
            DEFAULT_IDLE_TIMEOUT_SECONDS
        );
        assert_eq!(
            effective_idle_timeout(Some(1)),
            DEFAULT_IDLE_TIMEOUT_SECONDS
        );
        // ...nor into one the shift session outlives.
        assert_eq!(
            effective_idle_timeout(Some(IDLE_TIMEOUT_MAX_SECONDS as i32 + 1)),
            DEFAULT_IDLE_TIMEOUT_SECONDS
        );
        assert_eq!(
            effective_idle_timeout(Some(IDLE_TIMEOUT_MAX_SECONDS as i32)),
            IDLE_TIMEOUT_MAX_SECONDS
        );
    }

    #[test]
    fn an_idle_timeout_outside_the_usable_range_is_refused() {
        // Omitted stays omitted: the row must not freeze a copy of today's
        // default, or changing the default would leave old kiosks behind.
        assert_eq!(validate_idle_timeout(None).unwrap(), None);
        assert_eq!(validate_idle_timeout(Some(300)).unwrap(), Some(300));
        assert_eq!(
            validate_idle_timeout(Some(IDLE_TIMEOUT_MIN_SECONDS)).unwrap(),
            Some(IDLE_TIMEOUT_MIN_SECONDS as i32)
        );
        assert_eq!(
            validate_idle_timeout(Some(IDLE_TIMEOUT_MAX_SECONDS)).unwrap(),
            Some(IDLE_TIMEOUT_MAX_SECONDS as i32)
        );
        // Zero is ambiguous ("instantly" or "never"), so it is refused rather
        // than guessed at; past the shift length it could never fire.
        for bad in [0, 1, IDLE_TIMEOUT_MAX_SECONDS + 1, u32::MAX] {
            assert!(
                matches!(
                    validate_idle_timeout(Some(bad)),
                    Err(DeviceError::BadIdleTimeout)
                ),
                "{bad} should not be storable"
            );
        }
    }

    /// The ceiling is the shift, not a second copy of twelve.
    #[test]
    fn the_idle_ceiling_tracks_the_shift_length() {
        assert_eq!(
            IDLE_TIMEOUT_MAX_SECONDS as i64,
            super::super::frontline::SHIFT_HOURS * 3600
        );
    }

    /// The PATCH body has to carry three states, not two.
    ///
    /// "Leave it alone", "put this tablet back on the platform default" and
    /// "make it 900 seconds" are three different requests, and the middle one
    /// is the whole reason a kiosk no longer has to be re-enrolled to change
    /// its sign-out. A plain `Option` folds `null` into absent and quietly
    /// deletes that request — which is why the field is a double option and
    /// why this asserts on the wire form rather than on the struct.
    #[test]
    fn a_patch_body_tells_absent_from_null_from_a_number() {
        let parse = |s: &str| {
            serde_json::from_str::<UpdateDeviceRequest>(s)
                .expect("a PATCH body")
                .idle_timeout_seconds
        };
        assert_eq!(parse("{}"), None, "absent must leave the row alone");
        assert_eq!(
            parse(r#"{"idle_timeout_seconds":null}"#),
            Some(None),
            "null must be expressible, or nothing can go back to the default"
        );
        assert_eq!(parse(r#"{"idle_timeout_seconds":900}"#), Some(Some(900)));
        // A body that only renames says nothing about the timeout.
        assert_eq!(parse(r#"{"name":"Counter 1"}"#), None);
    }

    /// A number the wire type cannot hold never reaches the bounds check.
    ///
    /// `idle_timeout_seconds` is a `u32`, so `-1`, `1.5` and anything past
    /// `u32::MAX` are refused inside axum's `Json` extractor: `422`, with
    /// axum's own plain-text body rather than `{"error": …}`. The `400` and its
    /// bounds sentence are for a whole number out of range. Pinned because the
    /// route's docs (here, `types/frontline.ts`, `frontline-identity.md`) say
    /// which is which.
    #[tokio::test]
    async fn a_timeout_the_wire_type_cannot_hold_is_the_extractors_422() {
        use axum::extract::FromRequest;
        for body in [
            r#"{"idle_timeout_seconds":-1}"#,
            r#"{"idle_timeout_seconds":1.5}"#,
            r#"{"idle_timeout_seconds":4294967296}"#,
        ] {
            let req = axum::http::Request::builder()
                .method("PATCH")
                .header(header::CONTENT_TYPE, "application/json")
                .body(axum::body::Body::from(body))
                .expect("request");
            let refused = Json::<UpdateDeviceRequest>::from_request(req, &())
                .await
                .expect_err("not a u32, so not a body");
            assert_eq!(
                refused.into_response().status(),
                StatusCode::UNPROCESSABLE_ENTITY,
                "{body}"
            );
        }
    }

    /// One name rule, shared by enrolment and rename — a rename that accepted
    /// what enrolment refuses would leave a kiosk nobody can pick out of a list.
    #[test]
    fn a_kiosk_name_is_trimmed_and_never_blank() {
        assert_eq!(validate_name("  Front counter  ").unwrap(), "Front counter");
        for bad in ["", "   ", &"x".repeat(NAME_MAX_CHARS + 1)] {
            assert!(matches!(validate_name(bad), Err(DeviceError::BadName)));
        }
        assert!(validate_name(&"x".repeat(NAME_MAX_CHARS)).is_ok());
    }

    #[test]
    fn a_secret_is_long_and_never_repeats() {
        let a = random_secret();
        assert_eq!(a.len(), 64);
        assert_ne!(a, random_secret());
    }

    #[test]
    fn the_cookie_carries_the_attributes_the_session_cookie_does() {
        let c = kiosk_cookie("id.secret", true);
        for part in [
            "oxy_kiosk=id.secret",
            "Path=/",
            "HttpOnly",
            "SameSite=Lax",
            "Secure",
        ] {
            assert!(c.contains(part), "{c} lacks {part}");
        }
        assert!(!kiosk_cookie("id.secret", false).contains("Secure"));
    }
}
