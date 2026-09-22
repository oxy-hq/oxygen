//! Guards for the production wiring the `custom_app_functions_*` tests copy
//! instead of running. Source scans only, no database.
//!
//! Those tests drive the real handlers, driver entry point and executor, but
//! three pieces they cannot reach are private, `pub(crate)` or assembled inside
//! `create_web_application`, so they restate them:
//!
//! - **The executor registry.** Production registers `AppFunctionTaskExecutor`
//!   under `APP_FUNCTION_KIND` in `router::recovery::build_custom_task_registry`,
//!   and `drive_pending` hands that registry to `recover_pending_global_runs`.
//!   The manual-run test registers its own. Delete the production registration
//!   and every Run now, scheduled function and webhook job stays queued forever,
//!   while the end-to-end test stays green.
//! - **The admin layers and routes.** `admin::router()` is `pub(crate)`, so the
//!   manual-run test mounts the two handlers behind a hand-built copy of its
//!   guards.
//! - **The serve mount.** `custom_app_functions_fixture::serve_router` restates
//!   the `/customer-apps/{*path}` route from `cli/commands/serve.rs` with only
//!   the query-executor extension. Drop that extension in production and every
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
use crate::custom_app_functions_manual_run::{RUN_DETAIL_ROUTE, RUNS_ROUTE};

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
    let src = code("crates/app/src/server/router/recovery.rs");
    assert!(
        fn_body(&src, "build_custom_task_registry")
            .contains("reg.register(APP_FUNCTION_KIND,Arc::new(AppFunctionTaskExecutor{"),
        "`build_custom_task_registry` no longer registers `AppFunctionTaskExecutor` under \
         `APP_FUNCTION_KIND`. Every queued `app_function` task (Run now, scheduled functions, \
         webhook jobs) would stay queued, and `custom_app_functions_manual_run` would not \
         notice: it registers its own executor."
    );
    let driver = fn_body(&src, "drive_pending");
    assert!(
        driver.contains("letcustom_executors=Some(build_custom_task_registry(db,preagg));"),
        "`drive_pending` no longer builds its executors with `build_custom_task_registry`"
    );
    assert!(
        between(driver, "recover_pending_global_runs(", ".await").contains(",custom_executors,"),
        "`drive_pending` no longer passes its registry to `recover_pending_global_runs`"
    );
}

#[test]
fn the_admin_stack_the_manual_run_test_copies_still_matches_production() {
    let admin = code("crates/app/src/server/api/admin/mod.rs");
    let router = fn_body(&admin, "router");
    assert!(
        router.contains(
            ".merge(apps::router().route_layer(middleware::from_fn(app_scope_guard::enforce_app_scope)).route_layer(cap(Action::PlatformApps)))"
        ),
        "the apps merge in `admin::router()` no longer applies exactly `enforce_app_scope`, \
         then `cap(Action::PlatformApps)`"
    );
    assert!(
        between(router, "letstaff_surface=", ";")
            .ends_with(".route_layer(middleware::from_fn(assume::block_admin_while_acting))"),
        "`block_admin_while_acting` is no longer the last layer on the staff surface"
    );

    let global = code("crates/app/src/server/router/global.rs");
    assert!(
        global.contains(
            ".nest_declared(\"/admin\",admin::router().layer(middleware::from_fn(oxy_owner_or_app_admin_guard::oxy_owner_or_app_admin_guard_middleware)),admin::router_roles())"
        ),
        "the `/admin` nest no longer applies exactly `oxy_owner_or_app_admin_guard_middleware`"
    );

    let apps = code("crates/app/src/server/api/admin/apps/mod.rs");
    for route in [
        format!(".route(\"{RUNS_ROUTE}\",post(handlers::run_function_job))"),
        format!(".route(\"{RUN_DETAIL_ROUTE}\",get(functions::get_function_run))"),
    ] {
        assert!(
            apps.contains(&route),
            "`admin::apps::router()` no longer mounts `{route}`; the manual-run test \
             exercises a path production does not serve"
        );
    }

    let copy = code("crates/app/tests/custom_apps/custom_app_functions_manual_run.rs");
    assert!(
        copy.contains(
            ".route_layer(middleware::from_fn(app_scope_guard::enforce_app_scope)).route_layer(middleware::from_fn(platform_cap_guard::require(Action::PlatformApps))).route_layer(middleware::from_fn(block_admin_while_acting));"
        ) && copy.contains(
            ".nest(\"/api/admin\",apps.layer(middleware::from_fn(oxy_owner_or_app_admin_guard_middleware)))"
        ),
        "the manual-run test's admin stack no longer matches the production shape asserted above"
    );
}

#[test]
fn the_serve_mount_the_functions_fixture_copies_still_matches_production() {
    let serve = code("crates/app/src/cli/commands/serve.rs");
    let executor = ".layer(axum::Extension(std::sync::Arc::new(DataPlaneQueryExecutor)asstd::sync::Arc<dynFunctionQueryExecutor>))";
    let production = format!(
        ".route(\"/customer-apps/{{*path}}\",any(custom_apps_serve::serve_dispatch).layer(ServiceBuilder::new().layer(axum::extract::DefaultBodyLimit::max(32*1024*1024)).layer(CompressionLayer::new()).layer(axum::middleware::from_fn(oxy_telemetry::http_trace::record_error_body)){executor}.layer(axum::Extension(preagg_ctx))))"
    );
    assert!(
        serve.contains(&production),
        "the `/customer-apps/{{*path}}` mount in `serve.rs` no longer stacks exactly the 32 MiB \
         body limit, compression, `record_error_body`, the `DataPlaneQueryExecutor` extension \
         and the preagg extension onto `serve_dispatch`. The functions fixture's `serve_router` \
         copies the executor extension and leaves the others off by name; re-check that each \
         omission still holds for what the end-to-end tests send, then update both sides."
    );

    let copy = code("crates/app/tests/custom_apps/custom_app_functions_fixture.rs");
    assert!(
        fn_body(&copy, "serve_router").contains(
            ".route(\"/customer-apps/{*path}\",any(custom_apps_serve::serve_dispatch).layer(axum::Extension(Arc::new(DataPlaneQueryExecutor)asArc<dynFunctionQueryExecutor>)))"
        ),
        "the functions fixture's `serve_router` no longer mounts `serve_dispatch` on \
         `/customer-apps/{{*path}}` with the `DataPlaneQueryExecutor` extension production \
         carries; every function it calls would 500 for a reason production does not have"
    );
}
