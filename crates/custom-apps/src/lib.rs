//! Custom Apps domain logic: the modules of the custom-apps surface that need
//! nothing from `oxy-app` and no web framework (the serve resolver takes the
//! request's `http` headers and answers with an `http` status, nothing more).
//!
//! A domain crate in the sense of `internal-docs/domain-boundaries.md` rule S3:
//! it sits below `oxy-app` and every `api-*` surface, so a surface that needs
//! custom-app behavior imports it from here rather than reaching into `oxy-app`.
//! It holds no routes and no handlers. Built as pure relocations
//! (S6 step 3); `oxy-app` keeps each module reachable at its old
//! `server::api::custom_apps_*` path through a re-export.
//!
//! - `auth`: who may open an app, app and org roles, the access verdict cache.
//! - `cache`: the bounded TTL caches of the serve chain and their invalidation.
//! - `env_resolve`: which build an app environment serves.
//! - `non_production`: may this caller open a non-production environment.
//! - `agent_built`: the mark of a build a sandbox agent token published.
//! - `audience`: who can reach an app (the rule `ctx.org.*` and `ctx.notify` share).
//! - `function_trigger`: why a one-off function job was queued, and what the queue reads back.
//! - `build_state`: what a build row says about whether it may go live.
//! - `environments`, `publish_authz`: pointer writes and publish authority.
//! - `staging_pin`: a draft build's semantic pin: read it, validate it at publish,
//!   resolve it for a non-production request, and the promote drift notice.
//! - `resolve`, `authenticate`, `agent`: the one cached chain every app-keyed
//!   request runs (credential, user, app rows, access verdict), the flow handlers
//!   call over it, and the admission rules for a sandbox agent token.
//! - `storage`: the per-app S3 silo behind `ctx.storage` (quota, metering,
//!   retention, the sweeper).
//! - `build_store`, `bundle_cache`, `precompress`, `asset_manifest`: where a
//!   published build's files live, the byte cache in front of them, their
//!   build-time brotli copies and the manifest the service worker reads.
//! - `manifest`, `source`, `sync`: `oxy-app.json`, where an app's source lives,
//!   and the channel a request reads it from.
//! - `telemetry`, `sandbox_instance`: the serve path's metrics, and which
//!   instance of a sandbox a log line belongs to.
//! - `tracking`, `usage_queue`, `usage_airhouse`: page views and custom events,
//!   the bounded queue they are `try_send`-ed onto and the sink that drains it.
//! - `usage_backfill`: copies a workspace's usage history from Postgres into its
//!   Airhouse, one control-plane row per workspace.
//! - `usage_reads`: the Activity tab's and the weekly report's reads of usage,
//!   from a workspace's Airhouse once its backfill finished and from Postgres
//!   until then.
//! - `gate`: the data-plane gate's decision half (user, workspace row, access
//!   verdict) and the context it resolves; `oxy-app` keeps the request-facing entry.
//! - `call_trace`, `call_trace_store`: every `ctx.*` call of one function
//!   invocation as a timed record (the console's waterfall), and its table.
//! - `custom_domains`: customer-owned hostnames for an app — claim, DNS proof,
//!   removal — and the sign-in handoff that gives a browser a session on one.
//! - `updates`: how a long-open tab picks up a new build (the platform mode,
//!   an app's `autoUpdate`, the `/fn` build handshake headers).

// The usage sink's chained connector futures (`usage_airhouse`) exceed the
// default type-layout query depth of 128, as the same code did in `oxy-app`.
#![recursion_limit = "256"]

pub mod agent;
pub mod agent_built;
pub mod asset_manifest;
pub use oxy_app_core::audience;
pub mod auth;
pub mod authenticate;
pub mod build_state;
pub mod build_store;
pub mod bundle_cache;
pub mod cache;
pub mod call_trace;
pub mod call_trace_store;
pub mod custom_domains;
pub mod env_resolve;
pub mod environments;
pub mod function_trigger;
pub mod gate;
pub mod github_oidc_keys;
pub mod manifest;
pub mod non_production;
pub mod org_directory;
pub mod precompress;
pub mod publish_authz;
pub mod reach;
pub mod resolve;
pub mod sandbox_instance;
pub mod secret_audit;
pub mod secret_scope;
pub mod source;
pub mod staging_held;
pub mod staging_pin;
pub mod storage;
pub mod sync;
pub mod telemetry;
pub mod thread_source;
pub mod tracking;
pub mod updates;
pub mod usage_airhouse;
pub mod usage_backfill;
#[cfg(any(test, feature = "test-fixtures"))]
pub mod usage_fixtures;
pub mod usage_queue;
pub mod usage_reads;
pub mod usage_rows;
pub mod usage_writes;
pub mod write_record;
