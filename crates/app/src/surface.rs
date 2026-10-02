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

/// Org-scoped request context: the middleware that resolves `{org_id}` and the
/// extractor it leaves behind.
pub use crate::server::api::middlewares::org_context::{OrgContext, org_middleware};

/// Subscription gating for org-scoped routes.
pub use crate::server::api::middlewares::subscription_guard::subscription_guard_middleware;

/// Workspace-scoped request context, as the workspace seam provides it.
pub use crate::server::api::middlewares::workspace_context::{
    WorkspaceManagerReadOnly, WorkspaceManagerWorkingCopy, WorkspacePath, enqueue_lazy_compile,
};

/// Session, token and request helpers shared by every sign-in path.
pub mod session {
    pub use crate::server::api::auth::{
        build_session_cookie_with_max_age, create_auth_token, create_auth_token_with_ttl,
        extract_base_url_from_headers, is_request_secure, session_cookie_user_id,
        validate_return_to_url,
    };
}

/// The CORS origin allowlist, for routes that gate on where a call came from.
pub use crate::server::router::is_allowed_origin;

/// Router state for tests that mount a single handler.
pub use crate::server::router::bare_app_state;

/// Route-role (pod placement) declarations and the classifier that reads them.
pub mod roles {
    pub use crate::server::role_manifest::{
        RouteRole, classify, ensure_fs_writable, install_route_declarations_for_tests_with,
    };
}

/// Staff assume-role sessions — platform authz, not a tenant context: whether
/// a staff member is acting inside an org right now.
pub use crate::server::api::admin::assume;
