// The router-only middlewares (`api_key_query`, `app_grant_scope`,
// `app_publish_token_scope`, `local_context`, `request_id`, `timeout`,
// `token_grant_scope`, `token_usage`) went up into `oxy-server`'s
// `router::middlewares` with the router (decompose LIFT, 2026-11-23); the
// `oxy_owner_guard` and `sentry_surface` shims went with them (import
// `oxy_server_authz::oxy_owner_guard` and `oxy_telemetry::sentry_surface`).
// The re-exports of `workspace_extractor` and of `oxy-app-core::guards`
// (`org_context`, `partner_authz`, `subscription_guard`) had no caller left
// and were deleted in decompose SHM (2026-11-23).

// The workspace middleware went down into `oxy-workspace-runtime::request`
// (decompose W7, 2026-11-22); re-exported at its old path because
// `tests/platform` still names it (`host` and `oxy-server` stopped in decompose
// RPT, 2026-11-24).
pub use oxy_workspace_runtime::request::workspace_context;
