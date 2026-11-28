use serde_json::Value;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct AnomalyRecord {
    pub id: Uuid,
    pub measure: String,
    pub time_dimension: String,
    pub granularity: String,
    /// ISO 8601 timestamp string
    pub period_start: String,
    /// ISO 8601 timestamp string
    pub period_end: String,
    pub observed: f64,
    pub expected: f64,
    pub lower: f64,
    pub upper: f64,
    pub z_score: f64,
    pub severity: String,
    pub status: String,
    /// Dominant seasonal cycle length (in units of `granularity`) from the
    /// monitor's detection config, snapshotted at scan time. Drives the
    /// same-phase comparison window in root-cause explains. `None` for rows
    /// detected before the column existed — callers fall back to the
    /// granularity default via [`crate::anomaly_period::resolve_seasonal_period`].
    pub seasonal_period: Option<i32>,
    /// The monitor filters this anomaly was detected under, as a JSON array of
    /// `{member, values}` (e.g. the per-restaurant `group_by` value). Used to
    /// scope root-cause explains to the anomaly's segment. `None`/empty for
    /// chain-wide monitors. See [`segment_query_filters`].
    pub filters: Option<Value>,
}

/// Convert an anomaly row's persisted monitor filters (a JSON array of
/// `{member, values}`) into airlayer equals-filters, used to scope a
/// root-cause explain to the anomaly's segment. Malformed / empty input
/// yields no filters (an unscoped, chain-wide explain).
pub fn segment_query_filters(
    filters: &Option<Value>,
) -> Vec<oxy_airlayer_compat::engine::query::QueryFilter> {
    use oxy_airlayer_compat::engine::query::{FilterOperator, QueryFilter};
    let Some(Value::Array(arr)) = filters else {
        return vec![];
    };
    arr.iter()
        .filter_map(|f| {
            let member = f.get("member")?.as_str()?.to_string();
            let values = f
                .get("values")?
                .as_array()?
                .iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect::<Vec<_>>();
            if values.is_empty() {
                return None;
            }
            Some(QueryFilter {
                member: Some(member),
                operator: Some(FilterOperator::Equals),
                values,
                and: None,
                or: None,
            })
        })
        .collect()
}

#[derive(Debug, Default, Clone)]
pub struct AnomalyFilter {
    pub measure: Option<String>,
    pub time_dimension: Option<String>,
    pub granularity: Option<String>,
    /// The period's first day (`YYYY-MM-DD`) or first instant (RFC 3339).
    /// A date selects buckets whose `period_start` is on or after 00:00 UTC
    /// that day; an instant, buckets whose `period_start` is at or after it.
    pub period_start_gte: Option<String>,
    /// The period's last day (`YYYY-MM-DD`, included whole) or end instant
    /// (RFC 3339). A date selects buckets whose `period_start` is before
    /// 00:00 UTC the next day; an instant, buckets whose (exclusive)
    /// `period_end` is at or before it. Any other format is
    /// [`AnomalyStoreError::InvalidFilter`], never ignored.
    pub period_end_lte: Option<String>,
}

/// One `detect_and_upsert` call: a measure's series and the period to score.
#[derive(Debug, Clone)]
pub struct DetectRequest {
    pub measure: String,
    pub time_dimension: String,
    pub granularity: String,
    /// `(bucket, value)` rows as the warehouse returned them: the history
    /// before `period`, then `period` itself.
    pub observations: Vec<(String, f64)>,
    /// The `(start, end)` the caller asked about, both inclusive. Every
    /// bucket in it is scored; the buckets before it are history.
    pub period: (String, String),
}

#[derive(Debug, Clone)]
pub struct DetectAndUpsertResult {
    pub anomalies: Vec<AnomalyRecord>,
    pub total_observations: usize,
    /// Set when there are not enough observations to run detection.
    pub message: Option<String>,
    /// Buckets inside the period with no row at all (`YYYY-MM-DD`), such as
    /// a day with no orders. The detector does not score them, because a
    /// missing row may be missing data, so the caller must report them.
    pub missing_periods: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum AnomalyStoreError {
    #[error("database error: {0}")]
    Db(String),
    #[error("detection error: {0}")]
    Detection(String),
    #[error("not found: {0}")]
    NotFound(String),
    /// The filter has a value the store cannot read, such as a period bound
    /// in neither accepted format. The message says what is accepted.
    #[error("invalid filter: {0}")]
    InvalidFilter(String),
}

#[async_trait::async_trait]
pub trait AnomalyStore: Send + Sync {
    async fn list(
        &self,
        workspace_id: Uuid,
        filter: AnomalyFilter,
    ) -> Result<Vec<AnomalyRecord>, AnomalyStoreError>;

    async fn get(
        &self,
        id: Uuid,
        workspace_id: Uuid,
    ) -> Result<Option<AnomalyRecord>, AnomalyStoreError>;

    /// Run anomaly detection on raw `(bucket, value)` observations, persist
    /// any flagged anomalies, and return them. The implementation calls
    /// `metric_monitoring::detect::detect()` internally.
    async fn detect_and_upsert(
        &self,
        workspace_id: Uuid,
        request: DetectRequest,
    ) -> Result<DetectAndUpsertResult, AnomalyStoreError>;

    async fn get_explain_cache(
        &self,
        id: Uuid,
        workspace_id: Uuid,
    ) -> Result<Option<Value>, AnomalyStoreError>;

    async fn set_explain_cache(
        &self,
        id: Uuid,
        workspace_id: Uuid,
        result: Value,
    ) -> Result<(), AnomalyStoreError>;
}
