//! The per-table summary of an airway run (moved out of `queries.rs`
//! unchanged).

use sea_orm::{DatabaseBackend, DatabaseConnection, DbErr, FromQueryResult, Statement};

/// Per-table summary of an airway (ELT) run — rows in / rows out plus
/// the phase timestamps that let the dashboard spot a slow extract or a
/// slow load. Reconstructed at query time from the `extract_*` /
/// `table_loaded` events the airway worker already forwards; no
/// instrumentation work needed, no extension table.
#[derive(Debug, Clone, FromQueryResult, serde::Serialize)]
pub struct AirwayTableSummary {
    pub table_name: String,
    /// Final extracted row count from the source connector.
    pub rows_extracted: Option<i64>,
    /// Final loaded row count on the destination.
    pub rows_loaded: Option<i64>,
    /// `extracting` | `extracted` | `loading` | `loaded` | `failed` —
    /// derived from which event types have fired so far.
    pub status: String,
    pub extract_started_at: Option<sea_orm::prelude::DateTimeWithTimeZone>,
    pub extract_completed_at: Option<sea_orm::prelude::DateTimeWithTimeZone>,
    pub loaded_at: Option<sea_orm::prelude::DateTimeWithTimeZone>,
}

/// One row per table touched by the run. Walks the per-table airway
/// events for the given run id and rolls them up — the same data the
/// existing `/pages/airway/` UI consumes, but aggregated server-side so
/// the coordinator's polymorphic run-detail can render it without
/// re-implementing the airway reducer.
pub async fn airway_table_summary_for_run(
    db: &DatabaseConnection,
    run_id: &str,
) -> Result<Vec<AirwayTableSummary>, DbErr> {
    let rows = AirwayTableSummary::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        // `MIN(seq)` ordering preserves the natural extract order rather
        // than alphabetical, which makes the card read like the timeline
        // it summarises. The status enum collapses presence of later-
        // stage events back to a single string for the UI.
        "SELECT \
            payload->>'table' AS table_name, \
            MAX(CASE WHEN event_type = 'extract_completed' \
                     THEN (payload->>'rows_extracted')::bigint END) AS rows_extracted, \
            MAX(CASE WHEN event_type = 'table_loaded' \
                     THEN (payload->>'rows')::bigint END) AS rows_loaded, \
            CASE \
              WHEN COUNT(*) FILTER (WHERE event_type = 'resource_failed') > 0 THEN 'failed' \
              WHEN COUNT(*) FILTER (WHERE event_type = 'table_loaded') > 0 THEN 'loaded' \
              WHEN COUNT(*) FILTER (WHERE event_type = 'table_load_started') > 0 THEN 'loading' \
              WHEN COUNT(*) FILTER (WHERE event_type = 'extract_completed') > 0 THEN 'extracted' \
              WHEN COUNT(*) FILTER (WHERE event_type = 'extract_started') > 0 THEN 'extracting' \
              ELSE 'pending' \
            END AS status, \
            MIN(CASE WHEN event_type = 'extract_started' \
                     THEN created_at END) AS extract_started_at, \
            MAX(CASE WHEN event_type = 'extract_completed' \
                     THEN created_at END) AS extract_completed_at, \
            MAX(CASE WHEN event_type = 'table_loaded' \
                     THEN created_at END) AS loaded_at \
         FROM agentic_run_events \
         WHERE run_id = $1 \
           AND payload->>'table' IS NOT NULL \
           AND event_type IN ( \
             'extract_started', 'extract_completed', \
             'table_load_started', 'table_loaded', \
             'resource_failed' \
           ) \
         GROUP BY payload->>'table' \
         ORDER BY MIN(seq)",
        [run_id.into()],
    ))
    .all(db)
    .await?;
    Ok(rows)
}
