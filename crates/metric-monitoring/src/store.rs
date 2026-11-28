use std::sync::Arc;

use agentic_analytics::anomaly_store::{
    AnomalyFilter, AnomalyRecord, AnomalyStoreError, DetectAndUpsertResult,
};
use chrono::{DateTime, Datelike, Duration, FixedOffset, NaiveDate, NaiveTime, TimeZone, Utc};
use entity::metric_anomalies::{self, Entity as AnomaliesEntity};
use sea_orm::ActiveValue::Set;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, IntoActiveModel, QueryFilter,
    QueryOrder,
};
use uuid::Uuid;

use crate::config::{Granularity, Sensitivity};
use crate::detect::{DetectInputs, Observation, Severity, detect};
use crate::gates::min_history_buckets;

pub struct OxyAnomalyStore {
    pub db: Arc<DatabaseConnection>,
}

fn parse_utc(s: &str) -> Option<DateTime<Utc>> {
    s.parse::<DateTime<FixedOffset>>()
        .ok()
        .map(|d| d.with_timezone(&Utc))
}

/// The formats a `period_start` / `period_end` filter bound accepts, named in
/// the error a caller (and through the tool, the model) sees for any other.
const BOUND_FORMATS: &str = "a date (YYYY-MM-DD, e.g. 2026-06-01) or an RFC 3339 \
    timestamp (e.g. 2026-06-01T00:00:00Z)";

/// One bound of [`AnomalyFilter`]'s period, as a caller wrote it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PeriodBound {
    /// A plain `YYYY-MM-DD`: a calendar day, taken in UTC.
    Date(NaiveDate),
    /// An RFC 3339 timestamp, converted to UTC.
    Instant(DateTime<Utc>),
}

fn parse_bound(field: &str, raw: &str) -> Result<PeriodBound, AnomalyStoreError> {
    let s = raw.trim();
    if let Ok(d) = NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        return Ok(PeriodBound::Date(d));
    }
    parse_utc(s).map(PeriodBound::Instant).ok_or_else(|| {
        AnomalyStoreError::InvalidFilter(format!("{field} {raw:?} is not {BOUND_FORMATS}"))
    })
}

/// The row column a [`PeriodCondition`] tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PeriodColumn {
    PeriodStart,
    PeriodEnd,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PeriodCmp {
    Gte,
    Lt,
    Lte,
}

/// One SQL condition the period bounds of a filter resolve to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PeriodCondition {
    column: PeriodColumn,
    cmp: PeriodCmp,
    at: DateTime<Utc>,
}

impl PeriodCondition {
    fn condition(&self) -> sea_orm::Condition {
        let col = match self.column {
            PeriodColumn::PeriodStart => metric_anomalies::Column::PeriodStart,
            PeriodColumn::PeriodEnd => metric_anomalies::Column::PeriodEnd,
        };
        let at: DateTime<FixedOffset> = self.at.into();
        let all = sea_orm::Condition::all();
        match self.cmp {
            PeriodCmp::Gte => all.add(col.gte(at)),
            PeriodCmp::Lt => all.add(col.lt(at)),
            PeriodCmp::Lte => all.add(col.lte(at)),
        }
    }

    /// What [`Self::condition`] means for one row, so the semantics can be tested
    /// without a database.
    #[cfg(test)]
    fn matches(&self, period_start: DateTime<Utc>, period_end: DateTime<Utc>) -> bool {
        let v = match self.column {
            PeriodColumn::PeriodStart => period_start,
            PeriodColumn::PeriodEnd => period_end,
        };
        match self.cmp {
            PeriodCmp::Gte => v >= self.at,
            PeriodCmp::Lt => v < self.at,
            PeriodCmp::Lte => v <= self.at,
        }
    }
}

fn utc_midnight(d: NaiveDate) -> DateTime<Utc> {
    d.and_time(NaiveTime::MIN).and_utc()
}

/// Resolve a filter's period bounds to SQL conditions. A bound that is
/// neither format is an error, never dropped: dropping it returned every
/// stored anomaly for the measure, whatever period was asked.
///
/// A **date** bound selects buckets by the UTC date of their `period_start`,
/// the date the Insights Inbox and the Slack notice name a bucket by: from
/// 00:00 UTC on the start date up to, but not including, 00:00 UTC on the day
/// after the end date, so the end date is included whole. The store keeps no
/// timezone, so a non-UTC `datetime` monitor's day bucket (`06-30T07:00Z` to
/// `07-01T07:00Z`) counts as 30 June, and a week or month bucket counts in the
/// period it starts in. A `type: date` dimension is bucketed raw, so its rows
/// start at UTC midnight and the UTC date is its local date.
///
/// An **RFC 3339** bound is an instant, tested against the column it names:
/// `period_start >= start` and `period_end <= end` (`period_end` is exclusive,
/// the start of the next bucket).
fn period_conditions(filter: &AnomalyFilter) -> Result<Vec<PeriodCondition>, AnomalyStoreError> {
    let mut out = Vec::with_capacity(2);
    if let Some(raw) = &filter.period_start_gte {
        let at = match parse_bound("period_start", raw)? {
            PeriodBound::Date(d) => utc_midnight(d),
            PeriodBound::Instant(t) => t,
        };
        out.push(PeriodCondition {
            column: PeriodColumn::PeriodStart,
            cmp: PeriodCmp::Gte,
            at,
        });
    }
    if let Some(raw) = &filter.period_end_lte {
        out.push(match parse_bound("period_end", raw)? {
            PeriodBound::Date(d) => {
                let next = d.succ_opt().ok_or_else(|| {
                    AnomalyStoreError::InvalidFilter(format!("period_end {raw:?} is out of range"))
                })?;
                PeriodCondition {
                    column: PeriodColumn::PeriodStart,
                    cmp: PeriodCmp::Lt,
                    at: utc_midnight(next),
                }
            }
            PeriodBound::Instant(t) => PeriodCondition {
                column: PeriodColumn::PeriodEnd,
                cmp: PeriodCmp::Lte,
                at: t,
            },
        });
    }
    Ok(out)
}

fn seasonal_periods(granularity: &str) -> Vec<usize> {
    match granularity {
        "day" => vec![7],
        "week" => vec![4],
        "month" => vec![12],
        "quarter" => vec![4],
        _ => vec![7],
    }
}

/// Map this path's free-form granularity label onto the enum the shared
/// history floor is defined over. `"quarter"` has no [`Granularity`] variant;
/// it is coarse-grained, so it takes the coarse-grained floor.
fn granularity_for_history(granularity: &str) -> Granularity {
    match granularity {
        "week" => Granularity::Week,
        "month" | "quarter" => Granularity::Month,
        _ => Granularity::Day,
    }
}

fn granularity_period_end(start: DateTime<Utc>, granularity: &str) -> DateTime<Utc> {
    match granularity {
        "week" => start + Duration::weeks(1),
        "month" => next_month_start(start.date_naive())
            .and_hms_opt(0, 0, 0)
            .and_then(|dt| Utc.from_local_datetime(&dt).single())
            .unwrap_or(start + Duration::days(31)),
        "quarter" => next_quarter_start(start.date_naive())
            .and_hms_opt(0, 0, 0)
            .and_then(|dt| Utc.from_local_datetime(&dt).single())
            .unwrap_or(start + Duration::days(91)),
        _ => start + Duration::days(1),
    }
}

fn next_month_start(d: NaiveDate) -> NaiveDate {
    if d.month() == 12 {
        NaiveDate::from_ymd_opt(d.year() + 1, 1, 1)
    } else {
        NaiveDate::from_ymd_opt(d.year(), d.month() + 1, 1)
    }
    .unwrap_or(d + Duration::days(31))
}

fn next_quarter_start(d: NaiveDate) -> NaiveDate {
    let next_quarter_month = ((d.month() - 1) / 3 + 1) * 3 + 1;
    let (y, m) = if next_quarter_month > 12 {
        (d.year() + 1, next_quarter_month - 12)
    } else {
        (d.year(), next_quarter_month)
    };
    NaiveDate::from_ymd_opt(y, m, 1).unwrap_or(d + Duration::days(91))
}

fn severity_str(s: Severity) -> &'static str {
    match s {
        Severity::Low => "low",
        Severity::Medium => "medium",
        Severity::High => "high",
    }
}

fn to_record(m: &metric_anomalies::Model) -> AnomalyRecord {
    AnomalyRecord {
        id: m.id,
        measure: m.measure.clone(),
        time_dimension: m.time_dimension.clone(),
        granularity: m.granularity.clone(),
        period_start: m.period_start.to_rfc3339(),
        period_end: m.period_end.to_rfc3339(),
        observed: m.observed,
        expected: m.expected,
        lower: m.lower_bound,
        upper: m.upper_bound,
        z_score: m.z_score,
        severity: m.severity.clone(),
        status: m.status.clone(),
        seasonal_period: m.seasonal_period,
        filters: m.filters.clone(),
    }
}

#[async_trait::async_trait]
impl agentic_analytics::anomaly_store::AnomalyStore for OxyAnomalyStore {
    async fn list(
        &self,
        workspace_id: Uuid,
        filter: AnomalyFilter,
    ) -> Result<Vec<AnomalyRecord>, AnomalyStoreError> {
        // Resolved before the other fields move out of `filter`, and before
        // any query runs, so a bad bound is refused rather than ignored.
        let period = period_conditions(&filter)?;
        // Rank on severity (envelope headroom — the axis these are meant to be
        // triaged on), then |z| as a stable tiebreak within a band. This
        // orders the `AnomalyStore` trait's consumers — the agentic-analytics
        // chat/SDK tool path; the Insights Inbox has its own ordering in
        // `list_anomalies`. `ABS(z_score)`, not signed z, so a chain-wide
        // *drop* (the motivating case) isn't sorted below spikes — matching the
        // frontend, which picks its event peak by `Math.abs(z_score)`. There is
        // no LIMIT here, so per-row severity ordering is safe (contrast the
        // inbox, which ranks per event to keep an event's rows on one page).
        let severity_rank = sea_orm::sea_query::Expr::cust(crate::detect::severity_rank_case_sql());
        let mut q = AnomaliesEntity::find()
            .filter(metric_anomalies::Column::WorkspaceId.eq(workspace_id))
            .order_by(severity_rank, sea_orm::Order::Desc)
            .order_by(
                sea_orm::sea_query::Expr::cust("ABS(z_score)"),
                sea_orm::Order::Desc,
            );

        if let Some(m) = filter.measure {
            q = q.filter(metric_anomalies::Column::Measure.eq(m));
        }
        if let Some(td) = filter.time_dimension {
            q = q.filter(metric_anomalies::Column::TimeDimension.eq(td));
        }
        if let Some(g) = filter.granularity {
            q = q.filter(metric_anomalies::Column::Granularity.eq(g));
        }
        for cond in &period {
            q = q.filter(cond.condition());
        }

        let rows = q
            .all(self.db.as_ref())
            .await
            .map_err(|e| AnomalyStoreError::Db(e.to_string()))?;
        Ok(rows.iter().map(to_record).collect())
    }

    async fn get(
        &self,
        id: Uuid,
        workspace_id: Uuid,
    ) -> Result<Option<AnomalyRecord>, AnomalyStoreError> {
        let row = AnomaliesEntity::find_by_id(id)
            .filter(metric_anomalies::Column::WorkspaceId.eq(workspace_id))
            .one(self.db.as_ref())
            .await
            .map_err(|e| AnomalyStoreError::Db(e.to_string()))?;
        Ok(row.as_ref().map(to_record))
    }

    async fn detect_and_upsert(
        &self,
        workspace_id: Uuid,
        measure: &str,
        time_dimension: &str,
        granularity: &str,
        observations: Vec<(String, f64)>,
    ) -> Result<DetectAndUpsertResult, AnomalyStoreError> {
        let series: Vec<Observation> = observations
            .iter()
            .filter_map(|(ts, val)| {
                // Caller-supplied series: every point is a real reading, and
                // this path does no gap-filling of its own.
                parse_utc(ts).map(|timestamp| Observation::measured(timestamp, *val))
            })
            .collect();

        let seasonal = seasonal_periods(granularity);
        // Nearest seasonal cycle, snapshotted onto the row so explain aligns
        // its comparison window to the same phase one cycle back.
        let seasonal_period = seasonal.iter().copied().min().map(|p| p as i32);
        let max_period = seasonal.iter().copied().max().unwrap_or(7);
        // Two floors, both of which must hold. The algebraic one mirrors
        // detect()'s formula exactly — (max_period*2).max(10) + test_window,
        // with test_window hardcoded to 1 below — so detect() cannot return
        // SeriesTooShort on a series that passed this guard. The statistical one
        // is the same trust threshold `scan_one` applies, so the AI-tool path
        // and the scheduled scan agree on what counts as enough history.
        let min_obs = min_history_buckets(granularity_for_history(granularity), &seasonal)
            .max((max_period * 2).max(10) + 1);

        if series.len() < min_obs {
            return Ok(DetectAndUpsertResult {
                anomalies: vec![],
                total_observations: series.len(),
                message: Some(format!(
                    "Not enough data: {} observations, need at least {}",
                    series.len(),
                    min_obs
                )),
            });
        }

        let inputs = DetectInputs {
            series: &series,
            seasonal_periods: seasonal,
            test_window: 1,
            sensitivity: Sensitivity::Medium,
            interval_level: 0.95,
            // The AI-tool path scores a single bucket from a caller-supplied
            // series and has no segment context to look events up by.
            continuation: None,
        };
        let detected = detect(inputs).map_err(|e| AnomalyStoreError::Detection(e.to_string()))?;
        let total = series.len();
        let now = Utc::now();
        let mut records = Vec::with_capacity(detected.len());

        for anomaly in &detected {
            let period_start: DateTime<FixedOffset> = anomaly.timestamp.into();
            let period_end: DateTime<FixedOffset> =
                granularity_period_end(anomaly.timestamp, granularity).into();

            // detect_and_upsert is the AI-tool path — it has no segment context,
            // so it always stores dimension_key = "". Scope the lookup to that
            // value to avoid matching periodic-scan rows for specific segments.
            let existing = AnomaliesEntity::find()
                .filter(metric_anomalies::Column::WorkspaceId.eq(workspace_id))
                .filter(metric_anomalies::Column::Measure.eq(measure))
                .filter(metric_anomalies::Column::TimeDimension.eq(time_dimension))
                .filter(metric_anomalies::Column::DimensionKey.eq(""))
                .filter(metric_anomalies::Column::PeriodStart.eq(period_start))
                .one(self.db.as_ref())
                .await
                .map_err(|e| AnomalyStoreError::Db(e.to_string()))?;

            let model = if let Some(found) = existing {
                let preserved = if found.status == "dismissed" {
                    "dismissed"
                } else {
                    "new"
                };
                let mut a = found.into_active_model();
                a.observed = Set(anomaly.observed);
                a.expected = Set(anomaly.expected);
                a.lower_bound = Set(anomaly.lower);
                a.upper_bound = Set(anomaly.upper);
                a.z_score = Set(anomaly.z_score);
                a.severity = Set(severity_str(anomaly.severity).to_string());
                a.status = Set(preserved.to_string());
                a.seasonal_period = Set(seasonal_period);
                a.updated_at = Set(now.into());
                a.update(self.db.as_ref())
                    .await
                    .map_err(|e| AnomalyStoreError::Db(e.to_string()))?
            } else {
                metric_anomalies::ActiveModel {
                    // Events are assigned by the scheduled scan, which knows
                    // the segment; this path has no segment identity. Cohorts
                    // need more than that again — a whole scan's worth of
                    // sibling segments to take a share over — so an ad-hoc
                    // single-series detection can never be in one.
                    event_id: Set(None),
                    cohort_id: Set(None),
                    cohort_deviation: Set(None),
                    cohort_label: Set(None),
                    id: Set(Uuid::new_v4()),
                    workspace_id: Set(workspace_id),
                    measure: Set(measure.to_string()),
                    time_dimension: Set(time_dimension.to_string()),
                    granularity: Set(granularity.to_string()),
                    period_start: Set(period_start),
                    period_end: Set(period_end),
                    observed: Set(anomaly.observed),
                    expected: Set(anomaly.expected),
                    lower_bound: Set(anomaly.lower),
                    upper_bound: Set(anomaly.upper),
                    z_score: Set(anomaly.z_score),
                    severity: Set(severity_str(anomaly.severity).to_string()),
                    status: Set("new".to_string()),
                    label: Set(None),
                    dimension_key: Set(String::new()),
                    filters: Set(None),
                    seasonal_period: Set(seasonal_period),
                    explain_cache: Set(None),
                    explain_cached_at: Set(None),
                    detected_at: Set(now.into()),
                    updated_at: Set(now.into()),
                }
                .insert(self.db.as_ref())
                .await
                .map_err(|e| AnomalyStoreError::Db(e.to_string()))?
            };

            records.push(to_record(&model));
        }

        Ok(DetectAndUpsertResult {
            anomalies: records,
            total_observations: total,
            message: None,
        })
    }

    async fn get_explain_cache(
        &self,
        id: Uuid,
        workspace_id: Uuid,
    ) -> Result<Option<serde_json::Value>, AnomalyStoreError> {
        let row = AnomaliesEntity::find_by_id(id)
            .filter(metric_anomalies::Column::WorkspaceId.eq(workspace_id))
            .one(self.db.as_ref())
            .await
            .map_err(|e| AnomalyStoreError::Db(e.to_string()))?;
        Ok(row.and_then(|r| r.explain_cache))
    }

    async fn set_explain_cache(
        &self,
        id: Uuid,
        workspace_id: Uuid,
        result: serde_json::Value,
    ) -> Result<(), AnomalyStoreError> {
        let now = Utc::now();
        let row = AnomaliesEntity::find_by_id(id)
            .filter(metric_anomalies::Column::WorkspaceId.eq(workspace_id))
            .one(self.db.as_ref())
            .await
            .map_err(|e| AnomalyStoreError::Db(e.to_string()))?
            .ok_or_else(|| AnomalyStoreError::NotFound(id.to_string()))?;

        let mut a = row.into_active_model();
        a.explain_cache = Set(Some(result));
        a.explain_cached_at = Set(Some(now.into()));
        a.update(self.db.as_ref())
            .await
            .map_err(|e| AnomalyStoreError::Db(e.to_string()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utc(s: &str) -> DateTime<Utc> {
        parse_utc(s).expect("a test timestamp")
    }

    fn between(start: &str, end: &str) -> AnomalyFilter {
        AnomalyFilter {
            period_start_gte: Some(start.into()),
            period_end_lte: Some(end.into()),
            ..AnomalyFilter::default()
        }
    }

    /// Whether a bucket starting at `start`, `granularity` long, passes every
    /// condition the filter resolves to — what the SQL selects.
    fn selects(filter: &AnomalyFilter, start: &str, granularity: &str) -> bool {
        let start = utc(start);
        let end = granularity_period_end(start, granularity);
        period_conditions(filter)
            .expect("the filter parses")
            .iter()
            .all(|c| c.matches(start, end))
    }

    fn invalid(filter: &AnomalyFilter) -> String {
        match period_conditions(filter) {
            Err(AnomalyStoreError::InvalidFilter(msg)) => msg,
            other => panic!("expected InvalidFilter, got {other:?}"),
        }
    }

    #[test]
    fn no_bounds_resolve_to_no_conditions() {
        assert!(
            period_conditions(&AnomalyFilter::default())
                .expect("no bounds parse")
                .is_empty()
        );
    }

    #[test]
    fn plain_dates_resolve_to_utc_midnights_on_period_start() {
        let conds = period_conditions(&between("2026-06-01", "2026-06-30")).unwrap();
        assert_eq!(
            conds,
            vec![
                PeriodCondition {
                    column: PeriodColumn::PeriodStart,
                    cmp: PeriodCmp::Gte,
                    at: utc("2026-06-01T00:00:00Z"),
                },
                PeriodCondition {
                    column: PeriodColumn::PeriodStart,
                    cmp: PeriodCmp::Lt,
                    at: utc("2026-07-01T00:00:00Z"),
                },
            ]
        );
    }

    #[test]
    fn rfc3339_bounds_keep_their_columns_and_convert_to_utc() {
        let conds = period_conditions(&between(
            "2026-06-01T02:00:00+02:00",
            "2026-07-01T00:00:00Z",
        ))
        .unwrap();
        assert_eq!(
            conds,
            vec![
                PeriodCondition {
                    column: PeriodColumn::PeriodStart,
                    cmp: PeriodCmp::Gte,
                    at: utc("2026-06-01T00:00:00Z"),
                },
                PeriodCondition {
                    column: PeriodColumn::PeriodEnd,
                    cmp: PeriodCmp::Lte,
                    at: utc("2026-07-01T00:00:00Z"),
                },
            ]
        );
    }

    #[test]
    fn a_date_period_includes_its_first_and_last_day_buckets_only() {
        let june = between("2026-06-01", "2026-06-30");
        assert!(!selects(&june, "2026-05-31T00:00:00Z", "day"));
        assert!(selects(&june, "2026-06-01T00:00:00Z", "day"));
        assert!(selects(&june, "2026-06-15T00:00:00Z", "day"));
        // The end date is included whole: its bucket ends 07-01T00:00Z.
        assert!(selects(&june, "2026-06-30T00:00:00Z", "day"));
        assert!(!selects(&june, "2026-07-01T00:00:00Z", "day"));
    }

    #[test]
    fn a_one_day_period_selects_that_days_bucket() {
        let day = between("2026-06-30", "2026-06-30");
        assert!(selects(&day, "2026-06-30T00:00:00Z", "day"));
        assert!(!selects(&day, "2026-06-29T00:00:00Z", "day"));
        assert!(!selects(&day, "2026-07-01T00:00:00Z", "day"));
    }

    /// A non-UTC `datetime` monitor's day bucket starts at local midnight
    /// (07:00Z in Los Angeles in June) and ends past the next UTC midnight.
    /// It counts on the UTC date of its start, as the inbox labels it.
    #[test]
    fn an_offset_day_bucket_counts_on_the_utc_date_it_starts() {
        let june = between("2026-06-01", "2026-06-30");
        assert!(selects(&june, "2026-06-30T07:00:00Z", "day"));
        assert!(!selects(&june, "2026-07-01T07:00:00Z", "day"));
        assert!(selects(&june, "2026-06-01T07:00:00Z", "day"));
    }

    /// A week or month bucket counts in the period it starts in, even when it
    /// ends after the period.
    #[test]
    fn a_coarser_bucket_counts_in_the_period_it_starts_in() {
        let june = between("2026-06-01", "2026-06-30");
        assert!(selects(&june, "2026-06-29T00:00:00Z", "week"));
        assert!(!selects(&june, "2026-05-25T00:00:00Z", "week"));
        assert!(selects(&june, "2026-06-01T00:00:00Z", "month"));
        assert!(!selects(&june, "2026-07-01T00:00:00Z", "month"));
    }

    /// An RFC 3339 end is tested against the exclusive `period_end`: a bound
    /// at the bucket's end includes it, one inside it does not.
    #[test]
    fn an_rfc3339_end_includes_only_buckets_that_end_by_it() {
        let f = between("2026-06-01T00:00:00Z", "2026-07-01T00:00:00Z");
        assert!(selects(&f, "2026-06-30T00:00:00Z", "day"));
        let f = between("2026-06-01T00:00:00Z", "2026-06-30T12:00:00Z");
        assert!(!selects(&f, "2026-06-30T00:00:00Z", "day"));
    }

    #[test]
    fn an_unreadable_bound_is_an_error_naming_the_formats() {
        for bad in ["June 2026", "", "2026-13-01", "06/01/2026"] {
            let msg = invalid(&AnomalyFilter {
                period_start_gte: Some(bad.into()),
                ..AnomalyFilter::default()
            });
            assert!(msg.starts_with("period_start "), "{msg}");
            assert!(msg.contains("YYYY-MM-DD"), "{msg}");
            assert!(msg.contains("RFC 3339"), "{msg}");
        }
        let msg = invalid(&between("2026-06-01", "end of June"));
        assert!(msg.starts_with("period_end "), "{msg}");
    }

    #[test]
    fn surrounding_whitespace_is_ignored() {
        assert_eq!(
            period_conditions(&between(" 2026-06-01 ", "2026-06-30\n")).unwrap(),
            period_conditions(&between("2026-06-01", "2026-06-30")).unwrap(),
        );
    }
}
