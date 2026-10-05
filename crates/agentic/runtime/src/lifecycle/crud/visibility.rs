//! Which runs a workspace's own members are shown.
//!
//! A run row belongs to a workspace, but three kinds of run in it are Oxy
//! staff's work rather than the customer's, and each is told apart by what
//! its seeder stamped on `metadata`:
//!
//! - a **workspace preview's dry run** — a staffer running an unmerged
//!   branch — carries `trigger = "preview"`;
//! - a **preview-pinned run** — an agent ask from a custom app's staging
//!   host, or any run started on a request pinned to a workspace preview —
//!   carries [`PREVIEW_STAMP_KEY`] (agentic-pipeline's `preview_stamp`);
//! - a **custom-app check run outside production** — a staffer running a
//!   function in `staging` or a sandbox — carries [`RUN_ENVIRONMENT_KEY`]
//!   naming that environment.
//!
//! Each rule is stated once, here, as SQL over `agentic_runs.metadata`; the
//! queries compose them rather than restating them. All keep a run with no
//! metadata, or without the key: an untagged run is the customer's.
//!
//! | Read | Preview dry run | Preview-pinned run | Non-production check run |
//! | ---- | --------------- | ------------------ | ------------------------ |
//! | the run feed ([`in_customer_feed`]) | hidden | hidden | hidden |
//! | what counts as the workspace's own work ([`customer_run_sql`]) | not counted | not counted | not counted |
//! | anything else a member reads ([`in_production`]) | shown | shown | hidden |
//!
//! The first two rows are one predicate, [`customer_run_sql`]: the feed and
//! `oxy-app`'s workspace health (a failed staff run says nothing about the
//! workspace production serves) read the same text, so they cannot drift.
//!
//! The second row is narrower on purpose. A preview's dry run is watched
//! through the workspace's own run routes by the staffer who started it, so
//! by id it stays readable; so is a preview-pinned run, whose stream and
//! cancel are the workspace's own ask routes. That is why the pinned run is
//! told apart by its stamp and not given an `environment`: that key would
//! hide it from those routes too. A non-production check run is read back through
//! the app's staff routes (`crud::get_run`, unfiltered) and never these.

use sea_orm::sea_query::{Expr, SimpleExpr};
use sea_orm::{ColumnTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter};
use uuid::Uuid;

use crate::lifecycle::entity::run;

/// The `metadata` key naming the custom-app environment a run was queued in
/// (`staging`, `dev-<handle>`). Absent on every production run: the seeder
/// stamps it only for a named non-production environment.
pub const RUN_ENVIRONMENT_KEY: &str = "environment";

/// The `metadata` key a run started on a preview-pinned request carries —
/// `{"workspace_preview": {"revision_id": …}}`, plus `"app_id"` for a custom
/// app's staging ask. Agentic-pipeline's `preview_stamp` writes it under
/// this name (its `RUN_STAMP` is this constant).
pub const PREVIEW_STAMP_KEY: &str = "workspace_preview";

/// What [`RUN_ENVIRONMENT_KEY`] would say for production, were it stamped —
/// so a row that names production outright is still the customer's.
const PRODUCTION_ENVIRONMENT: &str = "production";

/// SQL for "not queued in a non-production app environment", for a
/// hand-written statement over `agentic_runs`. `alias` qualifies the column
/// where the statement joins the table (`"r."`), and is empty otherwise; it
/// is the calling code's own text, never a request's.
fn in_production_sql_for(alias: &str) -> String {
    format!(
        "COALESCE({alias}metadata->>'{RUN_ENVIRONMENT_KEY}', '{PRODUCTION_ENVIRONMENT}') = \
         '{PRODUCTION_ENVIRONMENT}'"
    )
}

/// [`in_production_sql_for`] an unaliased `agentic_runs`.
pub(super) fn in_production_sql() -> String {
    in_production_sql_for("")
}

/// SQL for "this run is the customer's own work": not a preview's dry run,
/// not a preview-pinned run, and not a check run queued outside production.
/// `alias` as in [`in_production_sql_for`].
///
/// `IS DISTINCT FROM` keeps runs with no metadata (a bare `<>` would drop
/// them), and so do the `IS NULL` and the `COALESCE`: an untagged run is the
/// customer's.
pub fn customer_run_sql(alias: &str) -> String {
    format!(
        "({alias}metadata->>'trigger' IS DISTINCT FROM 'preview') AND \
         ({alias}metadata->'{PREVIEW_STAMP_KEY}' IS NULL) AND ({})",
        in_production_sql_for(alias)
    )
}

/// Keeps a non-production check run out of what a workspace's members read.
///
/// It is an ordinary `app_function` run (other code keys on that
/// `source_type`), so it cannot be hidden by source type: a production
/// `app_function` run — a schedule, a webhook, Run now — carries no
/// environment and stays visible exactly as before.
pub(super) fn in_production() -> SimpleExpr {
    Expr::cust(in_production_sql())
}

/// Keeps staff's runs out of the customer's run feed: a preview's dry run, a
/// preview-pinned run, and a non-production check run.
///
/// Unlike `SYSTEM_SOURCE_TYPES` they are hidden even with `include_system`:
/// that toggle is the customer's own.
pub(super) fn in_customer_feed() -> SimpleExpr {
    Expr::cust(customer_run_sql(""))
}

/// The run `run_id`, when it is `workspace_id`'s and its members may read it.
///
/// `None` for a run that does not exist, one in another workspace, and one
/// queued in a non-production app environment — the three are one answer, so
/// a caller holding an id learns nothing from asking.
pub async fn get_run_in_workspace(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    run_id: &str,
) -> Result<Option<run::Model>, DbErr> {
    run::Entity::find_by_id(run_id.to_string())
        .filter(run::Column::WorkspaceId.eq(workspace_id))
        .filter(in_production())
        .one(db)
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The key is the one the seeder writes, and production is the default a
    /// run without it reads as.
    #[test]
    fn a_run_without_an_environment_reads_as_production() {
        assert_eq!(
            in_production_sql(),
            "COALESCE(metadata->>'environment', 'production') = 'production'"
        );
    }

    /// A read by id hides the check run and nothing else: both preview rules
    /// are the feed's alone, so a staging ask's stream and cancel find it.
    #[test]
    fn the_preview_rule_belongs_to_the_feed_only() {
        assert!(!in_production_sql().contains("preview"));
        assert!(!in_production_sql().contains(PREVIEW_STAMP_KEY));
    }

    /// The customer's-own-work rule is both staff rules, and an alias
    /// qualifies every column it reads — a joined statement must not pick up
    /// another table's `metadata`.
    #[test]
    fn the_customers_own_work_is_neither_staff_run_under_any_alias() {
        assert_eq!(
            customer_run_sql(""),
            "(metadata->>'trigger' IS DISTINCT FROM 'preview') AND \
             (metadata->'workspace_preview' IS NULL) AND \
             (COALESCE(metadata->>'environment', 'production') = 'production')"
        );
        assert_eq!(
            customer_run_sql("r."),
            "(r.metadata->>'trigger' IS DISTINCT FROM 'preview') AND \
             (r.metadata->'workspace_preview' IS NULL) AND \
             (COALESCE(r.metadata->>'environment', 'production') = 'production')"
        );
    }
}
