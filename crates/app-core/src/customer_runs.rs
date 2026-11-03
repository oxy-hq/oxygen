//! Which `agentic_runs` rows are a customer's own work, as a SQL predicate.
//!
//! Workspace Health and `ctx.platform.facts()` count runs with
//! [`exclude_daemon_runs`], so the two cannot disagree on what a customer run is.

/// Run `source_type`s the health signal must ignore: platform daemon runs the
/// workspace never asked for.
///
/// `health_eval_workspace` above all — those rows are seeded by *this very
/// health check* (`start_health_eval_run` inserts one `agentic_runs` row per
/// workspace per `health_check.interval`, default 1h). Counting them makes the
/// check grade itself: an idle workspace's entire denominator is health runs, so
/// a broken eval task reads as "23/24 runs failed (96%)" on every tenant
/// simultaneously — including empty demo/test orgs that ran nothing at all.
///
/// The test is *whose failure it is*, not whether a human pressed a button.
/// A `health_eval_workspace` or `preagg_cycle` failure is a platform problem —
/// it says nothing about the tenant, and both fire on a timer, so they also
/// reach workspaces with no content at all. Other auto-generated runs are
/// deliberately **kept** counted, because their failure *is* a workspace fault
/// and they only exist where the workspace created the work:
/// - `compile` (auto/lazy compile on workspace-context load) fails when the
///   tenant's own `config.yml` / YAML is broken — exactly what an unhealthy
///   workspace looks like.
/// - `monitor_scan` only runs if the workspace defined a `.monitor.yml`, and
///   fails on that monitor's definition or warehouse.
/// - `anomaly_notify` (Insights delivery) only runs if that file has a
///   `notify:` block and a scan found something to announce, and fails on the
///   block's channel or the org's Slack connection.
///
/// Deliberately kept local instead of folded into `agentic_runtime`'s
/// `SYSTEM_SOURCE_TYPES`: that list also drives the coordinator run feed and the
/// per-job "Recent runs" history, where scheduled health fires are meant to stay
/// visible (see `start_health_eval_run`'s `schedule_id` stamping). A new daemon
/// `source_type` therefore has to be weighed against **both** lists.
///
/// `preview_analyze` is staff checking a *branch* (workspace previews),
/// `preview_schema_drop` the TTL drop of a preview's schemas, and
/// `preview_compare` a preview build's compare with live, `preview_airway_sample`
/// staff sampling a branch's pipeline: no such failure says anything about the
/// workspace production serves. Nor does `custom_app_staging_migrations`, a
/// custom app's staging homes migrated after a publish, nor a sandbox's
/// (`custom_app_sandbox_migrations`, `custom_app_sandbox_oltp`,
/// `custom_app_env_oltp_branch`) or its teardown
/// (`custom_app_sandbox_teardown`): staff trying a build, not the tenant's work.
/// Nor does `custom_app_usage_backfill`, the platform copying a workspace's usage
/// history to its Airhouse.
const NON_WORKSPACE_RUN_SOURCES: &[&str] = &[
    "health_eval_workspace",
    "preagg_cycle",
    "preview_analyze",
    "preview_schema_drop",
    "preview_compare",
    "preview_airway_sample",
    "custom_app_staging_migrations",
    "custom_app_sandbox_migrations",
    "custom_app_sandbox_oltp",
    "custom_app_env_oltp_branch",
    "custom_app_sandbox_teardown",
    "custom_app_usage_backfill",
];

/// SQL predicate excluding [`NON_WORKSPACE_RUN_SOURCES`], for a table aliased
/// `alias` (empty for an unaliased `agentic_runs`).
///
/// Rows with a NULL `source_type` are explicitly kept: a bare `NOT IN` evaluates
/// to NULL for them, which the WHERE clause treats as false and would silently
/// drop legacy runs out of the denominator.
///
/// Two kinds of staff run cannot be told apart by `source_type` — a held
/// preview procedure run is an ordinary `workflow` run, and a custom-app check
/// run queued in `staging` or a sandbox an ordinary `app_function` run — so
/// they are told apart by what their seeder stamped on `metadata`. A staffer's
/// dry run of a branch, or check of an unpromoted build, failing says nothing
/// about the workspace production serves. That rule is not restated here: it
/// is `agentic_runtime`'s `customer_run_sql`, the predicate the coordinator
/// run feed hides the same runs with, so the two cannot drift. A production
/// `app_function` run carries no environment and is counted as before.
///
/// `agentic_runs` has no supporting index at all for the `created_at` window
/// every caller here filters on (PR #3381 review nit 3), so every one of
/// these queries is already an unbounded scan *before* this predicate — a
/// pre-existing condition, out of this predicate's scope to fix. Against
/// that baseline, the `metadata` predicate is a cheap residual: two more
/// per-row JSONB reads on a scan that was already reading every row in the
/// window. It stops being cheap the day
/// `agentic_runs` gets a `created_at` index and this predicate is the thing
/// keeping it non-sargable — see the follow-up in
/// `internal-docs/workspace-previews.md`.
pub fn exclude_daemon_runs(alias: &str) -> String {
    let col = format!("{alias}source_type");
    let list = NON_WORKSPACE_RUN_SOURCES
        .iter()
        .map(|s| format!("'{s}'"))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        " AND ({col} IS NULL OR {col} NOT IN ({list})) AND {}",
        agentic_runtime::crud::customer_run_sql(alias)
    )
}
