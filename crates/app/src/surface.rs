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
    WorkspaceManagerReadOnly, WorkspaceManagerWorkingCopy, WorkspacePath, enqueue_lazy_compile,
};

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
        build_session_cookie_with_max_age, create_auth_token, create_auth_token_with_ttl,
        extract_base_url_from_headers, extract_link_base_for_authenticated_request,
        is_request_secure, session_cookie_user_id, validate_return_to_url,
    };
}

/// Platform notices a surface raises when something it drives happens — the
/// notifications service is platform, whichever context the event belongs to.
pub mod notices {
    pub use crate::server::api::notifications::platform::workspace_ready::notify_workspace_ready;
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
/// request that needs the Factory is relayed to it.
pub mod fleet {
    pub use crate::server::{ide_proxy, role_middleware, serve_safety};
}

/// The notification service's parts the staff console's "Test notifications"
/// sends through: the same send path `ctx.notify` uses, the live catalogue and
/// the per-caller cooldown.
pub mod notifications {
    pub use crate::server::api::notifications::catalog::live;
    pub use crate::server::api::notifications::catalog::source::SourceCatalogue;
    pub use crate::server::api::notifications::platform::self_test::{COOLDOWN, cooldown_left};
    pub use crate::server::api::notifications::send::{
        self, AppRef, Outgoing, SendError, key_hash,
    };
}

/// The token lifecycle audit a sandbox's teardown writes when a sandbox agent
/// token ended (`token.expired_sandboxes_queued`).
pub mod token_audit {
    pub use crate::server::api::user_tokens::sandboxes_queued;
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
/// (`SurfaceSeams`): the task executors, the background jobs and the demo seed's
/// half.
pub mod seams {
    pub use crate::server::router::{
        BackgroundDeps, BackgroundSeam, ExampleAppsSeam, MigrateReport, TaskDeps, TaskSeam,
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
        custom_apps_gates, notifications, projects, schedules, secrets, semantic, world_model_graph,
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
        install_route_declarations_for_tests_with,
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

/// Transactional email delivery (SES, or the local preview provider).
pub use crate::emails;
