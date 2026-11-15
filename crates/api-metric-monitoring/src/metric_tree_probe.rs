//! Diagnostic probe for the `/semantic/metric-tree*` endpoints.
//!
//! Exists to answer one question that no amount of reading the code settles:
//! when the Metric Tree pegs a backend core, is that **many cheap requests**
//! (something is looping), **one request that never returns** (a hang in a
//! warehouse-executing op), or **neither** (the burn is backend-internal and
//! the page merely coincides with it)?
//!
//! Measured cost of one `GET /semantic/metric-tree` against the
//! `example_new/semantics` fixture is ~11ms in a debug build, so a pegged core
//! needs roughly a hundred requests a second. That is far outside anything the
//! UI does on purpose, which is why a sustained rate is logged as a `warn` and
//! not left for someone to notice in a trace.
//!
//! Reading the output:
//!
//! | What you see | What it means |
//! | ------------ | ------------- |
//! | `flooding` warn lines | A request loop. Find the caller, not the handler — the window is per `(label, workspace)`, so the tenant named is the one that tripped it. |
//! | `still in flight` warn | One request hangs — `label` and `workspace` name it. |
//! | Nothing at all, CPU still pegged | Not these routes. Look at the workers. |
//!
//! The third row is only trustworthy because the second one can actually fire.
//! A hung request never reaches the end of its own middleware, so nothing it
//! does on the way out can report it: the stuck check has to be run by some
//! *other* request, against a registry of what is currently open. That is what
//! `IN_FLIGHT` is — a map of open requests scanned at the START of every new
//! one — and not a counter, which can say "3 are open" but never which three
//! nor since when.
//!
//! **This ships permanently**, not as a temporary instrument: the flood
//! threshold is the cheapest guard the metric-tree surface has against a
//! re-render loop, and the class of bug it catches recurs. Everything below a
//! tripped threshold is `debug!`, so a quiet backend stays quiet — per-window
//! bookkeeping is not worth an `info!` per endpoint per tenant forever.
//!
//! Self-arming otherwise: it costs an `Instant::now()`, one map insert and one
//! removal per request, and says nothing unless a threshold trips.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use axum::extract::Request;
use axum::middleware::Next;
use axum::response::Response;

/// Sustained per-endpoint rate that means "looping", not "being used".
/// `predict` legitimately fires on a 300ms debounce while someone types a
/// lever value — ~3/s in bursts — so the bar sits above that AND has to hold
/// for a whole window, which typing cannot do.
const FLOOD_RATE_PER_SEC: f64 = 5.0;

/// How long a rate has to hold before it counts as a flood.
const WINDOW: Duration = Duration::from_secs(10);

/// A single request past this is reported on its own. The warehouse-executing
/// ops carry their own 30–45s timeouts, so this is a heads-up, not a verdict.
const SLOW_REQUEST: Duration = Duration::from_secs(5);

/// A request in flight this long has almost certainly hung: every op on these
/// routes is either pure (~11ms) or timeout-capped well under this.
const STUCK_REQUEST: Duration = Duration::from_secs(60);

/// Per-`(endpoint, workspace)` counters for the current window.
#[derive(Debug)]
struct Window {
    started: Instant,
    requests: u32,
    slowest: Duration,
}

impl Window {
    fn new() -> Self {
        Self {
            started: Instant::now(),
            requests: 0,
            slowest: Duration::ZERO,
        }
    }
}

/// Keyed on `(label, workspace)`, not `label` alone: a flood window that
/// spans tenants would close on whichever request happened to age it out and
/// name that tenant in the warning, which is exactly the caller the "find the
/// caller" advice above is for. The extra `String` per pair is the accepted
/// cost — see `record`'s doc.
static WINDOWS: LazyLock<Mutex<HashMap<(&'static str, String), Window>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// One request currently between `probe`'s entry and its exit.
#[derive(Debug)]
struct InFlight {
    label: &'static str,
    workspace: String,
    started: Instant,
    /// Set once this entry has been reported, so a genuine hang costs one warn
    /// line rather than one per subsequent request for as long as it hangs.
    reported: bool,
}

/// Every request currently open on these routes, keyed by a per-process id.
///
/// A count would be cheaper and useless: the whole point is to name a request
/// that will never come back, and only a start instant per request can do that.
static IN_FLIGHT: LazyLock<Mutex<HashMap<u64, InFlight>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

static NEXT_ID: AtomicU64 = AtomicU64::new(0);

/// The workspace segment of an `/api/{workspace}/semantic/metric-tree…` path.
///
/// Without it a flood is attributed to a label only, so "find the caller"
/// starts missing the one field that narrows it to a tenant. Read off the path
/// for the same reason `label_for` is — `MatchedPath` is unreliable this deep.
fn workspace_for(path: &str) -> String {
    path.split_once("/semantic/metric-tree")
        .map(|(prefix, _)| prefix)
        .and_then(|prefix| prefix.rsplit('/').find(|s| !s.is_empty()))
        .unwrap_or("unknown")
        .to_string()
}

/// Which metric-tree op a request path names.
///
/// Derived from the path suffix rather than axum's `MatchedPath`, which
/// `role_middleware` documents as unreliable once routes sit two `nest()`
/// levels down — the same trap applies here.
fn label_for(path: &str) -> &'static str {
    let Some((_, rest)) = path.split_once("/semantic/metric-tree") else {
        return "other";
    };
    if rest.is_empty() || rest == "/" {
        return "tree";
    }
    if rest.ends_with("/sensitivity") {
        return "sensitivity";
    }
    match rest.trim_start_matches('/') {
        "predict" => "predict",
        "explain" => "explain",
        "opportunity" => "opportunity",
        "drill" => "drill",
        "time-dimensions" => "time-dimensions",
        "distribution" => "distribution",
        "baseline" => "baseline",
        "projection" => "projection",
        _ => "other",
    }
}

/// Fold one finished request into its window, logging if a threshold trips.
///
/// The window is per `(label, workspace)`, not per `label`: keyed on the
/// label alone, a window straddling several tenants closes on whichever
/// request's `record` call happens to cross `WINDOW`, and the flood warning
/// then names that request's workspace — a tenant that may have sent only one
/// of the requests that tripped it. One `Window` (and one `String`) per pair
/// costs more entries but means every warning names a tenant that is actually
/// responsible for the rate reported.
fn record(label: &'static str, workspace: &str, elapsed: Duration, in_flight: usize) {
    if elapsed >= SLOW_REQUEST {
        tracing::warn!(
            label,
            workspace,
            elapsed_ms = elapsed.as_millis(),
            in_flight,
            "metric-tree probe: slow request"
        );
    }

    let Ok(mut windows) = WINDOWS.lock() else {
        return; // A poisoned diagnostic lock must never take the server with it.
    };
    let key = (label, workspace.to_string());
    let window = windows.entry(key.clone()).or_insert_with(Window::new);
    window.requests += 1;
    window.slowest = window.slowest.max(elapsed);

    let age = window.started.elapsed();
    if age < WINDOW {
        return;
    }
    let rate = f64::from(window.requests) / age.as_secs_f64();
    if rate >= FLOOD_RATE_PER_SEC {
        tracing::warn!(
            label,
            workspace,
            requests = window.requests,
            window_s = age.as_secs_f64(),
            rate_per_s = rate,
            slowest_ms = window.slowest.as_millis(),
            in_flight,
            "metric-tree probe: flooding — this endpoint is being called in a loop"
        );
    } else {
        // Below the threshold this is bookkeeping, not news. At `info!` it was
        // a line per endpoint per 10s window per tenant, forever.
        tracing::debug!(
            label,
            workspace,
            requests = window.requests,
            window_s = age.as_secs_f64(),
            rate_per_s = rate,
            slowest_ms = window.slowest.as_millis(),
            "metric-tree probe: window"
        );
    }
    // Swept, not just this key removed. Keyed on `label` alone the map was
    // bounded by the (fixed, tiny) label set, so resetting leaked nothing;
    // keyed on `(label, workspace)` a tenant whose *last* request lands
    // inside its still-open window returns early above and never runs this
    // line for its own key — the only removal that ever fires for a pair is
    // triggered by a LATER request from that same pair, and a tenant that
    // went away sends none. Sweeping the whole map on every close catches
    // those too.
    //
    // The bound is `WINDOW * 2`, not `WINDOW`, because a window aging past
    // `WINDOW` and a window getting REPORTED are different events: a pair's
    // own window is only reported (and removed) on that pair's *next*
    // `record` call, which can land well after `started + WINDOW`. Sweeping
    // on the tighter `WINDOW` bound deleted any other pair's window the
    // instant it crossed that line, whether or not its owner had been given
    // the chance to report it — silently discarding a genuine flood window,
    // counters and `slowest` included, with no `warn!` and no `debug!`. A
    // full extra `WINDOW` of grace guarantees every window gets at least one
    // shot at its own report before anyone else can collect it, while still
    // bounding the map to pairs seen in the last two windows — a departed
    // tenant is still caught, just one sweep later.
    windows.retain(|k, w| k != &key && w.started.elapsed() < WINDOW * 2);
}

/// Open a registry entry for this request, returning its id and the resulting
/// in-flight count.
fn register(label: &'static str, workspace: String) -> (u64, usize) {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let Ok(mut open) = IN_FLIGHT.lock() else {
        return (id, 0); // A poisoned diagnostic lock must never take the server with it.
    };
    open.insert(
        id,
        InFlight {
            label,
            workspace,
            started: Instant::now(),
            reported: false,
        },
    );
    let count = open.len();
    (id, count)
}

/// Close this request's registry entry, returning how many are still open.
fn deregister(id: u64) -> usize {
    let Ok(mut open) = IN_FLIGHT.lock() else {
        return 0;
    };
    open.remove(&id);
    open.len()
}

/// Removes a request's registry entry however the request ends.
///
/// A `deregister` on the normal return path is not enough: axum drops the
/// middleware future when the client goes away, so a tab closed or reloaded
/// mid-baseline — routine, since a baseline may run 30s — would leave the
/// entry behind forever. That is worse than the counter this replaced, which
/// leaked a number but never lied: an orphan warns "suspected hang" 60s later
/// about a request that no longer exists, inflates `in_flight` on every later
/// line, and is scanned under the lock by every subsequent request, so the
/// cost grows without bound.
///
/// `Drop` runs on cancellation too, which is the whole point of using one.
struct InFlightGuard {
    id: u64,
}

impl InFlightGuard {
    fn new(label: &'static str, workspace: String) -> (Self, usize) {
        let (id, in_flight) = register(label, workspace);
        (Self { id }, in_flight)
    }

    /// Close the entry early and report how many remain, so the normal path
    /// can log an accurate count. Idempotent with the `Drop` below.
    fn finish(self) -> usize {
        let remaining = deregister(self.id);
        std::mem::forget(self);
        remaining
    }
}

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        // Only reached when the request was cancelled — `finish` forgets the
        // guard on the normal path.
        let remaining = deregister(self.id);
        tracing::debug!(
            id = self.id,
            in_flight = remaining,
            "metric-tree probe: request cancelled before completing"
        );
    }
}

/// Entries open longer than `threshold` and not yet reported, marking them
/// reported as it goes.
///
/// Split out from the logging so the once-per-hang rule is testable without a
/// 60-second wait or a tracing subscriber.
fn take_stuck(
    open: &mut HashMap<u64, InFlight>,
    threshold: Duration,
) -> Vec<(&'static str, String, u64)> {
    let mut stuck = Vec::new();
    for entry in open.values_mut() {
        let elapsed = entry.started.elapsed();
        if entry.reported || elapsed < threshold {
            continue;
        }
        entry.reported = true;
        stuck.push((entry.label, entry.workspace.clone(), elapsed.as_secs()));
    }
    stuck
}

/// Report every request that has been open longer than `STUCK_REQUEST`.
///
/// Run at the START of each new request, which is the only place it can work:
/// a hung request never reaches the end of its own middleware, so it can never
/// report itself. Each stuck entry warns once — a hang that outlives a hundred
/// later requests is one line, not a hundred.
fn warn_if_stuck() {
    let Ok(mut open) = IN_FLIGHT.lock() else {
        return;
    };
    let in_flight = open.len();
    for (label, workspace, elapsed_s) in take_stuck(&mut open, STUCK_REQUEST) {
        tracing::warn!(
            label,
            workspace,
            elapsed_s,
            in_flight,
            "metric-tree probe: request still in flight — suspected hang"
        );
    }
}

/// Middleware over the metric-tree routes. See the module docs for how to
/// read what it emits.
pub async fn probe(req: Request, next: Next) -> Response {
    let path = req.uri().path();
    let label = label_for(path);
    let workspace = workspace_for(path);
    let started = Instant::now();

    // Before this request's own entry goes in, so a request cannot report
    // itself as stuck the moment it starts.
    warn_if_stuck();
    let (guard, in_flight) = InFlightGuard::new(label, workspace.clone());
    tracing::debug!(label, workspace, in_flight, "metric-tree probe: start");

    let response = next.run(req).await;

    let elapsed = started.elapsed();
    let remaining = guard.finish();
    tracing::debug!(
        label,
        workspace,
        elapsed_ms = elapsed.as_millis(),
        in_flight = remaining,
        "metric-tree probe: end"
    );
    record(label, &workspace, elapsed, remaining);
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_the_bare_tree_route() {
        assert_eq!(
            label_for("/api/1c9d.../semantic/metric-tree"),
            "tree",
            "the collection route is the tree itself, not an op"
        );
    }

    #[test]
    fn labels_sensitivity_past_its_measure_id() {
        // The measure id is a free-form `view.measure`, so the op has to be
        // read off the tail — matching on the segment after `metric-tree`
        // would label this one `orders.revenue`.
        assert_eq!(
            label_for("/api/w/semantic/metric-tree/orders.revenue/sensitivity"),
            "sensitivity"
        );
    }

    #[test]
    fn labels_each_op_route() {
        for (path, expected) in [
            ("/api/w/semantic/metric-tree/predict", "predict"),
            ("/api/w/semantic/metric-tree/baseline", "baseline"),
            // The scenario canvas's third leg; unlabelled it folded into
            // `other`, so a projection loop read as an unrelated flood.
            ("/api/w/semantic/metric-tree/projection", "projection"),
            ("/api/w/semantic/metric-tree/explain", "explain"),
            ("/api/w/semantic/metric-tree/opportunity", "opportunity"),
            ("/api/w/semantic/metric-tree/drill", "drill"),
            ("/api/w/semantic/metric-tree/distribution", "distribution"),
            (
                "/api/w/semantic/metric-tree/time-dimensions",
                "time-dimensions",
            ),
        ] {
            assert_eq!(label_for(path), expected, "for {path}");
        }
    }

    #[test]
    fn labels_an_unrelated_path_other() {
        assert_eq!(label_for("/api/w/semantic/world-model"), "other");
    }

    #[test]
    fn reads_the_workspace_off_the_path() {
        assert_eq!(
            workspace_for("/api/1c9d-4f/semantic/metric-tree/predict"),
            "1c9d-4f",
            "a flood attributed to a label alone omits the field that finds the caller"
        );
        assert_eq!(workspace_for("/semantic/metric-tree"), "unknown");
        assert_eq!(workspace_for("/api/w/semantic/world-model"), "unknown");
    }

    #[test]
    fn two_workspaces_on_the_same_label_get_independent_windows() {
        // Keyed on `label` alone, tenant-b's request would land in tenant-a's
        // window and inflate its count — and whichever of the two happened to
        // close the window would have its workspace on the eventual flood
        // warning, naming a tenant that only sent one of the requests. Keyed
        // on `(label, workspace)`, each tenant's counters stay its own.
        let label = "two_workspaces_on_the_same_label_get_independent_windows";
        record(label, "tenant-a", Duration::from_millis(1), 0);
        record(label, "tenant-a", Duration::from_millis(1), 0);
        record(label, "tenant-b", Duration::from_millis(1), 0);

        let windows = WINDOWS.lock().unwrap();
        let a = windows
            .get(&(label, "tenant-a".to_string()))
            .expect("tenant-a has its own window entry");
        let b = windows
            .get(&(label, "tenant-b".to_string()))
            .expect("tenant-b has its own window entry, not folded into tenant-a's");
        assert_eq!(
            a.requests, 2,
            "tenant-a's two requests must not be shared with tenant-b"
        );
        assert_eq!(
            b.requests, 1,
            "tenant-b's one request must not have counted against tenant-a's window"
        );
    }

    /// Keyed on `(label, workspace)` the map grows with the tenant set, not
    /// with the (fixed, tiny) label set — so a closed window must be REMOVED,
    /// not reset in place, or every tenant that ever touched these routes
    /// leaves a permanent entry behind in a diagnostic that is supposed to
    /// cost nothing.
    #[test]
    fn a_closed_window_is_dropped_rather_than_reset() {
        let label = "a_closed_window_is_dropped_rather_than_reset";
        let key = (label, "tenant-c".to_string());
        let already_closed = Instant::now()
            .checked_sub(WINDOW + Duration::from_secs(1))
            .expect("process has been up longer than one window");
        {
            let mut windows = WINDOWS.lock().unwrap();
            windows.insert(
                key.clone(),
                Window {
                    started: already_closed,
                    requests: 1,
                    slowest: Duration::ZERO,
                },
            );
        }

        // Folds in, sees the window has aged past `WINDOW`, reports and closes.
        record(label, "tenant-c", Duration::from_millis(1), 0);

        let windows = WINDOWS.lock().unwrap();
        assert!(
            !windows.contains_key(&key),
            "a closed window stayed in the map — keyed per tenant that is an \
             unbounded leak, not a reset"
        );
    }

    /// The leak the removal-on-close fix left behind: a tenant that sends
    /// exactly one request, whose window then ages past `WINDOW`, never gets
    /// its entry removed by `windows.remove(&key)` in `record` — that call
    /// only ever runs for the key of the request that triggered it, and this
    /// tenant never sends another. Its entry is now "closed" but immortal.
    /// Only a later, unrelated pair's close happening to sweep the whole map
    /// can catch it.
    ///
    /// Backdated past `WINDOW * 2`, not just `WINDOW`: a window that has
    /// aged past only one `WINDOW` is still within the grace period that
    /// protects it from someone else's sweep (see the comment on `retain` in
    /// `record`) — it takes a second full window with nobody around to
    /// report it before a truly abandoned pair is fair game.
    #[test]
    fn a_pair_that_never_returns_is_swept_by_another_pairs_close() {
        let gone_label = "a_pair_that_never_returns_is_swept_by_another_pairs_close_gone";
        let gone_key = (gone_label, "tenant-gone".to_string());
        let already_closed = Instant::now()
            .checked_sub(WINDOW * 2 + Duration::from_secs(1))
            .expect("process has been up longer than two windows");
        {
            let mut windows = WINDOWS.lock().unwrap();
            windows.insert(
                gone_key.clone(),
                Window {
                    started: already_closed,
                    requests: 1,
                    slowest: Duration::ZERO,
                },
            );
        }

        // A wholly unrelated pair's window closes and its own key is
        // removed — that alone must not be what leaves `gone_key` behind.
        let other_label = "a_pair_that_never_returns_is_swept_by_another_pairs_close_other";
        record(other_label, "tenant-other", Duration::from_millis(1), 0);
        {
            let mut windows = WINDOWS.lock().unwrap();
            windows
                .get_mut(&(other_label, "tenant-other".to_string()))
                .unwrap()
                .started = already_closed;
        }
        record(other_label, "tenant-other", Duration::from_millis(1), 0);

        let windows = WINDOWS.lock().unwrap();
        assert!(
            !windows.contains_key(&gone_key),
            "a tenant that sent one request and never returned left a permanent \
             entry — the sweep on another pair's close must catch stale \
             windows too, not just the one that triggered it"
        );
    }

    /// FINDING: a window's OWNER only reports it on that pair's own next
    /// `record` call — a separate event from the window merely aging past
    /// `WINDOW`. Sweeping any window that has aged out, instead of only
    /// windows that already got a full window's chance to be reported by
    /// their own owner, means an unrelated pair's close can delete a
    /// genuine flood window before it ever names itself — the one warning
    /// this module exists to emit, gone with no `warn!` and no `debug!`.
    #[test]
    fn a_window_that_just_closed_survives_an_unrelated_pairs_close_before_it_can_report_itself() {
        let victim_label = "a_window_that_just_closed_survives_an_unrelated_pairs_close_before_it_can_report_itself_victim";
        let victim_key = (victim_label, "tenant-victim".to_string());
        // Just past `WINDOW` — old enough that its OWN next `record` call
        // would report and remove it, but it has not yet been given that
        // call.
        let just_closed = Instant::now()
            .checked_sub(WINDOW + Duration::from_millis(500))
            .expect("process has been up longer than one window");
        {
            let mut windows = WINDOWS.lock().unwrap();
            windows.insert(
                victim_key.clone(),
                Window {
                    started: just_closed,
                    requests: 1,
                    slowest: Duration::ZERO,
                },
            );
        }

        // A wholly unrelated pair closes its own window in the same instant.
        let closer_label = "a_window_that_just_closed_survives_an_unrelated_pairs_close_before_it_can_report_itself_closer";
        let closer_key = (closer_label, "tenant-closer".to_string());
        let also_closed = Instant::now()
            .checked_sub(WINDOW + Duration::from_millis(500))
            .expect("process has been up longer than one window");
        {
            let mut windows = WINDOWS.lock().unwrap();
            windows.insert(
                closer_key,
                Window {
                    started: also_closed,
                    requests: 1,
                    slowest: Duration::ZERO,
                },
            );
        }
        record(closer_label, "tenant-closer", Duration::from_millis(1), 0);

        let windows = WINDOWS.lock().unwrap();
        assert!(
            windows.contains_key(&victim_key),
            "an unrelated pair's close swept a window before its own owner \
             had a chance to report it — the flood warning it would have \
             produced is now gone with no trace"
        );
    }

    fn entry(label: &'static str, age: Duration) -> InFlight {
        InFlight {
            label,
            workspace: "w".to_string(),
            started: Instant::now() - age,
            reported: false,
        }
    }

    #[test]
    fn a_request_that_never_returns_is_reported_by_the_next_one() {
        // The whole reason the registry replaced a counter. The stuck request
        // is still open — nothing it does can report it, because it never
        // reaches the end of its own middleware. A later request has to.
        let mut open = HashMap::new();
        open.insert(1, entry("predict", Duration::from_secs(120)));
        open.insert(2, entry("tree", Duration::from_millis(3)));

        let stuck = take_stuck(&mut open, STUCK_REQUEST);

        assert_eq!(stuck.len(), 1, "only the hung one: {stuck:?}");
        assert_eq!(stuck[0].0, "predict");
        assert_eq!(stuck[0].1, "w");
        assert!(stuck[0].2 >= 120);
        assert_eq!(open.len(), 2, "reporting must not close the entry");
    }

    #[test]
    fn a_hang_is_reported_once_however_many_requests_follow() {
        let mut open = HashMap::new();
        open.insert(1, entry("predict", Duration::from_secs(120)));

        assert_eq!(take_stuck(&mut open, STUCK_REQUEST).len(), 1);
        for _ in 0..5 {
            assert!(
                take_stuck(&mut open, STUCK_REQUEST).is_empty(),
                "a hang outliving later requests must cost one line, not one each"
            );
        }
    }

    #[test]
    fn a_merely_slow_request_is_not_a_hang() {
        // `SLOW_REQUEST` already covers this on the way out; a second warn for
        // the same request would make a slow warehouse read as a hang.
        let mut open = HashMap::new();
        open.insert(1, entry("baseline", Duration::from_secs(30)));
        assert!(take_stuck(&mut open, STUCK_REQUEST).is_empty());
    }

    #[test]
    fn register_and_deregister_track_what_is_open() {
        let before = IN_FLIGHT.lock().unwrap().len();
        let (id, in_flight) = register("tree", "w".to_string());
        assert_eq!(in_flight, before + 1);
        assert_eq!(deregister(id), before);
    }

    #[test]
    fn a_cancelled_request_still_leaves_the_registry() {
        // The failure mode a plain `deregister` on the return path cannot
        // cover: axum drops the middleware future when the client goes away,
        // and a baseline may run 30s, so a reload mid-request is routine. A
        // leaked entry warns "suspected hang" about a request that is gone.
        let before = IN_FLIGHT.lock().unwrap().len();
        {
            let (_guard, in_flight) = InFlightGuard::new("baseline", "w".to_string());
            assert_eq!(in_flight, before + 1);
        } // dropped without `finish`, i.e. cancelled
        assert_eq!(
            IN_FLIGHT.lock().unwrap().len(),
            before,
            "a cancelled request must not stay in the registry forever"
        );
    }

    #[test]
    fn finishing_normally_removes_the_entry_exactly_once() {
        let before = IN_FLIGHT.lock().unwrap().len();
        let (guard, _) = InFlightGuard::new("tree", "w".to_string());
        assert_eq!(guard.finish(), before);
        assert_eq!(IN_FLIGHT.lock().unwrap().len(), before);
    }
}
