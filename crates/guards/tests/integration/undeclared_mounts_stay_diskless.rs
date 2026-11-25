//! The routers `oxy-app` mounts without declaring a role must not reach disk.
//!
//! A merge at a tree's ROOT has no prefix to hang a declaration on, so
//! `RoleRouter::merge_undeclared` mounts those routers and their routes fall to
//! `classify`'s FleetOk default — the one hole the type-level gate cannot close.
//!
//! That is safe today for a reason nothing was checking: neither crate reaches
//! the workspace working copy, `.git`, or the local state dir at all. Measured
//! when this file was written — `oxy-cameras` mounts 75 routes and `airhouse` 6,
//! and the count of workspace-FS accesses across both crates is zero.
//!
//! So the guard is not "classify these routes correctly", it is "keep them
//! diskless". A camera handler that started reading the working copy would be
//! served on a replica that has none, with nothing to catch it — the exact
//! failure this branch exists to make unrepresentable.

use std::path::Path;

/// Every way a handler reaches node-local state, as the route-classification
/// skill defines it.
const WORKSPACE_FS: &[&str] = &[
    "workspace_path",
    "effective_workspace_path",
    "resolve_state_dir",
    "GitClient",
    "WorkspaceManagerWorkingCopy",
    "WorkspaceRootWorkingCopy",
    "ConfigManager",
    // Creating a workspace scaffolds its working copy.
    "workspace_provisioning",
    "register_workspace",
];

/// Accesses a scanned crate makes behind a route it DOES declare, so the
/// FleetOk default never applies to them. `(file suffix, needle)` — per needle,
/// so the exemption cannot quietly cover a second kind of access in that file.
const DECLARED_ACCESSES: &[(&str, &str)] = &[
    // `POST /webhooks/toast/orders` (`oxy_api_world_model::public_route_roles`,
    // FleetOk) is a public route with no workspace middleware, so it resolves
    // the workspace's path itself (`effective_workspace_path`, and
    // `resolve_local_workspace_path` for the nil workspace, which the
    // `workspace_path` needle matches) — but only as the label and fallback of
    // a slot-less manager: config comes from the compile boundary first
    // (#2816), and the FS read is reached only where no revision is promoted.
    // `workspace_path_backdoor` and `compiled_reader_is_not_a_back_door` list
    // the same file for the same reason. The other needles still apply to it.
    ("api-world-model/src/toast_webhook.rs", "workspace_path"),
    (
        "api-world-model/src/toast_webhook.rs",
        "effective_workspace_path",
    ),
    // `POST /{workspace_id}/source-uploads/reports` reads the pipeline
    // definition through `ConfigManager`, which owns the compiled-vs-disk
    // choice and pins the request's revision; `oxy_api_airway::
    // route_roles()` declares the route FleetOk. Today the call is spelled as a
    // field (`workspace.config_manager`), so nothing matches — this reserves the
    // slot so naming the type later does not read as a new disk access. The
    // working-copy needles still apply to the file.
    ("api-airway/src/source_upload.rs", "ConfigManager"),
    // A view's and a topic's details (`oxy_api_semantic::workspace_route_roles`,
    // FleetOk) resolve the file through the compile boundary first
    // (`scan::read_semantic_entity`) and fall back to `try_resolve_file`, which
    // has an impl for each capability, so a node without a working copy answers
    // "not compiled" instead of reading. The bound that says so names the type
    // (`ConfigManager<S>: ResolveWorkspaceFile`). The working-copy needles
    // still apply to the file. As it was in `oxy-app`'s `api/semantic.rs`,
    // where `route_fleet`'s handler type held the same line.
    ("api-semantic/src/semantic.rs", "ConfigManager"),
    // The demo seed (`oxy dev seed`, and the integration fixture) deploys the example
    // apps from a workspace path it is handed. It is a CLI step, mounted on no
    // route at all, so no pod placement applies.
    ("api-custom-apps/src/seed_apps.rs", "workspace_path"),
    ("api-custom-apps/src/seed.rs", "workspace_path"),
    // The compile console's "By workspace" rows (`GET /admin/compiles/
    // workspaces`, out of `oxy-app` in the admin split's PR 6, nested FleetOk
    // as an `AdminNest`) REPORT each workspace's configured path: the field is
    // `workspaces.path` read back from Postgres (`w.path AS workspace_path`)
    // and shown to staff. No file is opened. The other needles still apply.
    (
        "api-previews/src/admin/compiles/workspaces.rs",
        "workspace_path",
    ),
    // `GET /{workspace_id}/simulations` and `POST …/simulations/{name}/runs`
    // read the declared world through `ConfigManager`, which owns the
    // compiled-vs-disk choice (`simulation_definitions` through the compile
    // boundary, the working copy only where one is held) and pins the
    // request's revision; both routes are FleetOk by default, as they were under
    // `route_fleet`. The working-copy needles still apply to both files.
    (
        "api-experiment/src/api/simulation/worlds.rs",
        "ConfigManager",
    ),
    ("api-experiment/src/api/simulation/runs.rs", "ConfigManager"),
    // The custom-app automation run's start (`POST …/procedures/{procedure_id}/runs`)
    // finds the automation through `ConfigManager::list_automations`, which
    // owns the compiled-vs-disk choice and reads the request's revision; the
    // route is declared FleetOk in `public_route_roles`, as it was under
    // `route_fleet` in `oxy-app`. The working-copy needles still apply.
    ("api-custom-apps/src/automation_run/mod.rs", "ConfigManager"),
    // The eval-run route reads the test files from the compile boundary
    // (`eval_test_definitions`) through `ConfigManager`, which owns the
    // compiled-vs-disk choice.
    ("api-eval/src/runs/handlers.rs", "ConfigManager"),
    // Its unit tests build a manager over a temp directory to drive that
    // lookup without a database. Test code, mounted on no route.
    (
        "api-custom-apps/src/automation_run/tests.rs",
        "workspace_path",
    ),
];

/// Every `.rs` file under `dir` — or `dir` itself, when it names one file.
fn rust_sources(dir: &Path) -> Vec<(String, String)> {
    let mut out = Vec::new();
    if dir.is_file() {
        if let Ok(body) = std::fs::read_to_string(dir) {
            out.push((dir.display().to_string(), body));
        }
        return out;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(rust_sources(&path));
        } else if path.extension().and_then(|e| e.to_str()) == Some("rs")
            && let Ok(body) = std::fs::read_to_string(&path)
        {
            out.push((path.display().to_string(), body));
        }
    }
    out
}

#[test]
fn the_crates_mounted_without_a_declaration_never_touch_the_working_copy() {
    let mut offenders = Vec::new();
    let mut files_scanned = 0;

    for crate_dir in [
        "../cameras/src",
        "../airhouse/src",
        // Declared FleetOk wholesale by the `/admin` wildcard in global.rs, so a
        // route added under one inherits FleetOk without anyone deciding. Measured
        // diskless when those declarations were written; the reconcile config
        // read under `/admin/workspace-health` runs on the WORKER, not on the
        // request path, which is why it is not an exception here.
        // Extracted from `src/server/api/partner_console` into a sibling crate,
        // now the `partner_console` module of `oxy-api-tenancy`. ONLY that
        // module: the crate's `onboarding` module clones and scaffolds on purpose
        // and declares every route IdeOnly, so scanning it here would be wrong.
        // The guard asserts its own sources are non-empty precisely so a move
        // like that fails loudly instead of silently covering nothing. Merged
        // at the root with no prefix; the one route that needs the ide is in
        // its `route_roles()`, and exempted by `DECLARED_ACCESSES`.
        "../api-tenancy/src/partner_console",
        // The rest of tenancy, moved out of `oxy-app`'s `build_global_routes`.
        // There the routes were `route_fleet`, whose state type refused a
        // working-copy extractor at compile time; as plain axum routes merged
        // through the seam they lost that gate, so this scan is what is left.
        // All FleetOk except `POST /admin/orgs`, exempted above.
        "../api-tenancy/src/organizations",
        "../api-tenancy/src/org_teams",
        "../api-tenancy/src/admin",
        // One file, not a directory: `PUT/DELETE /orgs/{org_id}/logo`. Logo
        // bytes live in Postgres; this is the only guard left on that.
        "../api-tenancy/src/org_logo.rs",
        // Three files that left `oxy-app` on 2026-11-16, where their routes
        // were `route_fleet` (a `FleetState` handler type refused a
        // working-copy extractor): the consent switch, the org subdomain's
        // status and the logo. Declared FleetOk by hand now in
        // `oxy_api_tenancy::{route_roles, workspace_route_roles}`; each reads
        // or writes Postgres rows only.
        "../api-tenancy/src/partner_publish_consent.rs",
        "../api-tenancy/src/org_subdomain.rs",
        "../api-tenancy/src/workspace_logo.rs",
        // Extracted from `src/server/api/documents`, merged at the root the same
        // way. Postgres + presigned S3 everywhere; `POST /documents/ask` resolves
        // an agent config out of the working copy (through the project context,
        // not a needle below) and is declared IdeOnly in its `route_roles()`.
        "../api-documents/src",
        // Extracted from `src/server/api/frontline*.rs`. Its routes ARE declared
        // (`route_roles` / `public_route_roles`), but every declaration is a
        // hand-written FleetOk: leaving `RoleRouter::route_fleet` lost the type
        // gate that stopped one of these handlers taking a working copy, so
        // "stays diskless" is the check that is left.
        "../api-frontline/src",
        // The operating graph and work items, moved out of `build_global_routes`
        // (2026-11-14) where all nine paths were `route_fleet`. Declared FleetOk
        // by hand in its `route_roles()`; this scan is the type gate that is left.
        "../api-operating-graph/src",
        // The custom-app serving half: `/customer-apps/{*path}` and the health, logs
        // and client-error reads. Declared FleetOk in `public_route_roles` /
        // `serve_dispatch_roles`; the bundle and its maps come from the build store
        // and the function entry reads the promoted revision, so nothing here
        // touches a working copy.
        "../api-custom-apps/src",
        // Extracted from `src/server/api/source_upload.rs`, merged inside the
        // `/{workspace_id}` nest. Declared FleetOk in its `route_roles()`: it
        // writes to S3 and reads the pipeline definition through
        // `ConfigManager`, which owns the compiled-vs-disk choice — the handler
        // itself never reaches for a working copy.
        "../api-airway/src",
        // Experiments and declared worlds, moved out of `build_workspace_routes`
        // where 18 of the 19 routes were `route_fleet`: the `MethodRouter<FleetState>`
        // type refused a working-copy extractor at compile time, and as plain
        // axum routes merged through the workspace seam they lost that gate, so
        // this scan is what is left. The one IdeOnly route (write-back apply) and
        // the two compile-boundary `ConfigManager` reads are exempted above.
        "../api-experiment/src",
        // Notifications, moved out of `build_global_routes` where all 13 routes
        // were `route_fleet`. Declared FleetOk by hand in its `route_roles()`
        // now, so this scan is what stops a handler taking a working copy.
        "../api-notifications/src",
        // Workspace previews' upper half, moved out of `oxy-app` on 2026-11-14,
        // where its ten routes were `route_fleet` (a `FleetState` handler type
        // refused a working-copy extractor). Declared FleetOk by hand now in
        // `oxy_api_previews::route_roles`, so this scan is what is left: create
        // and refresh compile a branch from the commit GitHub has, and the rest
        // touch only Postgres.
        "../api-previews/src",
        // The task fleet's operator console (`/admin/internal-jobs`), out of
        // `oxy-app`'s `api/admin/` in the admin split's PR 7, nested FleetOk as
        // an `AdminNest`: the queue, run and workspace tables in Postgres and
        // the durable transport's reaper and retention, never a working copy.
        // Since 2026-11-20 also assume-role (`/assume*`), merged UNDECLARED at
        // the protected tree's root through `SurfaceSeams::api`, as
        // `oxy-app`'s `merge_undeclared` had it: the session and audit tables
        // only, so the FleetOk default holds while this scan stays clean.
        "../api-admin/src",
        // The weekly custom-app usage report (`/admin/usage-report*`), out of
        // `oxy-app`'s `api/admin/` in the admin split's PR 8b, two
        // `AdminSection`s placed by the console's FleetOk wildcard: the report,
        // preference and delivery rows and the staff grants, never a working copy.
        // Since PR 9 also the Billing queue (a third `AdminSection`, placed the
        // same way), and the tenant's billing and Stripe's webhook, declared
        // FleetOk by hand in `route_roles()` / `public_route_roles()` now that
        // they left `nest_all` / `route_fleet`: the Stripe customer,
        // subscription and webhook-event rows, and Stripe, never a working copy.
        "../api-billing/src",
        // The metric tree and the anomaly inbox, moved out of
        // `build_workspace_routes` on 2026-11-15, where all seventeen routes
        // were `route_fleet` (a `FleetState` handler type refused a
        // working-copy extractor). Declared FleetOk by hand now in
        // `oxy_api_metric_monitoring::workspace_route_roles`, so this scan is
        // what is left: the tree resolves its scan through the compile boundary,
        // the inbox reads Postgres and the compiled `.monitor.yml`.
        "../api-metric-monitoring/src",
        // The observability console (`/traces`, `/metrics`,
        // `/execution-analytics`), moved out of `build_workspace_routes` on
        // 2026-11-15, where each was a FleetOk `nest_all`. Declared FleetOk
        // by hand now in `oxy_api_observability::workspace_route_roles`;
        // the handlers read the ClickHouse product store, never the
        // working copy.
        "../api-observability/src",
        // The camera list and the weather (`/world-model/cameras`,
        // `/world-model/weather/*`), moved out of both `build_workspace_routes`
        // and `build_external_workspace_routes` on 2026-11-15, where each was
        // `route_fleet` (a `FleetState` handler type refused a working-copy
        // extractor). Declared FleetOk by hand now in
        // `oxy_api_world_model::workspace_route_roles`; the handlers call
        // UniFi and OpenWeatherMap with the workspace's keys, read through
        // `WorkspaceManagerReadOnly`'s config.
        "../api-world-model/src/video.rs",
        // The automation listing and `/agentic-schedules`, moved out of
        // `build_workspace_routes` on 2026-11-15, where all eight routes were
        // `route_fleet` (a `FleetState` handler type refused a working-copy
        // extractor). Declared FleetOk by hand now in
        // `oxy_api_automations::workspace_route_roles`; the listing reads the
        // compile boundary through `ConfigManager`, the schedules Postgres.
        "../api-automations/src",
        // The workspace's databases and secrets, moved out of
        // `build_workspace_routes` (and `router/secrets.rs`) on 2026-11-15,
        // where all nine routes were `route_fleet`. Declared FleetOk by hand
        // now in `oxy_api_workspace_config::workspace_route_roles`; the list
        // reads the in-memory config (its datasets only where
        // `workspace_file_resolver` finds files), the schema and inspection
        // talk to the warehouse, the secrets to Postgres. Since 2026-11-21 also
        // the OAuth connects and callbacks, out of `build_integration_routes`
        // and `build_public_routes`, where all four were `route_fleet`;
        // declared FleetOk by hand in `workspace_route_roles` and
        // `public_route_roles`: a Postgres state row, the vendor's token
        // endpoint and the workspace's secrets, never a working copy.
        "../api-workspace-config/src",
        // The GitHub App's webhook receiver (`POST /webhooks/github`), moved
        // out of `build_public_routes` on 2026-11-15, where it was
        // `route_fleet` (a `FleetState` handler type refused a working-copy
        // extractor). Declared FleetOk by hand now in
        // `oxy_api_github::public_route_roles`; the handler verifies an HMAC,
        // reads Postgres and queues a compile.
        "../api-github/src/webhook.rs",
        // The Toast order webhook (`POST /webhooks/toast/orders`), moved out of
        // `build_public_routes` on 2026-11-15, where it was `route_fleet`.
        // Declared FleetOk by hand now in
        // `oxy_api_world_model::public_route_roles`; its two path resolutions
        // are exempted above, so a working-copy extractor or a `GitClient`
        // added there is still caught.
        "../api-world-model/src/toast_webhook.rs",
        // Workspace health's console routes, moved out of `admin/` onto the
        // admin seam. The handlers read and enqueue in Postgres; the live
        // runners that build a workspace context run in the task executor, on
        // a worker, not on a request path.
        "../api-workspace-health/src/handlers.rs",
        // The health probes and route discovery, moved UP into `oxy-server`
        // on 2026-11-16, where all five were `route_fleet` (a `FleetState`
        // handler type refused a working-copy extractor). Declared FleetOk by
        // hand now in `core_routes::{public_route_roles, catalog_route_roles}`;
        // they read a ping, the build stamp and a compiled-in table.
        "../server/src/core_routes",
        // The conversation surface, out of `oxy-app` on 2026-11-16, where
        // every route was `route_fleet` (a `FleetState` handler type refused
        // a working-copy extractor). Declared FleetOk by hand now in
        // `oxy_api_threads::workspace_route_roles`; Postgres, the read-only
        // workspace manager and the S3-mirrored runtime artifacts.
        "../api-threads/src",
        // Three of the workspaces surface's files, out of `oxy-app` on
        // 2026-11-16, where every route was `route_fleet`. Declared FleetOk by
        // hand now in `oxy_api_workspaces::workspace_route_roles`; Postgres
        // and the read-only workspace manager. By name, not the crate: the
        // workspace details (`workspaces/`) resolve the working copy where the
        // serving process owns one (local mode's details, a delete's files),
        // which `workspace_path_backdoor` lists.
        "../api-workspaces/src/workspace_members.rs",
        "../api-workspaces/src/workspace_oxy_access.rs",
        "../api-workspaces/src/apps.rs",
        // The semantic surface, out of `oxy-app` on 2026-11-16, where all
        // thirteen routes were `route_fleet` (a `FleetState` handler type
        // refused a working-copy extractor). Declared FleetOk by hand now in
        // `oxy_api_semantic::workspace_route_roles`; every handler takes
        // `WorkspaceManagerReadOnly`, the scan resolves through the compile
        // boundary first, the revision reads are compiled rows.
        "../api-semantic/src",
        // The eval runs (`oxyc evals run`), new on 2026-11-21 and declared
        // FleetOk in `oxy_api_eval::workspace_route_roles`: the handlers read the
        // compile boundary and enqueue; the executor runs on a worker over the
        // promoted revision. Only `runs/`: the crate's `oxy test` command reads
        // a local working copy on purpose and mounts no route.
        "../api-eval/src/runs",
        "../app/src/server/api/billing",
    ] {
        let sources = rust_sources(Path::new(crate_dir));
        assert!(
            !sources.is_empty(),
            "no sources under {crate_dir} — the crate moved and this guard \
             stopped covering it",
        );
        files_scanned += sources.len();

        for (path, body) in sources {
            for needle in WORKSPACE_FS {
                if DECLARED_ACCESSES
                    .iter()
                    .any(|(file, declared)| path.ends_with(file) && declared == needle)
                {
                    continue;
                }
                for (index, line) in body.lines().enumerate() {
                    // A mention in prose is not an access.
                    let code = line.split("//").next().unwrap_or(line);
                    if code.contains(needle) {
                        offenders.push(format!("{path}:{} — {needle}", index + 1));
                    }
                }
            }
        }
    }

    assert!(
        files_scanned > 20,
        "scanned only {files_scanned} files — the walk broke, and a guard that \
         reads nothing passes for the wrong reason",
    );

    assert!(
        offenders.is_empty(),
        "these crates are mounted with `merge_undeclared`, so their routes are \
         served on any pod — including replicas with no working copy. One of \
         them now reaches for one:\n  {}\n\nEither drop the access, or mount \
         the router through a door that declares a role.",
        offenders.join("\n  "),
    );
}
