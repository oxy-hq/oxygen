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
        // Barrier 2's other half. Sentry Logs is a separate surface with a
        // separate callback: `before_send` never sees a log, so without this a
        // custom-app line dropped as an issue stays searchable in Sentry.
        .before_send_log(filter_log)
}

/// `build_sha` is the commit this binary was built from, short form. It is passed
/// in rather than read from a `rustc-env` here so that this crate needs no build
/// script: see `oxy_app::BUILD_SHA`, which is where it comes from in the binary.
/// `"dev"`, `"unknown"` and `""` all mean "no commit" and yield the bare release.
pub fn init_sentry(build_sha: &str) -> Option<sentry::ClientInitGuard> {
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

    let release = release_id(env!("CARGO_PKG_VERSION"), build_sha);

    // sentry 0.49 made `ClientOptions` `#[non_exhaustive]`, so it can no longer be
    // built with a struct literal — the consuming builder methods are the supported
    // path. `dsn` stays a direct field assignment (still allowed on a
    // `#[non_exhaustive]` struct) because the `.dsn()` builder *panics* on an
    // unparseable value, and a malformed `SENTRY_DSN` must keep degrading to "no
    // error reporting", not take the process down at startup. It is also the one
    // option that cannot live in `client_options`, which a test builds.
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

/// Sentry's last look at every **log**, the surface `before_send` cannot reach.
///
/// Judged on the emitting module path, which is what a `Log` carries; the
/// custom-app surface tag is not on it. Two rules, both held and documented in
/// `oxy_telemetry::sentry_filter`: `drop_log` drops a custom-app module's line
/// outright, and `is_data_plane_module` / `keep_log_shape_only` strip a
/// query-running platform module's line to its shape, since its text is the
/// warehouse's and the request it served cannot be told here.
fn filter_log(mut log: sentry::protocol::Log) -> Option<sentry::protocol::Log> {
    let module = log
        .attributes
        .get(oxy_telemetry::sentry_filter::LOG_MODULE_ATTRIBUTE)
        .and_then(|attribute| attribute.0.as_str());
    if oxy_telemetry::sentry_filter::drop_log(module) {
        return None;
    }
    if module.is_some_and(oxy_telemetry::sentry_filter::is_data_plane_module) {
        oxy_telemetry::sentry_filter::keep_log_shape_only(&mut log);
    }
    Some(log)
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

/// The Sentry release for a build: `oxy@<version>`, plus the build commit when
/// there is one.
///
/// The sha is what makes it a *deploy* identity. `CARGO_PKG_VERSION` is the last
/// published semver, so on the deploy train it is the same string for every digest
/// between two publishes — `promote.yaml` asks Sentry "what is new in this
/// release?" as a promotion gate, and without the sha that question spans weeks of
/// deploys and always answers yes.
///
/// A local build has no sha and gets the bare form rather than a fake one: an
/// invented sha would pool every developer's laptop into one release.
fn release_id(version: &str, build_sha: &str) -> String {
    // `build.rs` writes "dev" for a local build and CI writes "unknown" when it
    // has nothing — the same two sentinels `/version` checks. Neither is a
    // commit, and pooling every developer's laptop into `oxy@0.5.149+dev` is
    // worse than the bare form.
    if matches!(build_sha, "" | "dev" | "unknown") {
        format!("oxy@{version}")
    } else {
        format!("oxy@{version}+{build_sha}")
    }
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
        let guard = init_sentry("dev");
        assert!(guard.is_none());
    }

    #[test]
    fn test_sentry_init_with_empty_dsn() {
        // Test that Sentry doesn't initialize with empty DSN
        unsafe {
            env::set_var("SENTRY_DSN", "");
        }
        let guard = init_sentry("dev");
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

    /// I1. The **wiring**, not the filter. `filter_event` is only barrier 2 if
    /// `init_sentry` actually installs it, so read the callback back off the
    /// options `init_sentry` builds and run the same two events through *that*.
    ///
    /// The test above passes whether or not `.before_send(filter_event)` is in
    /// the chain — which is the gap. Deleting that one line is silent, and it
    /// fails open: every custom-app event reaches Sentry.
    #[test]
    fn init_installs_the_custom_app_filter_as_before_send() {
        use oxy_telemetry::sentry_filter::{CUSTOM_APP_SURFACE, CUSTOM_APP_SURFACE_TAG};

        let before_send = client_options("test".to_string(), "oxy@test".to_string())
            .before_send
            .clone()
            .expect("init_sentry must install a before_send callback; it is barrier 2");

        let mut tagged = sentry::protocol::Event::default();
        tagged.tags.insert(
            CUSTOM_APP_SURFACE_TAG.to_string(),
            CUSTOM_APP_SURFACE.to_string(),
        );
        assert!(
            before_send(tagged).is_none(),
            "the installed callback must drop a custom-app-tagged event"
        );
        assert!(
            before_send(sentry::protocol::Event::default()).is_some(),
            "and must keep an ordinary platform event"
        );
    }

    fn tracing_log(module: Option<&str>) -> sentry::protocol::Log {
        let mut log = sentry::protocol::Log {
            level: sentry::protocol::LogLevel::Error,
            body: "a failure".to_string(),
            trace_id: None,
            timestamp: std::time::SystemTime::now(),
            severity_number: None,
            attributes: Default::default(),
        };
        if let Some(module) = module {
            log.attributes.insert(
                oxy_telemetry::sentry_filter::LOG_MODULE_ATTRIBUTE.to_string(),
                sentry::protocol::LogAttribute(module.into()),
            );
        }
        log
    }

    /// I1, for Logs. The same shape as the `before_send` pin above: read the
    /// callback back off the options `init_sentry` builds, so deleting
    /// `.before_send_log(filter_log)` fails here rather than silently reopening
    /// the third Sentry surface.
    #[test]
    fn init_installs_the_custom_app_log_filter_as_before_send_log() {
        let before_send_log = client_options("test".to_string(), "oxy@test".to_string())
            .before_send_log
            .clone()
            .expect("init_sentry must install a before_send_log callback; it is barrier 2");

        assert!(
            before_send_log(tracing_log(Some(
                "oxy_app::server::api::custom_apps_functions::runtime"
            )))
            .is_none(),
            "the installed callback must drop a custom-app module's log"
        );
        assert!(
            before_send_log(tracing_log(Some("oxy_app::server::api::threads"))).is_some(),
            "and must keep an ordinary platform log"
        );
    }

    /// A line as sentry-tracing 0.49.1 hands it to `before_send_log`: the
    /// rendered message in `body`, the call's own fields and an enclosing
    /// span's `span:field` copy in `attributes`, the `code.*` location keys
    /// beside them.
    fn data_bearing_log(module: &str) -> sentry::protocol::Log {
        let mut log = tracing_log(Some(module));
        log.body = "query failed: relation \"orders\" does not exist".to_string();
        for (key, value) in [
            (
                "code.file.path",
                serde_json::json!("crates/app/src/server/api/projects/query.rs"),
            ),
            ("code.line.number", serde_json::json!(295)),
            (
                "msg",
                serde_json::json!("relation \"orders\" does not exist"),
            ),
            (
                "semantic_query:oxy.sql",
                serde_json::json!("select count(*) from orders"),
            ),
        ] {
            log.attributes
                .insert(key.to_string(), sentry::protocol::LogAttribute(value));
        }
        log
    }

    /// Review I1, through the client `init_sentry` really builds, not just the
    /// callback read back off it. The client stamps its own attributes on a
    /// log *before* `before_send_log` runs (sentry-core 0.49.1 `prepare_log`:
    /// `Scope::apply_to_log`, then `sentry.environment`, `sentry.release`,
    /// `sentry.sdk.*`, then the callback), so a shape rule that kept only the
    /// `code.*` keys threw the environment and release out with the tenant's
    /// values — and Sentry Logs filters on exactly those two, so the shaped
    /// line vanished from a production-scoped view and joined no release.
    /// Asserted on the line as it leaves the client: level and location kept,
    /// deployment identity kept, body and every field value gone. Delete the
    /// `is_data_plane_module` branch of `filter_log` and the body assertion
    /// fails; drop `sentry.environment` from `LOG_SHAPE_ATTRIBUTES` and the
    /// identity one does.
    ///
    /// `init_sentry`'s `sentry::init` also stamps `server.address` and `os.*`
    /// through `apply_defaults`; the test client skips those defaults, so they
    /// are pinned on the keep-list in `oxy_telemetry::sentry_filter` instead.
    #[test]
    fn a_data_plane_log_leaves_with_its_shape_only() {
        use oxy_telemetry::sentry_filter::{LOG_BODY_WITHHELD, LOG_MODULE_ATTRIBUTE};
        use sentry::protocol::{EnvelopeItem, ItemContainer};

        let envelopes = sentry::test::with_captured_envelopes_options(
            || {
                sentry::Hub::current()
                    .capture_log(data_bearing_log("oxy_app::server::api::projects::query"));
            },
            client_options("test".to_string(), "oxy@test".to_string()),
        );
        let sent: Vec<&sentry::protocol::Log> = envelopes
            .iter()
            .flat_map(|envelope| envelope.items())
            .filter_map(|item| match item {
                EnvelopeItem::ItemContainer(ItemContainer::Logs(logs)) => Some(logs.iter()),
                _ => None,
            })
            .flatten()
            .collect();
        let [sent] = sent[..] else {
            panic!(
                "a data-plane log is shaped, not dropped: the count is still signal; got {sent:?}"
            );
        };

        assert_eq!(sent.body, LOG_BODY_WITHHELD);
        let mut kept: Vec<&str> = sent.attributes.keys().map(String::as_str).collect();
        kept.sort_unstable();
        assert_eq!(
            kept,
            [
                "code.file.path",
                "code.line.number",
                LOG_MODULE_ATTRIBUTE,
                "sentry.environment",
                "sentry.release",
                "sentry.sdk.name",
                "sentry.sdk.version",
            ],
            "the location keys and the client's own stamps leave; every value from the call stays behind"
        );
        assert_eq!(
            sent.attributes["sentry.environment"].0,
            serde_json::json!("test")
        );
        assert_eq!(
            sent.attributes["sentry.release"].0,
            serde_json::json!("oxy@test")
        );
        assert!(
            !sent
                .attributes
                .values()
                .any(|attribute| attribute.0.to_string().contains("orders")),
            "no value may still name the tenant's table: {:?}",
            sent.attributes
        );
    }

    /// The other side of the same rule: a platform module that runs no
    /// warehouse query keeps its line whole — body, fields and all — because
    /// its text is Oxy's own. Compared as a whole `Log`, so a future "just
    /// trim this one attribute" cannot creep in unasserted.
    #[test]
    fn a_platform_log_from_elsewhere_is_untouched() {
        let before_send_log = client_options("test".to_string(), "oxy@test".to_string())
            .before_send_log
            .clone()
            .expect("init_sentry must install a before_send_log callback");
        let log = data_bearing_log("oxy_app::server::api::threads");

        assert_eq!(before_send_log(log.clone()), Some(log));
    }

    /// Why [`filter_log`] judges by module path and not by the custom-app
    /// surface tag: the tag never arrives. `Scope::apply_to_log` copies
    /// `trace_id`, `parent_span_id` and `user.*` onto a log and nothing else —
    /// `scope.tags` is not among them, and `protocol::Log` has no tags field to
    /// put them in.
    ///
    /// Pinned rather than merely written down, because this is a *dependency's*
    /// behaviour and the thing it costs us is real (a platform error logged
    /// during a custom-app request keeps its log line). If a future sentry-core
    /// starts carrying tags, this test fails and says: tighten `filter_log` to
    /// the tag rule.
    #[test]
    fn a_scope_tag_does_not_reach_a_log() {
        use oxy_telemetry::sentry_filter::{CUSTOM_APP_SURFACE, CUSTOM_APP_SURFACE_TAG};

        let mut scope = sentry::Scope::default();
        scope.set_tag(CUSTOM_APP_SURFACE_TAG, CUSTOM_APP_SURFACE);
        let mut log = tracing_log(Some("oxy_app::server::api::threads"));
        scope.apply_to_log(&mut log);

        assert!(
            !log.attributes.contains_key(CUSTOM_APP_SURFACE_TAG),
            "sentry-core does not copy scope tags onto a log; if it now does, \
             filter_log should drop on the tag instead of the module path"
        );
        assert!(
            filter_log(log).is_some(),
            "so a platform module's log survives even under a tagged scope — the \
             documented residual gap, asserted so it cannot change unnoticed"
        );
    }

    /// The other pin in the same chain. A transaction carries span fields
    /// (`oxy.sql` among them) that must not leave the process, so tracing being
    /// off is a property of the build, not a deployment knob. Asserted as the
    /// invariant rather than the spelling: both ways of saying "off" pass, a
    /// non-zero rate or a sampler function does not.
    #[test]
    fn traces_are_pinned_off() {
        match client_options("test".to_string(), "oxy@test".to_string()).traces_sampling_strategy {
            sentry::TracesSamplingStrategy::Disabled => {}
            sentry::TracesSamplingStrategy::FixedRate(rate) => {
                assert_eq!(rate, 0.0, "no deployment may turn performance traces on")
            }
            other => panic!("traces must be off, got {other:?}"),
        }
    }

    #[test]
    fn a_release_id_names_the_deploy_not_just_the_version() {
        assert_eq!(
            super::release_id("0.5.149", "a1b2c3d"),
            "oxy@0.5.149+a1b2c3d",
            "two digests that share a published version must not share a release"
        );
        for absent in ["", "dev", "unknown"] {
            assert_eq!(
                super::release_id("0.5.149", absent),
                "oxy@0.5.149",
                "'{absent}' is not a commit; a release must not be minted from it"
            );
        }
    }
}
