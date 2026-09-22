//! Emit side of the custom-app wide-event stream.
//!
//! One place that knows how a served request, a function invocation and a
//! browser beacon each become a [`CustomAppEventRecord`], so the hot paths that
//! call in stay one-liners and the classification below is testable without a
//! server.
//!
//! Everything here is fire-and-forget: [`oxy_observability::record_custom_app_event`]
//! pushes onto an unbounded channel and a background bridge owns retry and
//! backoff. Nothing on a serving path ever waits for ClickHouse, and with
//! `OXY_OBSERVABILITY_BACKEND` unset every call is a no-op — the default state
//! for a developer's `oxy serve`.
//!
//! See `internal-docs/2026-09-04-custom-app-observability-design.md` §3.

use oxy_observability::custom_app_sink;
use oxy_observability::types::{
    CustomAppClientErrorRecord, CustomAppEventRecord, CustomAppLogRecord, custom_app_kind,
    custom_app_outcome,
};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use uuid::Uuid;

/// Policy-failure threshold: a request that succeeded but took longer than this
/// counts against availability.
///
/// The SRE taxonomy's third error class ("if you committed to one second, any
/// request over one second is an error") — without it, an app that answers
/// every request in 40s reads as 100% available. 10s is deliberately generous:
/// this is a *failure* threshold, not a performance target, and a false
/// positive here burns an error budget that should be spent on real outages.
const SLOW_THRESHOLD_MS: u32 = 10_000;

/// Milliseconds since the epoch, stamped where the event happens.
///
/// Not left to the flush: a batch can sit for up to the flush interval, and an
/// availability window computed from flush time attributes an outage to the
/// wrong minute — which is exactly the minute an operator is looking at.
pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// Classify one served response into the `outcome` taxonomy.
///
/// `status` alone is not enough and that is the whole point of storing this:
/// a custom-app host answers 200 with the SPA shell for every path, so the
/// implicit-failure case (served fine, never mounted) is invisible here and is
/// resolved later by the client's `oxy-app-ready` beacon. What this *can* decide
/// is the explicit and policy classes.
fn outcome_for(status: u16, duration_ms: u32) -> &'static str {
    // `is_app_fault`, not `status >= 500`. They differ on 408 and 429, and the
    // difference was a real hole: a throttling app tagged `error_kind: http_429`
    // and `outcome: ok`, so the SLI (which counts `outcome != 'ok'`) read 100%
    // available while every other request was being turned away.
    if is_app_fault(status) {
        custom_app_outcome::ERROR
    } else if duration_ms > SLOW_THRESHOLD_MS {
        custom_app_outcome::SLOW
    } else {
        custom_app_outcome::OK
    }
}

/// A 4xx is the caller's problem, not the app's — except 408/429, which are the
/// platform declining to serve.
///
/// Availability must not be dented by a browser asking for a page that does not
/// exist; if it were, an app could improve its SLI by removing its 404 handler.
fn is_app_fault(status: u16) -> bool {
    status >= 500 || status == 408 || status == 429
}

/// Common fields for one custom-app request.
pub struct ServeEvent<'a> {
    pub org_id: Uuid,
    pub app_id: Uuid,
    pub build_id: Option<Uuid>,
    pub request_id: Option<Uuid>,
    pub session_id: Option<&'a str>,
    pub user_id: Uuid,
    /// `true` for a user-visible page load, `false` for a bundle asset. Assets
    /// are recorded but excluded from the SLI — a cancelled image is not an
    /// outage, and asset volume would bury a real shell failure.
    pub is_html: bool,
    pub route: &'a str,
    pub status: u16,
    pub duration_ms: u32,
}

/// Record one served request (HTML shell or bundle asset).
pub fn record_serve(event: ServeEvent<'_>) {
    // Metrics first, and deliberately **outside** the sink gate below. That
    // gate asks whether the tenant-facing ClickHouse store is configured
    // (`OXY_CLICKHOUSE_*`), which has no default in any mode — so on a
    // deployment without it, every line after the gate is dead. The metrics
    // plane is a different backend with a different switch, and letting an
    // unset ClickHouse silently disable Prometheus series would reproduce the
    // exact "no traces" confusion that store is already known for.
    record_serve_metrics(&event);

    if !custom_app_sink::is_enabled() {
        return;
    }
    let outcome = outcome_for(event.status, event.duration_ms);
    let (trace_id, span_id) = trace_ids();
    custom_app_sink::record_event(CustomAppEventRecord {
        timestamp_ms: now_ms(),
        org_id: event.org_id.to_string(),
        app_id: event.app_id.to_string(),
        build_id: opt_id(event.build_id),
        request_id: opt_id(event.request_id),
        session_id: event.session_id.unwrap_or_default().to_string(),
        user_id: event.user_id.to_string(),
        kind: if event.is_html {
            custom_app_kind::SERVE
        } else {
            custom_app_kind::ASSET
        }
        .to_string(),
        route: event.route.to_string(),
        status: event.status,
        duration_ms: event.duration_ms,
        host_calls: 0,
        init_ms: 0,
        bytes: 0,
        app_role: String::new(),
        outcome: outcome.to_string(),
        error_kind: if is_app_fault(event.status) {
            format!("http_{}", event.status)
        } else {
            String::new()
        },
        error_detail: String::new(),
        trace_id,
        span_id,
    });
}

// ── Invocation meters ────────────────────────────────────────────────────────
//
// These live here rather than in `custom_apps_functions::runtime` because that
// module is behind `#[cfg(feature = "custom-app-functions")]` and this type is
// not: it is plain atomics with no deno_core in it, and the ungated emit path
// below reads it. A `--no-default-features` build of `oxy-app` is a CI job, so
// an ungated reference to a gated symbol fails the build rather than just
// whoever happens to run it.

/// Per-invocation counters the caller reads once the run is over.
///
/// Shared handles rather than return values, because the isolate thread can
/// **outlive `run`** — on the `runtime::TIMEOUT_GRACE` path it is detached and
/// keeps going — so anything it writes has to live somewhere the caller still
/// owns.
/// Same shape, and the same reason, as the `logs` buffer beside it.
#[derive(Clone)]
pub struct InvocationMeters {
    /// Host ops dispatched over the broker channel: Cloudflare's "subrequest
    /// count", and the number that separates "the warehouse is slow" from "this
    /// function runs 200 queries in a loop". Those two are otherwise
    /// indistinguishable in a `duration_ms`.
    pub host_calls: Arc<AtomicU32>,
    /// Wall milliseconds from [`InvocationMeters::start`] to the moment tenant
    /// code first runs — OS thread spawn, isolate creation, bootstrap and
    /// script compile.
    ///
    /// We build a fresh isolate *and* a fresh OS thread per invocation with no
    /// code cache. Cloudflare reuses isolates, so it does not pay this and does
    /// not measure it; we pay it on every call, and without this field "the
    /// function is slow" and "the platform is slow to start the function"
    /// produce the same `duration_ms`.
    pub init_ms: Arc<AtomicU32>,
    /// The zero point `init_ms` is measured from. Carried here rather than
    /// passed alongside so the isolate thread needs one argument, not two.
    started: std::time::Instant,
}

impl Default for InvocationMeters {
    fn default() -> Self {
        Self::start()
    }
}

impl InvocationMeters {
    /// Begin measuring. Called by the request handler just before it runs the
    /// function, so `init_ms` covers everything between deciding to run and
    /// tenant code actually running.
    pub fn start() -> Self {
        Self {
            host_calls: Arc::new(AtomicU32::new(0)),
            init_ms: Arc::new(AtomicU32::new(0)),
            started: std::time::Instant::now(),
        }
    }

    /// Stamp `init_ms`. One call site: the boundary in
    /// `runtime::execute_isolate` between platform setup and the tenant's own
    /// module evaluating.
    ///
    /// That caller is behind `custom-app-functions`, so without the feature
    /// nothing calls this — the meters stay at their zero values, which is the
    /// honest reading for a build that cannot run a function at all.
    #[cfg_attr(not(feature = "custom-app-functions"), allow(dead_code))]
    pub(crate) fn mark_tenant_code_entered(&self) {
        let ms = self.started.elapsed().as_millis().min(u32::MAX as u128) as u32;
        self.init_ms.store(ms, Ordering::Relaxed);
    }

    /// Host ops dispatched so far.
    pub fn host_calls(&self) -> u32 {
        self.host_calls.load(Ordering::Relaxed)
    }

    /// Setup time in milliseconds, or `None` if tenant code was never reached —
    /// a build that failed to compile, or a timeout during setup. A zero would
    /// claim setup was instant; absence says it never finished.
    pub fn init_ms(&self) -> Option<u32> {
        match self.init_ms.load(Ordering::Relaxed) {
            0 => None,
            ms => Some(ms),
        }
    }
}

/// One Oxy Function invocation, in any mode.
pub struct FunctionEvent<'a> {
    pub org_id: Uuid,
    pub app_id: Uuid,
    pub build_id: Uuid,
    pub request_id: Option<Uuid>,
    pub user_id: Uuid,
    pub function_name: &'a str,
    pub mode: &'a str,
    /// The invocation row's status: `success` | `error` | `cancelled` | `timeout`.
    pub status_label: &'a str,
    pub http_status: u16,
    pub duration_ms: u32,
    /// Host ops the invocation made — the subrequest count. Distinguishes a
    /// slow dependency from a function looping queries, which `duration_ms`
    /// alone cannot.
    pub host_calls: u32,
    /// Platform setup time (thread spawn, isolate creation, bootstrap, compile).
    /// `None` when tenant code was never reached — a compile failure, or a
    /// timeout during setup — because a `0` would claim setup was instant.
    pub init_ms: Option<u32>,
    pub error: Option<&'a str>,
}

/// Record one function invocation.
pub fn record_function(event: FunctionEvent<'_>) {
    // Ahead of the sink gate, for the reason spelled out in `record_serve`.
    record_function_metrics(&event);

    if !custom_app_sink::is_enabled() {
        return;
    }
    // A cancellation is not a failure of the app — someone asked for it to
    // stop. A timeout is. Keeping these apart matters because "user navigated
    // away mid-invoke" is the single most common non-event on this path, and
    // counting it would make every chatty app look unreliable.
    let outcome = match event.status_label {
        "success" => {
            if event.duration_ms > SLOW_THRESHOLD_MS {
                custom_app_outcome::SLOW
            } else {
                custom_app_outcome::OK
            }
        }
        "cancelled" => custom_app_outcome::OK,
        // A shed is the platform declining to start the work for want of a
        // concurrency permit. Charging it to the app's error rate would dent an
        // availability verdict with a capacity decision the app had no part in
        // — the same reasoning that keeps `cancelled` out of it. The platform
        // counts sheds in `oxy_custom_app_admission_shed_total`, labelled by
        // which limit bound.
        "shed" => custom_app_outcome::OK,
        _ => custom_app_outcome::ERROR,
    };
    let (trace_id, span_id) = trace_ids();
    custom_app_sink::record_event(CustomAppEventRecord {
        timestamp_ms: now_ms(),
        org_id: event.org_id.to_string(),
        app_id: event.app_id.to_string(),
        build_id: event.build_id.to_string(),
        request_id: opt_id(event.request_id),
        session_id: String::new(),
        user_id: event.user_id.to_string(),
        kind: custom_app_kind::FUNCTION.to_string(),
        route: event.function_name.to_string(),
        status: event.http_status,
        duration_ms: event.duration_ms,
        host_calls: event.host_calls,
        init_ms: event.init_ms.unwrap_or(0),
        bytes: 0,
        app_role: event.mode.to_string(),
        outcome: outcome.to_string(),
        error_kind: if outcome == custom_app_outcome::ERROR {
            event.status_label.to_string()
        } else {
            String::new()
        },
        error_detail: event.error.unwrap_or_default().to_string(),
        trace_id,
        span_id,
    });
}

/// Mirror one serve event into the metrics plane.
///
/// Deliberately a much smaller fact than the ClickHouse row beside it: no
/// route, no user, no session, no build. Those are fields you filter a *trace*
/// or an event row by; putting them on a time series would multiply its
/// cardinality by the product of their ranges. What survives is what a
/// dashboard or an alert reads — who, what kind, how it ended, how long.
fn record_serve_metrics(event: &ServeEvent<'_>) {
    // This runs on every bundle-asset response — the custom-app hot path that
    // `oxy-customer-apps-perf` governs — and the two `to_string()`s below each
    // allocate a 36-byte UUID rendering before `with_instruments` gets a chance
    // to discard them. On a deployment with no meter provider that is pure
    // waste on the busiest path in the process, so it is skipped by an atomic
    // load instead.
    if !oxy_telemetry::metrics::is_installed() {
        return;
    }
    oxy_telemetry::metrics::record::custom_app_request(
        &event.org_id.to_string(),
        &event.app_id.to_string(),
        if event.is_html { "html" } else { "asset" },
        event.status,
        f64::from(event.duration_ms) / 1000.0,
    );
}

/// Mirror one function invocation into the metrics plane.
///
/// The outcome here is the invocation's own `status_label` rather than the
/// derived `outcome` the ClickHouse row carries, because the two answer
/// different questions and conflating them loses the distinction that matters
/// most on this path: `cancelled` folds into `ok` for SLO purposes (someone
/// navigated away — counting it would make every chatty app look unreliable),
/// but for "what is this function doing" a cancellation is not a success.
fn record_function_metrics(event: &FunctionEvent<'_>) {
    // Same guard as `record_serve_metrics`, for consistency rather than cost —
    // an invocation is orders of magnitude more expensive than two UUID
    // renderings. Keeping both the same shape means neither drifts into being
    // the one that allocates unconditionally.
    if !oxy_telemetry::metrics::is_installed() {
        return;
    }
    oxy_telemetry::metrics::record::custom_app_function(
        &event.org_id.to_string(),
        &event.app_id.to_string(),
        event.function_name,
        event.status_label,
        f64::from(event.duration_ms) / 1000.0,
        event.init_ms.map(|ms| f64::from(ms) / 1000.0),
        event.host_calls,
    );
}

/// Persist a run's `ctx.log()` / `console.*` output.
///
/// Route-mode logs otherwise exist only inside the HTTP response that carries
/// them — gone the moment the caller navigates away. `seq` preserves write
/// order within a millisecond.
#[allow(clippy::too_many_arguments)]
pub fn record_function_logs(
    org_id: Uuid,
    app_id: Uuid,
    build_id: Uuid,
    invocation_id: Uuid,
    request_id: Option<Uuid>,
    function_name: &str,
    mode: &str,
    lines: &[(String, String)],
) {
    if !custom_app_sink::is_enabled() || lines.is_empty() {
        return;
    }
    let (trace_id, span_id) = trace_ids();
    let timestamp_ms = now_ms();
    let records = lines
        .iter()
        .enumerate()
        .map(|(seq, (level, message))| CustomAppLogRecord {
            timestamp_ms,
            org_id: org_id.to_string(),
            app_id: app_id.to_string(),
            build_id: build_id.to_string(),
            invocation_id: invocation_id.to_string(),
            request_id: opt_id(request_id),
            function_name: function_name.to_string(),
            mode: mode.to_string(),
            log_level: level.clone(),
            seq: seq as u32,
            message: message.clone(),
            trace_id: trace_id.clone(),
            span_id: span_id.clone(),
        })
        .collect();
    custom_app_sink::record_logs(records);
}

/// A browser-reported platform event (`oxy-app-ready`, `oxy-error`, …).
///
/// `oxy-app-ready` is the one that resolves the implicit-failure class: a
/// served shell with no `app-ready` behind it is a white screen, and no status
/// code anywhere can say so.
pub fn record_client_event(
    org_id: Uuid,
    app_id: Uuid,
    user_id: Uuid,
    session_id: &str,
    event_name: &str,
    path: &str,
    outcome: &'static str,
) {
    if !custom_app_sink::is_enabled() {
        return;
    }
    let (trace_id, span_id) = trace_ids();
    custom_app_sink::record_event(CustomAppEventRecord {
        timestamp_ms: now_ms(),
        org_id: org_id.to_string(),
        app_id: app_id.to_string(),
        build_id: String::new(),
        request_id: String::new(),
        session_id: session_id.to_string(),
        user_id: user_id.to_string(),
        kind: custom_app_kind::CLIENT.to_string(),
        route: path.to_string(),
        status: 0,
        duration_ms: 0,
        host_calls: 0,
        init_ms: 0,
        bytes: 0,
        app_role: String::new(),
        outcome: outcome.to_string(),
        error_kind: event_name.to_string(),
        error_detail: String::new(),
        trace_id,
        span_id,
    });
}

/// Max detail records accepted from one `oxy-error` EVENT.
///
/// A beacon batch carries up to 20 events (`MAX_BATCH` in `runtime.js`), so the
/// real per-request ceiling is 20 × this. That is a deliberate ceiling rather
/// than the tight bound the client's own cap of 5 suggests — stated here
/// because "the server's cap is the one that holds" is only useful if the
/// number it holds at is written down.
///
/// The client already caps itself at five, and dedups per session on top. This
/// is the server-side floor under that: the client is a bundle we inject into,
/// not a bundle we control, and an app's own JavaScript can post to this
/// endpoint with any payload it likes (the module docs on `custom_apps_beacon`
/// are explicit that this route is not a forgery defence). So the cap is
/// enforced on both sides, and the server's is the one that holds.
const MAX_ERROR_DETAILS_PER_BATCH: usize = 10;

/// Pull the `details` array out of an `oxy-error` payload and record each entry.
///
/// Every field is client-supplied and treated as such: strings are bounded on
/// write (`clamp_to` in the ClickHouse layer), and `build` is accepted only if
/// it parses as a UUID. The client is nonetheless the right source for `build` —
/// it is the only party that knows which build actually served the document,
/// and by the time this beacon lands the app may have been re-published.
pub fn record_client_errors(
    org_id: Uuid,
    app_id: Uuid,
    user_id: Uuid,
    session_id: Uuid,
    payload: &serde_json::Value,
    headers: &axum::http::HeaderMap,
) {
    if !custom_app_sink::is_enabled() {
        return;
    }
    let Some(details) = payload.get("details").and_then(|d| d.as_array()) else {
        return;
    };
    if details.is_empty() {
        return;
    }

    let build_id = payload
        .get("build")
        .and_then(|b| b.as_str())
        .and_then(|b| Uuid::parse_str(b).ok())
        .map(|id| id.to_string())
        .unwrap_or_default();
    let user_agent = headers
        .get(axum::http::header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .chars()
        .take(256)
        .collect::<String>();
    let timestamp_ms = now_ms();

    let records: Vec<CustomAppClientErrorRecord> = details
        .iter()
        .take(MAX_ERROR_DETAILS_PER_BATCH)
        .map(|d| CustomAppClientErrorRecord {
            timestamp_ms,
            org_id: org_id.to_string(),
            app_id: app_id.to_string(),
            build_id: build_id.clone(),
            session_id: session_id.to_string(),
            user_id: user_id.to_string(),
            error_name: str_field(d, "n", "Error"),
            message: str_field(d, "m", ""),
            stack: str_field(d, "s", ""),
            stack_hash: str_field(d, "h", ""),
            path: str_field(d, "p", ""),
            kind: str_field(d, "k", "error"),
            user_agent: user_agent.clone(),
            // `t` is the trace of the invoke the page was awaiting when it
            // threw, when the SDK could name one; the browser has no span.
            trace_id: trace_id_field(d),
            span_id: String::new(),
        })
        .collect();
    custom_app_sink::record_client_errors(records);
}

fn str_field(value: &serde_json::Value, key: &str, fallback: &str) -> String {
    value
        .get(key)
        .and_then(|v| v.as_str())
        .unwrap_or(fallback)
        .to_string()
}

/// The current platform-trace ids, or two empty strings outside a traced
/// span (a one-shot CLI process, or `OTEL_SDK_DISABLED`). Every row this module
/// writes carries them, so the product tables join the operator's trace.
fn trace_ids() -> (String, String) {
    oxy_telemetry::propagation::current_ids().unwrap_or_default()
}

/// The `t` of an error detail — the trace of the invoke the page was awaiting
/// when it threw — accepted only as 32 lowercase hex, the shape the SDK mints.
/// Same posture as `build` above: an app's own JavaScript can post anything to
/// this endpoint, and this column is documented as a HyperDX join key.
fn trace_id_field(detail: &serde_json::Value) -> String {
    detail
        .get("t")
        .and_then(|v| v.as_str())
        .filter(|t| {
            t.len() == 32
                && t.bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        })
        .map(str::to_string)
        .unwrap_or_default()
}

/// An absent id stays absent. A nil UUID would look like a value and join rows
/// that have nothing to do with each other.
fn opt_id(id: Option<Uuid>) -> String {
    id.map(|v| v.to_string()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 404 is the caller asking for something that isn't there. Counting it
    /// against availability would mean an app could raise its SLI by deleting
    /// its not-found handler.
    #[test]
    fn client_errors_are_not_app_faults() {
        for status in [400, 401, 403, 404, 422] {
            assert!(!is_app_fault(status), "{status} should not dent the SLI");
        }
    }

    /// 408 and 429 are the platform declining to serve, not the caller being
    /// wrong — the app really was unavailable to that request.
    #[test]
    fn timeouts_and_throttles_are_app_faults_despite_being_4xx() {
        assert!(is_app_fault(408));
        assert!(is_app_fault(429));
    }

    #[test]
    fn server_errors_are_app_faults() {
        for status in [500, 502, 503, 504] {
            assert!(is_app_fault(status));
        }
    }

    /// The hole this closed: `is_app_fault` covered 408/429 but `outcome_for`
    /// tested `status >= 500`, so a throttling app scored `ok` and the SLI —
    /// which counts `outcome != 'ok'` — reported it perfectly available.
    #[test]
    fn a_throttled_or_timed_out_request_is_not_ok() {
        assert_eq!(outcome_for(429, 10), custom_app_outcome::ERROR);
        assert_eq!(outcome_for(408, 10), custom_app_outcome::ERROR);
        // A plain client error still is not the app's fault.
        assert_eq!(outcome_for(404, 10), custom_app_outcome::OK);
    }

    /// The policy class: succeeded, but outside the objective.
    #[test]
    fn a_slow_success_is_a_failure_class_of_its_own() {
        assert_eq!(outcome_for(200, 1_000), custom_app_outcome::OK);
        assert_eq!(
            outcome_for(200, SLOW_THRESHOLD_MS + 1),
            custom_app_outcome::SLOW
        );
        // Explicit beats policy — a slow 500 is an error, not "slow".
        assert_eq!(
            outcome_for(500, SLOW_THRESHOLD_MS + 1),
            custom_app_outcome::ERROR
        );
    }

    /// An absent id must not become `00000000-0000-0000-0000-000000000000`,
    /// which reads as a real value and would join unrelated rows.
    #[test]
    fn an_absent_id_serialises_empty_not_nil() {
        assert_eq!(opt_id(None), "");
        assert_ne!(opt_id(Some(Uuid::nil())), "");
    }
}

#[cfg(test)]
mod trace_id_field_tests {
    use super::*;

    #[test]
    fn only_a_lowercase_32_hex_trace_id_is_kept() {
        let ok = serde_json::json!({ "t": "0af7651916cd43dd8448eb211c80319c" });
        assert_eq!(trace_id_field(&ok), "0af7651916cd43dd8448eb211c80319c");
        for bad in [
            serde_json::json!({ "t": "0AF7651916CD43DD8448EB211C80319C" }),
            serde_json::json!({ "t": "x".repeat(32) }),
            serde_json::json!({ "t": "0af7651916cd43dd8448eb211c80319" }),
            serde_json::json!({ "t": "a".repeat(1_000_000) }),
            serde_json::json!({ "t": 42 }),
            serde_json::json!({}),
        ] {
            assert_eq!(trace_id_field(&bad), "", "{}", bad.to_string().len());
        }
    }
}
