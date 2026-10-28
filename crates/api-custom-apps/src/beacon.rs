//! `POST <app-base>/__oxy/beacon` — ingest for the platform's own
//! auto-instrumentation.
//!
//! ## Why this is not the SDK's `/events` endpoint
//!
//! `POST /api/customer-apps/<project>/events` exists for events an *app author*
//! chooses to send. This one exists for events the *platform* sends on every
//! app's behalf, without the author doing anything — SPA pageviews, Core Web
//! Vitals, engagement time, uncaught-error counts.
//!
//! Keeping them apart buys three things that matter:
//!
//! 1. **The `oxy-` namespace cannot be entered by accident.** Only this route
//!    writes an `oxy-*` event name, and only from a fixed allowlist
//!    ([`PLATFORM_EVENTS`]); the SDK route rejects the prefix with an
//!    explanation, so an author who picks `oxy-export` learns to rename instead
//!    of silently landing rows the Activity tab groups with platform events.
//!
//!    **It is not a forgery defence, and must not be described as one.** This
//!    route is dispatched inside the app's own gate and makes no second
//!    authorization decision — by design. The app's own JavaScript runs on the
//!    app's origin as the authenticated viewer, so it can `fetch` this endpoint
//!    with any allowlisted name and any object payload, exactly as the platform
//!    runtime does. Read an `oxy-*` row as "the platform's client shape wrote
//!    this", never as "the app could not have written this". Making it the
//!    latter would need a second authorization decision here, which is the one
//!    thing this route is built not to have.
//! 2. **Separate rate budgets.** Platform telemetry must not consume the
//!    author's 60-events-per-minute allowance, and a chatty app must not be
//!    able to starve the platform's four-batches-per-session.
//! 3. **It rides the app's own origin and auth gate.** The route is dispatched
//!    from inside `crate::serve::serve_pretty`, *after* the same
//!    authentication and access check every asset passes — so it works
//!    identically on the subpath and subdomain surfaces, needs no CORS, and
//!    cannot be reached by anyone who could not already open the app.
//!
//! ## Shape of the wire format
//!
//! ```json
//! { "v": 1, "events": [ { "n": "oxy-pageview", "t": 1730000000000, "p": {…} } ] }
//! ```
//!
//! Batched because the sender is a `sendBeacon` on page-hide: one request that
//! may not survive the unload is better than four that certainly won't.
//! `t` (the client clock) is accepted and **ignored** for storage — rows are
//! stamped server-side. It is parsed only so the field is part of the contract
//! for a future ordering fix; trusting a client clock to order an analytics
//! table is how you get rows in 1970 and 2087.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use uuid::Uuid;

/// The complete set of event names this route accepts. An allowlist, not a
/// prefix check: the beacon is reachable by any authenticated viewer of the
/// app, so "anything starting with `oxy-`" would make it an open-ended write
/// channel into the events table. Bounding it to four known names keeps the
/// blast radius of that reachability to four row shapes the Activity tab
/// already knows how to render — it does not make those four unforgeable (see
/// the module docs).
pub const PLATFORM_EVENTS: &[&str] = &[
    // A client-side route change in a single-page app. The server records one
    // view per HTML navigation, which for an SPA is one row per session — this
    // is what makes "which screens get used" answerable at all.
    "oxy-pageview",
    // LCP / CLS / INP / FCP / TTFB for one page load.
    "oxy-web-vitals",
    // Time a screen was actually visible, plus scroll depth.
    "oxy-engagement",
    // Uncaught errors. The `counts` map (by constructor name) feeds the
    // Activity tab's 90-day rollup; the `details` array — message, stack,
    // stack hash, build — fans out to `custom_app_client_errors`, a separate
    // table with 30-day retention behind the same app-admin gate.
    //
    // **This module doc used to say "never messages or stacks", and that rule
    // was traded deliberately in 2026-09** (design doc §5.1). It made a
    // white-screened app report `{TypeError: 3}` and nothing anyone could act
    // on. The posture that pays for the text: a separate table, shorter
    // retention, no widening of who may read it — and the residual is stated
    // rather than papered over, because an error message can contain
    // application data and no amount of pattern-matching would reliably
    // remove it.
    "oxy-error",
    // The app's bundle actually mounted. A HEALTH SIGNAL, not analytics: it is
    // exempt from the `analytics: false` opt-out (see `runtime.js`), and it
    // carries no path — only that something rendered, and by which route
    // (`auto` heuristic vs the app calling `window.__oxyAppReady()`).
    //
    // Read it by its ABSENCE. A custom-app host answers 200 with the SPA shell
    // for every path, so a served view with no `app-ready` behind it is the
    // white screen no status code can report. The client deliberately never
    // sends a negative: a boot failure hard enough to blank the page is not
    // reliably alive enough to report itself.
    "oxy-app-ready",
    // A long-open tab noticed a new build (`phase: detected`), reloaded onto it
    // (`applied`), or gave up after a reload landed on the old build twice
    // (`guarded`). Carries the two build ids, the phase, what triggered it
    // (`how`) and how long the tab ran stale — no path, so it is health, not
    // analytics, and rides past `analytics: false`. See `custom_apps_updates`.
    "oxy-update",
];

/// The subset of [`PLATFORM_EVENTS`] that is platform health rather than the
/// author's product analytics, and therefore rides past `analytics: false`.
///
/// Kept here as well as in `runtime.js` on purpose: the client decides what to
/// send, this decides what is mirrored into the operational event stream, and a
/// name drifting out of one list should not silently change the other's meaning.
pub const HEALTH_EVENTS: &[&str] = &["oxy-app-ready", "oxy-error", "oxy-update"];

/// A page reporting its own open, because the server never saw it.
///
/// The serve path records a view when it serves a page, but skips a
/// speculative fetch (`is_speculative_request`): a hover is not an open. When
/// the person then opens the app, the browser can show the page it prefetched
/// without asking the server again, and `runtime.js` sends this instead.
///
/// Not an event row. It is accepted here, taken out of the batch, and written
/// as the same `custom_app_view_event` row the serve path writes, through the
/// caller's recorder. Like that row it is never opt-out-able, and like every
/// name on this route it is not unforgeable: an app's own script could post it,
/// which buys it nothing a reload would not.
pub const VIEW_EVENT: &str = "oxy-view";

/// Is `name` one this route accepts at all?
fn is_accepted(name: &str) -> bool {
    name == VIEW_EVENT || PLATFORM_EVENTS.contains(&name)
}

/// Take the reported open out of a batch: its referrer (empty when the page had
/// none) if the batch carries one, and the events left to record. Only the
/// first counts; a page opens once.
fn take_view(
    events: Vec<(String, serde_json::Value)>,
) -> (Option<String>, Vec<(String, serde_json::Value)>) {
    let mut view = None;
    let mut rest = Vec::with_capacity(events.len());
    for (name, payload) in events {
        if name != VIEW_EVENT {
            rest.push((name, payload));
        } else if view.is_none() {
            let referrer = payload.get("referrer").and_then(|v| v.as_str());
            view = Some(referrer.unwrap_or_default().to_string());
        }
    }
    (view, rest)
}

/// Events accepted per request. Matches the client's own batch cap, so the
/// normal path never trips it — a request over the cap is a client that has
/// been tampered with, and truncating quietly would hide that.
const MAX_EVENTS_PER_BATCH: usize = 20;

/// Bytes accepted per request body. Twenty events at the per-event payload cap
/// would be ~80 KiB, but real platform payloads are tens of bytes each; this
/// bounds what one caller can make the server parse.
pub const MAX_BODY_BYTES: usize = 16 * 1024;

/// Batches accepted per `(user, app)` per minute. A well-behaved client sends
/// at most a handful per session: one flush at ten seconds, one per
/// visibility change, one on unload.
const RATE_PER_MIN: u64 = 30;

const RATE_BUCKET_TTL: Duration = Duration::from_secs(120);

struct RateBucket {
    window_start: Instant,
    count: u64,
}

fn rate_table() -> &'static Mutex<HashMap<(Uuid, Uuid), RateBucket>> {
    static TABLE: OnceLock<Mutex<HashMap<(Uuid, Uuid), RateBucket>>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Per-process, like the SDK route's limiter — `N` replicas mean an effective
/// `RATE_PER_MIN × N`. Called out rather than fixed here because it is the
/// same pre-existing scaling gap tracked for Functions, and closing it needs a
/// shared counter rather than a second copy of the workaround.
fn would_exceed_rate(user_id: Uuid, app_id: Uuid) -> bool {
    let now = Instant::now();
    let Ok(mut table) = rate_table().lock() else {
        // A poisoned lock means a previous caller panicked while holding it.
        // Fail *open* on a telemetry limiter: dropping the whole analytics
        // stream for the life of the process is a worse outcome than an
        // unbounded minute, and nothing here is a security boundary.
        return false;
    };
    table.retain(|_, b| now.duration_since(b.window_start) < RATE_BUCKET_TTL);
    let bucket = table
        .entry((user_id, app_id))
        .or_insert_with(|| RateBucket {
            window_start: now,
            count: 0,
        });
    if now.duration_since(bucket.window_start).as_secs() >= 60 {
        bucket.window_start = now;
        bucket.count = 0;
    }
    bucket.count += 1;
    bucket.count > RATE_PER_MIN
}

#[derive(Debug, Deserialize)]
struct BeaconBatch {
    #[serde(default)]
    v: u32,
    #[serde(default)]
    events: Vec<BeaconEvent>,
}

#[derive(Debug, Deserialize)]
struct BeaconEvent {
    /// Event name — must be one of [`PLATFORM_EVENTS`].
    n: String,
    /// Client timestamp, accepted and ignored. See the module docs.
    #[allow(dead_code)]
    #[serde(default)]
    t: Option<i64>,
    #[serde(default)]
    p: Option<serde_json::Value>,
}

/// Wire version this route understands. A future client sending `v: 2` is
/// rejected rather than parsed optimistically — the sender is served by the
/// same deploy that handles it, so a mismatch means something is genuinely
/// wrong.
const WIRE_VERSION: u32 = 1;

/// The decision half of the handler, separated from the database write so it
/// can be tested without one. Returns the events to record, or the response to
/// send instead.
fn admit(body: &[u8]) -> Result<Vec<(String, serde_json::Value)>, Response> {
    if body.len() > MAX_BODY_BYTES {
        return Err(StatusCode::PAYLOAD_TOO_LARGE.into_response());
    }
    let batch: BeaconBatch = match serde_json::from_slice(body) {
        Ok(b) => b,
        // No error body: the sender is `sendBeacon`, which cannot read one.
        // Anything descriptive here would go only to a log the client can
        // trigger at will.
        Err(_) => return Err(StatusCode::BAD_REQUEST.into_response()),
    };
    if batch.v != WIRE_VERSION {
        return Err(StatusCode::BAD_REQUEST.into_response());
    }
    if batch.events.len() > MAX_EVENTS_PER_BATCH {
        return Err(StatusCode::PAYLOAD_TOO_LARGE.into_response());
    }
    let mut out = Vec::with_capacity(batch.events.len());
    for event in batch.events {
        if !is_accepted(&event.n) {
            // One bad name fails the batch rather than being skipped. A client
            // sending a name this route doesn't know is not a client whose
            // other events are worth keeping — and failing loudly is what makes
            // a client/server version skew visible instead of silently lossy.
            return Err(StatusCode::BAD_REQUEST.into_response());
        }
        let payload = event.p.unwrap_or_else(|| serde_json::json!({}));
        if !payload.is_object() {
            return Err(StatusCode::BAD_REQUEST.into_response());
        }
        out.push((event.n, payload));
    }
    Ok(out)
}

/// Handle a beacon POST.
///
/// The caller has already authenticated the request and confirmed the viewer
/// may open this app — this route is dispatched from inside the same gate the
/// bundle's bytes pass through, so there is no second authorization decision
/// here by design.
///
/// Always answers `204` on success and never blocks on the write: the sender is
/// a page that is usually in the middle of unloading, and a slow insert would
/// hold the browser's unload path open for telemetry. The write is a `try_send`
/// onto the bounded usage queue (`custom_apps_usage_queue`).
///
/// `record_view` writes a reported open ([`VIEW_EVENT`]) with the page's
/// referrer, and returns the tracking session's `Set-Cookie` when it minted one;
/// the response carries it, so the session starts with the open, as it does
/// when the server records the view itself.
#[allow(clippy::too_many_arguments)]
pub async fn handle(
    app_id: Uuid,
    org_id: Uuid,
    user_id: Uuid,
    user_email: String,
    headers: &HeaderMap,
    body: axum::body::Bytes,
    environment: String,
    record_view: impl FnOnce(Option<&str>) -> Option<String>,
) -> Response {
    let events = match admit(&body) {
        Ok(e) => e,
        Err(response) => return response,
    };
    if events.is_empty() {
        return StatusCode::NO_CONTENT.into_response();
    }
    if would_exceed_rate(user_id, app_id) {
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    }
    let (view, events) = take_view(events);
    let set_cookie =
        view.and_then(|referrer| record_view(Some(referrer.as_str()).filter(|r| !r.is_empty())));
    // The session cookie is set on the HTML response that loaded this page, so
    // it is present for every real beacon. A fresh id when it is missing keeps
    // the row rather than dropping it — an orphan session is a worse answer
    // than no answer only if you were counting sessions, and the view rows
    // (which do have the cookie) are what that count comes from.
    let session_id =
        oxy_custom_apps::tracking::session_id_from_headers(headers).unwrap_or_else(Uuid::new_v4);

    // Error DETAIL — message and stack — goes to its own table, not to either
    // of the two above. See `CREATE_CUSTOM_APP_CLIENT_ERRORS_TABLE`: shorter
    // retention and the same app-admin gate, because free text a page threw can
    // carry application data that a count cannot.
    for (name, payload) in &events {
        if name != "oxy-error" {
            continue;
        }
        oxy_custom_apps::telemetry::record_client_errors(
            org_id,
            app_id,
            user_id,
            session_id,
            payload,
            headers,
            &environment,
        );
    }

    // Health signals are mirrored into the operational event stream as well as
    // the Activity tab's Postgres rows. Two sinks, on purpose: the Activity tab
    // answers "who used this app" over 90 days, and availability answers "is it
    // working right now" over minutes. One store cannot be tuned for both, and
    // the `oxy-app-ready` row is the half that makes a white screen visible.
    for (name, payload) in &events {
        if !HEALTH_EVENTS.contains(&name.as_str()) {
            continue;
        }
        let outcome = if name == "oxy-error" {
            oxy_observability::types::custom_app_outcome::ERROR
        } else {
            oxy_observability::types::custom_app_outcome::OK
        };
        let path = payload
            .get("path")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        oxy_custom_apps::telemetry::record_client_event(oxy_custom_apps::telemetry::ClientEvent {
            org_id,
            app_id,
            user_id,
            session_id: &session_id.to_string(),
            event_name: name,
            path,
            outcome,
            environment: &environment,
            build_id: client_build(payload),
            detail: update_detail(name, payload),
        });
    }

    // Queued, not spawned: the drain validates and batch-inserts, and a full
    // queue drops the row. Losing a telemetry row is the documented acceptable
    // outcome, and there is nobody left on the other end of the request to tell.
    for (name, payload) in events {
        oxy_custom_apps::tracking::queue_event(
            app_id,
            user_id,
            user_email.clone(),
            session_id,
            name,
            payload,
            environment.clone(),
        );
    }

    let mut response = StatusCode::NO_CONTENT.into_response();
    if let Some(hv) = set_cookie.and_then(|c| axum::http::HeaderValue::from_str(&c).ok()) {
        response
            .headers_mut()
            .append(axum::http::header::SET_COOKIE, hv);
    }
    response
}

/// The build the reporting document runs, as the `app_builds.id` UUID — or
/// empty. The runtime sends `build` (its `window.__OXY_APP__.buildId`) on
/// `app-ready`, `oxy-error` and `oxy-update`; it is page-supplied, so only a
/// well-formed UUID is stored, never arbitrary text in an id column.
fn client_build(payload: &serde_json::Value) -> String {
    payload
        .get("build")
        .and_then(|v| v.as_str())
        .and_then(|s| Uuid::parse_str(s).ok())
        .map(|id| id.to_string())
        .unwrap_or_default()
}

/// `phase:how` for an `oxy-update` row (`detected:poll`, `applied:route`,
/// `guarded:idle`), empty for every other event. Both halves come from small
/// fixed vocabularies in `runtime.js`; anything else is dropped rather than
/// stored, since the page could send any string.
fn update_detail(name: &str, payload: &serde_json::Value) -> String {
    if name != "oxy-update" {
        return String::new();
    }
    const PHASES: &[&str] = &["detected", "stale", "applied", "guarded"];
    const HOWS: &[&str] = &["poll", "route", "idle", "resume", "chunk", "app"];
    let field = |key: &str, allowed: &[&str]| {
        payload
            .get(key)
            .and_then(|v| v.as_str())
            .filter(|v| allowed.contains(v))
            .unwrap_or("")
            .to_string()
    };
    format!("{}:{}", field("phase", PHASES), field("how", HOWS))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_build_keeps_only_a_uuid() {
        let id = Uuid::new_v4();
        assert_eq!(
            client_build(&serde_json::json!({ "build": id.to_string() })),
            id.to_string()
        );
        assert_eq!(client_build(&serde_json::json!({ "build": "build-7" })), "");
        assert_eq!(client_build(&serde_json::json!({ "build": 7 })), "");
        assert_eq!(client_build(&serde_json::json!({})), "");
    }

    #[test]
    fn update_detail_is_a_fixed_vocabulary() {
        let p = serde_json::json!({ "phase": "applied", "how": "route" });
        assert_eq!(update_detail("oxy-update", &p), "applied:route");
        // A tab still behind re-reports every 6 h (`runtime.js`).
        let stale = serde_json::json!({ "phase": "stale", "how": "poll" });
        assert_eq!(update_detail("oxy-update", &stale), "stale:poll");
        let junk = serde_json::json!({ "phase": "<script>", "how": "idle" });
        assert_eq!(update_detail("oxy-update", &junk), ":idle");
        assert_eq!(update_detail("oxy-app-ready", &p), "");
    }

    fn batch(json: &str) -> Result<Vec<(String, serde_json::Value)>, StatusCode> {
        admit(json.as_bytes()).map_err(|r| r.status())
    }

    #[test]
    fn admits_a_well_formed_platform_batch() {
        let events = batch(
            r#"{"v":1,"events":[
                {"n":"oxy-pageview","t":1730000000000,"p":{"path":"/orders"}},
                {"n":"oxy-web-vitals","p":{"lcp":812}}
            ]}"#,
        )
        .expect("admitted");
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].0, "oxy-pageview");
        assert_eq!(events[0].1["path"], "/orders");
    }

    /// The runtime's vitals carry the screen and the build they were measured
    /// on; the release timeline reads both, so the beacon keeps them as sent.
    #[test]
    fn admits_vitals_with_their_screen_and_build() {
        let build = "6e5e84a2-17f0-4879-8367-19211bd6a51a";
        let events = batch(&format!(
            r#"{{"v":1,"events":[{{"n":"oxy-web-vitals","p":{{"lcp":812,"cls":4,"path":"/store","build":"{build}"}}}}]}}"#
        ))
        .expect("admitted");
        assert_eq!(events[0].1["path"], "/store");
        assert_eq!(events[0].1["build"], build);
        assert_eq!(events[0].1["lcp"], 812);
    }

    /// The whole reason this route is separate from the SDK's: it must not be
    /// usable to write anything but the platform's own names.
    #[test]
    fn refuses_any_name_outside_the_platform_allowlist() {
        for name in [
            "export-clicked",
            "oxy-something-new",
            "oxy-",
            "OXY-PAGEVIEW",
            "",
        ] {
            let body = format!(r#"{{"v":1,"events":[{{"n":"{name}","p":{{}}}}]}}"#);
            assert_eq!(
                batch(&body).unwrap_err(),
                StatusCode::BAD_REQUEST,
                "{name:?} must be refused"
            );
        }
    }

    /// A batch is all-or-nothing. Silently dropping the unknown event would let
    /// a tampered client keep a foothold in the namespace for its other rows.
    #[test]
    fn one_bad_event_fails_the_whole_batch() {
        let body = r#"{"v":1,"events":[
            {"n":"oxy-pageview","p":{}},
            {"n":"not-ours","p":{}}
        ]}"#;
        assert_eq!(batch(body).unwrap_err(), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn rejects_the_wrong_wire_version_and_malformed_bodies() {
        assert_eq!(
            batch(r#"{"v":2,"events":[]}"#).unwrap_err(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(batch("not json").unwrap_err(), StatusCode::BAD_REQUEST);
        // A missing `v` defaults to 0, which is not the wire version.
        assert_eq!(
            batch(r#"{"events":[]}"#).unwrap_err(),
            StatusCode::BAD_REQUEST
        );
    }

    #[test]
    fn bounds_batch_size_and_body_size() {
        let many: Vec<String> = (0..MAX_EVENTS_PER_BATCH + 1)
            .map(|_| r#"{"n":"oxy-pageview","p":{}}"#.to_string())
            .collect();
        let body = format!(r#"{{"v":1,"events":[{}]}}"#, many.join(","));
        assert_eq!(batch(&body).unwrap_err(), StatusCode::PAYLOAD_TOO_LARGE);

        let huge = format!(
            r#"{{"v":1,"events":[{{"n":"oxy-pageview","p":{{"path":"{}"}}}}]}}"#,
            "x".repeat(MAX_BODY_BYTES)
        );
        assert_eq!(batch(&huge).unwrap_err(), StatusCode::PAYLOAD_TOO_LARGE);
    }

    /// A payload that is not an object cannot be stored in the `payload`
    /// column's contract (`record_event` rejects it too) — catch it here so the
    /// batch fails before any row of it lands.
    #[test]
    fn refuses_non_object_payloads() {
        assert_eq!(
            batch(r#"{"v":1,"events":[{"n":"oxy-pageview","p":[1,2]}]}"#).unwrap_err(),
            StatusCode::BAD_REQUEST
        );
        // Absent is fine — it means "no detail", not "malformed".
        let events = batch(r#"{"v":1,"events":[{"n":"oxy-pageview"}]}"#).expect("admitted");
        assert_eq!(events[0].1, serde_json::json!({}));
    }

    /// Every name the client is capable of sending must be one this route
    /// accepts, or the batch containing it is dropped whole. The client script
    /// is compiled into the binary, so this can be checked rather than assumed.
    #[test]
    fn client_runtime_only_emits_names_this_route_accepts() {
        let js = crate::client::runtime_script_tag();
        for captured in js.match_indices("push(\"oxy-") {
            let tail = &js[captured.0 + "push(\"".len()..];
            let name = tail.split('"').next().expect("quoted name");
            assert!(
                is_accepted(name),
                "client emits {name:?}, which the beacon refuses"
            );
        }
        // …and every accepted name is actually emitted, so the allowlist does
        // not accumulate entries nothing sends.
        for name in PLATFORM_EVENTS.iter().chain([&VIEW_EVENT]) {
            assert!(
                js.contains(&format!("push(\"{name}\"")),
                "{name:?} is accepted but never sent"
            );
        }
    }

    #[test]
    fn a_reported_open_is_taken_out_of_the_batch_once() {
        let events = batch(
            r#"{"v":1,"events":[
                {"n":"oxy-view","p":{"referrer":"https://app.example.com/"}},
                {"n":"oxy-app-ready","p":{"how":"auto"}},
                {"n":"oxy-view","p":{"referrer":"https://other.example.com/"}}
            ]}"#,
        )
        .expect("admitted");
        let (view, rest) = take_view(events);
        assert_eq!(view.as_deref(), Some("https://app.example.com/"));
        assert_eq!(rest.len(), 1, "only the event rows are left to record");
        assert_eq!(rest[0].0, "oxy-app-ready");
    }

    #[test]
    fn a_reported_open_without_a_referrer_still_counts() {
        let events = batch(r#"{"v":1,"events":[{"n":"oxy-view","p":{}}]}"#).expect("admitted");
        let (view, rest) = take_view(events);
        assert_eq!(view.as_deref(), Some(""));
        assert!(rest.is_empty());
    }

    #[test]
    fn a_batch_with_no_reported_open_records_no_view() {
        let events =
            batch(r#"{"v":1,"events":[{"n":"oxy-pageview","p":{"path":"/"}}]}"#).expect("admitted");
        let (view, rest) = take_view(events);
        assert_eq!(view, None);
        assert_eq!(rest.len(), 1);
    }

    #[test]
    fn rate_limiter_admits_a_normal_session_and_stops_a_flood() {
        let user = Uuid::new_v4();
        let app = Uuid::new_v4();
        for i in 1..=RATE_PER_MIN {
            assert!(!would_exceed_rate(user, app), "batch {i} should be allowed");
        }
        assert!(would_exceed_rate(user, app));
        // A different app is a different budget.
        assert!(!would_exceed_rate(user, Uuid::new_v4()));
    }
}
