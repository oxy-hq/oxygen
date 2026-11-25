//! The seams through which the composition root (`oxy-server`) hands extracted
//! surface crates to `oxy-app`'s `api_router` and `internal_api_router`.
//!
//! Each seam is a router plus what its routes declare about pod placement. They
//! are carried by NAME rather than as positional `(Router, Vec<RouteRoleDecl>)`
//! pairs because the three pairs share one type: swapping two of them compiled,
//! and would have put PIN sign-in behind auth or mounted org-admin routes
//! outside it. Which side of the auth stack a route lands on is the field it is
//! assigned to, so that decision is spelled out at every construction site.
//!
//! Down from `oxy-app`'s `server/router/seams.rs` (decompose R0, 2026-11-23), so
//! a surface that fills a seam need not depend on `oxy-app`. That file was a glob
//! re-export of this module until decompose R1, which repointed its users here
//! and deleted it with the prelude's `surface::seams`.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, OnceLock};

use agentic_runtime::worker::TaskExecutor;
use axum::Router;
use chrono::{DateTime, Utc};
use oxy_server_authz::Action;
use oxy_shared::fleet_role::{RouteRole, RouteRoleDecl};
use sea_orm::DatabaseConnection;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::AppState;
use crate::function_seam::FunctionRuntime;
use crate::preagg_cache_ctx::PreaggCacheCtx;

/// One surface seam: routes and their placement declarations, which travel
/// together. A route without a declaration takes the FleetOk default.
#[derive(Clone)]
pub struct SurfaceSeam {
    /// The surface's routes, relative to the tree the seam merges into.
    pub routes: Router<AppState>,
    /// Pod-placement declarations for `routes`, relative to the same tree (the
    /// router adds the `/api` prefix). Most surfaces are Postgres-only and the
    /// FleetOk default is the truth; a route that touches node-local disk must
    /// declare it here or it is served on the stateless fleet.
    pub decls: Vec<RouteRoleDecl>,
}

impl SurfaceSeam {
    /// A seam that mounts nothing: what every non-`serve` command, and every
    /// test that builds a router without extracted surfaces, passes.
    pub fn empty() -> Self {
        Self {
            routes: Router::new(),
            decls: Vec::new(),
        }
    }
}

impl Default for SurfaceSeam {
    fn default() -> Self {
        Self::empty()
    }
}

/// One staff-console section an extracted surface contributes under
/// `/api/admin`.
///
/// It merges INSIDE the admin router, so it inherits the outer staff guard
/// (`oxy_owner_or_app_admin_guard`) and the refusal while an operator is acting
/// as a tenant (`assume::block_admin_while_acting`). On top of those, every
/// in-tree section escalates to the capability its surface is about —
/// `audit` to `PlatformAudit`, `app_admins` to `PlatformGrants` (both
/// `oxy-api-admin`'s sections now). A section
/// therefore NAMES its capability and `oxy-app` applies the guard: an
/// extracted surface cannot mount an admin route that skips it, the way it
/// could if it had to remember the `route_layer` itself.
#[derive(Clone)]
pub struct AdminSection {
    /// The capability that admits a caller to this section
    /// (`platform_cap_guard::require`). Finer, per-verb checks stay in the
    /// handlers, as they do for the in-tree sections.
    pub capability: Action,
    /// The section's routes, relative to `/admin`. Must not be empty: the
    /// capability is applied with `route_layer`, which axum refuses on a
    /// router with no routes.
    pub routes: Router<AppState>,
    /// Pod-placement declarations, relative to `/admin`. The console is
    /// FleetOk wholesale (`admin::router_roles`), so only a route that touches
    /// node-local disk needs one — `POST /orgs` scaffolding a workspace, say.
    pub decls: Vec<RouteRoleDecl>,
}

/// One staff-console tree an extracted surface nests BESIDE the console's
/// router, at `/api/admin{prefix}`, rather than merging inside it.
///
/// For a tree whose routes are flat under one prefix and which carries its own
/// guard stack rather than the console's. `oxy-app` wraps `routes` in the three
/// `.layer`s the sibling nests (`/internal-jobs`, `/compiles`) always had
/// in-tree — the capability named here innermost, then the refusal while acting as
/// a tenant (`assume::block_admin_while_acting`), then the staff door
/// (`oxy_owner_or_app_admin_guard`) outermost — and mounts it with `nest_all`,
/// which declares `{prefix}` and `{prefix}/{*rest}` with `role`.
///
/// `.layer`, not a section's `route_layer`: a mounted path asked with a method
/// it does not take answers the guards before its 405. A path the tree does not
/// match at all falls to the outer router's 404 without reaching them — axum's
/// nest, read off the served router on `main` before this seam existed.
///
/// As with a section, the surface NAMES its capability and `oxy-app` applies
/// every guard: a surface cannot mount a staff tree that skips one.
#[derive(Clone)]
pub struct AdminNest {
    /// Relative to `/admin`, with its leading `/` and no trailing one
    /// (`"/compiles"`). Relative, so a guarded staff tree can only ever land
    /// under `/admin`.
    pub prefix: &'static str,
    /// The capability that admits a caller to the tree
    /// (`platform_cap_guard::require`), the innermost of the three layers.
    pub capability: Action,
    /// The tree's routes, relative to the prefix.
    pub routes: Router<AppState>,
    /// Pod placement for the whole prefix: `nest_all` declares both shapes.
    pub role: RouteRole,
    /// Why `role` is right, as `nest_all` asks of every caller.
    pub why: &'static str,
}

/// What a task executor is built from on the node that drains the queue.
pub struct TaskDeps<'a> {
    pub db: &'a DatabaseConnection,
    /// The node's Layer-1 preagg cache. Default (no cache) compiles to
    /// warehouse SQL.
    pub preagg: &'a PreaggCacheCtx,
    /// The driver's analytics schema cache, shared with the chat runs it
    /// drives. A custom-app ask's executor takes it; `None` builds the schema
    /// per run. Spelled out rather than named through
    /// `queued_analytics::SchemaCache` (the same type): the seams are base,
    /// and the queued-run machinery is not (`base_boundary`).
    pub schema_cache: Option<
        &'a std::sync::Arc<
            std::sync::Mutex<
                std::collections::HashMap<String, agentic_pipeline::AnalyticsSchemaCatalog>,
            >,
        >,
    >,
}

/// One `TaskSpec::Custom` kind an extracted surface runs, with the executor
/// that runs it. The kind string is the persisted task-queue contract: it
/// is written into `agentic_task_queue` rows, so it never changes with the code
/// that registers it.
#[derive(Clone, Copy)]
pub struct TaskSeam {
    pub kind: &'static str,
    /// Builds the executor. Called every time the driver builds its registry,
    /// so it must be cheap: a few `Arc` clones.
    pub build: fn(&TaskDeps<'_>) -> Arc<dyn TaskExecutor>,
}

/// What a background job is started from.
pub struct BackgroundDeps<'a> {
    pub db: &'a DatabaseConnection,
    /// Cancelled when the process shuts down.
    pub shutdown: &'a CancellationToken,
}

/// One process-lifetime job an extracted surface needs running in every serve
/// process. A job whose answer is global rather than per replica is a
/// [`FleetJobSeam`] instead.
#[derive(Clone, Copy)]
pub struct BackgroundSeam {
    /// Names the job in the startup check ([`SurfaceSeams::require_custom_apps`]).
    pub name: &'static str,
    /// Starts the job. A job that must finish a last flush on shutdown returns
    /// its handle, which the router registers as a shutdown hook.
    pub start: fn(&BackgroundDeps<'_>) -> Option<JoinHandle<()>>,
}

/// What a fleet job is started from.
pub struct FleetJobDeps<'a> {
    pub db: &'a DatabaseConnection,
    /// Cancelled when the process shuts down.
    pub shutdown: &'a CancellationToken,
}

/// One fleet job an extracted surface hands in. The job claims its own work
/// through [`fleet_claim`](crate::fleet_claim), so starting it on several replicas
/// at once is safe — that is the contract a job takes on by being one.
#[derive(Clone, Copy)]
pub struct FleetJobSeam {
    /// Names the job in the startup check and the boot log.
    pub name: &'static str,
    /// Starts the job. Spawns and returns; nothing waits on it at shutdown.
    pub start: fn(&FleetJobDeps<'_>),
}

/// The fleet jobs the custom-app surface must hand in. A worker without them
/// would boot, and the storage figures the staff console shows would stop
/// moving with nothing failing.
pub const CUSTOM_APP_FLEET_JOBS: [&str; 8] = [
    "custom_app_asset_lifecycle_verify",
    "custom_app_storage_sweeper",
    "custom_app_usage_backfill_sweep",
    // Without it, a published event a crash left in an org's outbox is never
    // relayed, and a stale outbox is never reported.
    "custom_app_event_relay_sweep",
    // Without it, a dead task never reaches its delivery, and events and
    // schema revisions are never pruned.
    "custom_app_event_retention",
    // Without it, a pipeline's or a rollup's change to a watched table never
    // reaches its listeners (a host write's nudge is the fast path only), and
    // the event log mirror is never written.
    "custom_app_table_watch_sweep",
    // Without it, a captured write whose nudge was lost or debounced away is
    // never shipped to Airhouse, and a stale change stream is never reported.
    "oltp_changes_relay_sweep",
    // Without it, a store whose writes no nudge announced is never drained.
    "oltp_changes_weekly_sweep",
];

/// Refuse a fleet-job list that lacks one [`CUSTOM_APP_FLEET_JOBS`] names, or
/// names one twice.
pub fn require_fleet_jobs(jobs: &[FleetJobSeam]) -> Result<(), String> {
    let mut seen = std::collections::HashSet::new();
    for j in jobs {
        if !seen.insert(j.name) {
            return Err(format!("fleet job `{}` is registered twice", j.name));
        }
    }
    let missing: Vec<&str> = CUSTOM_APP_FLEET_JOBS
        .iter()
        .copied()
        .filter(|n| !seen.contains(n))
        .collect();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "the surface seams start none of the fleet job(s) {missing:?}; on a split fleet \
             nothing else runs them"
        ))
    }
}

/// One piece of singleton work an extracted surface needs the global driver to
/// run on every pass, beside the schedule ticks `oxy-app` owns
/// (`tick_schedules`, `tick_health_schedules`, `tick_preagg_schedules`). The
/// experiment tick is the first: it advances every due run. Exactly-once across
/// replicas is the seam's own job — the experiment tick claims each run by a CAS
/// on `experiment_runs.next_check_at` — so several eligible nodes running it is
/// safe, as it is for the ticks beside it.
///
/// Installed process-wide ([`install_global_ticks`]) rather than threaded through
/// the driver's dozen signatures: the driver is `oxy-app`'s and names no surface,
/// the way route declarations are installed from the seams rather than passed.
#[derive(Clone, Copy)]
pub struct GlobalTickSeam {
    /// Names the tick in the driver's log line.
    pub name: &'static str,
    /// Where in the driver's loop it runs: inside the pass, after the schedule
    /// ticks, or once per iteration of the loop. See [`TickPass`].
    pub pass: TickPass,
    /// Runs one pass at `now`; how many items it advanced. Called every
    /// `OXY_INPROC_GLOBAL_WORKER_INTERVAL_SECS` (30 s), so it must be cheap when
    /// nothing is due: an indexed SELECT of due rows.
    pub tick: for<'a> fn(
        &'a DatabaseConnection,
        DateTime<Utc>,
    ) -> Pin<Box<dyn Future<Output = usize> + Send + 'a>>,
}

/// Where in the global driver's loop a [`GlobalTickSeam`] runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TickPass {
    /// Inside the driver's pass, after `oxy-app`'s schedule ticks
    /// ([`run_global_ticks`]): in the local pass once the workspace's context
    /// is built, and in the cloud pass only on a node that claimed a
    /// workspace this tick, which leaves an idle node's tick at the claim
    /// alone. The experiment tick and the token sweep.
    AfterSchedules,
    /// Once per iteration of the driver's loop (`recovery::spawn_recovery`),
    /// after the pass, on every process that runs the driver, in either mode
    /// and whatever it claimed ([`run_loop_ticks`]). For work that belongs to
    /// no workspace, which a claim-gated pass could starve: the weekly
    /// custom-app usage report, called inline there before it left `oxy-app`.
    EveryLoop,
}

static GLOBAL_TICKS: OnceLock<Vec<GlobalTickSeam>> = OnceLock::new();

/// Install the surfaces' global ticks for this process. First install wins: the
/// router and the worker command each install the composition root's list once,
/// before the driver is spawned, and a test that builds a router without seams
/// installs an empty list. A second, different list is a composition error and
/// is logged, not applied.
pub fn install_global_ticks(ticks: &[GlobalTickSeam]) {
    if GLOBAL_TICKS.set(ticks.to_vec()).is_err() {
        let installed: Vec<&str> = global_ticks().iter().map(|t| t.name).collect();
        let offered: Vec<&str> = ticks.iter().map(|t| t.name).collect();
        if installed != offered {
            tracing::warn!(
                ?installed,
                ?offered,
                "global ticks were already installed for this process; the later list is ignored"
            );
        }
    }
}

/// The installed ticks; empty until [`install_global_ticks`] runs.
pub fn global_ticks() -> &'static [GlobalTickSeam] {
    GLOBAL_TICKS.get().map(Vec::as_slice).unwrap_or(&[])
}

/// One pass of every installed [`TickPass::AfterSchedules`] tick, in order; how
/// many items they advanced in all. Called by the global driver
/// (`recovery.rs`) after its own schedule ticks, in both the local and the
/// cloud pass.
pub async fn run_global_ticks(db: &DatabaseConnection, now: DateTime<Utc>) -> usize {
    run_ticks(global_ticks(), TickPass::AfterSchedules, db, now).await
}

/// One run of every installed [`TickPass::EveryLoop`] tick, in order; how many
/// items they advanced in all. Called by the global driver's loop
/// (`recovery::spawn_recovery`) after each pass.
pub async fn run_loop_ticks(db: &DatabaseConnection, now: DateTime<Utc>) -> usize {
    run_ticks(global_ticks(), TickPass::EveryLoop, db, now).await
}

async fn run_ticks(
    ticks: &[GlobalTickSeam],
    pass: TickPass,
    db: &DatabaseConnection,
    now: DateTime<Utc>,
) -> usize {
    let mut total = 0;
    for seam in ticks.iter().filter(|t| t.pass == pass) {
        let advanced = (seam.tick)(db, now).await;
        if advanced > 0 {
            tracing::info!(target: "global_tick", tick = seam.name, advanced, "global tick advanced work");
        }
        total += advanced;
    }
    total
}

/// The notifications surface's process hooks (`oxy-api-notifications`).
///
/// Every `oxy serve` and `oxy worker` both raises notifications (billing, the
/// anomaly delivery, a failed run, a device-code request) and delivers them (the
/// `notification_deliver` task, or inline for an org with no workspace), so both
/// run these, at the points they ran when the surface was a module of this
/// crate, and both refuse to start without them
/// ([`SurfaceSeams::require_notifications`], [`require_notifications`]). A
/// process that skipped them would boot, write every inbox row, and deliver
/// nothing: the fail-silent inversion `internal-docs/domain-boundaries.md`
/// warns about.
#[derive(Clone, Copy)]
pub struct NotificationsSeam {
    /// Before migrations: install the inline transport the write path below this
    /// crate delivers through (`oxy_app_core::notifications::inline`), the
    /// agentic-runtime observers that raise run notices (`oxy.run_failed`,
    /// `oxy.ask_finished`, `oxy.ask_waiting`), and the push transports. Answers
    /// whether any push transport is configured, for the boot log.
    pub install: fn() -> bool,
    /// After migrations: the settings reconcile (mutes written while a release
    /// was rolled back, into the settings table). Best-effort, never a reason
    /// not to start.
    pub after_migrations: fn() -> Pin<Box<dyn Future<Output = ()> + Send>>,
}

/// Refuse to boot a process without the notifications hooks. `oxy worker` takes
/// the seam on its own (it is not handed all of [`SurfaceSeams`]), so the check
/// is a function as well as a method.
pub fn require_notifications(seam: Option<&NotificationsSeam>) -> Result<(), String> {
    match seam {
        Some(_) => Ok(()),
        None => Err(
            "the surface seams hand in no notifications hooks (`SurfaceSeams::notifications`); \
             without them this process would write inbox rows and deliver none of them"
                .to_string(),
        ),
    }
}

/// Builds the router `serve` mounts at the top level of the outer router,
/// beside `/api`. It is given the Layer-1 preagg cache the API router owns,
/// because a route mounted outside the API router has no `AppState` to read it
/// from.
pub type OuterMount = fn(PreaggCacheCtx) -> Router;

/// What `oxy migrate` prints after the migrations: a report the custom-app
/// surface produces (the functions preflight). Given the build's label.
pub type MigrateReport =
    fn(String) -> std::pin::Pin<Box<dyn std::future::Future<Output = String> + Send>>;

/// What the demo seed knows that its custom-app half asks: whether the
/// database is one a seed may touch, which workspace the partner seed made for
/// an org, and who the seeded App Operator is. The seed (`oxy dev seed`, in
/// `oxy-server`'s dev tooling) hands it to [`ExampleAppsSeam::deploy`] and
/// [`ExampleAppsSeam::fixtures`], so `oxy-api-custom-apps` never reaches the seed
/// itself (decompose SD, 2026-11-24). Functions rather than values: each is
/// asked at the moment, and in the place, the custom-app half asked it before.
#[derive(Clone, Copy)]
pub struct DemoSeedFacts {
    /// The database looks local, or `OXY_SEED_ALLOW_REMOTE` consents to demo
    /// rows on one that does not. Every seed write that fabricates data gates on
    /// this one definition.
    pub is_local_db: fn() -> bool,
    /// The workspace the partner seed made for an org (its slug and id), by its
    /// derived id; `None` for an org the seed does not define.
    pub seeded_workspace_id: fn(&str, uuid::Uuid) -> Option<uuid::Uuid>,
    /// The email of the seeded unscoped App Operator, the demo fixture's
    /// publisher.
    pub unscoped_app_operator_email: fn() -> Option<&'static str>,
}

/// The demo seed's custom-app half: the example apps it deploys and the storage
/// figures hung off them, and the matching teardown. `oxy dev seed` and the
/// integration-test fixture pass it to `seed_demo_with`; the seed (`oxy-server`'s
/// dev tooling) holds the rows the apps belong to (workspaces, orgs, teams) and
/// `oxy-api-custom-apps` the apps themselves.
#[derive(Clone, Copy)]
pub struct ExampleAppsSeam {
    /// Deploy the example apps to the demo workspace (id, workspace path) and to
    /// Acme's, restricted to `acme_restricted_team` when the partner seed made one.
    /// Warns instead of failing, like the rest of the seed's extras.
    pub deploy: for<'a> fn(
        &'a sea_orm::DatabaseConnection,
        uuid::Uuid,
        &'a str,
        Option<uuid::Uuid>,
        DemoSeedFacts,
    )
        -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>>,
    /// Build the demo fixture app (`oxy dev seed --fixtures`) in the demo workspace
    /// (its id), after the compile; the flag says the compile succeeded. Warns
    /// instead of failing.
    pub fixtures:
        for<'a> fn(
            &'a sea_orm::DatabaseConnection,
            uuid::Uuid,
            bool,
            DemoSeedFacts,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>>,
    /// Remove the seeded storage figures (before the apps, see `clear_demo`).
    pub clear_storage: for<'a> fn(
        &'a sea_orm::DatabaseConnection,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<(), oxy_shared::errors::OxyError>> + Send + 'a>,
    >,
    /// Remove the seeded example apps and the demo fixture; how many were
    /// removed.
    pub clear_apps: for<'a> fn(
        &'a sea_orm::DatabaseConnection,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<u64, oxy_shared::errors::OxyError>> + Send + 'a,
        >,
    >,
}

/// Every surface seam, by where it sits relative to the auth stack.
#[derive(Clone, Default)]
pub struct SurfaceSeams {
    /// AUTHENTICATED. Merged into the protected tree before `apply_middleware`
    /// (cloud mode), so these routes inherit the standard auth stack (auth /
    /// api-key / timeout / publish-token-scope); on the internal port they sit
    /// behind internal auth. A surface still re-applies its own inner
    /// middleware (e.g. org_middleware + subscription_guard). Cloud-only:
    /// local mode never mounts the org/global tree and drops this seam.
    pub api: SurfaceSeam,
    /// AUTHENTICATED, workspace-scoped. Merged INSIDE the `/{workspace_id}`
    /// nest, so these routes inherit the auth stack plus the workspace
    /// middleware. They also carry the process's `AgenticState` as an
    /// `Extension` (`protected::with_agentic_state`), which the anomaly inbox
    /// extracts.
    pub workspace: SurfaceSeam,
    /// AUTHENTICATED, workspace previews (`oxy-api-previews`). Nested at
    /// `/{workspace_id}/previews` BESIDE the workspace tree, behind
    /// `workspace_access_middleware` alone: `?branch=` there names a preview,
    /// not a working copy, and a request carrying the preview header must still
    /// manage previews, which the read-only guard in `workspace_middleware`
    /// refuses. Declarations relative to the nest. Cloud-only, like `api`.
    pub previews: SurfaceSeam,
    /// UNAUTHENTICATED. Merged beside `build_public_routes`, outside the auth
    /// stack, in every serve mode and on the internal port. Only routes that
    /// must be reachable without a session belong here — frontline PIN sign-in
    /// is how a worker gets a session at all.
    pub public: SurfaceSeam,
    /// API-KEY ONLY, workspace-scoped. Merged into the curated
    /// `/external/api/{workspace_id}` tree that standalone apps call, before its
    /// workspace context, API-key-only auth and wide-open CORS layers, so these
    /// routes inherit all three. Routes only, no declarations: the curated tree
    /// drops its own too, and every route on it falls to the FleetOk default.
    /// Mounted in both serve modes. Like `workspace`, it carries the
    /// `AgenticState` extension.
    pub external: Router<AppState>,
    /// STAFF CONSOLE. Sections merged inside `/api/admin`, each behind the
    /// capability it names — see [`AdminSection`]. Cloud-only, like `api`:
    /// local mode never mounts the global tree that holds `/admin`.
    pub admin: Vec<AdminSection>,
    /// STAFF CONSOLE, sibling trees: each nested at `/api/admin{prefix}`
    /// beside the console, behind the staff door, the refusal while acting and
    /// the capability it names, on every path it mounts, a wrong method
    /// included (an unmatched path is the outer 404) — see [`AdminNest`].
    /// Cloud-only, like `admin`.
    pub admin_nests: Vec<AdminNest>,
    /// The OpenAPI operations extracted surfaces document, merged into the
    /// served document (`/apidoc/openapi.json`, which `oxyc schema` reads).
    pub openapi: Vec<utoipa::openapi::OpenApi>,
    /// AUTHENTICATED, route discovery: `GET /api/_catalog`, with the route
    /// table of everything above bound in. Merged at the ROOT of the
    /// protected tree (beside the global tree in cloud mode, alone in local
    /// mode, behind internal auth on the internal port), so unlike `api` it
    /// is served in BOTH modes. The handler is `oxy-server`'s (`core_routes`,
    /// since 2026-11-16) and the table `oxy-route-catalog`'s, which sits
    /// above this crate so that editing a surface crate does not rebuild
    /// `oxy-app` (`internal-docs/domain-boundaries.md` S7). Empty by default,
    /// which is what tests building a router without the binary get.
    pub catalog: SurfaceSeam,
    /// The custom-app task kinds a node that drains the queue can run, with
    /// their executors. Consumed where the driver builds its
    /// `CustomTaskRegistry` (`serve`'s in-process driver and `oxy worker`).
    pub tasks: Vec<TaskSeam>,
    /// Process-lifetime jobs a serve process starts for custom apps, started
    /// by `serve`'s boot (`oxy-server`'s `commands::serve_background`). One-shot
    /// per-request spawns stay with their handlers.
    pub background: Vec<BackgroundSeam>,
    /// Jobs whose answer is global rather than per replica (the storage
    /// sweeper, the usage-backfill sweep, the asset-bucket lifecycle check).
    /// Started by every process that runs singleton work — each `oxy worker`,
    /// and an `all`/`ide` serve — and each claims its interval in
    /// `fleet_job_claims`, so the fleet runs it once per interval
    /// (`drivers/fleet_jobs.rs` starts them).
    pub fleet_jobs: Vec<FleetJobSeam>,
    /// Singleton work the global driver runs each pass for an extracted surface
    /// (the experiment tick). Installed process-wide by `serve`'s boot and
    /// `oxy worker` through [`install_global_ticks`]; the driver reads them
    /// beside its own schedule ticks.
    pub global_ticks: Vec<GlobalTickSeam>,
    /// The outer `/customer-apps/{*path}` mount, merged by `serve` beside
    /// `/api`. `None` mounts nothing; `serve` refuses to start without it.
    pub outer: Option<OuterMount>,
    /// Pod-placement declarations for [`outer`](Self::outer). Absolute paths, as
    /// the outer router sits beside `/api` and not under it, so unlike the
    /// declarations of the seams above they do not get the `/api` prefix.
    /// Installed with the others in cloud mode.
    pub outer_decls: Vec<RouteRoleDecl>,
    /// Runs a function request on a person's behalf for the `oxy-app` handlers
    /// that do (a notification's function action). Layered onto the API tree as
    /// an `Extension`; `None` leaves such an action answering that no runtime
    /// is available.
    pub function_runtime: Option<Arc<dyn FunctionRuntime>>,
    /// The report `oxy migrate` prints after the migrations.
    pub post_migrate: Option<MigrateReport>,
    /// The demo seed's custom-app half; `None` seeds the rows only.
    pub example_apps: Option<ExampleAppsSeam>,
    /// The notifications surface's boot hooks; `serve` and `worker` refuse to
    /// start without them.
    pub notifications: Option<NotificationsSeam>,
}

/// The `TaskSpec::Custom` kinds `oxy-app`'s own enqueue sites queue for custom
/// apps. The code that runs them is handed in through [`SurfaceSeams::tasks`];
/// a node that lacks one would fail every such task as an unknown kind.
///
/// The strings, not the constants that name them in `oxy-api-custom-apps`: the
/// executors are in that crate and `oxy-app` cannot name it. A test there
/// (`task_kinds_are_the_six_the_fleet_knew`) holds each constant to its string,
/// and `oxy-server`'s `the_task_kinds_are_exactly_these_ten` the list handed in.
///
/// `custom_app_agent_ask` is queued by every live ask's start, which moved to
/// `oxy-api-custom-apps` with its executor (2026-11-14). It is listed so a
/// process whose seams lack the executor refuses to start, rather than fail
/// every ask as an unknown kind; `agentic-pipeline`'s recovery and the chat
/// executor name the same string.
pub const CUSTOM_APP_TASK_KINDS: [&str; 10] = [
    "app_function",
    "custom_app_staging_migrations",
    "custom_app_sandbox_teardown",
    "custom_app_sandbox_migrations",
    "custom_app_sandbox_oltp",
    "custom_app_env_oltp_branch",
    "custom_app_agent_ask",
    // The automation run's queued executor, `oxy-api-custom-apps`' since
    // 2026-11-14 (in-tree before).
    "custom_app_procedure_run",
    // The app events relay: queued by a publish's nudge and by the relay
    // sweep, so a process without its executor would leave every org's
    // published events in the outbox.
    "app_events_relay",
    // The table tracker: queued by a host write's nudge and by its sweep, so
    // a process without its executor would leave every table change unread.
    "app_table_watch",
];

/// The `TaskSpec::Custom` kinds `oxy-app`'s own sources queue for the
/// notifications surface: `notification_deliver` (every send, from
/// `oxy_app_core::notifications`) and `notification_quiet_summary` (queued by a
/// delivery run, which is the surface's own, but drained by any node). Their
/// executors are `oxy-api-notifications`'s, handed in through
/// [`SurfaceSeams::tasks`]; `oxy-server`'s tests hold the constants to these
/// strings.
pub const NOTIFICATION_TASK_KINDS: [&str; 2] =
    ["notification_deliver", "notification_quiet_summary"];

/// The `TaskSpec::Custom` kind the health schedule tick and the console's
/// "Re-evaluate" / "Run now" queue (`agentic_pipeline::scheduler`): one
/// workspace's health eval. Its executor is `oxy-api-workspace-health`'s,
/// handed in through [`SurfaceSeams::tasks`]; a test there holds its constant
/// to this string.
pub const HEALTH_TASK_KINDS: [&str; 1] = ["health_eval_workspace"];

/// The `TaskSpec::Custom` kind a monitor scan queues when its `.monitor.yml`
/// has a `notify:` block (`oxy_metric_monitoring::notify::enqueue_if_due`,
/// called by the scheduled scan in `agentic_wiring` and the manual one in
/// `oxy-api-metric-monitoring`): one insights delivery. Its executor is
/// `oxy-api-metric-monitoring`'s since 2026-11-21 (in-tree before), handed in
/// through [`SurfaceSeams::tasks`]; a test there holds its constant to this
/// string. Without it a scan would queue deliveries that fail as unknown, and
/// nobody would be told.
pub const ANOMALY_TASK_KINDS: [&str; 1] = ["anomaly_notify"];

/// The `TaskSpec::Custom` kinds workspace previews queue: the change check
/// (queued by the compile worker through `previews::queue`), the TTL schema
/// drop, the transform compare and the Airway sample (queued by the run
/// queue's step). Their executors are `oxy-api-previews`'s since 2026-11-14,
/// handed in through [`SurfaceSeams::tasks`]; a test there holds its
/// constants to these strings.
pub const PREVIEW_TASK_KINDS: [&str; 4] = [
    "preview_analyze",
    "preview_schema_drop",
    "preview_compare",
    "preview_airway_sample",
];

/// The background job a serve process needs for previews: the TTL sweep that
/// queues schema drops and compares (`oxy-api-previews`, 2026-11-14).
pub const PREVIEW_BACKGROUND_JOBS: [&str; 1] = ["preview_maintenance"];

/// The background job a serve process needs for chat: the `LISTEN` connection
/// that fans a posted message's wake out to this replica's open streams
/// (`oxy-api-threads`, 2026-11-16; `new_agentic_state` started it before).
/// Without it chat still works on the replica that took the POST and on no
/// other: invisible on one box, total on a fleet.
pub const CHAT_BACKGROUND_JOBS: [&str; 1] = ["chat_delivery_listener"];

/// The background jobs a serve process needs for custom apps (usage rows,
/// sandbox expiry, retention). Without the usage drain, page views and events
/// queue until the bounded queue sheds them. The storage measurement and the
/// lifecycle check are fleet jobs ([`CUSTOM_APP_FLEET_JOBS`]).
pub const CUSTOM_APP_BACKGROUND_JOBS: [&str; 6] = [
    "custom_app_sandbox_maintenance",
    "custom_app_tracking_retention",
    "custom_app_usage_drain",
    "custom_app_tail_listener",
    "custom_app_function_limits",
    "custom_app_automation_run_sweep",
];

/// Refuse a task list that lacks a kind `oxy-app` queues, or names one twice.
/// A missing kind would otherwise surface only as queued tasks that fail.
pub fn require_task_kinds(tasks: &[TaskSeam]) -> Result<(), String> {
    let mut seen = std::collections::HashSet::new();
    for t in tasks {
        if !seen.insert(t.kind) {
            return Err(format!("task kind `{}` is registered twice", t.kind));
        }
    }
    let missing: Vec<&str> = CUSTOM_APP_TASK_KINDS
        .iter()
        .chain(NOTIFICATION_TASK_KINDS.iter())
        .chain(HEALTH_TASK_KINDS.iter())
        .chain(ANOMALY_TASK_KINDS.iter())
        .chain(PREVIEW_TASK_KINDS.iter())
        .copied()
        .filter(|k| !seen.contains(k))
        .collect();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "the surface seams hand in no executor for the task kind(s) {missing:?}; \
             tasks of those kinds would be queued and then fail as unknown"
        ))
    }
}

impl SurfaceSeams {
    /// No extracted surfaces at all.
    pub fn empty() -> Self {
        Self::default()
    }

    /// The startup check for a process that serves custom apps: every task
    /// kind and background job `oxy-app` depends on is present (the previews'
    /// sweep and chat's listener among them), and the outer mount is. Called
    /// by `serve` before it builds anything.
    pub fn require_custom_apps(&self) -> Result<(), String> {
        require_task_kinds(&self.tasks)?;
        require_fleet_jobs(&self.fleet_jobs)?;
        let missing: Vec<&str> = CUSTOM_APP_BACKGROUND_JOBS
            .iter()
            .chain(PREVIEW_BACKGROUND_JOBS.iter())
            .chain(CHAT_BACKGROUND_JOBS.iter())
            .copied()
            .filter(|n| !self.background.iter().any(|b| b.name == *n))
            .collect();
        if !missing.is_empty() {
            return Err(format!(
                "the surface seams start none of the custom-app background job(s) {missing:?}"
            ));
        }
        if self.outer.is_none() {
            return Err(
                "the surface seams hand in no `/customer-apps/{*path}` mount; without it the \
                 static fallback would answer for every custom app"
                    .to_string(),
            );
        }
        Ok(())
    }

    /// The startup check for any process that raises or delivers notifications
    /// (`serve`, `worker`): see [`NotificationsSeam`].
    pub fn require_notifications(&self) -> Result<(), String> {
        require_notifications(self.notifications.as_ref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentic_core::delegation::TaskAssignment;
    use agentic_runtime::worker::ExecutingTask;

    struct Dummy;

    #[async_trait::async_trait]
    impl TaskExecutor for Dummy {
        async fn execute(&self, _assignment: TaskAssignment) -> Result<ExecutingTask, String> {
            Err("dummy".to_string())
        }
    }

    fn task(kind: &'static str) -> TaskSeam {
        TaskSeam {
            kind,
            build: |_| Arc::new(Dummy),
        }
    }

    fn job(name: &'static str) -> BackgroundSeam {
        BackgroundSeam {
            name,
            start: |_| None,
        }
    }

    fn complete() -> SurfaceSeams {
        SurfaceSeams {
            tasks: CUSTOM_APP_TASK_KINDS
                .iter()
                .chain(NOTIFICATION_TASK_KINDS.iter())
                .chain(HEALTH_TASK_KINDS.iter())
                .chain(ANOMALY_TASK_KINDS.iter())
                .chain(PREVIEW_TASK_KINDS.iter())
                .map(|k| task(k))
                .collect(),
            background: CUSTOM_APP_BACKGROUND_JOBS
                .iter()
                .chain(PREVIEW_BACKGROUND_JOBS.iter())
                .chain(CHAT_BACKGROUND_JOBS.iter())
                .map(|n| job(n))
                .collect(),
            fleet_jobs: CUSTOM_APP_FLEET_JOBS
                .iter()
                .map(|&name| FleetJobSeam {
                    name,
                    start: |_| {},
                })
                .collect(),
            outer: Some(|_| Router::new()),
            ..SurfaceSeams::empty()
        }
    }

    /// The persisted task-queue contract: these strings are in
    /// `agentic_task_queue` rows, so a change here strands queued work.
    #[test]
    fn the_required_task_kinds_are_the_nine_the_fleet_knows() {
        assert_eq!(
            CUSTOM_APP_TASK_KINDS,
            [
                "app_function",
                "custom_app_staging_migrations",
                "custom_app_sandbox_teardown",
                "custom_app_sandbox_migrations",
                "custom_app_sandbox_oltp",
                "custom_app_env_oltp_branch",
                "custom_app_agent_ask",
                "custom_app_procedure_run",
                "app_events_relay",
                "app_table_watch",
            ]
        );
    }

    /// The install is first-wins and the second list is ignored, so a router
    /// built in a test (empty seams) cannot clear what the process installed —
    /// and no caller can double-register a tick.
    #[test]
    fn global_ticks_install_once() {
        fn tick(name: &'static str) -> GlobalTickSeam {
            GlobalTickSeam {
                name,
                pass: TickPass::AfterSchedules,
                tick: |_, _| Box::pin(async { 0usize }),
            }
        }
        install_global_ticks(&[tick("first")]);
        install_global_ticks(&[tick("second")]);
        let names: Vec<&str> = global_ticks().iter().map(|t| t.name).collect();
        assert_eq!(names, ["first"]);
    }

    /// Each runner runs the ticks of its own pass and no other, in order: a
    /// tick at the wrong pass runs on the wrong nodes, or not at all.
    #[tokio::test]
    async fn each_runner_runs_only_the_ticks_of_its_pass() {
        use std::sync::Mutex;
        static RAN: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());
        fn tick(name: &'static str, pass: TickPass) -> GlobalTickSeam {
            GlobalTickSeam {
                name,
                pass,
                tick: match name {
                    "a" => |_, _| {
                        Box::pin(async {
                            RAN.lock().unwrap().push("a");
                            1usize
                        })
                    },
                    "b" => |_, _| {
                        Box::pin(async {
                            RAN.lock().unwrap().push("b");
                            2usize
                        })
                    },
                    _ => |_, _| {
                        Box::pin(async {
                            RAN.lock().unwrap().push("c");
                            4usize
                        })
                    },
                },
            }
        }
        let ticks = [
            tick("a", TickPass::AfterSchedules),
            tick("b", TickPass::EveryLoop),
            tick("c", TickPass::AfterSchedules),
        ];
        let db = DatabaseConnection::default();
        let now = Utc::now();
        assert_eq!(
            run_ticks(&ticks, TickPass::AfterSchedules, &db, now).await,
            5
        );
        assert_eq!(std::mem::take(&mut *RAN.lock().unwrap()), ["a", "c"]);
        assert_eq!(run_ticks(&ticks, TickPass::EveryLoop, &db, now).await, 2);
        assert_eq!(std::mem::take(&mut *RAN.lock().unwrap()), ["b"]);
    }

    #[test]
    fn a_complete_composition_is_accepted() {
        assert_eq!(complete().require_custom_apps(), Ok(()));
    }

    /// No seams at all is what every router test builds, and not something a
    /// process that serves custom apps may start with.
    #[test]
    fn an_empty_composition_is_refused() {
        let err = SurfaceSeams::empty().require_custom_apps().unwrap_err();
        assert!(err.contains("app_function"), "{err}");
    }

    #[test]
    fn a_missing_task_kind_is_a_startup_error_that_names_it() {
        let mut seams = complete();
        seams.tasks.retain(|t| t.kind != "custom_app_sandbox_oltp");
        let err = seams.require_custom_apps().unwrap_err();
        assert!(err.contains("custom_app_sandbox_oltp"), "{err}");
        assert!(require_task_kinds(&seams.tasks).is_err());
    }

    #[test]
    fn a_missing_notification_kind_is_a_startup_error_that_names_it() {
        let mut seams = complete();
        seams.tasks.retain(|t| t.kind != "notification_deliver");
        let err = require_task_kinds(&seams.tasks).unwrap_err();
        assert!(err.contains("notification_deliver"), "{err}");
    }

    #[test]
    fn a_missing_health_kind_is_a_startup_error_that_names_it() {
        let mut seams = complete();
        seams.tasks.retain(|t| t.kind != "health_eval_workspace");
        let err = require_task_kinds(&seams.tasks).unwrap_err();
        assert!(err.contains("health_eval_workspace"), "{err}");
    }

    /// Insights delivery is `oxy-api-metric-monitoring`'s since 2026-11-21. A
    /// node without it would drain the queued deliveries and fail them as
    /// unknown: the scan succeeds, and nobody is told. So it is a startup error.
    #[test]
    fn a_missing_anomaly_kind_is_a_startup_error_that_names_it() {
        let mut seams = complete();
        assert_eq!(require_task_kinds(&seams.tasks), Ok(()));
        seams.tasks.retain(|t| t.kind != "anomaly_notify");
        let err = require_task_kinds(&seams.tasks).unwrap_err();
        assert!(err.contains("anomaly_notify"), "{err}");
        assert!(seams.require_custom_apps().is_err());
    }

    /// The persisted task-queue contract for the insights delivery.
    #[test]
    fn the_anomaly_task_kind_is_the_one_the_queue_holds() {
        assert_eq!(ANOMALY_TASK_KINDS, ["anomaly_notify"]);
    }

    /// Workspace previews' kinds are `oxy-api-previews`' since 2026-11-14; a
    /// node without one would fail the queued check, drop, compare or sample.
    #[test]
    fn a_missing_preview_kind_is_a_startup_error_that_names_it() {
        let mut seams = complete();
        seams.tasks.retain(|t| t.kind != "preview_analyze");
        let err = require_task_kinds(&seams.tasks).unwrap_err();
        assert!(err.contains("preview_analyze"), "{err}");
    }

    /// Without the TTL sweep, expired preview schemas are never dropped.
    #[test]
    fn a_missing_preview_maintenance_job_is_a_startup_error() {
        let mut seams = complete();
        seams.background.retain(|b| b.name != "preview_maintenance");
        let err = seams.require_custom_apps().unwrap_err();
        assert!(err.contains("preview_maintenance"), "{err}");
    }

    /// Without chat's listener a message reaches only the streams on the
    /// replica that took the POST, so it is a boot error.
    #[test]
    fn a_missing_chat_listener_is_a_startup_error() {
        let mut seams = complete();
        seams
            .background
            .retain(|b| b.name != "chat_delivery_listener");
        let err = seams.require_custom_apps().unwrap_err();
        assert!(err.contains("chat_delivery_listener"), "{err}");
    }

    /// No hooks means inbox rows with no delivery, so it is a boot error.
    #[test]
    fn missing_notification_hooks_are_a_startup_error() {
        assert!(SurfaceSeams::empty().require_notifications().is_err());
        let seams = SurfaceSeams {
            notifications: Some(NotificationsSeam {
                install: || false,
                after_migrations: || Box::pin(async {}),
            }),
            ..SurfaceSeams::empty()
        };
        assert_eq!(seams.require_notifications(), Ok(()));
    }

    #[test]
    fn a_task_kind_registered_twice_is_refused() {
        let mut seams = complete();
        seams.tasks.push(task("app_function"));
        let err = seams.require_custom_apps().unwrap_err();
        assert!(err.contains("registered twice"), "{err}");
    }

    /// On a split fleet the worker is the only process that runs these.
    #[test]
    fn a_missing_fleet_job_is_a_startup_error() {
        let mut seams = complete();
        seams
            .fleet_jobs
            .retain(|j| j.name != "custom_app_storage_sweeper");
        let err = seams.require_custom_apps().unwrap_err();
        assert!(err.contains("custom_app_storage_sweeper"), "{err}");
    }

    #[test]
    fn a_missing_background_job_or_outer_mount_is_a_startup_error() {
        let mut seams = complete();
        seams
            .background
            .retain(|b| b.name != "custom_app_usage_drain");
        let err = seams.require_custom_apps().unwrap_err();
        assert!(err.contains("custom_app_usage_drain"), "{err}");

        let mut seams = complete();
        seams.outer = None;
        let err = seams.require_custom_apps().unwrap_err();
        assert!(err.contains("/customer-apps/"), "{err}");
    }
}
