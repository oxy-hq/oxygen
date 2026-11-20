//! Usage reporting's domain logic (`internal-docs/domain-boundaries.md` S3,
//! S9): below `oxy-app` and every `api-*` surface, above `oxy-app-core`. No
//! routes, no entities, no migrator.
//!
//! The weekly custom-app usage report's pure half, out of `oxy-app`'s
//! `server::api::admin::usage_report`; the weekly job, the delivery, the
//! recipients and the routes are the surface's, `oxy-api-billing` (admin split
//! PR 8b). Operator notes: `internal-docs/custom-app-usage-report.md`.
//!
//! - [`period`] names the week; [`collect`] reads it out of Postgres (or the
//!   workspace's Airhouse) into a [`model::Snapshot`]; [`store`] keeps it.
//! - [`highlights`] is the judgement — what counts as quiet, falling, failing,
//!   growing — and is pure, so it is where the rules are tested.
//! - [`email`] writes the mail; [`view`] is the console's read of a report.
//!
//! And two readers' shared rules, out of `oxy-app-core`'s charter backlog:
//!
//! - [`customer_runs`]: which `agentic_runs` rows are a customer's own work,
//!   for workspace health and `ctx.platform.facts()`.
//! - [`llm_usage`]: a run's token columns and the rate table that prices them,
//!   for Admin → Metrics and `ctx.platform.facts()`.

pub mod collect;
pub mod customer_runs;
pub mod email;
pub mod highlights;
pub mod llm_usage;
pub mod model;
pub mod period;
pub mod store;
pub mod view;
