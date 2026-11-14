//! The seams through which the composition root (`oxy-server`) hands extracted
//! surface crates to [`api_router`](super::api_router) and
//! [`internal_api_router`](super::internal_api_router).
//!
//! Each seam is a router plus what its routes declare about pod placement. They
//! are carried by NAME rather than as positional `(Router, Vec<RouteRoleDecl>)`
//! pairs because the three pairs share one type: swapping two of them compiled,
//! and would have put PIN sign-in behind auth or mounted org-admin routes
//! outside it. Which side of the auth stack a route lands on is the field it is
//! assigned to, so that decision is spelled out at every construction site.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, OnceLock};

use agentic_runtime::worker::TaskExecutor;
use axum::Router;
use chrono::{DateTime, Utc};
use oxy_app_core::function_seam::FunctionRuntime;
use oxy_shared::fleet_role::RouteRoleDecl;
use sea_orm::DatabaseConnection;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use super::AppState;
use crate::server::api::middlewares::workspace_context::PreaggCacheCtx;
use crate::server::authz::Action;

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
/// `orgs_admin` to `PlatformOrgs`, `users_admin` to `PlatformUsers`. A section
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
    /// Whether this process is the role that runs singleton work
    /// (`inproc_global_worker_enabled`). A job whose answer is global rather
    /// than per replica returns early when it is false.
    pub singleton: bool,
}

/// One process-lifetime job an extracted surface needs running in a serve
/// process.
#[derive(Clone, Copy)]
pub struct BackgroundSeam {
    /// Names the job in the startup check ([`SurfaceSeams::require_custom_apps`]).
    pub name: &'static str,
    /// Starts the job. A job that must finish a last flush on shutdown returns
    /// its handle, which the router registers as a shutdown hook.
    pub start: fn(&BackgroundDeps<'_>) -> Option<JoinHandle<()>>,
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
    /// Runs one pass at `now`; how many items it advanced. Called every
    /// `OXY_INPROC_GLOBAL_WORKER_INTERVAL_SECS` (30 s), so it must be cheap when
    /// nothing is due: an indexed SELECT of due rows.
    pub tick: for<'a> fn(
        &'a DatabaseConnection,
        DateTime<Utc>,
    ) -> Pin<Box<dyn Future<Output = usize> + Send + 'a>>,
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

/// One pass of every installed tick, in order; how many items they advanced in
/// all. Called by the global driver (`recovery.rs`) after its own schedule
/// ticks, in both the local and the cloud pass.
pub(crate) async fn run_global_ticks(db: &DatabaseConnection, now: DateTime<Utc>) -> usize {
    let mut total = 0;
    for seam in global_ticks() {
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

/// The demo seed's custom-app half: the example apps it deploys and the storage
/// figures hung off them, and the matching teardown. `oxy seed` and the
/// integration-test fixture pass it to `seed_demo_with`; `oxy-app` holds the rows
/// the apps belong to (workspaces, orgs, teams) and `oxy-api-custom-apps` the
/// apps themselves.
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
    )
        -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>>,
    /// Build the demo fixture app (`oxy seed --fixtures`) in the demo workspace
    /// (its id), after the compile; the flag says the compile succeeded. Warns
    /// instead of failing.
    pub fixtures:
        for<'a> fn(
            &'a sea_orm::DatabaseConnection,
            uuid::Uuid,
            bool,
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
    /// middleware.
    pub workspace: SurfaceSeam,
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
    /// Mounted in both serve modes.
    pub external: Router<AppState>,
    /// STAFF CONSOLE. Sections merged inside `/api/admin`, each behind the
    /// capability it names — see [`AdminSection`]. Cloud-only, like `api`:
    /// local mode never mounts the global tree that holds `/admin`.
    pub admin: Vec<AdminSection>,
    /// The OpenAPI operations extracted surfaces document, merged into the
    /// served document (`/apidoc/openapi.json`, which `oxyc schema` reads).
    pub openapi: Vec<utoipa::openapi::OpenApi>,
    /// Not a seam but the route table of everything above — what
    /// `GET /api/_catalog` serves. Generated by `oxy-route-catalog`, which
    /// sits above this crate so that editing a surface crate does not rebuild
    /// `oxy-app` (`internal-docs/domain-boundaries.md` S7). Mounted in cloud
    /// AND local mode, unlike the api seam. Empty by default, which is what
    /// tests building a router without the generated table get.
    pub catalog: crate::server::route_catalog::RouteCatalog,
    /// The custom-app task kinds a node that drains the queue can run, with
    /// their executors. Consumed where the driver builds its
    /// `CustomTaskRegistry` (`serve`'s in-process driver and `oxy worker`).
    pub tasks: Vec<TaskSeam>,
    /// Process-lifetime jobs a serve process starts for custom apps, started
    /// by `api_router`. One-shot per-request spawns stay with their handlers.
    pub background: Vec<BackgroundSeam>,
    /// Singleton work the global driver runs each pass for an extracted surface
    /// (the experiment tick). Installed process-wide by `api_router` and
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
/// and `oxy-server`'s `the_task_kinds_are_exactly_these_eight` the list handed in.
///
/// `custom_app_agent_ask` is queued by every live ask's start, which moved to
/// `oxy-api-custom-apps` with its executor (2026-11-14). It is listed so a
/// process whose seams lack the executor refuses to start, rather than fail
/// every ask as an unknown kind; `agentic-pipeline`'s recovery and the chat
/// executor name the same string.
pub const CUSTOM_APP_TASK_KINDS: [&str; 7] = [
    "app_function",
    "custom_app_staging_migrations",
    "custom_app_sandbox_teardown",
    "custom_app_sandbox_migrations",
    "custom_app_sandbox_oltp",
    "custom_app_env_oltp_branch",
    "custom_app_agent_ask",
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

/// The background jobs a serve process needs for custom apps (usage rows,
/// storage measurement, sandbox expiry, retention). Without the usage drain,
/// page views and events queue until the bounded queue sheds them.
pub const CUSTOM_APP_BACKGROUND_JOBS: [&str; 7] = [
    "custom_app_asset_lifecycle_verify",
    "custom_app_storage_sweeper",
    "custom_app_sandbox_maintenance",
    "custom_app_tracking_retention",
    "custom_app_usage_drain",
    "custom_app_tail_listener",
    "custom_app_function_limits",
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
    /// kind and background job `oxy-app` depends on is present, and the outer
    /// mount is. Called by `serve` before it builds anything.
    pub fn require_custom_apps(&self) -> Result<(), String> {
        require_task_kinds(&self.tasks)?;
        let missing: Vec<&str> = CUSTOM_APP_BACKGROUND_JOBS
            .iter()
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
                .map(|k| task(k))
                .collect(),
            background: CUSTOM_APP_BACKGROUND_JOBS.iter().map(|n| job(n)).collect(),
            outer: Some(|_| Router::new()),
            ..SurfaceSeams::empty()
        }
    }

    /// The persisted task-queue contract: these strings are in
    /// `agentic_task_queue` rows, so a change here strands queued work.
    #[test]
    fn the_required_task_kinds_are_the_seven_the_fleet_knew() {
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
                tick: |_, _| Box::pin(async { 0usize }),
            }
        }
        install_global_ticks(&[tick("first")]);
        install_global_ticks(&[tick("second")]);
        let names: Vec<&str> = global_ticks().iter().map(|t| t.name).collect();
        assert_eq!(names, ["first"]);
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
