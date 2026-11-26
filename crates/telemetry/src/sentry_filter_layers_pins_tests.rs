//! Pins for [`sentry_tracing_layer`] after sentry-tracing 0.49.6 split the
//! combined `SentryLayer` into per-surface layers: every [`Disposition`] reaches
//! the same Sentry surfaces it did through the legacy `EventFilter` flags, and
//! `Drop` (barrier 1) reaches none. Beside `sentry_filter.rs` rather than inside
//! it: that file is over its size budget.

use tracing::Level;

use crate::sentry_filter::{
    Disposition, HTTP_TRACE_TARGET, Surfaces, sentry_disposition, sentry_tracing_layer,
};

/// Emit `f` through the shipped layer and return what reached Sentry's
/// issue stream and what reached Sentry Logs (via `before_send_log`, which
/// sees every log the client would send).
fn capture_events_and_logs(
    f: impl FnOnce(),
) -> (
    Vec<sentry::protocol::Event<'static>>,
    Vec<sentry::protocol::Log>,
) {
    use std::sync::{Arc, Mutex};
    use tracing_subscriber::layer::SubscriberExt;

    let logs: Arc<Mutex<Vec<sentry::protocol::Log>>> = Arc::default();
    let recorder = Arc::clone(&logs);
    let options = sentry::ClientOptions::new().before_send_log(move |log| {
        recorder.lock().expect("log recorder").push(log.clone());
        Some(log)
    });
    let events = sentry::test::with_captured_events_options(
        || {
            let subscriber = tracing_subscriber::registry().with(sentry_tracing_layer());
            tracing::subscriber::with_default(subscriber, f);
        },
        options,
    );
    let logs = std::mem::take(&mut *logs.lock().expect("log recorder"));
    (events, logs)
}

/// Barrier 1 through the per-surface layers: a [`Disposition::Drop`]
/// target reaches no Sentry surface at any level that the layer sees. No
/// event, no log, and no breadcrumb either, which shows as an empty
/// breadcrumb list on the platform error captured after it on the same hub.
#[test]
fn a_drop_target_reaches_no_sentry_surface_through_the_shipped_layers() {
    const DROPPED: [&str; 3] = [
        "custom_app_function",
        "oxy::app_function",
        "oxy_api_custom_apps::data_plane::query",
    ];
    for target in DROPPED {
        for level in [Level::ERROR, Level::WARN, Level::INFO] {
            assert_eq!(sentry_disposition(target, &level), Disposition::Drop);
        }
    }

    let (events, logs) = capture_events_and_logs(|| {
        tracing::error!(target: "custom_app_function", "tenant error");
        tracing::warn!(target: "custom_app_function", "tenant warn");
        tracing::info!(target: "custom_app_function", "tenant info");
        tracing::error!(target: "oxy::app_function", "pager error");
        tracing::warn!(target: "oxy::app_function", "pager warn");
        tracing::error!(target: "oxy_api_custom_apps::data_plane::query", "query error");
        tracing::warn!(target: "oxy_api_custom_apps::data_plane::query", "query warn");
        tracing::error!(target: "oxy_app::server::api::threads", "oxy's own error");
    });

    let messages: Vec<&str> = events
        .iter()
        .map(|event| event.message.as_deref().unwrap_or_default())
        .collect();
    assert_eq!(
        messages,
        vec!["oxy's own error"],
        "no dropped line may raise an issue"
    );
    assert!(
        events[0].breadcrumbs.values.is_empty(),
        "no dropped line may become a breadcrumb: {:?}",
        events[0].breadcrumbs.values
    );
    let bodies: Vec<&str> = logs.iter().map(|log| log.body.as_str()).collect();
    assert_eq!(
        bodies,
        vec!["oxy's own error"],
        "no dropped line may reach Sentry Logs"
    );
}

/// [`Disposition::LogOnly`] and [`Disposition::Breadcrumb`] through the
/// shipped layers: the 5xx line reaches Logs and nothing else, and a
/// platform WARN is both a breadcrumb and a log.
#[test]
fn log_only_and_breadcrumb_reach_their_surfaces_through_the_shipped_layers() {
    let (events, logs) = capture_events_and_logs(|| {
        tracing::error!(target: HTTP_TRACE_TARGET, status = 502, "request failed");
        tracing::warn!(target: "oxy_app::server::api::threads", "a platform warning");
        tracing::error!(target: "oxy_app::server::api::threads", "oxy's own error");
    });

    let messages: Vec<&str> = events
        .iter()
        .map(|event| event.message.as_deref().unwrap_or_default())
        .collect();
    assert_eq!(messages, vec!["oxy's own error"]);
    let crumbs: Vec<&str> = events[0]
        .breadcrumbs
        .values
        .iter()
        .filter_map(|crumb| crumb.message.as_deref())
        .collect();
    assert_eq!(
        crumbs,
        vec!["a platform warning"],
        "LogOnly must not leave a breadcrumb"
    );
    let bodies: Vec<&str> = logs.iter().map(|log| log.body.as_str()).collect();
    assert_eq!(
        bodies,
        vec!["request failed", "a platform warning", "oxy's own error"],
        "every disposition but Drop reaches Logs"
    );
}

/// The translation, pinned against the legacy `EventFilter` flags it
/// replaced: Event = Event|Log, Breadcrumb = Breadcrumb|Log, LogOnly = Log,
/// Drop = Ignore.
#[test]
fn surfaces_match_the_legacy_flags() {
    let s = |breadcrumb, event, log| Surfaces {
        breadcrumb,
        event,
        log,
    };
    assert_eq!(Disposition::Event.surfaces(), s(false, true, true));
    assert_eq!(Disposition::Breadcrumb.surfaces(), s(true, false, true));
    assert_eq!(Disposition::LogOnly.surfaces(), s(false, false, true));
    assert_eq!(Disposition::Drop.surfaces(), s(false, false, false));
}

/// Spans stay refused: even with every trace sampled, a span around a
/// platform error sends the error and no transaction.
#[test]
fn the_shipped_tracing_layer_sends_no_transaction() {
    use tracing_subscriber::layer::SubscriberExt;

    let envelopes = sentry::test::with_captured_envelopes_options(
        || {
            let subscriber = tracing_subscriber::registry().with(sentry_tracing_layer());
            tracing::subscriber::with_default(subscriber, || {
                let span = tracing::error_span!("a_request", oxy.sql = "select 1");
                let _entered = span.enter();
                tracing::error!(target: "oxy_app::server::api::threads", "oxy's own error");
            });
        },
        sentry::ClientOptions::new().traces_sample_rate(1.0),
    );

    let items: Vec<_> = envelopes
        .iter()
        .flat_map(|envelope| envelope.items())
        .collect();
    assert!(
        !items
            .iter()
            .any(|item| matches!(item, sentry::protocol::EnvelopeItem::Transaction(_))),
        "a span must never become a transaction"
    );
    assert!(
        items
            .iter()
            .any(|item| matches!(item, sentry::protocol::EnvelopeItem::Event(_))),
        "the error itself must still arrive"
    );
}
