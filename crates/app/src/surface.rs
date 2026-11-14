//! The surface prelude: everything an extracted HTTP surface crate
//! (`crates/api-<context>`) may take from `oxy-app`.
//!
//! Rule S4 in `internal-docs/domain-boundaries.md`. A surface crate imports
//! `oxy-app` through this module only, so `pub` deep inside `oxy-app` stops
//! meaning "some surface happened to need it" and the contract between the
//! platform runtime and the surfaces is one file you can read.
//!
//! What belongs here is **platform runtime** — the things every surface would
//! otherwise reimplement: authz guards and the middleware that resolves org /
//! workspace context, session and request helpers, route-role declarations,
//! staff assume-role. What does **not** belong here is another bounded
//! context's logic (custom apps, tenancy, the operating graph, …). A surface
//! that needs one of those reaches it through a domain crate below `oxy-app`
//! (rule S3); until that crate exists, the import is listed in the backlog of
//! `tests/routing/surface_contract.rs`, which fails on any new one.
//!
//! Add an item here only if it is platform. If it is someone's domain, lower
//! it instead.
//!
//! Not every entry below is platform yet: some are context reaches filed here
//! before their context left. Which entry goes down, up or sideways when
//! `oxy-app` dissolves is the table under "End state" in
//! `domain-boundaries.md`; the modules that go down are `BASE` in
//! `tests/routing/base_boundary.rs`.

/// Role guards — the axum extractors that decide access (`oxy-authz` rings).
/// Take one in a handler instead of deciding by hand (see `crates/authz/CLAUDE.md`).
pub use crate::server::api::middlewares::role_guards;

/// Org-scoped request context: the middleware that resolves `{org_id}`, the
/// extractor it leaves behind, and the same resolution for a route that names
/// its org outside the path (`resolve_org_context`).
pub use crate::server::api::middlewares::org_context::{
    OrgContext, OrgContextExtractor, org_middleware, resolve_org_context,
};

/// Subscription gating for org-scoped routes.
pub use crate::server::api::middlewares::subscription_guard::subscription_guard_middleware;

/// Workspace-scoped request context, as the workspace seam provides it.
pub use crate::server::api::middlewares::workspace_context::{
    EffectiveWorkspaceRole, PreaggCacheCtx, SemanticEngineCacheCtx, SemanticLayerCacheCtx,
    WorkspaceManagerReadOnly, WorkspaceManagerWorkingCopy, WorkspacePath, enqueue_import_compile,
    enqueue_lazy_compile,
};

/// Whether this deployment compiles workspaces from git: the push webhook, the
/// branch check and the first commit compile of a never-compiled workspace all
/// run only with the `compile_reconcile` flag on. A repository-backed
/// workspace with no working copy compiles nothing without it.
pub use crate::server::compile_reconcile::enabled as compiles_from_git;

/// The workspace row the workspace middleware resolved (401 where it did not run).
pub use crate::server::api::middlewares::workspace_extractor::WorkspaceExtractor;

/// The fence that keeps a sandbox agent token (`oxy_sbx_`) to the one shape it
/// may reach on the `/customer-apps` serve tree. Asked before authentication by
/// the serve route, which sits above this crate. Platform authz, not a tenant
/// context: it reads the presented credential's prefix and nothing else.
pub use crate::server::api::middlewares::app_grant_scope::serve_tree_refuses;

/// The project context a workspace's agents run with: its configuration, its
/// database secrets and the compiled revision, built the way the platform builds
/// it for every data-plane call. The document library's answering agent runs
/// against an org's workspace this way; it is not a custom app. The custom-app
/// gate's decisions are in `oxy_custom_apps::gate`; this builder is the part
/// that composes `OxyProjectContext`, which only `oxy-app` can name.
pub use crate::server::api::custom_apps_gates::build_project_context;

/// Session, token and request helpers shared by every sign-in path.
pub mod session {
    pub use crate::server::api::auth::{
        AuthResponse, build_session_cookie_with_max_age, create_auth_token,
        create_auth_token_with_ttl, extract_base_url_from_headers,
        extract_link_base_for_authenticated_request, is_request_secure, session_cookie_user_id,
        token_session_response, validate_return_to_url,
    };
}

/// The CORS origin allowlist, for routes that gate on where a call came from.
pub use crate::server::router::is_allowed_origin;

/// The staff-console guards: the layers a surface stacks on its staff routes
/// (`oxy_owner_or_app_admin_guard` admits an Oxy owner or an app admin;
/// `platform_cap_guard::require(Action)` the capability the section is about;
/// `app_scope_guard` which apps a bounded grant may touch). The surface names the
/// stack; `oxy-app` owns what each layer decides.
pub mod guards {
    pub use crate::server::api::middlewares::oxy_owner_or_app_admin_guard::oxy_owner_or_app_admin_guard_middleware;
    pub use crate::server::api::middlewares::{app_scope_guard, platform_cap_guard};
}

/// The workspace middleware's role resolution: the role an API token's grant
/// caps a caller at in a workspace, as every workspace guard sees it.
pub mod workspace_context {
    pub use crate::server::api::middlewares::workspace_context::resolve_effective_role;
}

/// Pod placement: whether this process holds the working copy, and how a
/// request that needs the Factory is relayed to it. `IdeState` is the router
/// state a working-copy extractor resolves from: a surface that mounts one such
/// handler on its (fleet-typed) seam router runs it on `IdeState` explicitly and
/// declares the route IdeOnly — the two facts `route_ide` states inside `oxy-app`.
pub mod fleet {
    pub use crate::server::router::IdeState;
    pub use crate::server::{ide_proxy, role_middleware, servability};
}

/// The workspace runtime a queued job or a handler rebuilds a workspace from:
/// the project context the global driver builds for a workspace (the promoted
/// revision's semantic model, never a working copy), the workspace manager
/// behind it, the compile-boundary scan root, and the driver's tick interval.
///
/// [`resolve_query_scan_source`](workspace_runtime::resolve_query_scan_source)
/// is the request-time resolution beside it: the compiled revision first, the
/// working copy second where this node holds one, so an IDE branch that has not
/// compiled yet still reads. With it, the `{ message }` envelope the semantic
/// routes answer a refusal with.
pub mod workspace_runtime {
    pub use crate::agentic_wiring::OxyProjectContext;
    pub use crate::agentic_wiring::string_literal::string_literal_of;
    pub use crate::server::api::semantic::{
        ErrorResponse, QueryScanSource, resolve_compiled_scan_source, resolve_query_scan_source,
        semantic_err,
    };
    pub use crate::server::api::typed_stream::typed_stream_to_json_objects;
    pub use crate::server::preagg_workspace::build_workspace_manager;
    pub use crate::server::router::recovery::{
        DEFAULT_INPROC_GLOBAL_WORKER_INTERVAL_SECS, build_workspace_ctx,
    };
}

/// The warehouse query executor the metric-tree surfaces run on, built the way
/// the platform builds it for every metric-tree read: the connector pool, the
/// pre-aggregation short-circuit (derived from the workspace manager, never the
/// caller), and the request-scoped preview write hold. With it, the airlayer
/// database configs a workspace declares, read through `dialect()` so a copy
/// cannot drift on airhouse/motherduck. Platform runtime, not metric-tree
/// logic: the metric-tree computation itself lives in `oxy-metric-monitoring`.
pub mod metric_tree_exec {
    pub use crate::agentic_wiring::metric_tree_runner::{
        OxyMetricTreeRunner, RunnerPreagg, build_query_executor,
    };
    pub use crate::server::api::metric_tree::workspace_databases;
    // What the customer-app twins (`oxy-api-custom-apps`' `data_plane::
    // metric_tree*`) share with the IDE handlers that stay here: the error the
    // projection core answers in, the tree query's shape, the one instance-scope
    // rule (it reads the world model's entities, which `oxy-metric-monitoring`
    // does not depend on), and the projection's executor-facing core.
    pub use crate::server::api::metric_tree::{MetricTreeError, TreeQuery, baseline_scope_core};
    pub use crate::server::api::metric_tree_projection::{
        ProjectionExec, ProjectionOutcome, run_projection,
    };
    /// Whether this request is a held preview — `build_query_executor`'s
    /// `holds_writes`, read on the request's own task.
    pub use oxy_app_core::write_hold::active as request_holds_writes;
}

/// The connector the platform builds for every Dev Portal query (the same
/// builder, subject, role and preview hold), and the two ways a query runs on
/// it. `oxy-api-world-model`'s `WorkspaceWarehouse` — the warehouse the World
/// Model Graph cores (`oxy_world_model::graph`) run on — is built from these,
/// and the IDE handlers' own fan-outs (filter counts, instance detail) use them
/// directly. Platform runtime, not graph logic.
pub mod world_model_exec {
    pub use crate::server::api::data::{
        SQLParams, SemanticQueryResponse, build_connector, run_via_agentic_connector,
        run_with_connector,
    };
}

/// What a custom app's semantic read does before it touches the model, as the
/// data plane does it for every such read (`/projects/{project_id}/semantic/*`):
/// the customer-app gate, the project context, the scan resolved compile
/// boundary first (a stateless replica with nothing compiled answers the
/// `503 semantic_needs_recompile` contract and queues a compile), the
/// project-scoped result cache (`project_id` first, read after the gate,
/// `?refresh` honoured), the `{ message, code }` envelope, and the versioned
/// body parser the bundle SDK's POSTs go through. Platform runtime on the
/// workspace runtime, not any one context's logic: `oxy-api-world-model`'s
/// world-model and cohort twins use it, and so do `oxy-api-custom-apps`'
/// metric-tree twins.
pub mod custom_app_semantic {
    pub use crate::server::api::custom_apps_gates::{
        CustomAppContextExt, build_caller_context, build_project_context_with_fresh_workspace,
        parse_versioned_body,
    };
    pub use crate::server::api::projects::semantic_boundary::{
        SemanticBoundary, cache_lookup, cache_store, enter_semantic_boundary, err, err_with_code,
        load_layer, wants_refresh,
    };
}

/// Carry a request's Sentry hub across an SSE body, which axum polls after the
/// handler (and the layer's `Hub::run`) has returned. The sentry-surface guard
/// pairs every `async_stream::stream!` on the custom-app data plane with one.
pub mod sentry_hub {
    pub use crate::server::api::middlewares::sentry_surface::bind_hub_stream;
}

/// An analytics run started from the queue: the claim-safe attempt every
/// executor of one shares (`queued_analytics`), and who a custom-app ask runs
/// as when something other than its request drives it (`caller`). The chat
/// executor and recovery stay here and use both; a custom-app ask's start,
/// cancel and executor are `oxy-api-custom-apps`'s.
pub mod queued_runs {
    pub use crate::server::queued_analytics::caller;
    pub use crate::server::queued_analytics::{
        INTERRUPTED_MESSAGE, SchemaCache, Settled, admission, attempt, beside_the_pipeline,
        heartbeat,
    };
}

/// The custom-app automation run's row: closed and begun by its state machine
/// (`settle`), a preview-mode run's row settled from its finished preview run,
/// the failure classifier. Stays because the workspace-preview sweep settles in
/// the same tick it finishes the preview run; the handlers, the executor and
/// the sweep are `oxy-api-custom-apps`' `automation_run`.
pub mod automation_runs {
    pub use crate::server::api::projects::automation_run::{
        automation_error_to_code, owns_run, settle, settle_custom_app_runs,
    };
}

/// The S3 mirror of a runtime artifact (a chart image, a query result) written
/// to one pod's disk, so a read on another pod finds it. The Slack bot mirrors
/// the chart PNGs it renders.
pub mod runtime_artifact {
    pub use crate::server::runtime_artifact::{chart_key, mirror};
}

/// Git operations on the workspace root working copy, as the workspace
/// handlers perform them: switching to and deleting a branch.
pub mod workspace_git {
    pub use crate::server::api::workspaces::{git_delete_branch, git_switch_branch};
}

/// The sandbox companies staff registered for a workspace's Airway samples
/// (`workspace_preview_sources`). The fetch plane admits a QuickBooks call
/// only to one of them.
pub mod previews {
    pub use crate::server::previews::namespace::preview_key;
    pub use crate::server::previews::runs::{advance, runs_enabled};
    pub use crate::server::previews::sources::list as list_sources;
    pub use crate::server::previews::sql_kind::StatementKind;
}

/// Project secrets: the email lookup the app-secret listing shares.
pub mod secrets {
    pub use crate::server::api::secrets::resolve_user_emails;
}

/// The Postgres LISTEN/NOTIFY connection settings a surface's own listener
/// rides, so it connects the way the task router does.
pub mod listener {
    pub use crate::server::task_router_listener::{
        listener_factory_from_env, listener_tls_verification_from_env,
    };
}

/// What the demo seed knows that a surface seeding its own rows needs: which
/// workspace the partner seed made for an org, and whether the database is one a
/// seed may touch.
pub mod seed {
    pub use crate::cli::commands::seed::{SeedOptions, clear_demo, seed_demo_with};
    pub use crate::cli::commands::seed_partners::{is_local_db, seeded_workspace_id};
    pub use crate::cli::commands::seed_platform_grants::unscoped_app_operator_email;
}

/// The seams a surface crate builds for the composition root to hand `oxy-app`
/// (`SurfaceSeams`): the task executors, the background jobs, the demo seed's
/// half, and the notifications surface's boot hooks with the task kinds
/// `oxy-app` requires of it.
pub mod seams {
    pub use crate::server::router::{
        BackgroundDeps, BackgroundSeam, ExampleAppsSeam, GlobalTickSeam, HEALTH_TASK_KINDS,
        MigrateReport, NOTIFICATION_TASK_KINDS, NotificationsSeam, TaskDeps, TaskSeam,
    };
}

/// What the integration tests of the surface crates drive: the data plane's
/// handlers and the served router, which the custom-app tests exercise in a
/// staging or sandbox environment, and the platform fixtures they stand up
/// around them. Tests only; no surface's `src` takes anything from here.
pub mod testing {
    pub use crate::agentic_wiring;
    pub use crate::server::api::admin::app_publish_tokens;
    pub use crate::server::api::middlewares::{app_publish_token_scope, workspace_context};
    pub use crate::server::api::{
        billing, custom_apps_gates, projects, schedules, secrets, semantic,
    };
    pub use crate::server::router::{
        SurfaceSeam, SurfaceSeams, api_router, external_auth_layers, flat_api_surface,
    };
    pub use crate::server::{compile_config_gate, compile_maintenance, previews};

    /// The router module itself, for the items tests name beside the ones above.
    pub use crate::server::router;
}

/// Router state for tests that mount a single handler.
pub use crate::server::router::bare_app_state;

/// A staff-console section under `/api/admin`, and the capabilities one may
/// name. `oxy-app` applies the capability guard; the surface only names it.
pub use crate::server::authz::Action;
pub use crate::server::router::AdminSection;

/// Route-role (pod placement) declarations and the classifier that reads them.
pub mod roles {
    pub use crate::server::role_manifest::{
        RouteRole, classify, current_process_role, ensure_fs_writable, init_process_role_from_env,
        install_route_declarations_for_tests_with, role_runs_inprocess_workers,
    };
}

/// Staff assume-role sessions — platform authz, not a tenant context: whether
/// a staff member is acting inside an org right now.
pub use crate::server::api::admin::assume;

/// Staff-scope fencing: whether a bounded platform grant reaches this org.
/// Platform authz, applied inside admin handlers where the org is known.
pub mod admin_scope {
    pub use crate::server::api::admin::org_filter::{scope_org_filter, scope_org_filter_checked};
    pub use crate::server::api::admin::scope::*;
}

/// The admin audit log's row as the console reads it (`GET /admin/audit`), so a
/// surface that lists audit rows (a legacy key's activity) answers with the same
/// wire shape.
pub mod admin_audit {
    pub use crate::server::api::admin::audit::AuditEventDto;
}

/// The admin console's connect-on-demand database handle and its JSON error
/// body (`{ code, message }`), so an extracted admin section answers a failed
/// connect or query exactly the way the in-tree sections do.
pub mod admin_db {
    pub use crate::server::api::admin::internal_jobs::{connect, db_err, error_body};
}

/// Whether Factory access is enforced in an org (the `apps_first_experience` flag),
/// for the staff console's Factory-access tab to say what a grant changes today.
pub mod factory_access {
    pub use crate::server::factory_access::enforced_in;
}
