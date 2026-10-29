//! Shared seam between the `oxy-app` HTTP layer and the crates split out of it.
//!
//! Everything here was previously a module of `oxy-app`. It lives in its own
//! crate so the per-surface crates (admin, integrations, custom apps, …) can
//! depend on the pieces they share *without* depending on each other — that is
//! what lets their frontends compile in parallel instead of serially inside one
//! 140k-line crate.
//!
//! Nothing in here may depend on a per-surface crate. If something in this crate
//! needs a surface, the dependency is pointing the wrong way.

pub mod agent_body;
/// A sandbox-agent credential and its request, for tests. Behind `test-fixtures`
/// (enabled from the dev-dependencies of the crates whose tests use it) so a
/// production build does not ship constructors for an admin-ceiling credential.
#[cfg(any(test, feature = "test-fixtures"))]
pub mod agent_fixture;
pub mod agent_refusal;
pub mod app_lifecycle;
pub mod app_state;
pub mod audience;
pub mod audit;
pub mod compiled_reader;
pub mod custom_app_env_request;
pub mod custom_app_environment;
pub mod custom_apps_host_dispatch;
pub mod custom_domain_host;
pub mod customer_runs;
pub mod default_branch;
pub mod forwarded;
pub mod function_answer;
pub mod function_seam;
pub mod gate_response;
pub mod http_cache;
pub mod internal_orgs;
pub mod llm_usage;
pub mod member_authz;
pub mod notifications;
pub mod operating_graph;
pub mod org_host_dispatch;
pub mod origin;
pub mod pagination;
pub mod preagg_cache_ctx;
pub mod preagg_context;
pub mod process_role;
pub mod semantic_limit;
pub mod serve_mode;
pub mod sql_kind;
pub mod staging_pin_scope;
pub mod ttl_lru;
pub mod usage_period;
pub mod usage_probe;
pub mod widgets;
pub mod workspace_cache;
pub mod workspace_org;
pub mod workspace_repo_facts;

pub use app_state::AppState;
