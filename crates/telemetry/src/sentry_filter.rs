//! What reaches Sentry, as pure rules a test can pin without a Sentry client.
//!
//! Sentry is for Oxy's own platform errors (design:
//! `internal-docs/2026-09-15-custom-app-guard-sentry-and-data-shapes-design.md` §2, §4.1).
//! Custom-app code and data stay out behind two barriers:
//!
//! 1. [`sentry_disposition`], the `tracing` layer's event filter. A custom-app
//!    *target* sends nothing at any level: `oxy::app_function` (the function
//!    pager), `custom_app_function` (a tenant's own `ctx.log()`, `error`
//!    included) and anything containing `custom_apps` (every `custom_apps_*`
//!    module's default target, and the explicit `custom_apps_serve`). Otherwise
//!    `error` is an event, `warn`/`info` a breadcrumb, and `debug`/`trace`
//!    nothing — which is sentry-tracing's own default mapping today, restated
//!    here so an upgrade cannot widen it. How far below `error` reaches the
//!    layer at all is bounded where it is installed (`oxy-server`'s `logging`
//!    stops at `warn`), so the two bottom levels are belt and braces for a
//!    second installer without that bound.
//!
//!    This is also the one place that decides what an Oxy target becomes, so
//!    the per-target signal rules live here too rather than in a second filter
//!    beside it: [`HTTP_TRACE_TARGET`]'s 5xx line is a duplicate issue and is
//!    demoted to [`Disposition::LogOnly`] (#3204). Two `event_filter`s cannot
//!    coexist on one layer — the second silently replaces the first — so a new
//!    rule belongs in [`sentry_disposition`], never in another filter function.
//! 2. [`drop_event`], `before_send`'s rule. It drops what the first barrier
//!    cannot see by target: callees outside `custom_apps_*`,
//!    `OxyError::capture_to_sentry` and panics. The signal that does that work
//!    is the [`CUSTOM_APP_SURFACE_TAG`] scope tag that `oxy-app`'s
//!    `sentry_surface` sets on a custom-app request's hub. The request URL is a
//!    second rule behind it, and **it is dormant**: see [`drop_event`].

use std::collections::BTreeMap;

use tracing::Level;

/// What the Sentry layer does with one `tracing` event.
///
/// Every variant but [`Drop`](Disposition::Drop) also reaches **Sentry Logs**,
/// which is a separate surface from issues and breadcrumbs and is cheap. That
/// mirrors `sentry-tracing`'s `default_event_filter`, which sets its `Log` flag
/// alongside `Event` / `Breadcrumb`. `Drop` clears every flag: a custom-app
/// target must not reach *any* Sentry surface, Logs included.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Disposition {
    /// Captured as a Sentry issue event.
    Event,
    /// Kept as a breadcrumb on the current hub; sent only with a later event.
    Breadcrumb,
    /// Sentry Logs only — no issue, no breadcrumb. For a line that is real
    /// signal in the log store but a duplicate as an issue.
    LogOnly,
    /// Not recorded at all, on any Sentry surface.
    Drop,
}

/// Targets owned by custom-app code. Prefix, not equality, so a sub-target
/// (`oxy::app_function::alert`) is covered too.
const CUSTOM_APP_TARGET_PREFIXES: [&str; 2] = ["oxy::app_function", "custom_app_function"];

/// Every `custom_apps_*` module's default target contains this, whatever crate
/// it lives in (`oxy_app::server::api::custom_apps_serve`,
/// `oxy_app_core::custom_apps_host_dispatch`).
///
/// **This deliberately costs us Sentry visibility into Oxy's own custom-app
/// control plane** — publish, secrets, migrations, storage and health all live
/// in `custom_apps_*` modules, so their `error!` lines become Sentry issues for
/// nobody. That is the spec's call, not an oversight: those messages routinely
/// carry tenant-supplied object keys, bundle manifests and app names, and a
/// substring rule cannot tell which line does. The errors are not lost — every
/// one still reaches the platform logs in HyperDX with full context
/// (`internal-docs/platform-telemetry.md`), which is where a control-plane
/// failure is triaged. Narrowing this marker to buy paging back would have to
/// re-answer the data question first.
const CUSTOM_APPS_MODULE_MARKER: &str = "custom_apps";

fn is_custom_app_target(target: &str) -> bool {
    CUSTOM_APP_TARGET_PREFIXES
        .iter()
        .any(|prefix| target.starts_with(prefix))
        || target.contains(CUSTOM_APPS_MODULE_MARKER)
}

/// The `tracing` target of [`crate::http_trace`]'s `OxyOnFailure` STATUS-CODE
/// arm — the 5xx line.
///
/// Matched exactly (`==`), never as a prefix, so it does not catch
/// `oxy_telemetry::http_trace::transport`. That is deliberate: see below.
pub const HTTP_TRACE_TARGET: &str = "oxy_telemetry::http_trace";

/// Barrier 1, plus the per-target signal rules. See the module docs.
///
/// **Why the 5xx line is [`Disposition::LogOnly`]** (#3204). `OxyOnFailure`
/// logs every 5xx at ERROR, which is right for the log store: it is the one
/// line per failed request. As a Sentry *issue* it is a copy. The handler that
/// produced the 5xx has already logged its own error with the cause (`cameras
/// request failed`, `SQL query execution failed`, …), while this line carries
/// only a status and a latency — no route, no reason. On 2026-09-15 it was the
/// largest issue in Sentry and ~3,000 lines a week in prod, i.e. most of prod's
/// error quota spent on the least informative event. The line itself is
/// untouched: it stays in the log store (stderr → HyperDX), which is the
/// complete record, and it still reaches Sentry Logs — cheap, searchable, and
/// not what was noisy. Only the Sentry ISSUE goes away.
///
/// **Only the status-code arm.** `OxyOnFailure`'s other arm — a transport /
/// connection failure — has no response and no handler error line behind it, so
/// that line is the ONLY record of it. It logs under its own target
/// (`…::http_trace::transport`) precisely so this rule cannot reach it;
/// `Metadata` carries no fields, so a filter could not tell the two apart any
/// other way.
///
/// **No `Breadcrumb` for it, deliberately.** It would not land where it looks
/// like it should. `create_trace_layer()` wraps the whole router from the
/// outside (`cli/commands/serve.rs`), while `NewSentryLayer` is applied inside
/// `finalize_router` (`server/router/entry.rs`) — so by the time `on_failure`
/// classifies the response, the per-request hub has been popped and the
/// handler's own event has ALREADY been captured. A breadcrumb added there goes
/// to the long-lived per-worker-thread hub, where it can only attach to some
/// LATER, unrelated event on that thread, carrying another request's status.
/// Attaching it properly would mean putting the request hub outside the trace
/// layer, which is a larger change than this one.
///
/// **Only that target's ERROR line.** The demotion is levelled as well as
/// targeted, because the target carries more than the 5xx line: `OxyOnResponse`
/// logs a span-less probe's 4xx at WARN (`http_trace.rs`, the `421` role
/// routing answers) and a request's own 4xx at INFO under the same module path.
/// Those are not duplicates of anything — no handler error sits behind a `421` —
/// and the rule this replaced could not reach them either: it ran
/// `default_event_filter` and removed only `EventFilter::Event`, which is a
/// no-op on a level that never had it. Dropping the level condition would take
/// the breadcrumb off every `421`, i.e. off exactly the signal split-fleet
/// routing is diagnosed with.
pub fn sentry_disposition(target: &str, level: &Level) -> Disposition {
    // Barrier 1 first and unconditionally: a custom-app target is refused every
    // Sentry surface, so no later rule may promote it back onto one.
    if is_custom_app_target(target) {
        Disposition::Drop
    } else if *level == Level::DEBUG || *level == Level::TRACE {
        // sentry-tracing's own floor, restated so it cannot move under us:
        // `default_event_filter` answers `EventFilter::Ignore` for these two
        // (sentry-tracing 0.49.1 `layer/mod.rs:89`) — not a log line, not a
        // breadcrumb. `logging`'s `LevelFilter::WARN` means nothing this low
        // reaches the layer in the shipped subscriber anyway; stating it keeps
        // the module doc's parity claim true for any other installer, and keeps
        // a `trace!` firehose out of `max_breadcrumbs(100)`.
        Disposition::Drop
    } else if target == HTTP_TRACE_TARGET && *level == Level::ERROR {
        Disposition::LogOnly
    } else if *level == Level::ERROR {
        Disposition::Event
    } else {
        Disposition::Breadcrumb
    }
}

/// The Sentry `tracing` layer, with barrier 1 attached — the layer `oxy-server`'s
/// `logging` installs, built here rather than at the call site so a test can
/// prove the filter is actually on it.
///
/// Caller-supplied level bounds still apply on top (`logging` adds
/// `LevelFilter::WARN`); nothing here widens what the subscriber offers. Spans
/// are refused outright: Sentry records errors and panics only, and a
/// transaction would carry span fields off the box.
///
/// **This is the layer's one and only `event_filter`.** `SentryLayer` holds a
/// single callback, so a second `.event_filter(..)` — here or at the call site
/// — does not compose, it replaces, silently. A new rule about what a target
/// becomes therefore goes into [`sentry_disposition`]; the closure below stays
/// a pure translation of it into sentry-tracing's flags.
///
/// `EventFilter::Log` rides along with `Event` and `Breadcrumb`, as
/// `default_event_filter` does, and is not inert: `ClientOptions::new()` is
/// `Default::default()` and `enable_logs` defaults to **true** (sentry-core
/// 0.49.1 `clientoptions.rs:859`). With the `logs` feature on — it is in
/// `sentry`'s default set — those lines reach Sentry Logs, which is cheap and
/// searchable and was never the noise. Only [`Disposition::Drop`] clears it: a
/// custom-app target is refused Logs too, which is the point of barrier 1.
pub fn sentry_tracing_layer<S>() -> sentry::integrations::tracing::SentryLayer<S>
where
    S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    sentry::integrations::tracing::layer()
        .event_filter(|metadata: &tracing::Metadata<'_>| {
            use sentry::integrations::tracing::EventFilter;
            match sentry_disposition(metadata.target(), metadata.level()) {
                Disposition::Event => EventFilter::Event | EventFilter::Log,
                Disposition::Breadcrumb => EventFilter::Breadcrumb | EventFilter::Log,
                Disposition::LogOnly => EventFilter::Log,
                Disposition::Drop => EventFilter::Ignore,
            }
        })
        .span_filter(|_| false)
}

/// The scope tag a custom-app surface puts on its hub, and its value.
pub const CUSTOM_APP_SURFACE_TAG: &str = "oxy.surface";
/// See [`CUSTOM_APP_SURFACE_TAG`].
pub const CUSTOM_APP_SURFACE: &str = "custom_app";

/// Whether a request is for a custom app. Two kinds match:
/// - a host with a `customer-apps` or `customer-apps-<env>` label (the
///   `<org>--<slug>.customer-apps[-<env>].<zone>` scheme);
/// - a `/customer-apps/**` path.
///
/// Looser than the router's own host parser on purpose: a false drop costs one
/// platform event, a false keep ships customer data. `/api/customer-apps/**` is
/// the platform's admin and publish API and does not match.
pub fn is_custom_app_request(host: Option<&str>, path: &str) -> bool {
    let custom_app_host = host.is_some_and(|host| {
        // No allocation: this runs on every captured event, and on the
        // custom-app data plane that is the hot path. Compare each label in
        // place rather than lowercasing the whole host.
        let host = host.split(':').next().unwrap_or(host);
        host.split('.').any(|label| {
            label.eq_ignore_ascii_case("customer-apps")
                || label
                    .get(.."customer-apps-".len())
                    .is_some_and(|head| head.eq_ignore_ascii_case("customer-apps-"))
        })
    });
    custom_app_host || path == "/customer-apps" || path.starts_with("/customer-apps/")
}

/// Barrier 2, `before_send`'s rule. Drop an event carrying the custom-app
/// surface tag. With no tag, drop one whose `request.url` is a custom-app
/// request (only events that carry a request have a URL).
///
/// **The tag does all of today's work; the URL arm is dormant.** Nothing in
/// this workspace populates `Event::request`: the only sentry-tower layer
/// mounted is `NewSentryLayer` (`server/router/entry.rs`, `router/openapi.rs`),
/// which binds a hub per request and sets no request data — `SentryHttpLayer`
/// is mounted nowhere, and there is no `scope.set_request` or
/// `add_event_processor` in the tree. So `request_url` arrives `None` on every
/// real event, and the two paths barrier 1 cannot see —
/// `OxyError::capture_to_sentry` and a panic — are covered by the TAG alone,
/// which is why `oxy-app`'s `sentry_surface` sets it at the top of
/// `check_custom_app_gates`, before authentication, rather than leaving them to
/// a URL.
///
/// It is kept rather than deleted because it is the fail-closed half: the day
/// someone mounts a request integration to get URLs onto issues, custom-app
/// URLs are already refused instead of shipping until someone notices. Keep the
/// asymmetry in mind when reading the tests below — `is_custom_app_request` is
/// live (`sentry_surface`'s URL rule calls it), `drop_event`'s URL arm is not.
pub fn drop_event(tags: &BTreeMap<String, String>, request_url: Option<&str>) -> bool {
    if tags.get(CUSTOM_APP_SURFACE_TAG).map(String::as_str) == Some(CUSTOM_APP_SURFACE) {
        return true;
    }
    request_url
        .and_then(|url| url.split('#').next())
        .and_then(|url| url.parse::<http::Uri>().ok())
        .is_some_and(|uri| is_custom_app_request(uri.host(), uri.path()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tracing::Level;

    const ALL_LEVELS: [Level; 5] = [
        Level::ERROR,
        Level::WARN,
        Level::INFO,
        Level::DEBUG,
        Level::TRACE,
    ];

    #[test]
    fn error_on_a_platform_target_is_an_event() {
        assert_eq!(
            sentry_disposition("oxy_app::server::api::threads", &Level::ERROR),
            Disposition::Event
        );
    }

    #[test]
    fn warn_on_a_platform_target_is_only_a_breadcrumb() {
        assert_eq!(
            sentry_disposition("oxy_app::server::api::threads", &Level::WARN),
            Disposition::Breadcrumb
        );
    }

    #[test]
    fn the_function_pager_target_is_dropped_at_every_level() {
        for level in ALL_LEVELS {
            assert_eq!(
                sentry_disposition("oxy::app_function", &level),
                Disposition::Drop,
                "{level}"
            );
        }
    }

    /// A tenant's `ctx.log()` line, `error` included (`custom_apps_functions/runtime.rs`).
    #[test]
    fn a_tenants_ctx_log_target_is_dropped_at_every_level() {
        for level in ALL_LEVELS {
            assert_eq!(
                sentry_disposition("custom_app_function", &level),
                Disposition::Drop,
                "{level}"
            );
        }
    }

    #[test]
    fn custom_apps_module_targets_are_dropped_at_every_level() {
        let targets = [
            "oxy_app::server::api::custom_apps_functions::runtime",
            "oxy_app::server::api::custom_apps_serve",
            "custom_apps_serve",
            "oxy_app_core::custom_apps_host_dispatch",
        ];
        for target in targets {
            for level in ALL_LEVELS {
                assert_eq!(
                    sentry_disposition(target, &level),
                    Disposition::Drop,
                    "{target} {level}"
                );
            }
        }
    }

    /// The drop rule must not swallow platform code that merely names an app:
    /// the app-function webhook's `db connection failed` is Oxy's own error.
    #[test]
    fn a_platform_target_that_only_mentions_apps_is_kept() {
        assert_eq!(
            sentry_disposition(
                "oxy_app::server::api::webhooks::app_function",
                &Level::ERROR
            ),
            Disposition::Event
        );
    }

    /// #3204. The 5xx line duplicates the handler's own error, so it raises no
    /// issue — but it is still real signal, so it is not dropped either.
    #[test]
    fn the_http_trace_5xx_line_is_logs_only() {
        assert_eq!(
            sentry_disposition(HTTP_TRACE_TARGET, &Level::ERROR),
            Disposition::LogOnly
        );
    }

    /// The transport arm is NOT a duplicate — nothing else logs a connection
    /// failure, so it must still raise an issue. Its separate target is the
    /// only thing standing between it and the rule above, which is why that
    /// rule matches `==` and never a prefix.
    #[test]
    fn the_http_trace_transport_arm_is_still_an_event() {
        assert_eq!(
            sentry_disposition("oxy_telemetry::http_trace::transport", &Level::ERROR),
            Disposition::Event
        );
    }

    /// The 5xx demotion is levelled, not just targeted. `OxyOnResponse` logs a
    /// span-less probe's 4xx — the `421` role routing answers — at WARN under
    /// this same target, and an ordinary 4xx at INFO. Neither duplicates a
    /// handler line, and the rule this replaced left them alone
    /// (`default_event_filter` then `remove(EventFilter::Event)` is a no-op on a
    /// level that never had `Event`). Losing the WARN breadcrumb would cost the
    /// `421` context on every issue raised after one.
    #[test]
    fn a_non_error_line_on_the_http_trace_target_is_still_a_breadcrumb() {
        assert_eq!(
            sentry_disposition(HTTP_TRACE_TARGET, &Level::WARN),
            Disposition::Breadcrumb
        );
        assert_eq!(
            sentry_disposition(HTTP_TRACE_TARGET, &Level::INFO),
            Disposition::Breadcrumb
        );
    }

    /// The module doc claims the level half matches sentry-tracing's default.
    /// That claim is only true if the two bottom levels reach no Sentry surface
    /// at all: `default_event_filter` answers `EventFilter::Ignore` for
    /// DEBUG/TRACE (sentry-tracing 0.49.1 `layer/mod.rs:89`), so a breadcrumb or
    /// a log line there would be us WIDENING what the SDK sends, in the one
    /// direction this module exists to prevent.
    #[test]
    fn debug_and_trace_reach_no_sentry_surface() {
        for level in [Level::DEBUG, Level::TRACE] {
            assert_eq!(
                sentry_disposition("oxy_app::server::api::threads", &level),
                Disposition::Drop,
                "{level}"
            );
            // Including on the target whose ERROR line is demoted: the arms must
            // not disagree about what a `debug!` there is worth.
            assert_eq!(
                sentry_disposition(HTTP_TRACE_TARGET, &level),
                Disposition::Drop,
                "{level}"
            );
        }
    }

    fn surface_tags(value: &str) -> BTreeMap<String, String> {
        BTreeMap::from([(CUSTOM_APP_SURFACE_TAG.to_string(), value.to_string())])
    }

    #[test]
    fn an_event_tagged_by_a_custom_app_surface_is_dropped() {
        assert!(drop_event(&surface_tags(CUSTOM_APP_SURFACE), None));
    }

    #[test]
    fn an_event_with_no_surface_tag_and_no_request_is_kept() {
        assert!(!drop_event(&BTreeMap::new(), None));
        assert!(!drop_event(&surface_tags("platform"), None));
    }

    #[test]
    fn an_event_from_a_custom_app_subdomain_url_is_dropped() {
        for url in [
            "https://acme--store.customer-apps.oxygen-hq.com/api/projects/1/query",
            "https://acme--store.customer-apps.staging.oxy.tech/",
            "https://mars--app.customer-apps-dev.oxygen-hq.com:443/assets/main.js",
        ] {
            assert!(drop_event(&BTreeMap::new(), Some(url)), "{url}");
        }
    }

    #[test]
    fn an_event_from_a_customer_apps_path_on_the_admin_host_is_dropped() {
        for url in [
            "https://app.oxygen-hq.com/customer-apps/acme/store/fn/orders?limit=5",
            "https://aip.dev.oxy.tech/customer-apps/acme/store#section",
        ] {
            assert!(drop_event(&BTreeMap::new(), Some(url)), "{url}");
        }
    }

    /// A `Host` header's case is not normalised for us, and the label match is
    /// allocation-free, so the case-insensitivity is load-bearing rather than
    /// incidental.
    #[test]
    fn a_custom_app_host_matches_whatever_its_case() {
        for host in [
            "ACME--STORE.CUSTOMER-APPS.OXYGEN-HQ.COM",
            "Acme--Store.Customer-Apps.oxygen-hq.com",
            "mars--app.Customer-Apps-Dev.oxygen-hq.com:443",
        ] {
            assert!(is_custom_app_request(Some(host), "/"), "{host}");
        }
        // A label that merely starts with the same letters is not a match.
        assert!(!is_custom_app_request(
            Some("customer-appsx.oxygen-hq.com"),
            "/"
        ));
    }

    /// Barrier 1 is only real if the filter is actually *attached* to the layer
    /// the subscriber installs. This drives [`sentry_tracing_layer`] — the same
    /// constructor `oxy-server`'s `logging` pushes — rather than re-deciding the
    /// rule, so deleting `.event_filter(..)` from it fails here instead of
    /// silently restoring sentry-tracing's default (which makes ERROR an event
    /// at every target, custom-app ones included).
    #[test]
    fn the_shipped_tracing_layer_carries_the_custom_app_filter() {
        use tracing_subscriber::layer::SubscriberExt;

        let events = sentry::test::with_captured_events(|| {
            let subscriber = tracing_subscriber::registry().with(sentry_tracing_layer());
            tracing::subscriber::with_default(subscriber, || {
                tracing::error!(target: "custom_app_function", "a tenant's ctx.log error");
                tracing::error!(target: "oxy::app_function", "the function pager");
                tracing::error!(target: "oxy_app::server::api::custom_apps_serve", "bundle");
                tracing::error!(target: "oxy_app::server::api::threads", "oxy's own error");
            });
        });

        let messages: Vec<&str> = events
            .iter()
            .map(|event| event.message.as_deref().unwrap_or_default())
            .collect();
        assert_eq!(
            messages,
            vec!["oxy's own error"],
            "only the platform ERROR may become an event"
        );
    }

    /// The other half of the same guarantee: a `warn` is kept as a breadcrumb,
    /// never an event. Pins `.span_filter(|_| false)` and the level rule as the
    /// layer really installs them.
    #[test]
    fn the_shipped_tracing_layer_makes_warn_a_breadcrumb_not_an_event() {
        use tracing_subscriber::layer::SubscriberExt;

        let events = sentry::test::with_captured_events(|| {
            let subscriber = tracing_subscriber::registry().with(sentry_tracing_layer());
            tracing::subscriber::with_default(subscriber, || {
                tracing::warn!(target: "oxy_app::server::api::threads", "a platform warning");
            });
        });
        assert!(events.is_empty(), "a warn must not raise an event");
    }

    /// The levelled 5xx rule, through the layer that really ships rather than
    /// through [`sentry_disposition`] alone: the ERROR line raises no issue, and
    /// the WARN line on the SAME target is still attached to the next issue as a
    /// breadcrumb. `EventFilter::Breadcrumb` only means something if sentry
    /// actually carries it onto an event, which is what this asserts.
    #[test]
    fn the_shipped_tracing_layer_keeps_the_probe_warn_as_a_breadcrumb() {
        use tracing_subscriber::layer::SubscriberExt;

        let events = sentry::test::with_captured_events(|| {
            let subscriber = tracing_subscriber::registry().with(sentry_tracing_layer());
            tracing::subscriber::with_default(subscriber, || {
                tracing::warn!(
                    target: "oxy_telemetry::http_trace",
                    status = 421,
                    "probe request did not succeed"
                );
                tracing::error!(
                    target: "oxy_telemetry::http_trace",
                    status = 502,
                    "request failed"
                );
                tracing::error!(target: "oxy_app::server::api::threads", "oxy's own error");
            });
        });

        let messages: Vec<&str> = events
            .iter()
            .map(|event| event.message.as_deref().unwrap_or_default())
            .collect();
        assert_eq!(
            messages,
            vec!["oxy's own error"],
            "the 5xx line raises no issue; only the handler's own error does"
        );
        let crumbs: Vec<&str> = events[0]
            .breadcrumbs
            .values
            .iter()
            .filter_map(|crumb| crumb.message.as_deref())
            .collect();
        assert_eq!(
            crumbs,
            vec!["probe request did not succeed"],
            "the 421 line must still reach the issue as a breadcrumb"
        );
    }

    /// `/api/customer-apps/**` is the platform's own admin and publish API.
    #[test]
    fn a_platform_url_is_kept() {
        for url in [
            "https://app.oxygen-hq.com/api/customer-apps/1f0c/builds",
            "https://app.oxygen-hq.com/api/threads",
            "https://pokehouse.oxygen-hq.com/api/threads",
            "not a url",
        ] {
            assert!(!drop_event(&BTreeMap::new(), Some(url)), "{url}");
        }
    }
}
