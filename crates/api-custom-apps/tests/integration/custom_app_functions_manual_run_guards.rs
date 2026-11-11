//! Guards for the production wiring the `custom_app_functions_*` tests copy
//! instead of running. Source scans only, no database.
//!
//! Those tests drive the real handlers, driver entry point and executor, but
//! three pieces they cannot reach are private, `pub(crate)` or assembled inside
//! `create_web_application`, so they restate them:
//!
//! - **The executor registry.** Production registers `AppFunctionTaskExecutor`
//!   under `APP_FUNCTION_KIND` in `crates/server/src/custom_apps_seams.rs`, hands
//!   it to `oxy-app` as `SurfaceSeams::tasks`, `build_custom_task_registry`
//!   registers every seam it is given, and `drive_pending` hands that registry to
//!   `recover_pending_global_runs`. The manual-run test registers its own. Delete
//!   the production registration and every Run now, scheduled function and
//!   webhook job stays queued forever, while the end-to-end test stays green.
//! - **The admin layers and routes.** `admin::router()` is `pub(crate)`, so the
//!   manual-run test mounts the two handlers behind a hand-built copy of its
//!   guards.
//! - **The serve mount.** `custom_app_functions_fixture::serve_router` restates
//!   the `/customer-apps/{*path}` route that `crates/server/src/custom_apps_seams.rs`
//!   builds and `serve.rs` merges, with only the query-executor extension. Drop that extension in production and every
//!   function 500s while every end-to-end test stays green; add a layer a
//!   function needs and they never see it.
//!
//! Each assertion states one shape on both sides — production and the copy — so
//! a change to either fails here. They see only what they name: a layer added
//! somewhere else, such as the protected tree's outer middleware in
//! `router/protected.rs` or a new sibling nest, is invisible to them.
//!
//! Matching is on source with whole-line comments dropped, all whitespace
//! removed and trailing commas before `)` folded away: reformatting passes, and a
//! comment naming a symbol cannot satisfy an assertion (the failure
//! `tests/authz/app_scope_boundary.rs` documents).

use crate::common::read_repo_file;
use crate::custom_app_functions_manual_run::{
    CALLS_ROUTE, ERRORS_ROUTE, FUNCTION_INVOCATIONS_ROUTE, FUNCTIONS_ROUTE, HELD_ROUTE,
    INVOCATIONS_ROUTE, LOGS_ROUTE, RUN_DETAIL_ROUTE, RUNS_ROUTE, SLUG_INVOCATIONS_ROUTE,
};

/// `rel` (from the repo root) with whole-line comments dropped, every whitespace
/// character removed, and `,)` folded to `)`.
fn code(rel: &str) -> String {
    let squashed: String = read_repo_file(rel)
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .flat_map(str::chars)
        .filter(|c| !c.is_whitespace())
        .collect();
    squashed.replace(",)", ")")
}

/// The body of the first `fn <name>(` in squashed source, by brace matching.
fn fn_body<'a>(src: &'a str, name: &str) -> &'a str {
    let sig = format!("fn{name}(");
    let at = src
        .find(&sig)
        .unwrap_or_else(|| panic!("`fn {name}` not found"));
    let open = at + src[at..].find('{').expect("a function body");
    let mut depth = 0;
    for (i, c) in src[open..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' if depth == 1 => return &src[open..=open + i],
            '}' => depth -= 1,
            _ => {}
        }
    }
    panic!("unbalanced braces in `fn {name}`")
}

/// `src` from `start` up to (not including) the first `end` after it.
fn between<'a>(src: &'a str, start: &str, end: &str) -> &'a str {
    let from = src
        .find(start)
        .unwrap_or_else(|| panic!("`{start}` not found"));
    let len = src[from..]
        .find(end)
        .unwrap_or_else(|| panic!("no `{end}` after `{start}`"));
    &src[from..from + len]
}

#[test]
fn the_production_registry_registers_the_app_function_executor_and_the_driver_uses_it() {
    let server = code("crates/server/src/custom_apps_seams.rs");
    assert!(
        fn_body(&server, "tasks").contains(
            "TaskSeam{kind:APP_FUNCTION_KIND,build:|deps|{Arc::new(AppFunctionTaskExecutor{"
        ),
        "`custom_apps_seams::tasks` no longer hands in `AppFunctionTaskExecutor` under \
         `APP_FUNCTION_KIND`. Every queued `app_function` task (Run now, scheduled functions, \
         webhook jobs) would stay queued, and `custom_app_functions_manual_run` would not \
         notice: it registers its own executor."
    );
    let main = code("crates/server/src/main.rs");
    assert!(
        fn_body(&main, "surface_seams").contains("tasks:custom_apps_seams::tasks()"),
        "`surface_seams` no longer fills `SurfaceSeams::tasks` from `custom_apps_seams::tasks()`"
    );

    let src = code("crates/app/src/server/router/recovery.rs");
    assert!(
        fn_body(&src, "build_custom_task_registry")
            .contains("forseamintasks{reg.register(seam.kind,(seam.build)(&deps));}"),
        "`build_custom_task_registry` no longer registers every executor `SurfaceSeams::tasks` \
         hands in, under its kind"
    );
    let driver = fn_body(&src, "drive_pending");
    assert!(
        // The arguments after `custom_tasks` are the registry's own business
        // (today the driver's schema cache, for the queued-ask executor).
        driver.contains(
            "letcustom_executors=Some(build_custom_task_registry(db,preagg,custom_tasks,"
        ),
        "`drive_pending` no longer builds its executors with `build_custom_task_registry`, \
         handing it the seam tasks"
    );
    assert!(
        between(driver, "recover_pending_global_runs(", ".await").contains(",custom_executors,"),
        "`drive_pending` no longer passes its registry to `recover_pending_global_runs`"
    );
}

/// The manual-run and publish-token tests call `admin::apps::guarded` themselves
/// and register the handlers at the paths below, so what they must still see in
/// production is: the stack's layers, in order, the mount wearing it, and the
/// handlers registered at those paths.
#[test]
fn the_admin_app_mount_wears_the_one_stack_the_manual_run_tests_call() {
    let apps = code("crates/api-custom-apps/src/admin/apps/mod.rs");
    assert!(
        fn_body(&apps, "guarded<S>").contains(
            "routes.layer(middleware::from_fn(block_admin_while_acting)).layer(middleware::from_fn(app_scope_guard::enforce_app_scope)).layer(middleware::from_fn(platform_cap_guard::require(Action::PlatformApps))).layer(middleware::from_fn(oxy_owner_or_app_admin_guard_middleware))"
        ),
        "`admin::apps::guarded` no longer layers `block_admin_while_acting`, \
         `enforce_app_scope`, `PlatformApps` and the owner-or-app-admin guard, in that order"
    );
    for route in [
        format!(".route(\"{RUNS_ROUTE}\",post(handlers::run_function_job))"),
        format!(".route(\"{RUN_DETAIL_ROUTE}\",get(functions::get_function_run))"),
        format!(".route(\"{FUNCTIONS_ROUTE}\",get(functions::list_functions))"),
        format!(".route(\"{FUNCTION_INVOCATIONS_ROUTE}\",get(functions::list_invocations))"),
        format!(".route(\"{INVOCATIONS_ROUTE}\",get(invocations::list_app_invocations))"),
        format!(".route(\"{HELD_ROUTE}\",get(held_writes::get_held_writes))"),
        format!(".route(\"{CALLS_ROUTE}\",get(invocation_calls::get_invocation_calls))"),
    ] {
        assert!(
            between(&apps, "fncore_routes(", "fnconsole_routes(").contains(&route),
            "`admin::apps::core_routes()` no longer registers `{route}`; the manual-run tests \
             exercise a path production does not serve"
        );
    }

    // The `/customer-apps` nest is `oxy-api-custom-apps`'s `routes()`, which
    // `oxy-server` merges at the protected tree's root; `oxy-app`'s own global
    // router mounts neither it nor `/admin/apps`.
    let lib = code("crates/api-custom-apps/src/lib.rs");
    assert!(
        fn_body(&lib, "routes")
            .contains(".nest(\"/customer-apps\",admin::apps::guarded(admin::apps::router()"),
        "the `/customer-apps` nest of `oxy_api_custom_apps::routes()` is no longer \
         `admin::apps::guarded(admin::apps::router()…)`"
    );
    let main = code("crates/server/src/main.rs");
    // Merged bare, or with the seams the composition root layers onto it (the
    // semantic pin port): either way `routes()` is the merged router.
    let seam_body = fn_body(&main, "api_seam_routes");
    let merges_routes = seam_body.contains(".merge(oxy_api_custom_apps::routes())")
        || seam_body.contains(".merge(oxy_api_custom_apps::routes().layer(");
    assert!(
        merges_routes,
        "`api_seam_routes` no longer merges `oxy_api_custom_apps::routes()` (bare, or with \
         `.layer(…)` on it), the only mount of the admin app handlers"
    );
    let global = code("crates/app/src/server/router/global.rs");
    assert!(
        !global.contains("\"/admin/apps\"") && !global.contains("\"/customer-apps\""),
        "`router/global.rs` mounts `/admin/apps` or `/customer-apps` again: \
         `oxy_api_custom_apps::routes()` is the only mount of the admin app handlers"
    );
    let admin = code("crates/app/src/server/api/admin/mod.rs");
    assert!(
        !fn_body(&admin, "router").contains("apps::router()"),
        "`admin::router()` registers the app handlers again; they are mounted once, in this crate's `routes()`"
    );
}

#[test]
fn the_serve_mount_the_functions_fixture_copies_still_matches_production() {
    let server = code("crates/server/src/custom_apps_seams.rs");
    let executor = ".layer(axum::Extension(std::sync::Arc::new(DataPlaneQueryExecutor)asstd::sync::Arc<dynFunctionQueryExecutor>))";
    let production = format!(
        ".route(\"/customer-apps/{{*path}}\",any(custom_apps_serve::serve_dispatch).layer(ServiceBuilder::new().layer(axum::extract::DefaultBodyLimit::max(32*1024*1024)).layer(custom_app_compression()).layer(axum::middleware::from_fn(oxy_telemetry::http_trace::record_error_body)){executor}.layer(axum::Extension(preagg_ctx))))"
    );
    assert!(
        fn_body(&server, "outer").contains(&production),
        "the `/customer-apps/{{*path}}` mount in `custom_apps_seams::outer` no longer stacks \
         exactly the 32 MiB body limit, compression, `record_error_body`, the \
         `DataPlaneQueryExecutor` extension and the preagg extension onto `serve_dispatch`. The \
         functions fixture's `serve_router` copies the executor extension and leaves the others \
         off by name; re-check that each omission still holds for what the end-to-end tests \
         send, then update both sides."
    );
    let main = code("crates/server/src/main.rs");
    assert!(
        fn_body(&main, "surface_seams").contains("outer:Some(custom_apps_seams::outer)"),
        "`surface_seams` no longer hands `custom_apps_seams::outer` to `SurfaceSeams::outer`"
    );
    // `serve` mounts whatever it is handed, and nothing of its own beside it.
    let serve = code("crates/server/src/commands/serve.rs");
    assert!(
        serve.contains(".merge(outer(preagg_ctx))"),
        "`serve.rs` no longer merges the outer mount `SurfaceSeams::outer` hands in"
    );
    assert!(
        !serve.contains("\"/customer-apps/{*path}\""),
        "`serve.rs` mounts `/customer-apps/{{*path}}` itself again; the route and its layers \
         belong to `custom_apps_seams::outer`, which the assertion above pins"
    );

    let copy = code("crates/api-custom-apps/tests/integration/custom_app_functions_fixture.rs");
    assert!(
        fn_body(&copy, "serve_router").contains(
            ".route(\"/customer-apps/{*path}\",any(custom_apps_serve::serve_dispatch).layer(axum::Extension(Arc::new(DataPlaneQueryExecutor)asArc<dynFunctionQueryExecutor>)))"
        ),
        "the functions fixture's `serve_router` no longer mounts `serve_dispatch` on \
         `/customer-apps/{{*path}}` with the `DataPlaneQueryExecutor` extension production \
         carries; every function it calls would 500 for a reason production does not have"
    );
}

/// `log_reads` mounts the log and client-error handlers on a router of its
/// own, because the production mounts are inside `public::router`. The routes
/// do their own authentication, so the copy needs no layer — only the same
/// handlers on the same paths.
#[test]
fn the_log_route_the_readback_test_copies_still_matches_production() {
    let public = code("crates/api-custom-apps/src/lib.rs");
    assert!(
        public.contains(&format!(".route(\"{LOGS_ROUTE}\",get(logs::get_logs))")),
        "`oxy-api-custom-apps` no longer mounts `logs::get_logs` on `{LOGS_ROUTE}`; \
         the read-back test exercises a path production does not serve"
    );
    assert!(
        public.contains(&format!(".route(\"{ERRORS_ROUTE}\",get(logs::get_errors))")),
        "`oxy-api-custom-apps` no longer mounts `logs::get_errors` on `{ERRORS_ROUTE}`; \
         the read-back test exercises a path production does not serve"
    );
    let copy = code(
        "crates/api-custom-apps/tests/integration/custom_app_functions_manual_run/log_reads.rs",
    );
    assert!(
        copy.contains(".route(LOGS_ROUTE,get(custom_apps_logs::get_logs))")
            && copy.contains(".route(ERRORS_ROUTE,get(custom_apps_logs::get_errors))"),
        "the read-back test no longer mounts the production log and client-error handlers"
    );
}

/// `slug_invocation_reads` mounts the slug invocation read on a router of its
/// own, for the reason the log read does: production mounts it inside
/// `public_routes`, and it authenticates in its handler.
#[test]
fn the_slug_invocation_route_the_test_copies_still_matches_production() {
    let public = code("crates/api-custom-apps/src/lib.rs");
    assert!(
        public.contains(&format!(
            ".route(\"{SLUG_INVOCATIONS_ROUTE}\",get(logs::invocations::get_invocations))"
        )),
        "`oxy-api-custom-apps` no longer mounts `logs::invocations::get_invocations` on \
         `{SLUG_INVOCATIONS_ROUTE}`; the read test exercises a path production does not serve"
    );
    let copy = code(
        "crates/api-custom-apps/tests/integration/custom_app_functions_manual_run/slug_invocation_reads.rs",
    );
    assert!(
        copy.contains(".route(SLUG_INVOCATIONS_ROUTE,get(slug_invocations::get_invocations))"),
        "the read test no longer mounts the production slug invocation handler"
    );
}
