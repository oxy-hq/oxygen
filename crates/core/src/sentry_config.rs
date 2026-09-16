use std::env;
use tracing::info;

/// Crates whose frames are "in app" — ours. Sentry matches these as prefixes of
/// a frame's function name (`oxy_app::…`, `agentic_runtime::…`), collapses the
/// tokio/axum/tower frames around them, and picks the culprit from them. Before
/// this, every frame arrived `inApp: false`.
const IN_APP_CRATE_PREFIXES: [&str; 3] = ["oxy", "agentic", "airhouse"];

/// The context `sentry-tracing` attaches to an event built from a `tracing`
/// call: `module_path`, `file`, `line`.
const TRACING_LOCATION_CONTEXT: &str = "Rust Tracing Location";

/// Every client option except the DSN, so a test can run events through the
/// SAME configuration prod uses rather than a hand-built approximation.
///
/// That matters twice over, both times for `before_send`:
///
/// * The shaping tests below construct a `TRACING_LOCATION_CONTEXT` themselves
///   and would keep passing if `sentry-tracing` renamed the context or changed
///   its shape — the fix would silently stop applying with the tests green.
///   `a_real_tracing_event_…` closes that by emitting through a real layer with
///   these options.
/// * `before_send` is also the whole of barrier 2 — the point where an event a
///   custom-app surface tagged is dropped — and it is installed by one
///   `.before_send(filter_event)` in this chain. Testing `filter_event`
///   directly proves the *filter* works; only reading the callback back off
///   these options proves it is *installed*. Deleting that one line is silent
///   otherwise, and it fails open: every custom-app event reaches Sentry.
fn client_options(environment: String, release: String) -> sentry::ClientOptions {
    sentry::ClientOptions::new()
        .environment(environment)
        .release(release)
        // Errors and panics only, never performance traces. Pinned, not read from
        // `SENTRY_TRACES_SAMPLE_RATE`: a transaction carries span fields (e.g.
        // `oxy.sql`) that must not leave the process, so no deployment value may
        // turn it on. 0.49 folded this into `traces_sampling_strategy`; the setter
        // writes the same fixed-rate strategy.
        .traces_sample_rate(0.0)
        .attach_stacktrace(true)
        .in_app_include(IN_APP_CRATE_PREFIXES)
        .send_default_pii(false) // Don't send personally identifiable information
        .max_breadcrumbs(100)
        .before_send(filter_event)
}

pub fn init_sentry() -> Option<sentry::ClientInitGuard> {
    let dsn = env::var("SENTRY_DSN").ok();
    if dsn.as_ref().map(|s| s.is_empty()).unwrap_or(true) {
        info!("Sentry DSN not found or empty in environment. Sentry will not be initialized.");
        return None;
    }

    let environment = if cfg!(debug_assertions) {
        "development".to_string()
    } else {
        env::var("SENTRY_ENVIRONMENT")
            .or_else(|_| env::var("ENVIRONMENT"))
            .or_else(|_| env::var("ENV"))
            .unwrap_or_else(|_| "production".to_string())
    };

    let release = format!("oxy@{}", env!("CARGO_PKG_VERSION"));

    // sentry 0.49 made `ClientOptions` `#[non_exhaustive]`, so it can no longer be
    // built with a struct literal — the consuming builder methods are the supported
    // path. `dsn` stays a direct field assignment (still allowed on a
    // `#[non_exhaustive]` struct) because the `.dsn()` builder *panics* on an
    // unparseable value, and a malformed `SENTRY_DSN` must keep degrading to "no
    // error reporting", not take the process down at startup.
    let mut options = client_options(environment.clone(), release.clone());
    options.dsn = dsn?.parse().ok();

    let guard = sentry::init(options);

    info!(
        environment = %environment,
        release = %release,
        "Sentry initialized successfully"
    );

    // Set user context if available
    sentry::configure_scope(|scope| {
        if let Ok(user_id) = env::var("USER_ID") {
            scope.set_user(Some(sentry::User {
                id: Some(user_id),
                ..Default::default()
            }));
        }

        // Add default tags
        scope.set_tag("component", "oxy-core");
        if let Ok(node_env) = env::var("NODE_ENV") {
            scope.set_tag("node_env", &node_env);
        }
    });

    Some(guard)
}

/// Sentry's last look at every event, after the scope (and so its tags) is
/// applied. Three steps, in this order:
///
/// 1. **Barrier 2.** Drop an event a custom-app surface tagged, or one whose
///    request is a custom-app request
///    (`oxy_telemetry::sentry_filter::drop_event`; the tag comes from
///    `oxy-app`'s `sentry_surface`). It runs FIRST and returns `None` — an
///    event that must not leave the process is not worth shaping, and nothing
///    downstream may resurrect it.
/// 2. **Shape a `tracing` event** so one call site is one issue
///    ([`normalize_tracing_event`]).
/// 3. **Strip absolute paths** out of the stack, so no local layout leaks.
fn filter_event(
    mut event: sentry::protocol::Event<'static>,
) -> Option<sentry::protocol::Event<'static>> {
    let request_url = event
        .request
        .as_ref()
        .and_then(|request| request.url.as_ref())
        .map(|url| url.as_str());
    if oxy_telemetry::sentry_filter::drop_event(&event.tags, request_url) {
        return None;
    }
    normalize_tracing_event(&mut event);
    if let Some(exception) = event.exception.iter_mut().next()
        && let Some(stacktrace) = &mut exception.stacktrace
    {
        for frame in &mut stacktrace.frames {
            // Remove absolute paths to avoid leaking system information
            if let Some(filename) = &mut frame.filename {
                let project_root = env!("CARGO_MANIFEST_DIR");
                if let Some(stripped) = filename.strip_prefix(project_root) {
                    *filename = stripped.trim_start_matches('/').to_string();
                }
            }
        }
    }
    Some(event)
}

/// Add context to Sentry scope for the current operation
pub fn add_operation_context(operation: &str, file_path: Option<&str>) {
    sentry::configure_scope(|scope| {
        scope.set_tag("operation", operation);
        if let Some(path) = file_path {
            scope.set_extra("file_path", path.into());
        }
        // No `scope.set_level` here. A scope level OVERRIDES the level of every
        // event captured under that scope, and this runs once at startup for
        // `serve` and `worker` — so it rewrote every ERROR the process ever
        // reported to `info`. On 2026-09-15 all 13 open issues read `info`,
        // which defeats any triage or rule that keys on level.
    });
}

/// Shape an event that came from a `tracing` call rather than a captured error
/// or a panic.
///
/// Two defects, both visible on the first day of events (2026-09-15):
///
/// * **Grouping.** `attach_stacktrace` gives such an event a stack captured
///   inside Sentry's own `prepare_event`, so one `error!` call grouped
///   differently depending on which tokio / tracing-subscriber frames sat above
///   it. `agentic task failed`, `connection budget is oversubscribed` and
///   `OXY_APP_ADMINS is set…` each arrived as two or three separate issues.
///
///   Dropping that stack leaves Sentry grouping on the message, which fans out
///   the other way: plenty of `error!` calls interpolate a value into the
///   MESSAGE rather than into a field (`"SSE db error: {e}"`), so each distinct
///   string becomes its own issue. The fingerprint is therefore set explicitly
///   from the tracing location — one issue per CALL SITE, which is what "the
///   same bug" means for a log line. It carries `module_path` and `line` and
///   nothing else: no message, no field values, so no PII travels in it.
///
///   The cost of that choice: inserting a line ABOVE an `error!` starts a new
///   issue, so an issue can go quiet after an edit that did not touch it. That
///   is inherent to per-call-site grouping and still preferable to one issue
///   per distinct interpolated string.
/// * **No culprit.** Every issue had an empty culprit. It is set from that
///   location, so an issue names the module and line that logged it.
///
/// Events that carry an exception — `capture_error`, panics — are untouched:
/// their stacks are real.
fn normalize_tracing_event(event: &mut sentry::protocol::Event<'static>) {
    if !event.exception.values.is_empty() {
        return;
    }
    let Some(sentry::protocol::Context::Other(location)) =
        event.contexts.get(TRACING_LOCATION_CONTEXT)
    else {
        return;
    };
    let module = location.get("module_path").and_then(|v| v.as_str());
    let line = location.get("line").and_then(|v| v.as_u64());
    let culprit = match (module, line) {
        (Some(module), Some(line)) => Some(format!("{module}:{line}")),
        (Some(module), None) => Some(module.to_string()),
        _ => None,
    };
    if culprit.is_some() {
        event.culprit = culprit;
    }
    // Group per call site. Only when BOTH parts are known — a module alone
    // would merge every line in a file into one issue, which is worse than the
    // message grouping it replaces.
    if let (Some(module), Some(line)) = (module, line) {
        event.fingerprint = std::borrow::Cow::Owned(vec![
            std::borrow::Cow::Owned(module.to_string()),
            std::borrow::Cow::Owned(line.to_string()),
        ]);
    }
    event.threads.values.clear();
}

pub fn add_database_context(database_name: &str, query_type: Option<&str>) {
    sentry::configure_scope(|scope| {
        scope.set_tag("database", database_name);
        if let Some(qt) = query_type {
            scope.set_tag("query_type", qt);
        }
    });
}

pub fn add_automation_context(automation_name: &str, step: Option<&str>) {
    sentry::configure_scope(|scope| {
        scope.set_tag("workflow", automation_name);
        if let Some(s) = step {
            scope.set_tag("workflow_step", s);
        }
    });
}

pub fn add_agent_context(agent_name: &str, question: Option<&str>) {
    sentry::configure_scope(|scope| {
        scope.set_tag("agent", agent_name);
        if let Some(q) = question {
            // Truncate question to avoid large context
            let truncated_question = if q.len() > 200 {
                format!("{}...", &q[..200])
            } else {
                q.to_string()
            };
            scope.set_extra("agent_question", truncated_question.into());
        }
    });
}

pub fn capture_error_with_context(error: &dyn std::error::Error, context: &str) {
    sentry::configure_scope(|scope| {
        scope.set_extra("context", context.into());
    });
    sentry::capture_error(error);
}

pub fn capture_message_with_context(message: &str, level: sentry::Level, context: &str) {
    sentry::configure_scope(|scope| {
        scope.set_extra("context", context.into());
    });
    sentry::capture_message(message, level);
}

/// Create a breadcrumb for tracking user actions
pub fn add_breadcrumb(message: &str, category: &str, level: sentry::Level) {
    sentry::add_breadcrumb(sentry::Breadcrumb {
        ty: "user".into(),
        category: Some(category.into()),
        message: Some(message.into()),
        level,
        timestamp: std::time::SystemTime::now(),
        ..Default::default()
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    #[test]
    fn test_sentry_init_without_dsn() {
        // Test that Sentry doesn't initialize without DSN
        unsafe {
            env::remove_var("SENTRY_DSN");
        }
        let guard = init_sentry();
        assert!(guard.is_none());
    }

    #[test]
    fn test_sentry_init_with_empty_dsn() {
        // Test that Sentry doesn't initialize with empty DSN
        unsafe {
            env::set_var("SENTRY_DSN", "");
        }
        let guard = init_sentry();
        assert!(guard.is_none());
        unsafe {
            env::remove_var("SENTRY_DSN");
        }
    }

    #[test]
    fn test_sentry_context_helpers() {
        // Test that context helpers don't panic
        add_operation_context("test", Some("/path/to/file.sql"));
        add_database_context("test_db", Some("SELECT"));
        add_automation_context("test_workflow", Some("step1"));
        add_agent_context("test_agent", Some("What is this?"));
        add_breadcrumb("Test action", "test", sentry::Level::Info);
    }

    fn tracing_location(fields: &[(&str, serde_json::Value)]) -> sentry::protocol::Context {
        sentry::protocol::Context::Other(
            fields
                .iter()
                .map(|(k, v)| (k.to_string(), v.clone()))
                .collect(),
        )
    }

    #[test]
    fn a_tracing_event_drops_its_synthetic_stack_and_gains_a_culprit() {
        let mut event = sentry::protocol::Event::default();
        event.contexts.insert(
            TRACING_LOCATION_CONTEXT.to_string(),
            tracing_location(&[
                (
                    "module_path",
                    serde_json::Value::from("oxy_cameras::routes::errors"),
                ),
                (
                    "file",
                    serde_json::Value::from("crates/cameras/src/routes/errors.rs"),
                ),
                ("line", serde_json::Value::from(50)),
            ]),
        );
        event
            .threads
            .values
            .push(sentry::protocol::Thread::default());

        normalize_tracing_event(&mut event);

        assert!(event.threads.values.is_empty());
        assert_eq!(
            event.culprit.as_deref(),
            Some("oxy_cameras::routes::errors:50")
        );
        assert_eq!(
            event.fingerprint.as_ref(),
            ["oxy_cameras::routes::errors", "50"],
            "grouping must be per call site, not per message string"
        );
    }

    /// The same shaping, but through the REAL client options and a REAL
    /// `sentry-tracing` layer — the tests above build the location context
    /// themselves, so they would keep passing if the crate renamed it or
    /// changed its shape, with the fix silently no longer applying.
    #[test]
    fn a_real_tracing_event_is_shaped_by_before_send() {
        use tracing_subscriber::layer::SubscriberExt;

        let events = sentry::test::with_captured_events_options(
            || {
                let subscriber =
                    tracing_subscriber::registry().with(sentry::integrations::tracing::layer());
                tracing::subscriber::with_default(subscriber, || {
                    tracing::error!("a real failure");
                });
            },
            client_options("test".to_string(), "oxy@test".to_string()),
        );

        assert_eq!(events.len(), 1, "an ERROR must arrive as one event");
        let event = &events[0];
        assert!(
            event.threads.values.is_empty(),
            "the stack sentry synthesises in prepare_event must be dropped"
        );
        let culprit = event
            .culprit
            .as_deref()
            .expect("culprit is set from the tracing location context");
        assert!(
            culprit.starts_with(module_path!()),
            "culprit should name this module, got {culprit}"
        );
        assert_eq!(
            event.fingerprint.len(),
            2,
            "fingerprint is [module_path, line], got {:?}",
            event.fingerprint
        );
        assert_eq!(event.fingerprint[0], module_path!());
    }

    #[test]
    fn an_event_with_an_exception_keeps_its_stack() {
        let mut event = sentry::protocol::Event::default();
        event.contexts.insert(
            TRACING_LOCATION_CONTEXT.to_string(),
            tracing_location(&[("module_path", serde_json::Value::from("oxy::x"))]),
        );
        event
            .exception
            .values
            .push(sentry::protocol::Exception::default());
        event
            .threads
            .values
            .push(sentry::protocol::Thread::default());

        normalize_tracing_event(&mut event);

        assert_eq!(event.threads.values.len(), 1);
        assert!(event.culprit.is_none());
    }

    #[test]
    fn an_event_not_built_from_tracing_is_left_alone() {
        let mut event = sentry::protocol::Event::default();
        event
            .threads
            .values
            .push(sentry::protocol::Thread::default());

        normalize_tracing_event(&mut event);

        assert_eq!(event.threads.values.len(), 1);
    }

    #[test]
    fn test_capture_helpers() {
        use std::io;

        let error = io::Error::new(io::ErrorKind::NotFound, "Test error");
        capture_error_with_context(&error, "Test context");

        capture_message_with_context("Test message", sentry::Level::Warning, "Test context");
    }

    #[test]
    fn filter_event_drops_what_a_custom_app_surface_tagged() {
        use oxy_telemetry::sentry_filter::{CUSTOM_APP_SURFACE, CUSTOM_APP_SURFACE_TAG};

        let mut tagged = sentry::protocol::Event::default();
        tagged.tags.insert(
            CUSTOM_APP_SURFACE_TAG.to_string(),
            CUSTOM_APP_SURFACE.to_string(),
        );
        assert!(filter_event(tagged).is_none());
        assert!(filter_event(sentry::protocol::Event::default()).is_some());
    }
}
