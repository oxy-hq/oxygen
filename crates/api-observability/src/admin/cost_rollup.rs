//! The SQL behind Admin → Observability's Cost panel: one org's LLM token
//! rollup over a window, grouped five ways.
//!
//! Design: `internal-docs/2026-11-16-admin-observability-design.md` §4.
//!
//! **Why five groupings and not one number.** `/admin/metrics/orgs/{id}/llm-usage`
//! already answers "how many dollars" and a daily series. It cannot answer
//! *why* the number is what it is, and that is the only question a cost figure
//! is useful for. The dimensions here are the ones already stamped on
//! `agentic_runs` and read by nothing:
//!
//! - **model** — a model-mix shift is the most common cause of a cost jump;
//! - **workspace** — which deployment inside the org;
//! - **surface** (`source_type`) — chat vs automation vs ELT vs builder;
//! - **attendance** — the one that matters most. Unattended spend grows
//!   silently: a nightly schedule costs the same whether or not anyone reads
//!   its answer, while a human-driven spike is self-limiting.
//!
//! **Attendance is not just `schedule_id IS NOT NULL`.** Pressing "Run now" on
//! an automation goes through `fire_schedule(.., "manual")`, which stamps the
//! run with the schedule's id like any tick does — so the obvious predicate
//! reports a person who clicked a button as an unattended nightly job, and
//! inflates the one number this panel leads with. The trigger is persisted as
//! `metadata->>'trigger'` (`scheduled` / `backfill` / `manual`), so the rule
//! is "a schedule seeded it **and** nobody fired it by hand".
//!
//! **Pricing is not here.** Every row carries tokens and a model; dollars come
//! from `oxy_usage_report::llm_usage::price`, which is the same rate table
//! `/admin/metrics` and `ctx.platform.facts()` use. Three surfaces pricing a
//! run three ways is the failure that sharing a table avoids.
//!
//! **Every statement is built by a pure `*_sql()` function**, so the properties
//! the tests hold — the org fence is bound, `model` survives every grouping,
//! each run collapses to one row — are checked against the SQL that actually
//! runs rather than a restatement of it.

use sea_orm::{DatabaseBackend, DbErr, FromQueryResult, Statement};
use uuid::Uuid;

/// One run's model and token sums — the columns every rollup below prices.
///
/// Taken from `oxy-usage-report` rather than restated, so this surface and
/// `/admin/metrics` cannot disagree about what a run's tokens *are* before they
/// disagree about what they cost.
use oxy_usage_report::llm_usage::RUN_TOKENS;

/// Per-run token rollup for one org, narrowed by a `workspaces.org_id` join.
///
/// Correct for **any** tenant, which the cross-tenant overview's `by_org` is
/// not: that list is a top-N leaderboard, so an org outside it reads as zero.
///
/// `$1` = days, `$2` = org id. The run's own dimensions ride along so the
/// groupings below need no second join: `workspace_id`, `source_type` and
/// whether a schedule seeded it.
///
/// `GROUP BY ar.id` is load-bearing: the `llm_start`/`llm_end` join multiplies
/// a run by its event count, so without the collapse every token sum downstream
/// is inflated by however many LLM calls the run made.
fn org_run_usage_cte() -> String {
    org_run_usage_cte_for(Period::Current)
}

/// The run CTE over either window. The prior one feeds the console's
/// comparison line; its statement differs from the current one only in the
/// `created_at` predicate, so the two cannot count different runs.
fn org_run_usage_cte_for(period: Period) -> String {
    format!(
        "WITH run_usage AS ( \
            SELECT ar.id AS run_id, \
                   ar.created_at AS created_at, \
                   ar.workspace_id AS workspace_id, \
                   COALESCE(ar.source_type, 'analytics') AS source_type, \
                   (ar.schedule_id IS NOT NULL \
                    AND COALESCE(ar.metadata->>'trigger', 'scheduled') <> 'manual') AS unattended, \
                   {RUN_TOKENS} \
            FROM agentic_runs ar \
            JOIN agentic_run_events e \
                ON e.run_id = ar.id AND e.event_type IN ('llm_start', 'llm_end') \
            JOIN workspaces w ON ar.workspace_id = w.id \
            WHERE {window} AND w.org_id = $2 \
            GROUP BY ar.id, ar.created_at, ar.workspace_id, ar.source_type, ar.schedule_id \
        ) ",
        window = period.created_at(),
    )
}

/// The token columns every `SELECT` below sums, so a grouping cannot be added
/// with one of them missing.
const TOKEN_SUMS: &str = "SUM(input_tokens)::bigint AS input_tokens, \
     SUM(output_tokens)::bigint AS output_tokens, \
     SUM(cache_creation)::bigint AS cache_creation, \
     SUM(cache_read)::bigint AS cache_read, \
     COUNT(*)::bigint AS run_count";

// ── Statements ────────────────────────────────────────────────────────────

/// `(day, model)` — the window total and the trend.
fn days_sql() -> String {
    days_sql_for(Period::Current)
}

fn days_sql_for(period: Period) -> String {
    format!(
        "{cte} SELECT to_char(date_trunc('day', created_at), 'YYYY-MM-DD') AS day, \
         model, {TOKEN_SUMS} \
         FROM run_usage GROUP BY day, model ORDER BY day",
        cte = org_run_usage_cte_for(period),
    )
}

/// `(model, model)` — the model breakdown. The bucket *is* the model, which is
/// why both columns read the same expression: it keeps one row shape across all
/// four breakdowns.
///
/// A run whose `llm_end` carried no model is `'unknown'` rather than dropped:
/// its tokens are real and its dollars are not knowable, and the panel names
/// that rather than quietly shrinking the total to match the priced part.
fn by_model_sql() -> String {
    format!(
        "{cte} SELECT COALESCE(model, 'unknown') AS bucket, \
         COALESCE(model, 'unknown') AS label, \
         model, {TOKEN_SUMS} \
         FROM run_usage GROUP BY bucket, label, model",
        cte = org_run_usage_cte(),
    )
}

/// `(workspace, model)` — which deployment inside the org spent it.
fn by_workspace_sql() -> String {
    format!(
        "{cte} SELECT ru.workspace_id::text AS bucket, \
         COALESCE(NULLIF(w.name, ''), ru.workspace_id::text) AS label, \
         ru.model AS model, \
         SUM(ru.input_tokens)::bigint AS input_tokens, \
         SUM(ru.output_tokens)::bigint AS output_tokens, \
         SUM(ru.cache_creation)::bigint AS cache_creation, \
         SUM(ru.cache_read)::bigint AS cache_read, \
         COUNT(*)::bigint AS run_count \
         FROM run_usage ru JOIN workspaces w ON ru.workspace_id = w.id \
         GROUP BY bucket, label, model",
        cte = org_run_usage_cte(),
    )
}

/// `(source_type, model)` — *what* spent it: `analytics` (chat), `workflow`
/// (an automation), `airway` (ELT), `builder`.
fn by_surface_sql() -> String {
    format!(
        "{cte} SELECT source_type AS bucket, source_type AS label, \
         model, {TOKEN_SUMS} \
         FROM run_usage GROUP BY bucket, label, model",
        cte = org_run_usage_cte(),
    )
}

/// `(attendance, model)` — scheduled against interactive.
///
/// The split a cost page exists for. Anything a schedule fires costs the same
/// whether or not anyone reads the answer, so unattended spend is the share
/// that grows without anybody deciding to spend it — which is why a "Run now"
/// must not land in it (see the module docs).
fn by_attendance_sql() -> String {
    format!(
        "{cte} SELECT CASE WHEN unattended THEN 'scheduled' ELSE 'interactive' END AS bucket, \
         CASE WHEN unattended THEN 'scheduled' ELSE 'interactive' END AS label, \
         model, {TOKEN_SUMS} \
         FROM run_usage GROUP BY bucket, label, model",
        cte = org_run_usage_cte(),
    )
}

/// Which window a fleet statement reads: the last `days` days, or the `days`
/// before those — the prior period the fleet list measures change against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Period {
    Current,
    Prior,
}

impl Period {
    /// The `created_at` predicate, bound to `$1` (days) either way so the
    /// window never reaches the SQL text. The prior window ends, exclusively
    /// on the current window's side, where the current one starts: the two
    /// tile, and a run on the boundary is counted once.
    fn created_at(self) -> &'static str {
        match self {
            Period::Current => "ar.created_at > now() - make_interval(days => $1)",
            Period::Prior => {
                "ar.created_at > now() - make_interval(days => $1 * 2) \
                 AND ar.created_at <= now() - make_interval(days => $1)"
            }
        }
    }
}

/// Every org with LLM usage in the window, plus its unattended share.
///
/// `scope` is a bounded platform grant's org list, each id bound as its own
/// placeholder numbered after the window. `None` is an unbounded reader and
/// produces **no clause at all** — not `IN ()`, which would match nothing and
/// report every org as having no spend. `Some(&[])` must never reach here; the
/// caller short-circuits it.
fn fleet_sql(scope: Option<&[Uuid]>, values: &mut Vec<sea_orm::Value>, period: Period) -> String {
    let fence = match scope {
        None => String::new(),
        Some(orgs) => {
            let placeholders = orgs
                .iter()
                .map(|org| {
                    values.push((*org).into());
                    format!("${}", values.len())
                })
                .collect::<Vec<_>>()
                .join(",");
            format!(" AND w.org_id IN ({placeholders})")
        }
    };
    format!(
        "WITH run_usage AS ( \
            SELECT ar.id AS run_id, w.org_id AS org_id, \
                   (ar.schedule_id IS NOT NULL \
                    AND COALESCE(ar.metadata->>'trigger', 'scheduled') <> 'manual') AS unattended, \
                   {RUN_TOKENS} \
            FROM agentic_runs ar \
            JOIN agentic_run_events e \
                ON e.run_id = ar.id AND e.event_type IN ('llm_start', 'llm_end') \
            JOIN workspaces w ON ar.workspace_id = w.id \
            WHERE {window}{fence} \
            GROUP BY ar.id, w.org_id, ar.schedule_id \
        ) \
        SELECT ru.org_id AS org_id, ru.model AS model, \
               SUM(ru.input_tokens)::bigint AS input_tokens, \
               SUM(ru.output_tokens)::bigint AS output_tokens, \
               SUM(ru.cache_creation)::bigint AS cache_creation, \
               SUM(ru.cache_read)::bigint AS cache_read, \
               COUNT(*)::bigint AS run_count, \
               COUNT(*) FILTER (WHERE ru.unattended)::bigint AS unattended_runs \
        FROM run_usage ru \
        GROUP BY ru.org_id, ru.model",
        window = period.created_at(),
    )
}

// ── Row shapes ────────────────────────────────────────────────────────────

/// A (day, model) bucket — the window total and the daily series.
///
/// Grouped by model as well as day because pricing is per model: summing
/// tokens across models first and pricing the sum would charge every token at
/// one rate.
#[derive(Debug, FromQueryResult)]
pub(super) struct DayModelRow {
    pub(super) day: String,
    pub(super) model: Option<String>,
    pub(super) input_tokens: i64,
    pub(super) output_tokens: i64,
    pub(super) cache_creation: i64,
    pub(super) cache_read: i64,
    pub(super) run_count: i64,
}

/// A (dimension value, model) bucket. One shape for all four breakdowns —
/// model, workspace, surface, attendance — because they differ only in what
/// `bucket` holds, and four near-identical row structs is four places for a
/// cast to drift.
#[derive(Debug, FromQueryResult)]
pub(super) struct BucketModelRow {
    /// The dimension's value: a model id, a workspace id, a `source_type`, or
    /// `scheduled` / `interactive`.
    pub(super) bucket: String,
    /// A label for the bucket where the id is not one (a workspace's name);
    /// equal to `bucket` otherwise.
    pub(super) label: String,
    pub(super) model: Option<String>,
    pub(super) input_tokens: i64,
    pub(super) output_tokens: i64,
    pub(super) cache_creation: i64,
    pub(super) cache_read: i64,
    pub(super) run_count: i64,
}

/// One org's priced tokens over the window, for the fleet list.
///
/// No name or slug: the fleet handler lists orgs separately — so that an org
/// with no spend still gets a row — and names them from that list. Joining
/// `organizations` here as well would mean two sources for one label.
#[derive(Debug, FromQueryResult)]
pub(super) struct FleetOrgRow {
    pub(super) org_id: Uuid,
    pub(super) model: Option<String>,
    pub(super) input_tokens: i64,
    pub(super) output_tokens: i64,
    pub(super) cache_read: i64,
    pub(super) cache_creation: i64,
    pub(super) run_count: i64,
    /// Runs a schedule seeded — the unattended share, per org.
    pub(super) unattended_runs: i64,
}

// ── Reads ─────────────────────────────────────────────────────────────────

async fn bucket_rows(
    db: &sea_orm::DatabaseConnection,
    sql: String,
    days: i32,
    org_id: Uuid,
) -> Result<Vec<BucketModelRow>, DbErr> {
    BucketModelRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        sql,
        [days.into(), org_id.into()],
    ))
    .all(db)
    .await
}

pub(super) async fn fetch_days(
    db: &sea_orm::DatabaseConnection,
    days: i32,
    org_id: Uuid,
) -> Result<Vec<DayModelRow>, DbErr> {
    DayModelRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        days_sql(),
        [days.into(), org_id.into()],
    ))
    .all(db)
    .await
}

/// The same daily series over the prior window, for the console's comparison.
pub(super) async fn fetch_prior_days(
    db: &sea_orm::DatabaseConnection,
    days: i32,
    org_id: Uuid,
) -> Result<Vec<DayModelRow>, DbErr> {
    DayModelRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        days_sql_for(Period::Prior),
        [days.into(), org_id.into()],
    ))
    .all(db)
    .await
}

pub(super) async fn fetch_by_model(
    db: &sea_orm::DatabaseConnection,
    days: i32,
    org_id: Uuid,
) -> Result<Vec<BucketModelRow>, DbErr> {
    bucket_rows(db, by_model_sql(), days, org_id).await
}

pub(super) async fn fetch_by_workspace(
    db: &sea_orm::DatabaseConnection,
    days: i32,
    org_id: Uuid,
) -> Result<Vec<BucketModelRow>, DbErr> {
    bucket_rows(db, by_workspace_sql(), days, org_id).await
}

pub(super) async fn fetch_by_surface(
    db: &sea_orm::DatabaseConnection,
    days: i32,
    org_id: Uuid,
) -> Result<Vec<BucketModelRow>, DbErr> {
    bucket_rows(db, by_surface_sql(), days, org_id).await
}

pub(super) async fn fetch_by_attendance(
    db: &sea_orm::DatabaseConnection,
    days: i32,
    org_id: Uuid,
) -> Result<Vec<BucketModelRow>, DbErr> {
    bucket_rows(db, by_attendance_sql(), days, org_id).await
}

pub(super) async fn fetch_fleet(
    db: &sea_orm::DatabaseConnection,
    days: i32,
    scope: Option<&[Uuid]>,
    period: Period,
) -> Result<Vec<FleetOrgRow>, DbErr> {
    let mut values: Vec<sea_orm::Value> = vec![days.into()];
    let sql = fleet_sql(scope, &mut values, period);
    FleetOrgRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        sql,
        values,
    ))
    .all(db)
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every per-org statement the module runs. The tests below hold their
    /// shared properties over this list, so a sixth grouping added without one
    /// of them fails rather than shipping.
    fn org_statements() -> Vec<String> {
        vec![
            days_sql(),
            days_sql_for(Period::Prior),
            by_model_sql(),
            by_workspace_sql(),
            by_surface_sql(),
            by_attendance_sql(),
        ]
    }

    /// Every per-org statement is fenced to the org by the join, with the id
    /// **bound** rather than formatted in. A rollup that forgot the fence would
    /// answer with every tenant's spend under one org's name, which is the
    /// worst defect this surface could ship.
    #[test]
    fn every_org_statement_is_fenced_to_the_bound_org() {
        for sql in org_statements() {
            assert!(
                sql.contains("JOIN workspaces w ON ar.workspace_id = w.id"),
                "the org fence needs the workspace join: {sql}"
            );
            assert!(
                sql.contains("w.org_id = $2"),
                "the org must be bound as $2, never formatted in: {sql}"
            );
        }
    }

    /// Pricing is per model, so every grouping keeps `model` in its projection
    /// **and** its `GROUP BY`. Summing tokens across models and pricing the sum
    /// charges every token at one rate — a number wrong by whatever the model
    /// mix is, and wrong in a direction nobody can see.
    #[test]
    fn every_grouping_keeps_model_so_each_bucket_prices_at_its_own_rate() {
        for sql in org_statements() {
            let group_by = sql
                .rsplit("GROUP BY ")
                .next()
                .expect("statement has a GROUP BY");
            assert!(
                group_by.contains("model"),
                "model must survive the grouping: {sql}"
            );
        }
    }

    /// The run CTE collapses each run to one row before any grouping. Without
    /// `GROUP BY ar.id`, the `llm_start`/`llm_end` join multiplies a run by its
    /// event count and every token sum is inflated by however many LLM calls
    /// the run made.
    #[test]
    fn every_statement_collapses_each_run_to_one_row() {
        for sql in org_statements().into_iter().chain([
            fleet_sql(None, &mut vec![], Period::Current),
            fleet_sql(None, &mut vec![], Period::Prior),
        ]) {
            assert!(
                sql.contains("GROUP BY ar.id,"),
                "each run must collapse to one row before the outer grouping: {sql}"
            );
        }
    }

    /// Attendance is read off `schedule_id`, not guessed from a source type.
    /// An automation can be run by hand and a chat thread cannot be scheduled,
    /// so `source_type` is not a proxy for who started the run.
    #[test]
    fn attendance_reads_schedule_id_not_source_type() {
        let sql = by_attendance_sql();
        assert!(
            sql.contains(
                "(ar.schedule_id IS NOT NULL \
                    AND COALESCE(ar.metadata->>'trigger', 'scheduled') <> 'manual') AS unattended"
            ),
            "unattended must come from schedule_id: {sql}"
        );
        assert!(
            fleet_sql(None, &mut vec![], Period::Current).contains(
                "(ar.schedule_id IS NOT NULL \
                    AND COALESCE(ar.metadata->>'trigger', 'scheduled') <> 'manual') AS unattended"
            ),
            "the fleet rollup must split attendance the same way"
        );
    }

    /// A run with no model keeps its tokens under `unknown` rather than
    /// vanishing. Dropping it would shrink the token total to match the dollar
    /// total, hiding exactly the usage nobody can price.
    #[test]
    fn a_run_with_no_model_is_named_not_dropped() {
        let sql = by_model_sql();
        assert!(
            sql.contains("COALESCE(model, 'unknown') AS bucket"),
            "an unpriced model must be named: {sql}"
        );
    }

    /// The fleet fence binds each org id as its own placeholder, numbered after
    /// the window. Interpolating the list would put ids from a grant table into
    /// SQL text; getting the numbering wrong would bind the window as an org.
    #[test]
    fn the_fleet_fence_binds_each_org_as_a_numbered_placeholder() {
        let mut values: Vec<sea_orm::Value> = vec![30i32.into()];
        let orgs = [Uuid::nil(), Uuid::from_u128(7)];
        let sql = fleet_sql(Some(&orgs), &mut values, Period::Current);
        assert!(
            sql.contains("AND w.org_id IN ($2,$3)"),
            "orgs bind after the window: {sql}"
        );
        assert_eq!(values.len(), 3, "the window plus one value per org");
    }

    /// An unbounded reader gets no fence clause at all. `IN ()` would match
    /// nothing, so every org would report no spend — a page of zeros that reads
    /// as a quiet platform rather than as a broken query.
    #[test]
    fn an_unbounded_reader_gets_no_fence_clause() {
        let sql = fleet_sql(None, &mut vec![30i32.into()], Period::Current);
        assert!(!sql.contains("w.org_id IN"), "no fence for an owner: {sql}");
        assert!(
            sql.contains("WHERE ar.created_at > now() - make_interval(days => $1) WITH")
                || sql.contains("make_interval(days => $1) GROUP BY"),
            "the window clause must close cleanly with no fence after it: {sql}"
        );
    }

    /// The window is bound too — `days` is a request parameter, and
    /// `make_interval(days => $1)` is the only place it may appear.
    #[test]
    fn the_window_is_bound_not_interpolated() {
        for sql in org_statements().into_iter().chain([
            fleet_sql(None, &mut vec![], Period::Current),
            fleet_sql(None, &mut vec![], Period::Prior),
        ]) {
            assert!(
                sql.contains("make_interval(days => $1)"),
                "the window must be bound: {sql}"
            );
        }
    }

    /// The prior window is the `days` before the current one: it starts
    /// `2 × days` back and ends where the current window starts, so the two
    /// tile. Both bounds ride the bound window parameter; neither is text.
    #[test]
    fn the_prior_fleet_window_tiles_against_the_current_one() {
        let sql = fleet_sql(None, &mut vec![30i32.into()], Period::Prior);
        assert!(
            sql.contains("ar.created_at > now() - make_interval(days => $1 * 2)"),
            "{sql}"
        );
        assert!(
            sql.contains("ar.created_at <= now() - make_interval(days => $1)"),
            "{sql}"
        );
        let current = fleet_sql(None, &mut vec![30i32.into()], Period::Current);
        assert!(
            !current.contains("$1 * 2"),
            "the current window is unchanged: {current}"
        );
    }
}
