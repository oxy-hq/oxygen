//! The custom-app data plane's query routes: what a bundle's `ctx`-less reads
//! go through from the browser, same-origin.
//!
//! - `POST /api/projects/{project_id}/query` ([`query::run_query`]) — SQL (or
//!   a semantic query, by shape) against one of the project's databases.
//! - `POST /api/projects/{project_id}/semantic-query`
//!   ([`semantic_query::run_semantic_query`]) — a semantic query compiled from
//!   the promoted revision's model.
//!
//! Both sit behind the customer-app gate (`check_custom_app_gates`, inline: a
//! session cookie or an API key), so they ride `SurfaceSeams::public` outside
//! the auth stack, declared FleetOk. Their result cache is
//! `oxy_custom_apps::result_cache`, keyed `project_id` first and read after the
//! gate, shared with the semantic boundary the metric-tree twins in `oxy-app`
//! go through. [`query::DataPlaneQueryExecutor`] is the same SQL path behind
//! `ctx.warehouse` / `ctx.semantic` in a Function; `oxy-server` installs it on
//! the function runtime.
//!
//! **Sentry.** These modules' paths contain `custom_apps`, so barrier 1 and
//! `drop_log` (`oxy_telemetry::sentry_filter`) keep every line they write out
//! of every Sentry surface, Logs included. In `oxy-app` they were shaped (level
//! and location, no text) instead; the drop was chosen when they moved
//! (2026-11-13), and `sentry_filter`'s tests pin it on this path. The full
//! lines reach the platform logs; the warehouse connector's own lines
//! (`agentic_connector`) are still shaped in Sentry Logs.

pub mod query;
mod query_params;
pub mod semantic_query;
