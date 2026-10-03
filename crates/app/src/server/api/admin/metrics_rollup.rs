//! The SQL behind `/admin/metrics/*`: a per-run token rollup over the window (the
//! run-usage CTE), grouped by day and model, by org and model, and — for the per-org
//! detail — by day and model within one org.
//!
//! Split out of `metrics.rs` by responsibility; the handlers there call these.

use sea_orm::{DatabaseBackend, DbErr, FromQueryResult, Statement};
use uuid::Uuid;

use super::scope::org_scope_clause;

// Shared CTE

/// Per-run token rollup over the window. Each run collapses to ONE model (the
/// max `llm_end.model` — runs are effectively single-model), so downstream
/// GROUP BYs can attribute the run's tokens to a priced model bucket. Token
/// casts mirror `agentic_runtime`'s per-run usage query.
///
/// `$1` is the window in days. A bounded grant's org set is bound next (see
/// [`org_scope_clause`]) and narrows the rollup to runs in those orgs'
/// workspaces — an inner lookup, so a run with no workspace or no org is
/// platform-level and counted only for an unbounded reader.
fn run_usage_cte(scope: Option<&[Uuid]>, values: &mut Vec<sea_orm::Value>) -> String {
    let in_scope = org_scope_clause("sw.org_id", scope, values);
    let in_scope = if in_scope.is_empty() {
        in_scope
    } else {
        format!(" AND ar.workspace_id IN (SELECT sw.id FROM workspaces sw WHERE TRUE{in_scope})")
    };
    format!(
        "WITH run_usage AS ( \
        SELECT \
            ar.id AS run_id, \
            ar.workspace_id AS workspace_id, \
            ar.created_at AS created_at, \
            max(e.payload->>'model') FILTER (WHERE e.event_type = 'llm_end') AS model, \
            COALESCE(SUM((e.payload->>'prompt_tokens')::bigint) \
                FILTER (WHERE e.event_type = 'llm_start'), 0) AS input_tokens, \
            COALESCE(SUM((e.payload->>'output_tokens')::bigint) \
                FILTER (WHERE e.event_type = 'llm_end'), 0) AS output_tokens, \
            COALESCE(SUM((e.payload->>'cache_creation_input_tokens')::bigint) \
                FILTER (WHERE e.event_type = 'llm_end'), 0) AS cache_creation, \
            COALESCE(SUM((e.payload->>'cache_read_input_tokens')::bigint) \
                FILTER (WHERE e.event_type = 'llm_end'), 0) AS cache_read \
        FROM agentic_runs ar \
        JOIN agentic_run_events e \
            ON e.run_id = ar.id AND e.event_type IN ('llm_start', 'llm_end') \
        WHERE ar.created_at > now() - make_interval(days => $1){in_scope} \
        GROUP BY ar.id, ar.workspace_id, ar.created_at \
    ) "
    )
}

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

#[derive(Debug, FromQueryResult)]
pub(super) struct OrgModelRow {
    pub(super) org_id: Uuid,
    pub(super) org_name: String,
    pub(super) org_slug: String,
    pub(super) model: Option<String>,
    pub(super) input_tokens: i64,
    pub(super) output_tokens: i64,
    pub(super) cache_creation: i64,
    pub(super) cache_read: i64,
    pub(super) run_count: i64,
}

pub(super) async fn fetch_day_model_rows(
    db: &sea_orm::DatabaseConnection,
    days: i32,
    scope: Option<&[Uuid]>,
) -> Result<Vec<DayModelRow>, DbErr> {
    let mut values: Vec<sea_orm::Value> = vec![days.into()];
    let cte = run_usage_cte(scope, &mut values);
    let sql = format!(
        "{cte} \
         SELECT to_char(date_trunc('day', created_at), 'YYYY-MM-DD') AS day, \
                model, \
                SUM(input_tokens)::bigint AS input_tokens, \
                SUM(output_tokens)::bigint AS output_tokens, \
                SUM(cache_creation)::bigint AS cache_creation, \
                SUM(cache_read)::bigint AS cache_read, \
                COUNT(*)::bigint AS run_count \
         FROM run_usage GROUP BY 1, 2 ORDER BY 1"
    );
    DayModelRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        sql,
        values,
    ))
    .all(db)
    .await
}

pub(super) async fn fetch_org_model_rows(
    db: &sea_orm::DatabaseConnection,
    days: i32,
    scope: Option<&[Uuid]>,
) -> Result<Vec<OrgModelRow>, DbErr> {
    let mut values: Vec<sea_orm::Value> = vec![days.into()];
    let cte = run_usage_cte(scope, &mut values);
    let sql = format!(
        "{cte} \
         SELECT w.org_id AS org_id, o.name AS org_name, o.slug AS org_slug, ru.model AS model, \
                SUM(ru.input_tokens)::bigint AS input_tokens, \
                SUM(ru.output_tokens)::bigint AS output_tokens, \
                SUM(ru.cache_creation)::bigint AS cache_creation, \
                SUM(ru.cache_read)::bigint AS cache_read, \
                COUNT(*)::bigint AS run_count \
         FROM run_usage ru \
         JOIN workspaces w ON ru.workspace_id = w.id \
         JOIN organizations o ON w.org_id = o.id \
         GROUP BY w.org_id, o.name, o.slug, ru.model"
    );
    OrgModelRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        sql,
        values,
    ))
    .all(db)
    .await
}

/// Per-(day, model) token rollup scoped to one org's workspaces. Same shape as
/// [`fetch_day_model_rows`] but the run-usage CTE is narrowed with a
/// `workspaces.org_id` join so the result is correct for any tenant, not just
/// the top-10 cost leaders the cross-tenant `by_org` keeps. $1 = days,
/// $2 = org_id.
pub(super) async fn fetch_org_usage_day_rows(
    db: &sea_orm::DatabaseConnection,
    days: i32,
    org_id: Uuid,
) -> Result<Vec<DayModelRow>, DbErr> {
    let sql = "\
        WITH run_usage AS ( \
            SELECT ar.id AS run_id, ar.created_at AS created_at, \
                max(e.payload->>'model') FILTER (WHERE e.event_type = 'llm_end') AS model, \
                COALESCE(SUM((e.payload->>'prompt_tokens')::bigint) \
                    FILTER (WHERE e.event_type = 'llm_start'), 0) AS input_tokens, \
                COALESCE(SUM((e.payload->>'output_tokens')::bigint) \
                    FILTER (WHERE e.event_type = 'llm_end'), 0) AS output_tokens, \
                COALESCE(SUM((e.payload->>'cache_creation_input_tokens')::bigint) \
                    FILTER (WHERE e.event_type = 'llm_end'), 0) AS cache_creation, \
                COALESCE(SUM((e.payload->>'cache_read_input_tokens')::bigint) \
                    FILTER (WHERE e.event_type = 'llm_end'), 0) AS cache_read \
            FROM agentic_runs ar \
            JOIN agentic_run_events e \
                ON e.run_id = ar.id AND e.event_type IN ('llm_start', 'llm_end') \
            JOIN workspaces w ON ar.workspace_id = w.id \
            WHERE ar.created_at > now() - make_interval(days => $1) AND w.org_id = $2 \
            GROUP BY ar.id, ar.created_at \
        ) \
        SELECT to_char(date_trunc('day', created_at), 'YYYY-MM-DD') AS day, \
               model, \
               SUM(input_tokens)::bigint AS input_tokens, \
               SUM(output_tokens)::bigint AS output_tokens, \
               SUM(cache_creation)::bigint AS cache_creation, \
               SUM(cache_read)::bigint AS cache_read, \
               COUNT(*)::bigint AS run_count \
        FROM run_usage GROUP BY 1, 2 ORDER BY 1";
    DayModelRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        sql,
        [days.into(), org_id.into()],
    ))
    .all(db)
    .await
}
