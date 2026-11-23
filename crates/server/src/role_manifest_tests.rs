use super::*;

/// Every guard below asserts policy about routes, and the routes now carry
/// their own roles — so classify against the real router rather than the
/// tables it is replacing. Shadows the module-level `classify`.
fn classify(method: &str, path: &str) -> RouteRole {
    install_route_declarations_for_tests();
    super::classify(method, path)
}

#[test]
fn only_serve_offloads_workers_single_all_in_one_drains_its_own_queue() {
    // The single-instance invariant: a plain `OXY_ROLE=all` node (and ide /
    // worker) runs the durable fleet + global driver in-process, so
    // scheduled + manual jobs execute without a second node. Only the
    // stateless `serve` replica offloads to the worker fleet. Pure over
    // `Role`, so it can assert every role without touching the process-role
    // OnceLock. Guards the DX regression where a lone node silently queues
    // jobs forever with nothing draining them.
    assert!(
        role_runs_inprocess_workers(Role::All),
        "a single OXY_ROLE=all instance must drain its own queue"
    );
    assert!(role_runs_inprocess_workers(Role::Ide));
    assert!(role_runs_inprocess_workers(Role::Worker));
    assert!(
        !role_runs_inprocess_workers(Role::Serve),
        "serve is a pure reader — it offloads to the worker fleet"
    );
}

// `every_database_route_is_fleet_ok` moved to `oxy-api-workspace-config`
// with the `/databases` routes (2026-11-15), as
// `every_database_and_secret_route_classifies_fleet_ok`: here it would pass on
// the FleetOk default, since this harness cannot see that crate's
// declarations.

#[test]
fn draining_the_queue_is_not_owning_a_working_copy() {
    // Worker is the role where these two diverge, and conflating them is what
    // let a diskless worker claim a Compile task and walk a tree that is not
    // there. `role_runs_inprocess_workers` is about who executes queued work;
    // `role_owns_workspace_files` is about who has files to execute it against.
    assert!(role_runs_inprocess_workers(Role::Worker));
    assert!(
        !role_owns_workspace_files(Role::Worker),
        "a worker drains the queue but holds no checkout"
    );

    assert!(role_owns_workspace_files(Role::Ide));
    assert!(role_owns_workspace_files(Role::All));
    assert!(!role_owns_workspace_files(Role::Serve));
}

#[test]
fn pattern_matches_concrete_uri() {
    // Single-segment params
    assert!(pattern_matches(
        "/api/{workspace_id}/compile",
        "/api/abc-123/compile"
    ));
    // Rest wildcards consume one OR more trailing segments
    assert!(pattern_matches(
        "/api/{workspace_id}/git/{*rest}",
        "/api/abc/git/status"
    ));
    assert!(pattern_matches(
        "/api/{workspace_id}/git/{*rest}",
        "/api/abc/git/commit/sha"
    ));
    // Non-match: wrong prefix
    assert!(!pattern_matches(
        "/api/{workspace_id}/compile",
        "/api/abc-123/threads"
    ));
    // Non-match: missing segment
    assert!(!pattern_matches(
        "/api/{workspace_id}/compile",
        "/api/abc-123"
    ));
    // Non-match: rest wildcard requires at least one segment
    assert!(!pattern_matches(
        "/api/{workspace_id}/git/{*rest}",
        "/api/abc/git"
    ));
}

#[test]
fn ide_routes_classify_against_live_uri() {
    // The real shape the middleware sees: `/api` prefix + nested workspace.
    // The IDE's file, git and compile routes are gone (factory-retirement.md,
    // phase 5), and since 2026-11-14 the airway backfill drive is a queued
    // task: nothing under a workspace is IdeOnly.
    assert_eq!(
        classify("POST", "/api/d9830be4-c6a4/agentic-airway/chunked-backfill"),
        RouteRole::FleetOk
    );
    // The anomaly scan (`/semantic/anomalies/scan`, which reads `.monitor.yml`
    // from the compiled revision) left with the inbox for
    // `oxy-api-metric-monitoring` (2026-11-15). Classifying it here would answer
    // the FleetOk default for an undeclared path, which proves nothing; that
    // crate's `the_installed_declarations_classify_fleet_ok` holds it.
    // The `/onboarding/*` routes moved to the `oxy-api-onboarding` sibling
    // crate, which declares them; `oxy-server`'s
    // `tests/integration/route_roles/` asserts them. They cannot be checked here: `oxy-app` does not
    // depend on that crate, so its declarations are absent from the set this
    // test installs and `classify` would answer with the FleetOk default —
    // a failure that says nothing about the product.
    // `/orgs/{id}/onboarding/github` moved with the rest of the onboarding
    // surface — asserted in `crates/server/tests/integration/route_roles/tenancy_onboarding.rs`.
    // The workspace's metadata (`GET /meta`), deletion and the sibling LIST
    // (`/orgs/{org_id}/workspaces`, DELETE and GET) left for
    // `oxy-api-workspaces` (2026-11-16), whose
    // `persisted_data_routes_are_declared_fleet_ok` pins all three against its
    // own declarations; here they would pass on the default.
    // `/world-model/events` used to belong in this list and no longer does.
    // Its publishers append to `world_model_events` and every pod tails that
    // table onto its own bus, so the feed is no longer process-local and any
    // replica can serve a subscriber.
    assert_eq!(
        classify("GET", "/api/d9830be4-c6a4/world-model/events"),
        RouteRole::FleetOk
    );
}

/// Paths nothing mounts under the run mounts take the FleetOk default now:
/// the IdeOnly catch-alls that used to catch them are gone, because nothing in
/// those routers reaches a working copy. The Builder (its start, its answers
/// and undoing its edits) was removed with the IDE, and a Data App's run with
/// Data Apps (#3835).
#[test]
fn unmounted_run_paths_take_the_fleet_default() {
    let ws = "d9830be4-c6a4";
    for (method, path) in [
        ("POST", format!("/api/{ws}/agentic-workflows/run")),
        ("POST", format!("/api/{ws}/agentic-airway/run")),
    ] {
        assert_eq!(
            classify(method, &path),
            RouteRole::FleetOk,
            "{method} {path}"
        );
    }
}

/// A chat and an automation are started, answered, cancelled and streamed by
/// any pod, and the automation files a customer opens are read from the
/// compiled revision. With the Factory scaled to zero these are what chat and
/// the automation page call.
#[test]
fn chat_and_automation_runs_are_fleet_ok() {
    let ws = "d9830be4-c6a4";
    for (method, path) in [
        ("POST", format!("/api/{ws}/analytics/runs")),
        ("GET", format!("/api/{ws}/analytics/runs/r-1/events")),
        ("POST", format!("/api/{ws}/analytics/runs/r-1/answer")),
        ("POST", format!("/api/{ws}/analytics/runs/r-1/cancel")),
        ("POST", format!("/api/{ws}/agentic-workflows/runs")),
        (
            "GET",
            format!("/api/{ws}/agentic-workflows/runs/r-1/events"),
        ),
        (
            "POST",
            format!("/api/{ws}/agentic-workflows/runs/r-1/cancel"),
        ),
        ("GET", format!("/api/{ws}/agentic-workflows/files")),
        ("GET", format!("/api/{ws}/agentic-workflows/files/cGF0aA")),
        ("POST", format!("/api/{ws}/agentic-automations/runs")),
        (
            "GET",
            format!("/api/{ws}/agentic-automations/runs/r-1/events"),
        ),
        (
            "POST",
            format!("/api/{ws}/agentic-automations/runs/r-1/cancel"),
        ),
        ("GET", format!("/api/{ws}/agentic-automations/files")),
        ("GET", format!("/api/{ws}/agentic-automations/files/cGF0aA")),
    ] {
        assert_eq!(
            classify(method, &path),
            RouteRole::FleetOk,
            "{method} {path} must be served by any replica"
        );
    }
}

/// Viewing a past conversation / run is a Postgres read and must NOT depend
/// on the ide singleton (HA). The run-history reads across all three agentic
/// surfaces are carved out to FleetOk even though they sit under the IdeOnly
/// `/analytics` `/agentic-workflows` `/agentic-airway` wildcards. (The run
/// routes beside them are FleetOk too now — see
/// `chat_and_automation_runs_are_fleet_ok` and
/// `airway_control_routes_are_fleet_ok` — except undoing a Builder edit.)
#[test]
fn agentic_run_history_reads_are_fleet_ok() {
    let ws = "d9830be4-c6a4";
    // Pure Postgres reads → FleetOk (serve from any replica).
    for path in [
        format!("/api/{ws}/analytics/threads/t-1/runs"), // list_runs_by_thread
        format!("/api/{ws}/analytics/threads/t-1/run"),  // get_run_by_thread
        format!("/api/{ws}/agentic-workflows/runs"),     // list_runs_for_workflow
        format!("/api/{ws}/agentic-workflows/runs/r-1"), // get_workflow_run
        format!("/api/{ws}/agentic-workflows/threads/t-1/run"), // latest_run_for_thread
        format!("/api/{ws}/agentic-airway/runs"),        // list_runs_for_pipeline
        // These two had a carve-out but no assertion — deleting their
        // declaration changed behaviour and nothing went red, which a
        // mutation test found while moving them into `agentic-http`.
        format!("/api/{ws}/agentic-airway/coverage"), // airway_coverage
        format!("/api/{ws}/agentic-airway/backfill-ranges"), // airway_backfill_ranges
    ] {
        assert_eq!(
            classify("GET", &path),
            RouteRole::FleetOk,
            "{path} is a Postgres run-history read — must serve from any replica"
        );
    }
}

/// The ops dashboard's run history, recovery counts and queue health are
/// SELECTs, and saving a chat's thinking mode is one UPDATE. They sat under
/// the `/analytics` IdeOnly wildcard, so with the ide down the page that shows
/// what the queue is doing could not load any of it.
#[test]
fn coordinator_postgres_reads_are_fleet_ok() {
    let ws = "d9830be4-c6a4";
    for (method, path) in [
        ("GET", format!("/api/{ws}/analytics/coordinator/runs")),
        ("GET", format!("/api/{ws}/analytics/coordinator/recovery")),
        ("GET", format!("/api/{ws}/analytics/coordinator/queue")),
        // Answered from the run rows, not this process's status map.
        (
            "GET",
            format!("/api/{ws}/analytics/coordinator/active-runs"),
        ),
        (
            "GET",
            format!("/api/{ws}/analytics/coordinator/runs/r-1/tree"),
        ),
        ("GET", format!("/api/{ws}/analytics/coordinator/live")),
        (
            "POST",
            format!("/api/{ws}/analytics/coordinator/runs/r-1/retry"),
        ),
        (
            "PATCH",
            format!("/api/{ws}/analytics/runs/r-1/thinking_mode"),
        ),
        // Asking to hear when a run ends: an UPDATE, and the notice it may send.
        ("POST", format!("/api/{ws}/analytics/runs/r-1/notify")),
        // The same router is mounted on the external API surface.
        (
            "GET",
            format!("/external/api/{ws}/analytics/coordinator/queue"),
        ),
    ] {
        assert_eq!(
            classify(method, &path),
            RouteRole::FleetOk,
            "{method} {path} reads or writes Postgres only — must serve from any replica"
        );
    }
}

/// No catch-all under `/analytics` any more: every mounted route has its own
/// FleetOk declaration, so an unmounted verb or path takes the default.
#[test]
fn undeclared_coordinator_paths_take_the_fleet_default() {
    let ws = "d9830be4-c6a4";
    for (method, path) in [
        // A declared path under a method it was not declared for.
        ("POST", format!("/api/{ws}/analytics/coordinator/runs")),
        ("DELETE", format!("/api/{ws}/analytics/coordinator/queue")),
        ("GET", format!("/api/{ws}/analytics/runs/r-1/thinking_mode")),
        ("GET", format!("/api/{ws}/analytics/runs/r-1/notify")),
        // Not routes today.
        ("GET", format!("/api/{ws}/analytics/coordinator/runs/r-1")),
        ("GET", format!("/api/{ws}/analytics/coordinator/workers")),
    ] {
        assert_eq!(
            classify(method, &path),
            RouteRole::FleetOk,
            "{method} {path}"
        );
    }
}

/// Starting, stopping and resetting an Airway pipeline must not need the
/// Factory. None of these handlers runs the pipeline or reads a working copy:
/// start and single-window backfill enqueue a `Global` task the worker fleet
/// drives, cancel writes a durable flag that fleet polls, the stream tails
/// `agentic_run_events`, and the resets resolve the spec through the compile
/// boundary and touch Postgres (plus the destination, over the network).
///
/// They sat under the `/agentic-airway` IdeOnly wildcard, so with the ide down
/// nobody could start or reset a pipeline by hand — a second, unnecessary way
/// for one pod to take ingestion with it.
#[test]
fn airway_control_routes_are_fleet_ok() {
    let ws = "d9830be4-c6a4";
    for (method, path) in [
        ("POST", format!("/api/{ws}/agentic-airway/runs")), // start
        ("POST", format!("/api/{ws}/agentic-airway/backfill")), // single window
        ("POST", format!("/api/{ws}/agentic-airway/runs/r-1/cancel")),
        ("GET", format!("/api/{ws}/agentic-airway/runs/r-1/events")),
        ("POST", format!("/api/{ws}/agentic-airway/reset-schema")),
        ("POST", format!("/api/{ws}/agentic-airway/reset-cursors")),
        ("GET", format!("/api/{ws}/agentic-airway/resource-cursors")),
    ] {
        assert_eq!(
            classify(method, &path),
            RouteRole::FleetOk,
            "{method} {path} needs no working copy and no in-process driver — \
             it must be served by any replica"
        );
    }
}

/// The chunked backfill pair queues its drive for the worker fleet
/// (`agentic_pipeline::backfill_task`), so no airway route stays on the ide.
#[test]
fn no_airway_route_stays_ide_only() {
    let ws = "d9830be4-c6a4";
    for path in ["chunked-backfill", "resume-backfill"] {
        assert_eq!(
            classify("POST", &format!("/api/{ws}/agentic-airway/{path}")),
            RouteRole::FleetOk,
            "POST {path} queues its drive"
        );
    }
    for (method, path) in [
        ("POST", format!("/api/{ws}/agentic-airway/run")),
        ("POST", format!("/api/{ws}/agentic-airway/runs/r-1/retry")),
        ("DELETE", format!("/api/{ws}/agentic-airway/runs")),
    ] {
        assert_eq!(
            classify(method, &path),
            RouteRole::FleetOk,
            "{method} {path}"
        );
    }
}

#[test]
fn workspace_health_is_fleet_ok() {
    assert_eq!(
        classify("GET", "/api/admin/workspace-health"),
        RouteRole::FleetOk
    );
}

#[test]
fn the_usage_report_is_fleet_ok() {
    // A stored row, a preference row and a mail send: nothing here reads a
    // working copy, so the report stays readable when the ide is down.
    for (method, path) in [
        ("GET", "/api/admin/usage-report"),
        ("GET", "/api/admin/usage-report/email-preference"),
        ("PUT", "/api/admin/usage-report/email-preference"),
        ("POST", "/api/admin/usage-report/send-to-me"),
        ("GET", "/api/admin/usage-report/recipients"),
        (
            "PUT",
            "/api/admin/usage-report/recipients/someone@example.com",
        ),
    ] {
        assert_eq!(
            classify(method, path),
            RouteRole::FleetOk,
            "{method} {path}"
        );
    }
}

#[test]
fn workspace_health_eval_is_fleet_ok() {
    // The on-demand eval handler is a pure Postgres enqueue: it seeds a
    // Global `health_eval_workspace` task and returns 202. The heavy work
    // (workspace-context build + reconcile.yml FS fallthrough) runs in the
    // fleet executor that drains the task, NOT in this handler — route class
    // doesn't govern task execution. So the POST is FS-free and must serve
    // FleetOk: pinning it IdeOnly would block an operator from triggering an
    // eval whenever the ide is down, undercutting the offload's whole point.
    let ws = "d9830be4-c6a4";
    assert_eq!(
        classify("POST", &format!("/api/admin/workspace-health/{ws}/eval")),
        RouteRole::FleetOk
    );
}

// The simulation and experiment routes left for `oxy-api-experiment`
// (2026-11-08), and their declaration tests went with them: this crate cannot
// depend on that one, so a test here can no longer see its mounts — an
// unmounted path classifies FleetOk by default, which is the pass that means
// nothing. `crates/server/tests/integration/route_roles/experiment.rs` installs
// the surface's declarations and classifies every route (write-back apply
// IdeOnly, the rest FleetOk); `every_route_is_mounted` in that crate's
// `lib.rs` holds the "declared by a mount" half; and
// `tests/routing/undeclared_mounts_stay_diskless.rs` scans the crate for
// working-copy access, which the `route_fleet` type gate used to refuse.

// `workspace_metric_tree_routes_are_fleet_ok` left with the metric tree for
// `oxy-api-metric-monitoring` (2026-11-15): this crate cannot see that one's
// declarations, so the same check here would pass on the FleetOk default. The
// outage behind it (oxy-hq/oxygen#878: every call 500'd on the serve fleet
// when the handlers read `semantics_scan_path()`) is now held by that crate's
// `every_route_is_declared_fleet_ok_exactly_once` and
// `the_installed_declarations_classify_fleet_ok`, and its
// `handlers_never_scan_the_working_copy`.

#[test]
fn persisted_data_reads_are_fleet_ok() {
    let ws = "d9830be4-c6a4";
    // The neighbours that must NOT be swept along: same prefixes, but pure
    // reads of persisted data, so pinning them to the singleton would make
    // *viewing* need the ide — the HA bug the split exists to prevent.
    // `GET /threads` left for `oxy-api-threads` (2026-11-16), whose
    // `persisted_data_reads_are_declared_fleet_ok` pins it against its own
    // declarations; here it would pass on the default.
    for (method, path) in [
        // Reads `world_model_events` from Postgres — it sits under a prefix
        // full of ide-pinned streams, which is exactly why it is named here.
        ("GET", format!("/api/{ws}/world-model/events")),
    ] {
        assert_eq!(
            classify(method, &path),
            RouteRole::FleetOk,
            "{method} {path} is a persisted-data read and must serve from any replica"
        );
    }
}

#[test]
fn unknown_routes_default_to_fleet_ok() {
    assert_eq!(
        classify("GET", "/api/d9830be4-c6a4/threads"),
        RouteRole::FleetOk
    );
    assert_eq!(classify("POST", "/api/analytics/runs"), RouteRole::FleetOk);
    assert_eq!(classify("GET", "/health"), RouteRole::FleetOk);
    assert_eq!(classify("GET", "/healthz"), RouteRole::FleetOk);
    // The run streams under /analytics and /agentic-workflows are declared
    // FleetOk (phase 3d): a stream polls the event log when another process
    // drives. An unknown route under those mounts takes the default too.
    assert_eq!(
        classify("GET", "/api/d9830be4-c6a4/analytics/runs/abc/events"),
        RouteRole::FleetOk
    );
    assert_eq!(
        classify(
            "GET",
            "/api/d9830be4-c6a4/agentic-workflows/runs/abc/events"
        ),
        RouteRole::FleetOk
    );
    assert_eq!(
        classify("GET", "/api/d9830be4-c6a4/analytics/not-a-route"),
        RouteRole::FleetOk
    );
    // Every `/world-model/*` read is FleetOk. They are all
    // `oxy-api-world-model`'s now (`/world-model/cameras` and the weather since
    // 2026-11-15), declared there and pinned by its
    // `the_camera_and_weather_routes_are_declared_fleet_ok`: this harness cannot
    // see that crate's declarations, so a pin here would pass on the default.
    // `/semantic/world-model*` and `/semantic/cohort` are `oxy-api-world-model`'s
    // now, declared FleetOk there and pinned by its
    // `the_graph_and_cohort_routes_are_declared_fleet_ok`: this harness cannot
    // see that crate's declarations, so a pin here would pass on the default.
    // Customer-apps batch mutations touch only Postgres + the S3 build
    // store (never the workspace FS), so — like their per-app siblings —
    // they classify FleetOk by default and serve from any replica.
    assert_eq!(
        classify("POST", "/api/customer-apps/batch/publish"),
        RouteRole::FleetOk
    );
    assert_eq!(
        classify("POST", "/api/customer-apps/batch/promote-latest"),
        RouteRole::FleetOk
    );
    assert_eq!(
        classify("POST", "/api/customer-apps/batch/unpublish"),
        RouteRole::FleetOk
    );
    assert_eq!(
        classify("POST", "/api/customer-apps/batch/delete"),
        RouteRole::FleetOk
    );
}

#[test]
fn org_subdomain_routes_are_fleet_ok() {
    // Both org-subdomain surfaces are Postgres-only (read workspace→org,
    // upsert the `org_subdomains` row) — no workspace FS — so they serve
    // from any replica, like `oxy-access`. See
    // `internal-docs/org-subdomain-infra.md`.
    let id = "d9830be4-c6a4";
    // The customer's read-only status (`GET /api/{id}/org-subdomain`) is
    // `oxy-api-tenancy`'s since 2026-11-16, and so is its pin
    // (`the_leaves_classify_fleet_ok_against_their_declarations`), asked of
    // that crate's declarations: here it would pass on the FleetOk default.
    // Oxy-staff control (admin surface).
    for method in ["GET", "PUT"] {
        assert_eq!(
            classify(method, &format!("/api/admin/orgs/{id}/subdomain")),
            RouteRole::FleetOk,
            "{method} admin org-subdomain must be FleetOk (Postgres-only)"
        );
    }
}

// `admin_create_org_reaches_the_ide_…` moved to `oxy-api-tenancy` with the org
// section (now `crates/server/tests/integration/route_roles/tenancy_admin.rs`), whose
// declaration this harness can no longer see.

#[test]
fn ide_only_accepted_by_ide_and_all_only() {
    let r = RouteRole::IdeOnly;
    assert!(r.accepted_by(Role::Ide));
    assert!(r.accepted_by(Role::All));
    assert!(!r.accepted_by(Role::Serve));
    assert!(!r.accepted_by(Role::Worker));
}

#[test]
fn fleet_ok_accepted_by_every_role_including_worker() {
    let r = RouteRole::FleetOk;
    assert!(r.accepted_by(Role::Ide));
    assert!(r.accepted_by(Role::Serve));
    assert!(r.accepted_by(Role::Worker));
    assert!(r.accepted_by(Role::All));
}

// The next two tests set OXY_ROLE + init the process-role OnceLock; they
// rely on nextest's per-test process isolation (CLAUDE.md mandates nextest),
// the same pattern the role_middleware/types tests use.

#[test]
fn fs_write_guard_refuses_on_serve_role() {
    // SAFETY: nextest isolates this test in its own single-threaded process.
    unsafe { std::env::set_var("OXY_ROLE", "serve") };
    init_process_role_from_env();
    assert!(
        !process_is_fs_writable(),
        "serve replica owns no filesystem"
    );
    assert!(
        ensure_fs_writable("test write").is_err(),
        "serve replica must refuse a workspace FS write (super_read_only)"
    );
}

#[test]
fn fs_write_guard_refuses_on_worker_role() {
    // SAFETY: nextest isolates this test in its own single-threaded process.
    // A worker owns no working copy either; it used to pass (`role != Serve`).
    unsafe { std::env::set_var("OXY_ROLE", "worker") };
    init_process_role_from_env();
    assert!(!process_is_fs_writable(), "a worker owns no working copy");
    assert!(
        ensure_fs_writable("test write").is_err(),
        "a worker must refuse a workspace FS write"
    );
}

#[test]
fn fs_write_guard_allows_fs_owning_roles() {
    // SAFETY: nextest isolates this test in its own single-threaded process.
    // `all` is the default; assert the guard is a no-op for an FS-owning role.
    unsafe { std::env::set_var("OXY_ROLE", "ide") };
    init_process_role_from_env();
    assert!(process_is_fs_writable(), "ide owns the working copy");
    assert!(
        ensure_fs_writable("test write").is_ok(),
        "an FS-owning role must permit workspace FS writes"
    );
}

#[test]
fn a_declaration_names_its_verb() {
    // A declaration for POST does not sweep another verb on the same path
    // along. No live route is IdeOnly any more, so the list is the test's own.
    let declared = vec![(
        "POST",
        "/api/{workspace_id}/ide-probe".to_string(),
        RouteRole::IdeOnly,
    )];
    assert_eq!(
        declared_role_in(&declared, "POST", "/api/abc/ide-probe"),
        Some(RouteRole::IdeOnly)
    );
    assert_eq!(
        declared_role_in(&declared, "GET", "/api/abc/ide-probe"),
        None
    );
}

// `every_integration_sub_route_is_classified` (the per-sub-route guard for
// `build_integration_routes`) moved to `oxy-api-workspace-config` on
// 2026-11-21 with the builder's last two routes, the OAuth connects: its ack
// rows are in that crate's
// `every_database_secret_and_integration_route_classifies_fleet_ok`, checked
// against the declarations it installs, and its shared body
// (`assert_sub_routes_classified`, with `route_paths_in_fn` and `concretize`)
// had no other caller and went with it.

/// `router/public.rs` mounts through the declaring door, not the silent one.
///
/// This file used to carry 51 bare `.route(` calls. Nothing was wrong with the
/// roles — every one of them is genuinely FleetOk — but nothing SAID so, and an
/// undeclared route is indistinguishable from a route nobody thought about. It
/// also meant the type gate did not apply: `.route()` accepts a
/// `MethodRouter<S>` for any state, so a handler asking for
/// `WorkspaceManagerWorkingCopy` would have compiled and shipped as a route a
/// stateless replica cannot serve. `route_fleet` takes `MethodRouter<FleetState>`
/// and rejects it — measured, by adding such a handler and watching
/// `cargo check` fail with E0308.
///
/// What this actually guards is narrower than it first looks, and the narrower
/// claim is the true one. `RoleRouter` has no `.route()` method at all, so a
/// single bare mount does not fail this test — it fails to COMPILE, which is
/// better. Measured: putting `.route("/ready", ..)` back gives E0308-adjacent
/// breakage at `cargo check`, not a red test.
///
/// So the scan exists for the reversion the compiler would accept: someone
/// changing `build_public_routes` back to a plain `Router<AppState>`, at which
/// point every `.route(` compiles again and all 51 declarations vanish at once.
/// The count assertion is the half that earns its place — it is what notices
/// the file going quiet.
#[test]
fn the_public_router_declares_every_route_it_mounts() {
    let src = include_str!("router/public.rs");
    // `.expect`, not `unwrap_or(0)`: falling back to 0 widens the scan to the
    // whole file, and since the count is unchanged both assertions below stay
    // green — so the ONE failure this test's message claims to report ("the
    // parser broke, or this file moved") was the one it could not.
    let start = src
        .find("pub(super) fn build_public_routes")
        .expect("build_public_routes not found in router/public.rs — it moved or was renamed");
    let body = &src[start..];

    let bare: Vec<&str> = body
        .lines()
        .filter(|l| l.trim_start().starts_with(".route("))
        .collect();
    assert!(
        bare.is_empty(),
        "these mounts in router/public.rs use the undeclaring door:\n  {}\n\n\
         Use `.route_fleet(..)` — or `.route_ide(..)` if the handler genuinely \
         needs the working copy, in which case the compiler will tell you: a \
         working-copy extractor types the handler as `IdeState` and will not \
         build through the fleet door.",
        bare.join("\n  ")
    );

    let declared = body.matches(".route_fleet(").count() + body.matches(".route_ide(").count();
    // The file declares 0 (2026-11-21: the two OAuth callbacks, its last
    // routes, left for `oxy-api-workspace-config`'s `public_routes`, which
    // declares them on the public seam). A floor of 0 would assert nothing
    // (and `>= 0` on a `usize` is a clippy error), so the count is pinned
    // exactly instead: the bare-`.route(` half above still guards the
    // reversion, and a route mounted here again has to raise this number on
    // purpose. Floor `>= 2` until then. The file declared 3 (read off this assertion on 2026-11-16, after the
    // four health probes left for `oxy-server`'s `core_routes`; floor `>= 7` ->
    // `>= 2`. 7 until then: the GitHub and Toast webhook receivers had left
    // on 2026-11-15 without this note following; 9 before that, 20 until
    // sign-in's ten `/auth/*` routes and
    // `GET /user` left for `oxy-api-identity`, 2026-11-15; 29 until the metric tree's nine customer-app
    // twins left for `oxy-api-custom-apps`, 2026-11-14; 32 until the automation run's start, poll and
    // cancel left for `oxy-api-custom-apps`, 2026-11-14; 33 until the agent-run
    // stream left, 2026-11-14; 35 until the agent ask's start and
    // cancel left, 2026-11-14; 37 until the custom-app data plane's
    // `query` and `semantic-query` left, 2026-11-14; Slack's five
    // public routes left for `oxy-api-slack` on 2026-11-12). A floor of 40 once let eleven disappear unnoticed, which
    // is most of a surface — near enough to the real count to notice a subtree
    // going quiet, loose enough that adding or removing a handful does not make
    // this the test everyone edits without reading.
    assert_eq!(
        declared, 0,
        "router/public.rs declares {declared} mounts; it declared none since \
         2026-11-21 (its routes are surface crates' now, on `SurfaceSeams::public`). \
         If you mounted one here on purpose, raise this number; if not, the parser \
         broke or this file moved.",
    );
}

/// The Postgres-backed run-state siblings of the customer-app data plane.
///
/// This test used to also pin the four EXECUTION routes IdeOnly, on the
/// grounds that they "build a WorkspaceManager from the working copy and run
/// inline". They no longer do — see
/// `the_customer_app_data_plane_is_fleet_ok`, which now covers them. What
/// remains here is the half that was always Postgres-only, kept because
/// these routes live in `router/public.rs`, outside `build_workspace_routes`,
/// so the workspace-mount drift test cannot see them.
#[test]
fn custom_app_run_state_routes_are_fleet_ok() {
    let pid = "d9830be4-c6a4";
    // Postgres-backed run-state siblings are cross-process safe → FleetOk.
    // The shell-context bootstrap (SDK shell chrome) is likewise a pure
    // persisted-data read and must never get pinned to the singleton.
    let fleet_ok = [
        ("GET", format!("/api/projects/{pid}/shell-context")),
        ("GET", format!("/api/projects/{pid}/threads")),
        ("GET", format!("/api/projects/{pid}/threads/t-1")),
    ];
    for (method, path) in &fleet_ok {
        assert_eq!(
            classify(method, path),
            RouteRole::FleetOk,
            "custom-app run-state route {method} {path} must stay FleetOk \
             (reads/writes Postgres run state, no working copy)"
        );
    }
}

// The whole customer-app data plane is FleetOk. Its last routes here, the
// metric tree's twins (`/api/projects/{id}/semantic/metric-tree*`), moved to
// `oxy-api-custom-apps` on 2026-11-14, and this test went with them
// (`crates/server/tests/integration/route_roles/custom_apps.rs`,
// `the_metric_tree_twins_are_declared_fleet_ok`): asserted here, `classify`
// would answer with the FleetOk default and pass for the wrong reason. The
// SQL-executing half used to be IdeOnly because it built a connector from the
// FS-fallback config; it reads the compile boundary now, so `databases` comes
// from the promoted revision and an ide restart does not take the apps down.

// The custom-app serve route and its function entry (`/customer-apps/{*path}`,
// `…/fn/{name}`) are FleetOk, and so is the external health read. Both moved to
// the `oxy-api-custom-apps` sibling crate with the routes, and their guards moved
// with them (`crates/server/tests/integration/route_roles/custom_apps.rs`). They
// cannot be asserted here: `oxy-app` does not declare them any more, so
// `classify` would answer with the FleetOk default and pass for the wrong reason.

/// Every way a route reaches the router. All of them take the path first,
/// so a parser needs the names and nothing else.
const MOUNT_MARKERS: &[&str] = &[
    ".route_ide(",
    ".route_fleet(",
    ".route_fleet_optional_working_copy(",
    ".route_split(",
    ".route_split_optional_working_copy(",
];

/// Coverage guard for routes that read NODE-LOCAL state the compile
/// boundary does not materialise (git state, process-local BROADCASTER
/// SSE). A serve replica has no working copy and no in-process run owner,
/// so these degrade silently there ("Workspace directory not found", a
/// truncated stream) unless classified IdeOnly. Hand-maintained, same
/// limitation as any hand-written list: it catches a route
/// moved off `route_ide`, not a brand-new router route nobody listed —
/// that's the behavioral canary's job (see internal-docs/compile-boundary.md
/// "the role-classification canary"). Name a new state-touching route in TWO
/// places: its `route_ide` mount, and here.
#[test]
fn manifest_covers_state_touching_routes() {
    let ws = "/api/d9830be4-c6a4";
    // Counter-guard: routes that LOOK similar but are cross-process safe
    // must stay FleetOk, or we needlessly pin the chat data plane to the
    // ide singleton. (A chat's and an automation's start, cancel and stream
    // read the compiled revision and the run rows, and /agentic-airway's are
    // queue-driven: all belong on this side.)
    let fleet_ok = [
        // The airway chunked backfill queues its drive (2026-11-14).
        ("POST", format!("{ws}/agentic-airway/chunked-backfill")),
        ("POST", format!("{ws}/analytics/runs")),
        ("GET", format!("{ws}/analytics/runs/r1/events")),
        ("GET", format!("{ws}/agentic-workflows/files")),
        ("GET", format!("{ws}/agentic-workflows/runs/r1/events")),
        ("POST", format!("{ws}/agentic-airway/runs")),
        ("GET", format!("{ws}/agentic-airway/runs/r1/events")),
        // `/world-model/cameras` left for `oxy-api-world-model` (2026-11-15),
        // whose own test pins it FleetOk; here it would pass on the default.
        // So did the parquet result cache (`/results/files/{file_id}`, GET and
        // DELETE) for `oxy-api-threads` (2026-11-16), whose
        // `persisted_data_reads_are_declared_fleet_ok` pins both.
    ];
    for (method, path) in &fleet_ok {
        assert_eq!(
            classify(method, path),
            RouteRole::FleetOk,
            "cross-process route {method} {path} must stay FleetOk"
        );
    }
}

// ── Stage 0b: router-derived completeness gate ─────────────────────────
//
// The FIRST test that derives its route set FROM the router source rather
// than a hand-maintained list. It parses every `.route()` / `.nest()` mount
// in `build_workspace_routes` (router/workspace.rs) and asserts each is
// EXPLICITLY classified — IdeOnly by its `route_ide` mount, or acknowledged
// FleetOk below. A new mount nobody classified fails CI here instead of
// silently defaulting to FleetOk and 404ing on a serve replica that has no
// working copy (the `/apps/source` outage class, oxygen-internal#2531).
//
// Scope/limits (honest): it guards the top-level mount surface. It does not
// introspect nested builders' sub-routes (axum exposes no route table) nor
// per-method gaps inside a builder — those stay guarded by the per-builder
// tests above + the behavioral canary (internal-docs/compile-boundary.md).
// `.merge(...)` mounts carry no path literal (git: see
// `manifest_covers_every_git_route`).

/// Workspace mounts whose DEFAULT is FleetOk: served statelessly (compile
/// boundary + Postgres + S3). Kept in sync with `build_workspace_routes` —
/// the test rejects stale entries.
///
/// A mount here may still be **MIXED** — `/apps` and `/integrations` each
/// have IdeOnly sub-routes (`/tests` was the third, until it went with the
/// tests UI). This list cannot express
/// that: `every_workspace_mount_is_classified` short-circuits on a match, so
/// an entry here means "do not ask about the mount", not "nothing under it
/// touches disk". The earlier wording claimed the latter and was false the
/// moment a sub-route was pinned.
///
/// **A mixed mount needs a per-sub-route guard.** See
/// `every_app_sub_route_is_classified` and the one beside it — without one,
/// every sub-route under the mount is classified by reviewer attention
/// alone, which is exactly how publish/unpublish came to write the working
/// copy while classified FleetOk.
///
/// REVIEW before adding one: if a handler under the mount reads local disk
/// and has no sub-route guard, it belongs on `route_ide`.

/// Extracts `(is_nest, path)` for every `.route("…")` / `.nest("…")` mount
/// in `body` (handles the multi-line `.route(\n  "…"` form).
fn parse_mounts(body: &str) -> Vec<(bool, String)> {
    let mut out = Vec::new();
    let nests: &[&str] = &[".nest(", ".nest_all(", ".nest_declared(", ".nest_typed("];
    for marker in MOUNT_MARKERS.iter().chain(nests) {
        let is_nest = nests.contains(marker);
        let mut hay = body;
        while let Some(i) = hay.find(marker) {
            let after = skip_ws_and_comments(&hay[i + marker.len()..]);
            if let Some(rest) = after.strip_prefix('"')
                && let Some(end) = rest.find('"')
            {
                out.push((is_nest, rest[..end].to_string()));
            }
            hay = &hay[i + marker.len()..];
        }
    }
    out
}

/// The source text of `fn <name>`, up to its closing brace.
fn fn_body<'a>(src: &'a str, name: &str) -> &'a str {
    let start = src
        .find(&format!("fn {name}"))
        .unwrap_or_else(|| panic!("{name} fn present"));
    let body = &src[start..];
    &body[..body.find("\n}\n").unwrap_or(body.len())]
}

/// Names of the builders `body` `.merge`s in, in source order.
///
/// Discovered rather than listed so a second merged builder is picked up
/// without anyone remembering to add it here. `.nest`ed builders are
/// deliberately not matched — see the note at the call site.
///
/// Takes the text up to the next `(`, which for a `.merge(some_router)` that is
/// not a call would run on to an unrelated paren — the `build_` prefix filter
/// is what contains that, so a non-call argument is skipped rather than
/// mis-parsed.
fn merged_builders(body: &str) -> Vec<&str> {
    let mut names = Vec::new();
    for (idx, _) in body.match_indices(".merge(") {
        let rest = &body[idx + ".merge(".len()..];
        let Some(end) = rest.find('(') else { continue };
        let name = rest[..end].trim();
        if name.starts_with("build_") && !names.contains(&name) {
            names.push(name);
        }
    }
    names
}

#[test]
fn discovers_every_merged_builder() {
    // A fixture, not the live router: the one merged builder
    // `build_workspace_routes` had (`build_metric_tree_routes`) left with the
    // metric tree for `oxy-api-metric-monitoring` on 2026-11-15, and a check of
    // the parser must not depend on the router happening to carry one.
    let src = "fn build_workspace_routes() {\n    RoleRouter::new(s)\n        \
               .merge(build_probe_routes(&app_state))\n        \
               .nest(\"/threads\", build_thread_routes(&app_state))\n        \
               .merge({ inline() })\n}\n";
    let merged = merged_builders(fn_body(src, "build_workspace_routes"));
    assert_eq!(
        merged,
        vec!["build_probe_routes"],
        "the merged builder must be discovered, and nothing else: {merged:?}"
    );
    // A `.nest`ed builder must NOT be picked up — its paths are relative to the
    // nest prefix, so classifying them here would use the wrong URI.
    assert!(
        !merged.contains(&"build_thread_routes"),
        "nested builders must stay out: {merged:?}"
    );
}

/// Skip whitespace AND `//` comments before a mount's path literal.
///
/// Without this, `rustfmt` moving a comment *inside* the `.route(` call —
/// which is exactly what happened to `/oxy-access` — makes the mount
/// invisible to this test. That fails silently in the dangerous direction: an
/// unclassified FS-touching route would slip through the very check that
/// exists to catch it. (It surfaced as a bogus "stale acknowledgement",
/// which is the lucky case.)
fn skip_ws_and_comments(mut s: &str) -> &str {
    loop {
        s = s.trim_start();
        let Some(rest) = s.strip_prefix("//") else {
            return s;
        };
        s = rest.find('\n').map_or("", |i| &rest[i..]);
    }
}

/// The door that let a FleetOk route hold a `WorkspaceManager<WorkingCopy>` is
/// gone, and this is what stops it coming back.
///
/// It existed because a handler reading the compile boundary FIRST and the
/// working copy only on a miss still had to name `WorkingCopy` in its
/// signature, which `route_fleet` refuses. The count went 24 -> 17 -> 8 -> 6
/// -> 5 -> 4 -> 0 as those handlers moved to `ConfigManager` reads that own
/// both arms, and the last four turned out not to belong on the fleet at all:
/// the metric-tree runner parses the semantic model off the workspace root, so
/// `/scan` and `/{id}/explain` failed on a replica rather than degrading.
///
/// A new one would mean a route claiming FleetOk while its handler requires a
/// disk. The two honest answers are `WorkspaceManagerReadOnly` — whose slot
/// carries `Option<WorkingCopy>`, so the boundary-miss fallback still works on
/// a node that owns files — or `route_ide`.
#[test]
fn the_optional_working_copy_door_stays_shut() {
    let router = include_str!("../../app-core/src/pods/role_router.rs");
    for gone in [
        "fn route_fleet_optional_working_copy",
        "fn route_split_optional_working_copy",
    ] {
        assert!(
            !router.contains(gone),
            "`{gone}` is back. It mounts a handler that requires a working copy \
             on a route declared FleetOk, which is a promise a stateless replica \
             cannot keep — the four routes it last held failed there with an IO \
             error naming a path the caller never wrote.\n\n\
             Use `WorkspaceManagerReadOnly` when the disk is a FALLBACK (its \
             slot is an `Option`, so a node with files still serves it), or \
             `route_ide` when the handler genuinely cannot run without one."
        );
    }

    let workspace = include_str!("router/workspace.rs");
    let workspace = &workspace[..workspace.find("#[cfg(test)]").unwrap_or(workspace.len())];
    for marker in [
        ".route_fleet_optional_working_copy(",
        ".route_split_optional_working_copy(",
    ] {
        assert!(
            !workspace.contains(marker),
            "`{marker}` is mounted again — see the message above."
        );
    }
}

/// Every mount in `build_workspace_routes` must be DECLARED. Before
/// `RoleRouter` this checked something weaker — that a mount was matched by
/// some pattern in a hand-written table, or listed in a 49-entry
/// acknowledgement of deliberate FleetOk mounts. Both are gone: a mount
/// carries its role now, so the only failure left is a mount the router
/// somehow serves without declaring one.
#[test]
fn every_workspace_mount_is_declared() {
    let src = include_str!("router/workspace.rs");
    // `build_workspace_routes` plus every builder it `.merge`s. The `.nest`ed
    // builders are deliberately excluded — their sub-routes carry no prefix at
    // this layer, so a path parsed out of them would be checked against the
    // wrong URI. A MERGED builder is the opposite case: its paths are absolute
    // and land in the tree verbatim, so leaving one out would hide those routes
    // from the very check that exists to catch an undeclared mount.
    //
    // So the merged builders are DISCOVERED, not listed: a hand-maintained list
    // has to be remembered when a second one is added, and nothing fails if it
    // isn't — the new routes just quietly stop being checked.
    let root = fn_body(src, "build_workspace_routes");
    // None today: the last one (`build_metric_tree_routes`) left with the
    // metric tree for `oxy-api-metric-monitoring` on 2026-11-15. The discovery
    // stays so the next one is checked without anyone remembering to list it;
    // `discovers_every_merged_builder` holds the parser on a fixture.
    let merged = merged_builders(root);
    let mut bodies = root.to_string();
    for name in &merged {
        bodies.push_str(fn_body(src, name));
    }
    let mounts = parse_mounts(&bodies);
    // 25 on 2026-11-16 (read off this assertion), after Wave B moved seven
    // mounts (the `/threads` and `/agents` nests, `/artifacts/{id}`,
    // `/charts/{file_path}`, `/logs`, `/builder-availability`,
    // `/results/files/{file_id}`) to `oxy-api-threads`, floor `> 30` ->
    // `> 20`; 20 the same day (read off this assertion) after Wave C moved
    // five more (`/meta`, `/members`, `/members/{user_id}`, `/oxy-access`,
    // the `/app-integrations` nest) to `oxy-api-workspaces`, floor `> 20` ->
    // `> 15`; 7 the same day (read off this assertion) after Wave D2 moved
    // thirteen more (`/sql/{pathb64}`, `/sql/query`, `/semantic`,
    // `/semantic/compile`, the topic and view details, `/semantic/objects`,
    // `/context-graph/sources`, the three `/revision*` reads and the two
    // `/semantic/preagg-*` routes) to `oxy-api-semantic`, floor `> 15` ->
    // `> 5`. The floor guards the parser, not a count: surfaces leaving
    // lower it.
    assert!(
        mounts.len() > 5,
        "parser found only {} mounts — the router shape changed; fix parse_mounts",
        mounts.len()
    );

    let declared = crate::router::route_declarations();
    for (_, path) in &mounts {
        let full = format!("/api/{{workspace_id}}{path}");
        assert!(
            declared.iter().any(|(_, p, _)| p.starts_with(&full)),
            "workspace mount {path:?} is served but declares no role. Mount it \
             through `RoleRouter` so it states one, or — if another crate owns \
             the routes and mounts them at this tree's root — go through \
             `merge_undeclared` with the reason.",
        );
    }
}

/// The flag a long-running entry point that never declares its role actually
/// moves (`oxy-server`'s `commands::entry_point_tests` checks each declares one).
///
/// `role_owns_workspace_files` is what the extension publish gate and the
/// state-dir fallback read, so a worker declaring `All` is a worker that gets
/// handed a working copy it has no files for.
#[test]
fn a_worker_that_declares_itself_owns_no_workspace_files() {
    // SAFETY: nextest runs each test in its own process, so neither the env var
    // nor the `PROCESS_ROLE` OnceLock leaks into another test.
    unsafe { std::env::set_var("OXY_ROLE", "worker") };
    assert_eq!(init_process_role_from_env(), Role::Worker);
    assert!(
        !oxy::workspace_fs_probe::process_owns_workspace_files(),
        "a declared worker must not claim the workspace filesystem"
    );
}

/// And with `OXY_ROLE` unset, which is the deployment that was actually broken.
///
/// Adding the init call to `run_worker` only helped charts that already set the
/// variable — everywhere else the unset default is `Role::All`, the value that
/// claims the filesystem. Running `oxy worker` is the stronger statement, so
/// the command supplies its own default and an explicit value still wins.
#[test]
fn an_undeclared_worker_still_owns_no_workspace_files() {
    // SAFETY: nextest runs each test in its own process.
    unsafe { std::env::remove_var("OXY_ROLE") };
    assert_eq!(
        init_process_role_from_env_with_default(Role::Worker),
        Role::Worker,
        "the command it was launched as is the declaration"
    );
    assert!(!oxy::workspace_fs_probe::process_owns_workspace_files());
}

#[test]
fn airhouse_admin_routes_are_fleet_ok() {
    let ws = "3c6e0b8a-9c15-224a-8236-000000000001";
    for (method, path) in [
        ("GET", "/api/admin/airhouse".to_string()),
        (
            "POST",
            format!("/api/admin/workspaces/{ws}/airhouse/provision"),
        ),
        (
            "POST",
            format!("/api/admin/workspaces/{ws}/airhouse/rotate-sa"),
        ),
    ] {
        assert_eq!(
            classify(method, &path),
            RouteRole::FleetOk,
            "{method} {path} is a Postgres-plus-HTTP read/write — pinning it to \
             the ide makes a tenant's warehouse surface die with the singleton"
        );
    }
}

// ── Ported from origin/main's inline `mod tests` at the #2851 merge ──────
// This branch moved that module to this file; these are the cases it had that
// this file did not.
//
// Three of main's did NOT come across, and their absence is deliberate:
// `ide_down_degradable_routes`, `every_workspace_mount_is_classified` and its
// sibling pin `degrades_when_ide_unreachable` / `FLEET_OK_ACKNOWLEDGED`, both
// removed by 5f5c3bd15 — the degrade hook answered `false` for every route, so
// the arm it guarded was unreachable, and the mount census moved to
// `tests/routing/route_role_derivation.rs`, which derives its route set from the router
// instead of a hand-kept list. Re-adding them would pin behaviour that no
// longer exists.
//
// Two more did not come across, and this pair is a real disagreement rather
// than dead code. main asserts `POST /api/projects/{id}/query` and the
// metric-tree query routes are IdeOnly because they "build a connector from the
// FS-fallback config, empty on serve". That WAS true; this branch changed it —
// the data plane reads the compile boundary, so `databases` comes from the
// promoted revision and the surviving filesystem arms are gated on
// `can_read_disk()`. Pinning them IdeOnly again would put every deployed custom
// app back behind the single pod holding a checkout, which is exactly what
// `the_customer_app_data_plane_is_fleet_ok` above exists to prevent.
//
// The move is not finished, and the gap is measured rather than assumed:
// against the docker fleet, `POST /api/projects/<ws>/query {"database":"local"}`
// answers `DuckDB 'local': cannot resolve path '.db/'` on a replica and returns
// rows on the ide. A DuckDB `local` database still resolves to a node-local
// file that `can_read_disk()` does not cover, so it fails with an error instead
// of the retryable 503 the design intends. Recorded as finding #2 in
// `customer-apps-oxy-starter-fleet.flow.test.yml`.
//
// Two more did not come across, and this one is a real disagreement rather than
// dead code: `custom_app_execution_routes_are_ide_only` and
// `customer_app_metric_tree_query_routes_are_ide_only` assert that
// `POST /api/projects/{id}/query` and the metric-tree query routes are IdeOnly,
// on the grounds that they "build a connector from the FS-fallback config,
// empty on serve". That WAS true. This branch changed it — the data plane reads
// the compile boundary, so `databases` comes from the promoted revision, and
// the surviving filesystem arms are gated on `can_read_disk()`. Pinning them
// IdeOnly again would put every deployed custom app back behind the single pod
// holding a checkout, which is what the data plane's FleetOk declarations in
// `oxy-api-custom-apps` exist to prevent.
//
// The move is not finished, and the gap is measured, not assumed: against the
// docker fleet, `POST /api/projects/<ws>/query {"database":"local"}` answers
// `DuckDB 'local': cannot resolve path '.db/'` on a replica and returns rows on
// the ide. A DuckDB `local` database still resolves to a node-local file that
// `can_read_disk()` does not cover, so the failure is an error rather than the
// retryable 503 the design intends. Recorded in
// `customer-apps-oxy-starter-fleet.flow.test.yml`'s header as finding #2.
//
// The nine that did come across are verbatim, so a failure means a real
// disagreement about a route's pod rather than a transcription slip.

/// The per-org OLTP surface reads Postgres and nothing else — no workspace
/// working copy, no `.git`, no state dir — so every route must serve from
/// any replica.
///
/// There is no manifest entry for these (FleetOk is the default); this test
/// exists because the *default* is what protects them. A future broad
/// `IdeOnly` pattern — an `/api/{workspace_id}/{*rest}` say, where
/// `workspace_id` would happily match the literal `oltp` — would silently
/// pin a tenant's live business data behind the singleton, and the symptom
/// is an HA outage rather than a compile error.
#[test]
fn tenant_data_plane_routes_are_fleet_ok() {
    let org = "8f14e45f-ceea-167a-5a36";
    // A workspace id, not the org one — the airhouse route below takes a
    // workspace, and reusing `org` for it read as a copy-paste slip even
    // though classification is by path shape.
    let ws = "3c6e0b8a-9c15-224a-8236";
    for (method, path) in [
        // Data plane: status + read-only ERD for the caller's own org.
        ("GET", "/api/oltp/me/connection".to_string()),
        ("GET", "/api/oltp/me/erd".to_string()),
        // Staff plane: provisioning and credential issue. Authorization is
        // a `route_layer` (Action::PlatformOltp); placement is orthogonal
        // to it, and neither touches node-local disk.
        ("GET", format!("/api/admin/orgs/{org}/oltp")),
        // One session to the tenant's database as its analyst: Postgres over
        // the network, nothing node-local, so any replica answers it.
        (
            "POST",
            format!("/api/admin/orgs/{org}/oltp/test-connection"),
        ),
        ("POST", format!("/api/admin/orgs/{org}/oltp/provision")),
        ("POST", format!("/api/admin/orgs/{org}/oltp/credentials")),
        // Added with the admin UI. Both are org-keyed writes against
        // Postgres and a provider API — no node-local state — so the
        // default is right; this list is what pins it.
        ("POST", format!("/api/admin/orgs/{org}/oltp/visibility")),
        ("DELETE", format!("/api/admin/orgs/{org}/oltp")),
        // Releasing one app's store. Same shape as its two neighbours —
        // Postgres plus the provider API — and pinned for the same reason:
        // the default is what protects it, so the list is what keeps a
        // future broad `IdeOnly` pattern from quietly capturing it.
        (
            "POST",
            format!("/api/admin/orgs/{org}/oltp/deprovision-writer"),
        ),
        // The confinement audit and a writer's rotation (2026-11-21, were
        // `oxy oltp audit | rotate`): Postgres sessions to the tenant plus
        // the provider, no node-local state.
        ("POST", format!("/api/admin/orgs/{org}/oltp/audit")),
        (
            "POST",
            format!("/api/admin/orgs/{org}/oltp/roles/app:probe/rotate"),
        ),
        // Airhouse's admin surface, same shape: Postgres plus an HTTP
        // client, no node-local state. Pinning, not a fix — the default is
        // already right, and this is what keeps a future broad `IdeOnly`
        // pattern from quietly capturing it.
        ("GET", "/api/admin/airhouse".to_string()),
        (
            "POST",
            format!("/api/admin/workspaces/{ws}/airhouse/provision"),
        ),
        (
            "POST",
            format!("/api/admin/workspaces/{ws}/airhouse/rotate-sa"),
        ),
    ] {
        assert_eq!(
            classify(method, &path),
            RouteRole::FleetOk,
            "{method} {path} is a Postgres-only read/write — pinning it to \
                 the ide makes a tenant's OLTP surface die with the singleton"
        );
    }
}

/// Both routers must mount the OLTP surface.
///
/// This guards a failure that already happened once and is recorded in the
/// design doc: `/oltp/me/connection` was mounted only in
/// `build_local_protected_routes`, but cloud reaches these through
/// `build_global_routes`. A missing route is not a 404 here — the SPA
/// catch-all answers unknown paths with **HTTP 200 and `index.html`**, so
/// the settings panel read it as "not provisioned" and the bug looked like
/// a data problem. Source-level, like
/// `fully_fs_builder_routes_classify_ide_only`, because the failure is a
/// missing line rather than a wrong value.
#[test]
fn the_oltp_router_is_mounted_in_both_global_and_protected_routers() {
    for (file, src) in [
        ("router/global.rs", include_str!("router/global.rs")),
        ("router/protected.rs", include_str!("router/protected.rs")),
    ] {
        assert!(
            // The `.merge` prefix is part of the needle on purpose: matching
            // the bare path would let a doc comment — or a commented-out
            // mount, which is exactly how this broke the first time — satisfy
            // the test. Both doors count: this branch mounts another crate's
            // router through `merge_undeclared`, which carries the reason the
            // merge has no declaration to hang a role on.
            src.contains(".merge(oxy_oltp::api::router")
                || src.contains(".merge_undeclared(\n            oxy_oltp::api::router")
                || src.contains(".merge_undeclared(oxy_oltp::api::router"),
            "{file} must merge oxy_oltp::api::router — a route missing from \
                 one of the two routers answers 200 + index.html, not 404"
        );
    }
}

#[test]
fn airway_config_is_fleet_ok() {
    assert_eq!(
        classify("GET", "/api/admin/airway/config"),
        RouteRole::FleetOk,
        "airway config CRUD is Postgres-only and must stay HA"
    );
}

#[test]
fn airway_config_writes_are_fleet_ok() {
    let ws = "d9830be4-c6a4-4c1c-9c1e-000000000001";
    for (method, path) in [
        ("PUT", "/api/admin/airway/config/toast".to_string()),
        ("DELETE", "/api/admin/airway/config/toast".to_string()),
        (
            "PUT",
            format!("/api/admin/airway/config/toast/workspaces/{ws}"),
        ),
        (
            "DELETE",
            format!("/api/admin/airway/config/toast/workspaces/{ws}"),
        ),
    ] {
        assert_eq!(
            classify(method, &path),
            RouteRole::FleetOk,
            "{method} {path} is Postgres-only CRUD (stage 3 Task 2) and must stay HA"
        );
    }
}

/// The preview reads workspace *content*, which is usually the tell for an
/// IdeOnly route — but it reads that content from the compile boundary, so
/// it is FleetOk like the rest of the airway-config surface. Pinned
/// because the tempting misreading (`airway_run` resolves specs from the
/// working copy, so this must too) would pin a Postgres-only admin surface
/// to the singleton for no reason — and that reading is now doubly wrong,
/// since `airway_run` reads the compiled row as well.
#[test]
fn airway_policy_preview_is_fleet_ok() {
    assert_eq!(
        classify("GET", "/api/admin/airway/config/toast/preview"),
        RouteRole::FleetOk,
        "the preview reads compiled `airway_pipelines` rows scoped to each workspace's \
             promoted revision — never the workspace filesystem — so it serves from any replica"
    );
}

// `compiling_a_branch_for_a_staging_build_is_fleet_ok` moved to
// `oxy-api-previews` (`lib.rs` tests) with the `/compile/staging` routes on
// 2026-11-16, asked of that crate's declarations: this manifest no longer
// holds them, and `classify` would pass on the FleetOk default.

/// The operational tier's three routes are Postgres-only and must stay
/// HA. The read one is the interesting case: it also reads a process-local
/// `OnceLock` to report what the answering replica installed. That is
/// deliberately NOT a reason to pin it to the ide — a singleton would
/// report exactly one process's state too, just less available. The
/// honesty comes from `installed_scope` in the payload, not from the route
/// class.
#[test]
fn airway_deployment_config_is_fleet_ok() {
    for method in ["GET", "PUT", "DELETE"] {
        assert_eq!(
            classify(method, "/api/admin/airway/deployment-config"),
            RouteRole::FleetOk,
            "{method} /api/admin/airway/deployment-config is Postgres-only and must stay HA"
        );
    }
}

/// `deployment-config` must not be swallowed by the per-source-kind
/// pattern next door. Both live under `/api/admin/airway/`, and
/// `/config/{source_kind}` is one segment away from matching it — a
/// regression here would classify the deployment routes by whatever rule
/// the policy routes carry, silently.
#[test]
fn deployment_config_does_not_collide_with_a_source_kind_path() {
    assert_eq!(
        classify("PUT", "/api/admin/airway/config/deployment-config"),
        RouteRole::FleetOk,
        "control: this is the source-kind route, matched by its own pattern"
    );
    assert_eq!(
        classify("GET", "/api/admin/airway/deployment-config"),
        RouteRole::FleetOk
    );
}
