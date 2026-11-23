pub mod admin;
/// The session plumbing every sign-in path shares moved down to `oxy-app-core`
/// (2026-11-21); the old path stays so no call site changes.
pub use oxy_app_core::session as auth;
// `agent` (the agent listing) and `artifacts` moved to
// `oxy-api-threads` (2026-11-16), mounted on the workspace seam.
// `apps` (the app integrations `config.yml` declares, `/app-integrations`)
// moved to `oxy-api-workspaces` (2026-11-16), mounted on the workspace seam.
// `automation` (the compile-boundary automation listing) and `schedules`
// moved to the `oxy-api-automations` sibling crate, mounted by `oxy-server` on
// the workspace seam. oxy-app no longer depends on them.
pub mod billing;
// `catalog` (`GET /_catalog`) and `healthcheck` (`/health`, `/ready`, `/live`,
// `/version`) moved UP into `oxy-server` (`core_routes`, 2026-11-16), mounted
// through `SurfaceSeams::catalog` and `::public`.
// `chart` (a legacy chart file) moved to `oxy-api-threads` (2026-11-16).
// `compile` (the compile task enqueue) moved to `compile_request::enqueue_task`
// (2026-11-21), beside its one caller.
// `compile_staging` (`/compile/staging` and its status) moved to
// `oxy-api-previews` (2026-11-16), mounted on the workspace seam.
/// Moved to `oxy-app-core`; the old path stays so no call site changes.
pub use oxy_app_core::agent_body as custom_apps_agent_body;
#[cfg(test)]
pub(crate) use oxy_app_core::agent_fixture as custom_apps_agent_fixture;
pub use oxy_app_core::compiled_reader;
/// Moved to the `oxy-custom-apps` domain crate; the old path stays so no call site changes.
pub use oxy_custom_apps::asset_manifest as custom_apps_asset_manifest;
pub use oxy_custom_apps::build_state as custom_apps_build_state;
/// Moved to the `oxy-custom-apps` domain crate; the old path stays so no call site changes.
pub use oxy_custom_apps::build_store as custom_apps_build_store;
/// Moved to the `oxy-custom-apps` domain crate; the old path stays so no call site changes.
pub use oxy_custom_apps::bundle_cache as custom_apps_bundle_cache;
pub use oxy_custom_apps::cache as custom_apps_cache;
pub use oxy_custom_apps::env_resolve as custom_apps_env_resolve;
/// Moved to the `oxy-custom-apps` domain crate; the old path stays so no call site changes.
pub use oxy_custom_apps::environments as custom_apps_environments;
/// Moved to the `oxy-custom-apps` domain crate; the old path stays so no call site changes.
pub use oxy_custom_apps::manifest as custom_apps_manifest;
pub use oxy_custom_apps::non_production as custom_apps_non_production;
/// Moved to the `oxy-custom-apps` domain crate; the old path stays so no call site changes.
pub use oxy_custom_apps::precompress as custom_apps_precompress;
/// Moved to the `oxy-custom-apps` domain crate; the old path stays so no call site changes.
pub use oxy_custom_apps::publish_authz as custom_apps_publish_authz;
/// Moved to the `oxy-custom-apps` domain crate; the old path stays so no call site changes.
pub use oxy_custom_apps::sandbox_instance as custom_apps_sandbox_instance;
/// Moved to the `oxy-custom-apps` domain crate; the old path stays so no call site changes.
pub use oxy_custom_apps::source as custom_apps_source;
/// Moved to the `oxy-custom-apps` domain crate; the old path stays so no call site changes.
pub use oxy_custom_apps::storage as custom_apps_storage;
/// Moved to the `oxy-custom-apps` domain crate; the old path stays so no call site changes.
pub use oxy_custom_apps::sync as custom_apps_sync;
/// Moved to the `oxy-custom-apps` domain crate; the old path stays so no call site changes.
pub use oxy_custom_apps::telemetry as custom_apps_telemetry;
/// Moved to the `oxy-custom-apps` domain crate; the old path stays so no call site changes.
pub use oxy_custom_apps::tracking as custom_apps_tracking;
/// Moved to the `oxy-custom-apps` domain crate; the old path stays so no call site changes.
pub use oxy_custom_apps::updates as custom_apps_updates;
/// Moved to the `oxy-custom-apps` domain crate; the old path stays so no call site changes.
pub use oxy_custom_apps::usage_airhouse as custom_apps_usage_airhouse;
/// Moved to the `oxy-custom-apps` domain crate; the old path stays so no call site changes.
pub use oxy_custom_apps::usage_queue as custom_apps_usage_queue;
/// The data plane's gate and project-context composition root, down to
/// `oxy-workspace-runtime` on 2026-11-22 (W6a); the old path stays so no call
/// site changes.
pub use oxy_workspace_runtime::custom_apps_gates;
// `data` (the Dev Portal's `/sql` execute) moved to `oxy-api-semantic`
// (2026-11-16), mounted on the workspace and external seams.
// `database` (Workspace Settings → Databases, the schema and inspection
// routes) and `secrets` (the project secrets) moved to the
// `oxy-api-workspace-config` sibling crate, mounted by `oxy-server` on the
// workspace seam. oxy-app no longer depends on them.
// The document-library HTTP surface moved to the `oxy-api-documents` sibling
// crate, mounted by `oxy-server`. oxy-app no longer depends on it.
// The GitHub HTTP surface moved to the `oxy-api-github` sibling crate, mounted by
// `oxy-server`. oxy-app no longer depends on it.
// `chat` (org chat: channels, messages and the cross-replica delivery
// listener) moved to `oxy-api-threads` (2026-11-16), mounted on the api seam,
// its listener started through `SurfaceSeams::background`.
// `integration` (the Looker listing, `/integrations/looker`) moved to
// `oxy-api-workspace-config` (2026-11-16), mounted on the workspace seam.
pub mod local_setup;
// Lowered into the domain crate; re-exported so every `server::api::` path holds.
pub use oxy_metric_monitoring::{metric_tree_baseline, metric_tree_groups, metric_tree_inputs};
pub mod middlewares;
// `org_teams` (the cache-flushing `write_access` wrapper, the last of it) went
// down into `oxy-custom-apps` as `app_access`; the handlers are
// `oxy-api-tenancy`'s and the service `oxy-tenancy`'s.
// `preagg` (the pre-aggregation status and rebuild) moved to
// `oxy-api-semantic` (2026-11-16), mounted on the workspace seam.
pub mod projects;
// `result_files` (a run's result parquet) moved to `oxy-api-threads`
// (2026-11-16), mounted on the workspace seam.
// `semantic` (the semantic explorer's view, topic, compile and execute) and
// `semantic_scan` (the semantic object list, the Context Graph's sources and
// the compile boundary's revision reads) moved to `oxy-api-semantic`
// (2026-11-16), mounted on the workspace seam (and the external one for
// compile and execute).
// `thread` (a workspace's threads and the run logs) moved to
// `oxy-api-threads` (2026-11-16), mounted on the workspace seam.
// The tenant-facing observability console (`traces`, `metrics`,
// `execution_analytics`) moved to the `oxy-api-observability` sibling crate,
// mounted by `oxy-server` on the workspace seam. oxy-app no longer depends on it.
// `video` (the UniFi camera list and the weather tiles and readings) moved to
// the `oxy-api-world-model` sibling crate, mounted by `oxy-server` on the
// workspace and external seams. oxy-app no longer depends on it.
// `org_subdomain`, `workspace_logo` and `partner_publish_consent` moved to
// `oxy-api-tenancy` (2026-11-16), mounted by `oxy-server` on the workspace
// and api seams.
// `workspace_members` (a workspace's members and role overrides),
// `workspace_oxy_access` (the Oxy-staff lockdown) and `workspaces` (an org's
// workspace list, rename and delete, and a workspace's `/meta`) moved to
// `oxy-api-workspaces` (2026-11-16), mounted by `oxy-server` on the workspace
// and api seams.
pub mod workspace_org;
