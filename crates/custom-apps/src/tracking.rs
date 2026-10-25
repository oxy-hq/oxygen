//! Usage tracking for custom apps — view events (server-side,
//! automatic) and custom events (SDK-driven, opt-in).
//!
//! Two responsibilities:
//!
//! 1. **Session-cookie helpers.** A sticky 30-minute `oxy_custom_app_session`
//!    cookie groups multiple views from the same browser session into
//!    one unit. Set by [`session_id_for_serve`] when serving HTML;
//!    parsed by [`session_id_from_headers`] when the SDK posts a
//!    custom event.
//!
//! 2. **Recorders.** [`record_view`] is called from
//!    `custom_apps_serve::serve_dispatch` after the response has been built,
//!    and the beacon route calls [`queue_event`]; both only *enqueue* onto the
//!    bounded in-process queue in `custom_apps_usage_queue`, whose one drain
//!    task batch-inserts. [`record_event`] is the body of the
//!    `POST /api/customer-apps/<id>/events` handler, which must answer with
//!    the new row id and so writes synchronously. All write to PostgreSQL via
//!    SeaORM — see the design doc for the storage rationale (TL;DR: low volume
//!    + simpler Activity-tab queries + works in local dev). An Airhouse sink is
//!    a documented follow-up: `customer-apps-performance.md` § *Usage facts*.
//!
//! **What a view row captures, and why it needs no app cooperation.** The
//! bundle is not asked to report anything: `record_view` runs on the serve
//! path, so an app that never calls `useTrackEvent` still produces a complete
//! "who opened this, when, from where, in what capacity" trail. That is
//! deliberate — an uninstrumented app looking unused is indistinguishable from
//! an app that genuinely is, and the operator asking "who uses this" cannot fix
//! that by editing someone else's bundle. Custom events add detail on top; they
//! are not what makes usage visible.
//!
//! Roles (`app_role` / `org_role`) are snapshotted at view time by
//! [`record_view`], not joined at read time — see the column docs on
//! `entity::custom_app_view_event` for why a usage log that re-derives roles
//! rewrites its own history.
//!
//! Privacy notes — see `internal-docs/customer-apps.md` §13:
//! - Every recorded user is an authenticated Oxy/workspace member; no
//!   anonymous traffic bucket. Recording identity is not new exposure: the
//!   request was authenticated and authorized before it reached here, and the
//!   reader of this data is the app's own admin.
//! - The role columns add a *privilege* label to that identity. Same 90-day
//!   TTL, same app-admin gate — no separate retention or access posture.
//! - Referrer is allowlisted to same-host (third-party refs are dropped
//!   at insert time so external URLs don't leak into the DB).
//! - All data is app-admin-only behind the Activity tab gate.
//! - 90-day TTL via [`retention_cleanup`], run on a 6-hour interval by
//!   [`spawn_retention_cleanup`] which is invoked from
//!   `serve::start_server_and_web_app` at startup. Postgres has no native
//!   row expiry, so unlike observability (where ClickHouse TTL does this),
//!   the prune has to be an app-level loop — "internal-volume product
//!   analytics" doesn't need a tighter window than 6h.

use crate::usage_airhouse;
use crate::usage_queue;
use crate::usage_writes;
use chrono::Utc;
use entity::{custom_app_event, custom_app_view_event};
use http::{HeaderMap, header};
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter,
    TryIntoModel,
};
use uuid::Uuid;

/// Cookie name carrying the sticky session id for custom-app usage
/// tracking. Separate from `oxy_session` (the auth cookie) so an
/// operator can clear analytics state without logging the user out.
pub const SESSION_COOKIE_NAME: &str = "oxy_custom_app_session";

/// Idle timeout — a session ends after 30 minutes of no activity.
/// Standard analytics convention (Plausible / Fathom / GA all use 30m).
const SESSION_TTL_SECS: i64 = 30 * 60;

/// Maximum length of an engineer-supplied event_name. 64 chars is
/// generous for `feature-area-action` style names and short enough to
/// fail fast on accidental novel-length strings.
pub const MAX_EVENT_NAME_LEN: usize = 64;

/// Maximum serialized size of an engineer-supplied event payload.
/// 4 KiB is enough for "the row id, the format, a flag or two"
/// without being a place to dump entire DB rows or arbitrary blobs.
pub const MAX_EVENT_PAYLOAD_BYTES: usize = 4 * 1024;

/// Retention cutoff for both tables — rows older than 90 days are
/// dropped by [`retention_cleanup`]. Cascade-on-app-delete is the
/// other cleanup path.
pub const RETENTION_DAYS: i64 = 90;

// ── Session cookie ──────────────────────────────────────────────────────

/// Parse `oxy_custom_app_session=<uuid>` out of a `Cookie` header.
/// Returns `None` when absent or unparseable — the caller mints a fresh
/// session id in that case.
pub fn session_id_from_headers(headers: &HeaderMap) -> Option<Uuid> {
    let raw = headers.get(header::COOKIE)?.to_str().ok()?;
    for pair in raw.split(';') {
        let pair = pair.trim();
        if let Some(val) = pair.strip_prefix(&format!("{SESSION_COOKIE_NAME}="))
            && let Ok(id) = Uuid::parse_str(val.trim())
        {
            return Some(id);
        }
    }
    None
}

/// Resolve the session id for an incoming HTML serve and build the
/// `Set-Cookie` value that pushes it back to the browser. Returns
/// `(session_id, set_cookie_value)` so the handler can both stamp the
/// recorded view and inject the cookie on the response.
///
/// Behaviour:
/// - If the request already carries an `oxy_custom_app_session` cookie
///   with a parseable UUID, reuse it and refresh the TTL (so an active
///   user's session keeps rolling forward).
/// - Otherwise mint a fresh UUID and set the cookie.
///
/// Cookie is `HttpOnly` + `SameSite=Lax` + `Secure` (when scheme is
/// HTTPS) and scoped to `Path=/`. Domain is intentionally omitted so
/// the cookie defaults to the host that served the response — the
/// session naturally splits between subpath (admin host) and subdomain
/// access, which is the right granularity for "did the user keep
/// hitting THIS app from THIS surface".
pub fn session_id_for_serve(headers: &HeaderMap, secure: bool) -> (Uuid, String) {
    let id = session_id_from_headers(headers).unwrap_or_else(Uuid::new_v4);
    let secure_flag = if secure { "; Secure" } else { "" };
    let cookie = format!(
        "{SESSION_COOKIE_NAME}={id}; Path=/; HttpOnly; SameSite=Lax; Max-Age={SESSION_TTL_SECS}{secure_flag}"
    );
    (id, cookie)
}

// ── Recorders ───────────────────────────────────────────────────────────

/// Queue a view-event row. Synchronous and non-blocking: the caller is the
/// serve path's tail, and a navigation must not wait on — or even spawn for —
/// a usage write. The row is stamped here (`viewed_at` is the navigation's
/// time, not the drain's) and handed to [`usage_queue`], whose single drain
/// task resolves the viewer's roles ([`resolve_view_roles`]) and batch-inserts.
///
/// A full queue drops the row and counts it ([`usage_queue::dropped_total`]):
/// losing a view on overload is the same accepted failure mode as losing one
/// on a crash, and the alternative — backpressure into the HTML response — is
/// the one outcome tracking must never cause.
#[allow(clippy::too_many_arguments)]
pub fn record_view(
    app: entity::apps::Model,
    user_id: Uuid,
    user_email: Option<String>,
    session_id: Uuid,
    referrer: Option<String>,
    user_agent_class: String,
    source: String,
    environment: String,
) {
    usage_queue::enqueue(usage_queue::UsageJob::View(Box::new(
        usage_queue::ViewJob {
            app,
            user_id,
            user_email,
            session_id,
            referrer,
            user_agent_class,
            source,
            environment,
            viewed_at: Utc::now().fixed_offset(),
        },
    )));
}

/// The viewer's app + org standing at view time, for the snapshot columns on
/// `custom_app_view_event`.
///
/// Runs entirely inside the usage queue's drain task, which is what makes the
/// cost acceptable — it never touches the HTML response path, and it is per
/// *navigation*, not per request (the caller records views only for HTML
/// documents, so an asset storm doesn't multiply it). The drain resolves each
/// distinct `(app, viewer)` once per batch, so a burst of reloads by one person
/// costs one fact load, not one per view.
///
/// The cost is **roughly 7 indexed reads**, not the two call sites it looks like:
/// `resolve_app_role`'s fact load (`load_principal_facts_scoped` reads org
/// memberships, platform standing, partner standings and app grants) plus
/// `has_app_grant`'s union, then the membership row. Worth stating plainly,
/// because "two queries" is what this reads like at a glance and the real number
/// is what would matter if view volume ever grew — that is the threshold at which
/// the documented Airhouse sink stops being a follow-up.
///
/// Takes the app row rather than refetching it: `serve_dispatch` has already
/// resolved (and process-cached) it to answer the request at all.
///
/// It runs *before* the insert, which widens the window in which an instance
/// death loses the row. Accepted: losing a view on crash is already this
/// module's documented failure mode, and the alternative — insert, then update
/// with the roles — doubles the writes to narrow a window that costs a single
/// analytics row.
///
/// Every failure is `None`, matching the column contract: an unresolvable role is
/// unrecorded, never guessed. Tracking must not invent a label, and it must
/// certainly not invent `"admin"`.
///
/// The app role deliberately comes from the same `resolve_app_role` that feeds
/// `ctx.user.appRole` rather than a second query written here. The Activity tab
/// and the app's own gate must be describing the same thing, or the log becomes
/// evidence for a rule the platform doesn't actually enforce.
pub async fn resolve_view_roles(
    db: &sea_orm::DatabaseConnection,
    app: &entity::apps::Model,
    caller: &oxy_server_authz::Caller,
) -> (Option<String>, Option<String>) {
    let user_id = caller.user_id;
    let app_role = crate::auth::resolve_app_role(db, caller, app)
        .await
        .unwrap_or_else(|e| {
            tracing::warn!("custom-app view tracking: app role lookup failed: {e}");
            None
        })
        .map(str::to_string);

    let org_role = crate::auth::resolve_org_role(db, user_id, app.org_id)
        .await
        .unwrap_or_else(|e| {
            tracing::warn!("custom-app view tracking: org role lookup failed: {e}");
            None
        })
        .map(str::to_string);

    (app_role, org_role)
}

#[derive(Debug, thiserror::Error)]
pub enum EventError {
    #[error("event_name must match ^[a-z][a-z0-9-]{{0,63}}$ (got {0:?})")]
    BadName(String),
    #[error("payload must be a JSON object, not an array or primitive")]
    NotAnObject,
    #[error("payload exceeds {MAX_EVENT_PAYLOAD_BYTES} bytes serialized")]
    PayloadTooLarge,
    #[error("database error: {0}")]
    Db(String),
}

/// Validate + insert a custom event. Returns the new row id on success
/// so the SDK can correlate its in-memory event with the server-side
/// record if needed.
///
/// Validation rules (kept in sync with the design doc):
/// - `event_name` matches `^[a-z][a-z0-9-]{0,63}$`. Forces a clean
///   namespace and prevents accidental PII in the name itself.
/// - `payload` must be a JSON object (not array / primitive), serialized
///   length ≤ [`MAX_EVENT_PAYLOAD_BYTES`].
///
/// Rate limiting is the caller's responsibility (see the
/// `custom_apps_events` HTTP module which wires this up).
#[allow(clippy::too_many_arguments)]
#[tracing::instrument(skip_all, fields(app_id = %app_id, user_id = %user_id, event_name = %event_name))]
pub async fn record_event(
    db: &DatabaseConnection,
    app_id: Uuid,
    user_id: Uuid,
    user_email: String,
    session_id: Uuid,
    event_name: String,
    payload: serde_json::Value,
    environment: &str,
) -> Result<Uuid, EventError> {
    let model = build_event_model(
        app_id,
        user_id,
        user_email,
        session_id,
        event_name,
        payload,
        environment,
        Utc::now().fixed_offset(),
    )?;
    // A workspace that writes Airhouse only gets the event through the drain,
    // in the batch with the beacon's, and no Postgres row.
    if let Some((workspace_id, _)) = usage_writes::airhouse_only_home(db, app_id).await {
        let row = model
            .clone()
            .try_into_model()
            .map_err(|e| EventError::Db(e.to_string()))?;
        let (id, at) = (row.id, row.occurred_at.with_timezone(&Utc));
        if usage_queue::try_enqueue(usage_queue::UsageJob::Unwritten(Box::new(row))) {
            return Ok(id);
        }
        // The queue is full: keep the event, in Postgres, for a backfill pass
        // to copy, exactly as a failed append is kept.
        model
            .insert(db)
            .await
            .map_err(|e| EventError::Db(e.to_string()))?;
        usage_writes::park(db, &std::collections::HashMap::from([(workspace_id, at)])).await;
        return Ok(id);
    }
    let row = model
        .insert(db)
        .await
        .map_err(|e| EventError::Db(e.to_string()))?;
    // The drain appends a beacon event to the workspace's Airhouse as it writes
    // it; an SDK event gets the same copy, or the Activity tab, which reads
    // Airhouse once a workspace's history is copied, would never show it.
    usage_airhouse::mirror_event(row.clone());
    Ok(row.id)
}

/// Queue a platform-beacon event for the drain task. Same validation as
/// [`record_event`], run when the row is written, so a rejected event is logged
/// there rather than failing a request that has already answered `204`.
#[allow(clippy::too_many_arguments)]
pub fn queue_event(
    app_id: Uuid,
    user_id: Uuid,
    user_email: String,
    session_id: Uuid,
    event_name: String,
    payload: serde_json::Value,
    environment: String,
) {
    usage_queue::enqueue(usage_queue::UsageJob::Event(Box::new(
        usage_queue::EventJob {
            app_id,
            user_id,
            user_email,
            session_id,
            event_name,
            payload,
            environment,
            occurred_at: Utc::now().fixed_offset(),
        },
    )));
}

/// The validated, not-yet-inserted event row. The one place the name/payload
/// rules live, shared by the synchronous SDK route ([`record_event`]) and the
/// queued beacon path, so the two cannot drift.
#[allow(clippy::too_many_arguments)]
pub fn build_event_model(
    app_id: Uuid,
    user_id: Uuid,
    user_email: String,
    session_id: Uuid,
    event_name: String,
    payload: serde_json::Value,
    environment: &str,
    occurred_at: chrono::DateTime<chrono::FixedOffset>,
) -> Result<custom_app_event::ActiveModel, EventError> {
    if !is_valid_event_name(&event_name) {
        return Err(EventError::BadName(event_name));
    }
    if !payload.is_object() {
        return Err(EventError::NotAnObject);
    }
    let serialized = serde_json::to_vec(&payload).map_err(|e| EventError::Db(e.to_string()))?;
    if serialized.len() > MAX_EVENT_PAYLOAD_BYTES {
        return Err(EventError::PayloadTooLarge);
    }
    Ok(custom_app_event::ActiveModel {
        id: ActiveValue::Set(Uuid::new_v4()),
        app_id: ActiveValue::Set(app_id),
        user_id: ActiveValue::Set(user_id),
        user_email: ActiveValue::Set(user_email),
        session_id: ActiveValue::Set(session_id),
        event_name: ActiveValue::Set(event_name),
        payload: ActiveValue::Set(payload),
        occurred_at: ActiveValue::Set(occurred_at),
        environment: ActiveValue::Set(environment.to_string()),
    })
}

/// Drop view + custom-event rows older than [`RETENTION_DAYS`]. Called
/// periodically from the in-proc latency worker. Cascade-on-app-delete
/// is the other cleanup path for "this whole app went away."
///
/// Returns `(views_deleted, events_deleted)` for the log line.
pub async fn retention_cleanup(db: &DatabaseConnection) -> Result<(u64, u64), sea_orm::DbErr> {
    let cutoff = (Utc::now() - chrono::Duration::days(RETENTION_DAYS)).fixed_offset();
    let v = custom_app_view_event::Entity::delete_many()
        .filter(custom_app_view_event::Column::ViewedAt.lt(cutoff))
        .exec(db)
        .await?;
    let e = custom_app_event::Entity::delete_many()
        .filter(custom_app_event::Column::OccurredAt.lt(cutoff))
        .exec(db)
        .await?;
    Ok((v.rows_affected, e.rows_affected))
}

/// Spawn a background task that runs [`retention_cleanup`] every 6
/// hours so the 90-day TTL is actually enforced, and with it the function
/// call-trace prune (`call_trace_store::prune_expired`, a 3-day window). Called once from
/// `serve::start_server_and_web_app` at startup — mirrors how the
/// observability crate spawns its own retention task.
///
/// Cadence rationale: the worst-case lag between "a row passes the
/// 90-day boundary" and "the cleanup task notices" is 6 hours, which
/// is well inside the spirit of a 90-day retention policy. Tighter
/// intervals would just churn an idle DB for no privacy benefit.
///
/// Failure handling: each tick logs and continues. A transient DB
/// error doesn't abort the task — the next tick will retry, and a
/// 6-hour catch-up is well within the implied SLO. Permanent failure
/// modes (schema drift, FK breakage) require operator intervention
/// anyway; logging them is the most we can do without escalating
/// every transient error into a CrashLoopBackoff.
pub fn spawn_retention_cleanup() {
    tokio::spawn(async move {
        // First cleanup after 60s — give startup a chance to finish
        // migrations + the in-proc-worker boot before we run a big
        // delete on cold connections.
        tokio::time::sleep(std::time::Duration::from_secs(60)).await;

        let mut interval = tokio::time::interval(std::time::Duration::from_secs(6 * 3600));
        // Skip missed ticks (e.g. after a long debugger pause or a clock
        // jump) rather than firing a burst of catch-up runs — the
        // dataset doesn't care which tick within the 90-day window
        // ends up clearing it.
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            interval.tick().await;
            let db = match oxy::database::client::establish_connection().await {
                Ok(d) => d,
                Err(e) => {
                    tracing::warn!("custom-app retention cleanup: DB connect failed: {e}");
                    continue;
                }
            };
            match retention_cleanup(&db).await {
                Ok((0, 0)) => {
                    tracing::debug!("custom-app retention cleanup: nothing to purge");
                }
                Ok((v, e)) => {
                    tracing::info!(
                        "custom-app retention cleanup: purged {v} view rows + {e} event rows older than {RETENTION_DAYS}d"
                    );
                }
                Err(e) => {
                    tracing::warn!("custom-app retention cleanup: delete failed: {e}");
                }
            }
            // Function call traces ride the same loop, on their own (much
            // shorter) window: `call_trace_store::retention_days`.
            match crate::call_trace_store::prune_expired(&db).await {
                Ok(0) => {}
                Ok(n) => tracing::info!(
                    "custom-app retention cleanup: purged {n} function call traces older than {}d",
                    crate::call_trace_store::retention_days()
                ),
                Err(e) => {
                    tracing::warn!("custom-app retention cleanup: call trace prune failed: {e}")
                }
            }
        }
    });
}

/// Prefix the platform reserves for its own auto-instrumentation
/// (`custom_apps_beacon`). Names under it are written **only** by the beacon
/// route, which accepts a fixed allowlist and nothing else.
///
/// The SDK route rejects the prefix ([`is_reserved_event_name`]) so an author
/// cannot land rows in the platform's namespace *by accident* — they get a 400
/// naming the prefix and rename. That is the whole property: it is **not** a
/// forgery defence, because the beacon rides the app's own auth gate and the
/// app's own JavaScript can post to it directly. See `custom_apps_beacon`.
pub const RESERVED_EVENT_PREFIX: &str = "oxy-";

/// Is this an engineer-supplied name inside the platform's reserved namespace?
///
/// Checked at the SDK ingest route, not inside [`record_event`]: the beacon
/// writes through the same recorder and its whole purpose is to use these
/// names. Putting the check in the recorder would mean the platform having to
/// bypass its own validator, which is how a bypass becomes general.
pub fn is_reserved_event_name(name: &str) -> bool {
    name.starts_with(RESERVED_EVENT_PREFIX)
}

/// `^[a-z][a-z0-9-]{0,63}$` enforced manually — pulling in a regex
/// crate for a 10-line check would dwarf the function. Same shape the
/// slug validator uses.
pub fn is_valid_event_name(name: &str) -> bool {
    if name.is_empty() || name.len() > MAX_EVENT_NAME_LEN {
        return false;
    }
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_lowercase() {
        return false;
    }
    chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// Same-host allowlist for the `Referer` header. Third-party referrers
/// are dropped so external URLs (e.g. a Slack link → app navigation)
/// don't leak into our DB; same-host refs are kept so an operator can
/// see the in-app navigation path.
pub fn sanitize_referrer(referrer: Option<&str>, host: &str) -> Option<String> {
    let raw = referrer?.trim();
    if raw.is_empty() {
        return None;
    }
    let parsed = url::Url::parse(raw).ok()?;
    let ref_host = parsed.host_str()?;
    if ref_host.eq_ignore_ascii_case(host) {
        Some(raw.to_string())
    } else {
        None
    }
}

// ── Tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn headers_with_cookie(value: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(header::COOKIE, value.parse().unwrap());
        h
    }

    #[test]
    fn parses_session_cookie_when_present() {
        let id = Uuid::new_v4();
        let h = headers_with_cookie(&format!(
            "oxy_session=jwt; oxy_custom_app_session={id}; other=x"
        ));
        assert_eq!(session_id_from_headers(&h), Some(id));
    }

    #[test]
    fn missing_session_cookie_returns_none() {
        let h = headers_with_cookie("oxy_session=jwt");
        assert_eq!(session_id_from_headers(&h), None);
    }

    #[test]
    fn invalid_session_cookie_value_returns_none() {
        let h = headers_with_cookie("oxy_custom_app_session=not-a-uuid");
        assert_eq!(session_id_from_headers(&h), None);
    }

    #[test]
    fn session_id_for_serve_reuses_existing() {
        let id = Uuid::new_v4();
        let h = headers_with_cookie(&format!("oxy_custom_app_session={id}"));
        let (got, cookie) = session_id_for_serve(&h, true);
        assert_eq!(got, id);
        assert!(cookie.contains(&id.to_string()));
        assert!(cookie.contains("HttpOnly"));
        assert!(cookie.contains("Secure"));
    }

    #[test]
    fn session_id_for_serve_mints_when_absent() {
        let h = HeaderMap::new();
        let (id, cookie) = session_id_for_serve(&h, false);
        assert!(cookie.contains(&id.to_string()));
        assert!(!cookie.contains("Secure"));
    }

    #[test]
    fn event_name_validator_matches_design_doc_regex() {
        assert!(is_valid_event_name("a"));
        assert!(is_valid_event_name("export-clicked"));
        assert!(is_valid_event_name("a1-b2-c3"));
        assert!(is_valid_event_name(&"x".repeat(64)));

        assert!(!is_valid_event_name(""));
        assert!(!is_valid_event_name("Export-clicked")); // uppercase
        assert!(!is_valid_event_name("1starts-with-digit"));
        assert!(!is_valid_event_name("-leading-dash"));
        assert!(!is_valid_event_name("with_underscore"));
        assert!(!is_valid_event_name("with space"));
        assert!(!is_valid_event_name(&"x".repeat(65))); // too long
    }

    #[test]
    fn sanitize_referrer_keeps_same_host() {
        assert_eq!(
            sanitize_referrer(
                Some("https://app-dev.oxygen-hq.com/customer-apps/x/y/page"),
                "app-dev.oxygen-hq.com"
            ),
            Some("https://app-dev.oxygen-hq.com/customer-apps/x/y/page".to_string())
        );
    }

    #[test]
    fn sanitize_referrer_drops_third_party() {
        assert_eq!(
            sanitize_referrer(Some("https://slack.com/abc"), "app-dev.oxygen-hq.com"),
            None
        );
    }

    #[test]
    fn sanitize_referrer_drops_unparseable() {
        assert_eq!(
            sanitize_referrer(Some("not-a-url"), "app-dev.oxygen-hq.com"),
            None
        );
        assert_eq!(sanitize_referrer(Some(""), "host"), None);
        assert_eq!(sanitize_referrer(None, "host"), None);
    }
}
