//! Persist [`crate::ScanResult`] outcomes to the `metric_anomalies` table.
//!
//! Lives here (not in `oxy-app`) so both the HTTP handler and the cron tick
//! can call into the same upsert path. The unique index on
//! (workspace_id, measure, time_dimension, dimension_key, period_start) means
//! repeat scans update the existing row in place — dismissed rows stay
//! dismissed, others flip back to `new` so a recurring anomaly resurfaces.
//! `dimension_key` is part of the key (migration `m20260604_000001`) so two
//! segments of the same `group_by` monitor cannot clobber each other's rows.
//!
//! Because each scan re-scores a rolling test window, a bucket withheld by
//! `freshness` is **delayed, not lost**: it is scored on a later run and lands
//! as the same single row.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};
use entity::metric_anomalies::{self, Entity as AnomaliesEntity};
use entity::metric_monitor_coverage::{self, Entity as CoverageEntity};
use sea_orm::ActiveValue::Set;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, DbErr, DeleteMany, EntityTrait,
    IntoActiveModel, QueryFilter, QueryOrder, Select,
};
use uuid::Uuid;

use crate::MonitorEntry;
use crate::config::MonitorFilter;
use crate::detect::{CONTINUATION_GAP_BUCKETS, Continuation};
use crate::detect::{DetectedAnomaly, Severity};
use crate::service::{
    OpenEvents, ScanResult, SegmentKey, advance_period, resolve_local_midnight, retreat_period,
};

/// Share of a measure's *scored* segments that must fire in the same direction
/// on the same bucket before the cluster is treated as one event.
///
/// Universality is the signal. No holiday list contains every cause of a
/// chain-wide simultaneous drop — an outage, weather, a regional event — so
/// detecting it from the shape of the scan generalises where a calendar does
/// not. **Provisional**; calibrate against live scan volume.
const COHORT_SHARE_MIN: f64 = 0.6;

/// What makes two segments' anomalies the same moment: same measure, same
/// grain, same bucket, same direction. Deliberately *not* the segment — that
/// is what distinguishes a cohort from an `event_id` chain.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct CohortKey {
    measure: String,
    time_dimension: String,
    granularity: String,
    period_start: DateTime<Utc>,
    is_decrease: bool,
}

impl CohortKey {
    fn of(entry: &MonitorEntry, anomaly: &DetectedAnomaly) -> Self {
        Self {
            measure: entry.measure.clone(),
            time_dimension: entry.time_dimension.clone(),
            granularity: entry.granularity.airlayer_str().to_string(),
            period_start: anomaly.timestamp,
            is_decrease: anomaly.residual < 0.0,
        }
    }

    /// The (measure, time-dimension, grain) triple this cohort's share is
    /// taken over — the bucket and direction split the numerator, never the
    /// denominator.
    fn scope(&self) -> (String, String, String) {
        (
            self.measure.clone(),
            self.time_dimension.clone(),
            self.granularity.clone(),
        )
    }

    /// A stable cohort id derived from the key itself.
    ///
    /// Restatement windows (`lookback_period` / `freshness`) re-scan the same
    /// trailing buckets for days, so the same cohort is re-planned on every
    /// scan. A fresh `Uuid::new_v4()` each time would churn the id — any URL,
    /// notification, or "3 of 21 stores" reference to it would break on the
    /// next scan, and the workspace/cohort index would accumulate a new id per
    /// bucket per scan. `new_v5` over the key makes a re-scan that still sees
    /// the cohort converge on the same id, while a scan that no longer forms it
    /// simply writes `None` (membership stays recomputed, only the *identity*
    /// is stable). Direction is part of the key, so a drop and a spike on the
    /// same bucket keep distinct ids.
    fn cohort_id(&self) -> Uuid {
        let name = format!(
            "{}|{}|{}|{}|{}",
            self.measure,
            self.time_dimension,
            self.granularity,
            self.period_start.to_rfc3339(),
            self.is_decrease,
        );
        Uuid::new_v5(&COHORT_NAMESPACE, name.as_bytes())
    }
}

/// Namespace for deterministic cohort ids ([`CohortKey::cohort_id`]). A fixed,
/// arbitrary UUID so `new_v5` output is stable across processes and releases.
const COHORT_NAMESPACE: Uuid = Uuid::from_u128(0x6f78_795f_636f_686f_7274_5f6e_7331_0001);

/// What one segment contributed to a candidate cohort: the shared id, this
/// member's deviation from the cluster, and the calendar's name for the day.
type CohortPlan = HashMap<(CohortKey, String), (Uuid, f64, Option<String>)>;

/// Count the segments this scan actually *scored*, per measure triple.
///
/// A warming-up segment was never scored, so it cannot have failed to fire;
/// counting it would drag the share down and hide the event.
fn scored_segments(scan: &ScanResult) -> HashMap<(String, String, String), usize> {
    let mut scored = HashMap::new();
    for outcome in scan.outcomes.iter().filter(|o| !o.coverage.is_warming_up()) {
        let e = &outcome.entry;
        *scored
            .entry((
                e.measure.clone(),
                e.time_dimension.clone(),
                e.granularity.airlayer_str().to_string(),
            ))
            .or_default() += 1;
    }
    scored
}

/// A candidate cohort: the segments that fired on it, and the timezone its
/// bucket has to be read in.
#[derive(Default)]
struct Candidate {
    /// `(dim_key, observed/expected)` per member.
    members: Vec<(String, f64)>,
    /// Taken from any member — they share a measure and so a file-level
    /// timezone. `None` only before the first member is pushed.
    tz: Option<chrono_tz::Tz>,
}

/// Group this scan's anomalies by cohort key, carrying each member's
/// ratio-to-expectation.
///
/// The ratio, not the raw residual: segments of very different sizes have to be
/// comparable within one cluster. A zero or non-finite expectation carries no
/// ratio and contributes `NaN`, which still counts toward the share but is
/// skipped when the cluster's median is taken.
fn fired_members(scan: &ScanResult) -> HashMap<CohortKey, Candidate> {
    let mut fired: HashMap<CohortKey, Candidate> = HashMap::new();
    for outcome in scan.outcomes.iter().filter(|o| !o.coverage.is_warming_up()) {
        let dim_key = MonitorFilter::key_for(&outcome.entry.filters);
        for a in &outcome.anomalies {
            let ratio = if a.expected.abs() > f64::EPSILON {
                a.observed / a.expected
            } else {
                f64::NAN
            };
            let candidate = fired.entry(CohortKey::of(&outcome.entry, a)).or_default();
            candidate.members.push((dim_key.clone(), ratio));
            candidate
                .tz
                .get_or_insert_with(|| outcome.entry.effective_timezone());
        }
    }
    fired
}

/// The cluster's typical ratio — the median over members that have a finite
/// one. `NaN` when none do, which suppresses every deviation in that cohort
/// rather than inventing a centre.
fn cluster_center(members: &[(String, f64)]) -> f64 {
    let mut ratios: Vec<f64> = members
        .iter()
        .map(|(_, r)| *r)
        .filter(|r| r.is_finite())
        .collect();
    if ratios.is_empty() {
        return f64::NAN;
    }
    ratios.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    ratios[ratios.len() / 2]
}

/// Assign cohort ids and per-member deviations for one scan.
///
/// Pure and database-free: a cohort is a property of the *whole* scan — the
/// share of segments that fired — and `upsert_one` sees one row at a time,
/// which is why this cannot live in `resolve_event_id`.
fn plan_cohorts(scan: &ScanResult) -> CohortPlan {
    let scored = scored_segments(scan);
    let mut plan = CohortPlan::new();

    for (key, candidate) in fired_members(scan) {
        let members = candidate.members;
        let denom = scored.get(&key.scope()).copied().unwrap_or(0);
        // A single-segment measure has no cluster to be part of; requiring
        // more than one member keeps a chain-only monitor out of the cohort
        // machinery entirely.
        if denom < 2 || members.len() < 2 {
            continue;
        }
        if (members.len() as f64) / (denom as f64) < COHORT_SHARE_MIN {
            continue;
        }

        // The bucket's *local* date, not its UTC date — a cohort on a US
        // holiday starts at 07:00Z and would otherwise miss its own entry.
        let tz = candidate.tz.unwrap_or(chrono_tz::UTC);
        let local_date = key.period_start.with_timezone(&tz).date_naive();
        let label = scan
            .calendar
            .as_ref()
            .and_then(|c| c.get(&local_date))
            .cloned();

        let center = cluster_center(&members);
        // Deterministic, not random: a restatement re-scan of this bucket must
        // land on the same cohort id rather than mint a fresh one. See
        // [`CohortKey::cohort_id`].
        let cohort_id = key.cohort_id();
        for (dim_key, ratio) in members {
            // Deviation from the shared event: 1.0 is a typical member. It is
            // `ratio / center`, so it is direction-relative — for a *drop*
            // cohort the actionable outliers sit **below** 1.0 (fell further
            // than the shared event explains), but for an *increase* cohort the
            // outlier is the one **above** 1.0. A consumer ranking members must
            // read `CohortKey.is_decrease` to know which tail to sort toward.
            let deviation = if center.abs() > f64::EPSILON && ratio.is_finite() {
                ratio / center
            } else {
                f64::NAN
            };
            plan.insert(
                (key.clone(), dim_key),
                (cohort_id, deviation, label.clone()),
            );
        }
    }
    plan
}

/// Upsert every flagged anomaly from a scan into the database. Returns the
/// count of rows touched (inserted or updated). Failures are surfaced via
/// the first `DbErr`; callers should log + retry on transient errors.
///
/// `cohorts` comes from [`plan_cohorts`] over the whole scan, so a row is
/// written once already carrying its cohort rather than updated a second time.
pub async fn upsert_anomalies(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    scan: &ScanResult,
) -> Result<usize, DbErr> {
    upsert_anomalies_with(db, workspace_id, scan, &plan_cohorts(scan)).await
}

async fn upsert_anomalies_with(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    scan: &ScanResult,
    cohorts: &CohortPlan,
) -> Result<usize, DbErr> {
    let mut touched = 0usize;
    for outcome in &scan.outcomes {
        for anomaly in &outcome.anomalies {
            upsert_one(db, workspace_id, &outcome.entry, anomaly, cohorts).await?;
            touched += 1;
        }
    }
    Ok(touched)
}

/// Persist everything a scan learned: the anomalies it flagged **and** the
/// per-segment coverage that says whether it was scored at all.
///
/// Callers should prefer this over [`upsert_anomalies`] alone. Writing only the
/// anomalies is what left the Monitors tab unable to distinguish "healthy, no
/// anomalies" from "not scoring": a warming-up segment produces neither an
/// anomaly row nor a failure, so without the coverage row it is invisible.
///
/// Returns the number of anomaly rows touched. A coverage write that fails is
/// logged and swallowed — losing a status row must not fail a scan that
/// successfully detected something.
pub async fn persist_scan(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    scan: &ScanResult,
) -> Result<usize, DbErr> {
    let cohorts = plan_cohorts(scan);
    let touched = upsert_anomalies_with(db, workspace_id, scan, &cohorts).await?;
    if let Err(e) = upsert_coverage(db, workspace_id, scan).await {
        tracing::warn!(
            target: "metric_monitoring",
            workspace_id = %workspace_id,
            error = %e,
            "failed to persist monitor coverage; anomalies were still saved"
        );
    }
    Ok(touched)
}

/// Upsert one `metric_monitor_coverage` row per scanned segment, then drop rows
/// for segments that no longer exist.
///
/// The cleanup matters for `group_by` monitors: when a segment disappears (a
/// store closes, a dimension value stops being returned) its row would
/// otherwise linger forever and keep inflating the "N of M segments" the UI
/// reports. Deletes are scoped to the exact
/// (measure, time_dimension, granularity) triples this scan actually covered,
/// so a granularity-filtered scan cannot delete another grain's rows — and a
/// segment that errored is kept rather than read as vanished (see
/// [`prune_keep_lists`]).
pub async fn upsert_coverage(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    scan: &ScanResult,
) -> Result<usize, DbErr> {
    let now = Utc::now();
    let mut touched = 0usize;

    for outcome in &scan.outcomes {
        let entry = &outcome.entry;
        let dim_key = MonitorFilter::key_for(&entry.filters);
        let granularity = entry.granularity.airlayer_str().to_string();
        let filters_json = if entry.filters.is_empty() {
            None
        } else {
            serde_json::to_value(&entry.filters).ok()
        };
        // i32 to match the column; a bucket count can never approach the bound,
        // but saturate rather than wrap if one ever did.
        let measured = i32::try_from(outcome.coverage.measured).unwrap_or(i32::MAX);
        let required = i32::try_from(outcome.coverage.required).unwrap_or(i32::MAX);

        let existing = CoverageEntity::find()
            .filter(metric_monitor_coverage::Column::WorkspaceId.eq(workspace_id))
            .filter(metric_monitor_coverage::Column::Measure.eq(entry.measure.clone()))
            .filter(metric_monitor_coverage::Column::TimeDimension.eq(entry.time_dimension.clone()))
            .filter(metric_monitor_coverage::Column::Granularity.eq(granularity.clone()))
            .filter(metric_monitor_coverage::Column::DimensionKey.eq(dim_key.clone()))
            .one(db)
            .await?;

        if let Some(found) = existing {
            let mut active = found.into_active_model();
            active.granularity = Set(granularity);
            active.filters = Set(filters_json);
            active.label = Set(entry.label.clone());
            active.measured_buckets = Set(measured);
            active.required_buckets = Set(required);
            active.last_scanned_at = Set(now.into());
            active.update(db).await?;
        } else {
            metric_monitor_coverage::ActiveModel {
                id: Set(Uuid::new_v4()),
                workspace_id: Set(workspace_id),
                measure: Set(entry.measure.clone()),
                time_dimension: Set(entry.time_dimension.clone()),
                granularity: Set(granularity),
                dimension_key: Set(dim_key),
                filters: Set(filters_json),
                label: Set(entry.label.clone()),
                measured_buckets: Set(measured),
                required_buckets: Set(required),
                last_scanned_at: Set(now.into()),
            }
            .insert(db)
            .await?;
        }
        touched += 1;
    }

    prune_vanished_segments(db, workspace_id, &prune_keep_lists(scan)).await?;
    Ok(touched)
}

/// Which segments the prune must keep, per (measure, time-dim, granularity)
/// triple this scan covered.
///
/// Two rules, both about not deleting on incomplete information:
///
/// * Only triples with at least one **successful** outcome are pruned at all.
///   A `group_by` monitor whose segment discovery failed reports a single
///   failure for the parent entry; pruning that triple against just that one
///   key would delete every segment's row on a warehouse hiccup.
/// * Within a pruned triple, a segment that **errored** is kept. It has not
///   vanished — this scan simply does not know. No coverage row is written for
///   it either (the numbers would be stale but presented as fresh), so its last
///   known state stands until a scan can speak to it.
fn prune_keep_lists(scan: &ScanResult) -> HashMap<(String, String, String), HashSet<String>> {
    let triple_of = |entry: &MonitorEntry| {
        (
            entry.measure.clone(),
            entry.time_dimension.clone(),
            entry.granularity.airlayer_str().to_string(),
        )
    };

    let mut keep: HashMap<(String, String, String), HashSet<String>> = HashMap::new();
    for outcome in &scan.outcomes {
        keep.entry(triple_of(&outcome.entry))
            .or_default()
            .insert(MonitorFilter::key_for(&outcome.entry.filters));
    }
    for failure in &scan.failures {
        // `if let` rather than `entry()`: a failure must never *introduce* a
        // triple, only protect a key inside one the scan otherwise covered.
        if let Some(keys) = keep.get_mut(&triple_of(&failure.entry)) {
            keys.insert(MonitorFilter::key_for(&failure.entry.filters));
        }
    }
    keep
}

/// The delete for one scanned (measure, time-dim, granularity) triple: every
/// coverage row of that triple whose segment the scan did not produce.
///
/// Split out so the scoping — the subtle part, since this is the only code here
/// that deletes — can be asserted without a database. The triple filters are
/// what stop a granularity-filtered scan from wiping another grain's rows.
fn prune_query(
    workspace_id: Uuid,
    measure: &str,
    time_dimension: &str,
    granularity: &str,
    keys: &HashSet<String>,
) -> DeleteMany<CoverageEntity> {
    // Sorted so the generated statement is stable across runs; `HashSet`
    // iteration order is not.
    let mut keep: Vec<String> = keys.iter().cloned().collect();
    keep.sort();
    CoverageEntity::delete_many()
        .filter(metric_monitor_coverage::Column::WorkspaceId.eq(workspace_id))
        .filter(metric_monitor_coverage::Column::Measure.eq(measure.to_string()))
        .filter(metric_monitor_coverage::Column::TimeDimension.eq(time_dimension.to_string()))
        .filter(metric_monitor_coverage::Column::Granularity.eq(granularity.to_string()))
        .filter(metric_monitor_coverage::Column::DimensionKey.is_not_in(keep))
}

/// Delete coverage rows for segments this scan no longer produced.
async fn prune_vanished_segments(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    seen: &HashMap<(String, String, String), HashSet<String>>,
) -> Result<(), DbErr> {
    for ((measure, time_dimension, granularity), keys) in seen {
        prune_query(workspace_id, measure, time_dimension, granularity, keys)
            .exec(db)
            .await?;
    }
    Ok(())
}

/// Load the tail of every open anomaly event in a workspace, keyed by segment.
///
/// "Open" means not dismissed: dismissing an anomaly is the operator saying the
/// event is not real, so later buckets must argue their own case again rather
/// than inheriting a waived band from it.
///
/// Called once before a scan and handed to [`crate::scan_workspace`], which
/// keeps the scanning crate free of database access.
pub async fn load_open_events(
    db: &DatabaseConnection,
    workspace_id: Uuid,
) -> Result<OpenEvents, DbErr> {
    let rows = AnomaliesEntity::find()
        .filter(metric_anomalies::Column::WorkspaceId.eq(workspace_id))
        .filter(metric_anomalies::Column::Status.ne("dismissed"))
        .order_by_asc(metric_anomalies::Column::PeriodStart)
        .all(db)
        .await?;

    // Ascending order means the last write per segment wins, leaving each key
    // pointing at that segment's most recent flagged bucket.
    let mut out = OpenEvents::new();
    for row in rows {
        out.insert(
            SegmentKey {
                measure: row.measure,
                time_dimension: row.time_dimension,
                granularity: row.granularity,
                dimension_key: row.dimension_key,
            },
            Continuation {
                last_period: row.period_start.into(),
                is_decrease: row.observed < row.expected,
            },
        );
    }
    Ok(out)
}

/// The segment an anomaly row belongs to, as filter predicates.
///
/// Every lookup in this module scopes by exactly the four columns of
/// [`SegmentKey`] — including `granularity`. Leaving the grain out lets a
/// measure monitored daily *and* weekly cross-link: their buckets share a start
/// instant whenever a week opens on a scanned day, so a grain-blind query
/// returns the other monitor's row.
fn segment_scope(
    workspace_id: Uuid,
    entry: &MonitorEntry,
    dim_key: &str,
) -> Select<AnomaliesEntity> {
    AnomaliesEntity::find()
        .filter(metric_anomalies::Column::WorkspaceId.eq(workspace_id))
        .filter(metric_anomalies::Column::Measure.eq(entry.measure.clone()))
        .filter(metric_anomalies::Column::TimeDimension.eq(entry.time_dimension.clone()))
        .filter(
            metric_anomalies::Column::Granularity.eq(entry.granularity.airlayer_str().to_string()),
        )
        .filter(metric_anomalies::Column::DimensionKey.eq(dim_key.to_string()))
}

/// The earlier bucket that could carry this bucket's event, if any: same
/// segment, still open, within `CONTINUATION_GAP_BUCKETS` of it.
fn event_candidate_query(
    workspace_id: Uuid,
    entry: &MonitorEntry,
    dim_key: &str,
    period_start: DateTime<Utc>,
    earliest: DateTime<Utc>,
) -> Select<AnomaliesEntity> {
    segment_scope(workspace_id, entry, dim_key)
        .filter(metric_anomalies::Column::Status.ne("dismissed"))
        .filter(
            metric_anomalies::Column::PeriodStart
                .lt::<DateTime<chrono::FixedOffset>>(period_start.into()),
        )
        .filter(
            metric_anomalies::Column::PeriodStart
                .gte::<DateTime<chrono::FixedOffset>>(earliest.into()),
        )
        .order_by_desc(metric_anomalies::Column::PeriodStart)
}

/// The one row a re-scan of this bucket must update in place. Mirrors the
/// unique index (workspace, measure, time-dim, granularity, period, dim key).
fn existing_row_query(
    workspace_id: Uuid,
    entry: &MonitorEntry,
    dim_key: &str,
    period_start: DateTime<Utc>,
) -> Select<AnomaliesEntity> {
    segment_scope(workspace_id, entry, dim_key).filter(
        metric_anomalies::Column::PeriodStart
            .eq::<chrono::DateTime<chrono::FixedOffset>>(period_start.into()),
    )
}

/// The event this bucket belongs to: the one an earlier, same-direction,
/// still-open bucket of the same segment already belongs to, if that bucket is
/// close enough to be the same episode; otherwise a fresh event.
async fn resolve_event_id(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    entry: &MonitorEntry,
    dim_key: &str,
    period_start: DateTime<Utc>,
    is_decrease: bool,
) -> Result<Uuid, DbErr> {
    // Walk back `CONTINUATION_GAP_BUCKETS` periods in the monitor's own
    // calendar — a month is not 31 days and a DST week is not 168 hours, so
    // this cannot be done in fixed durations.
    let tz = entry.effective_timezone();
    let mut earliest = period_start.with_timezone(&tz).date_naive();
    for _ in 0..CONTINUATION_GAP_BUCKETS {
        earliest = retreat_period(earliest, entry.granularity);
    }
    let earliest = resolve_local_midnight(earliest, tz);

    let candidate = event_candidate_query(workspace_id, entry, dim_key, period_start, earliest)
        .one(db)
        .await?;

    if let Some(prev) = candidate
        && (prev.observed < prev.expected) == is_decrease
        && let Some(event_id) = prev.event_id
    {
        return Ok(event_id);
    }
    Ok(Uuid::new_v4())
}

async fn upsert_one(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    entry: &MonitorEntry,
    anomaly: &DetectedAnomaly,
    cohorts: &CohortPlan,
) -> Result<(), DbErr> {
    let now = Utc::now();
    let period_start = anomaly.timestamp;
    let period_end = period_end_for(period_start, entry);

    let dim_key = MonitorFilter::key_for(&entry.filters);
    let dim_key_for_event = dim_key.clone();
    let filters_json = if entry.filters.is_empty() {
        None
    } else {
        serde_json::to_value(&entry.filters).ok()
    };

    // Dominant (nearest) seasonal cycle from the monitor config. Snapshotted so
    // the explain path can align its comparison window to the same phase one
    // cycle back without re-reading the workspace config at request time.
    let seasonal_period = entry
        .effective_seasonality()
        .into_iter()
        .min()
        .map(|p| p as i32);

    let is_decrease = anomaly.residual < 0.0;

    // Recomputed from this scan rather than preserved: cohort membership is a
    // property of the scan that observed it, not a historical fact the way
    // `event_id` is. A re-scan that no longer sees a chain-wide drop should
    // clear the cohort, not keep asserting one.
    let cohort = cohorts.get(&(CohortKey::of(entry, anomaly), dim_key.clone()));
    let cohort_id = cohort.map(|(id, _, _)| *id);
    let cohort_deviation = cohort.and_then(|(_, d, _)| d.is_finite().then_some(*d));
    let cohort_label = cohort.and_then(|(_, _, l)| l.clone());

    let existing = existing_row_query(workspace_id, entry, &dim_key, period_start)
        .one(db)
        .await?;

    let severity = severity_to_str(anomaly.severity).to_string();

    if let Some(found) = existing {
        let preserved_status = if found.status == "dismissed" {
            "dismissed".to_string()
        } else {
            // Acknowledged → bumped back to new on a new detection so the
            // user sees the recurrence.
            "new".to_string()
        };
        let mut active = found.into_active_model();
        active.observed = Set(anomaly.observed);
        active.expected = Set(anomaly.expected);
        active.lower_bound = Set(anomaly.lower);
        active.upper_bound = Set(anomaly.upper);
        active.z_score = Set(anomaly.z_score);
        active.severity = Set(severity);
        active.status = Set(preserved_status);
        active.dimension_key = Set(dim_key);
        active.filters = Set(filters_json);
        active.seasonal_period = Set(seasonal_period);
        active.cohort_id = Set(cohort_id);
        active.cohort_deviation = Set(cohort_deviation);
        active.cohort_label = Set(cohort_label);
        // Keep an event already assigned: re-scoring a bucket must not split a
        // reported event in two. Only a row that never had one gets one now.
        if matches!(
            active.event_id,
            Set(None) | sea_orm::ActiveValue::Unchanged(None)
        ) {
            active.event_id = Set(Some(
                resolve_event_id(
                    db,
                    workspace_id,
                    entry,
                    &dim_key_for_event,
                    period_start,
                    is_decrease,
                )
                .await?,
            ));
        }
        active.updated_at = Set(now.into());
        active.update(db).await?;
    } else {
        metric_anomalies::ActiveModel {
            id: Set(Uuid::new_v4()),
            workspace_id: Set(workspace_id),
            measure: Set(entry.measure.clone()),
            time_dimension: Set(entry.time_dimension.clone()),
            granularity: Set(entry.granularity.airlayer_str().to_string()),
            period_start: Set(period_start.into()),
            period_end: Set(period_end.into()),
            observed: Set(anomaly.observed),
            expected: Set(anomaly.expected),
            lower_bound: Set(anomaly.lower),
            upper_bound: Set(anomaly.upper),
            z_score: Set(anomaly.z_score),
            severity: Set(severity),
            status: Set("new".to_string()),
            label: Set(entry.label.clone()),
            dimension_key: Set(dim_key),
            filters: Set(filters_json),
            seasonal_period: Set(seasonal_period),
            event_id: Set(Some(
                resolve_event_id(
                    db,
                    workspace_id,
                    entry,
                    &dim_key_for_event,
                    period_start,
                    is_decrease,
                )
                .await?,
            )),
            // Lazy: populated on first /explain call.
            explain_cache: Set(None),
            explain_cached_at: Set(None),
            // Assigned by the scan-wide cohort pass, not per row — a cohort is
            // a property of the whole scan and `upsert_one` sees one row.
            cohort_id: Set(cohort_id),
            cohort_deviation: Set(cohort_deviation),
            cohort_label: Set(cohort_label),
            detected_at: Set(now.into()),
            updated_at: Set(now.into()),
        }
        .insert(db)
        .await?;
    }
    Ok(())
}

pub(crate) fn severity_to_str(s: Severity) -> &'static str {
    match s {
        Severity::Low => "low",
        Severity::Medium => "medium",
        Severity::High => "high",
    }
}

/// The exclusive end of the bucket starting at `start`, computed in the
/// monitor's LOCAL calendar. A local month is not 31 UTC days, and a DST week
/// is not exactly 168 hours, so this must round-trip through the timezone.
fn period_end_for(start: DateTime<Utc>, entry: &MonitorEntry) -> DateTime<Utc> {
    let tz = entry.effective_timezone();
    let local = start.with_timezone(&tz).date_naive();
    let next = advance_period(local, entry.granularity);
    resolve_local_midnight(next, tz)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Direction, Granularity, Sensitivity};
    use chrono::NaiveDate;
    use sea_orm::QueryTrait;

    fn entry_at(granularity: Granularity, tz: &str) -> MonitorEntry {
        MonitorEntry {
            measure: "x.y".into(),
            time_dimension: "x.t".into(),
            granularity,
            lookback_days: 90,
            seasonality: Some(vec![7]),
            sensitivity: Sensitivity::Medium,
            label: None,
            filters: vec![],
            group_by: None,
            direction: Direction::Both,
            timezone: Some(tz.into()),
            freshness: None,
            week_start: None,
        }
    }

    fn sql_of(stmt: sea_orm::Statement) -> String {
        stmt.to_string()
    }

    /// Both row lookups must name the grain, or a measure monitored daily *and*
    /// weekly cross-links: a Monday daily bucket and the weekly bucket opening
    /// that same Monday share a `period_start`, so a grain-blind query returns
    /// the other monitor's row — the daily bucket inherits the weekly event id
    /// (and, via the `existing` lookup, overwrites the weekly row outright).
    #[test]
    fn segment_lookups_are_scoped_to_one_granularity() {
        let workspace_id = Uuid::nil();
        let period_start = NaiveDate::from_ymd_opt(2026, 7, 27)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap()
            .and_utc();
        let earliest = period_start - chrono::Duration::days(3);

        for granularity in [Granularity::Day, Granularity::Week] {
            let entry = entry_at(granularity, "UTC");
            let grain = granularity.airlayer_str();

            let candidate = sql_of(
                event_candidate_query(workspace_id, &entry, "", period_start, earliest)
                    .build(sea_orm::DbBackend::Postgres),
            );
            assert!(
                candidate.contains(&format!(r#""granularity" = '{grain}'"#)),
                "event candidate query is not scoped to {grain}: {candidate}"
            );

            let existing = sql_of(
                existing_row_query(workspace_id, &entry, "", period_start)
                    .build(sea_orm::DbBackend::Postgres),
            );
            assert!(
                existing.contains(&format!(r#""granularity" = '{grain}'"#)),
                "existing-row query is not scoped to {grain}: {existing}"
            );
        }
    }

    fn segment(granularity: Granularity, member: &str, value: &str) -> MonitorEntry {
        let mut entry = entry_at(granularity, "UTC");
        entry.filters = vec![MonitorFilter {
            member: member.into(),
            values: vec![value.into()],
        }];
        entry
    }

    fn outcome(entry: MonitorEntry) -> crate::service::MonitorOutcome {
        crate::service::MonitorOutcome {
            entry,
            anomalies: vec![],
            coverage: crate::service::Coverage {
                measured: 60,
                required: 56,
            },
        }
    }

    fn failure(entry: MonitorEntry) -> crate::service::MonitorFailure {
        crate::service::MonitorFailure {
            entry,
            error: crate::service::ScanError::ParseTimestamp {
                ts: "not-a-date".into(),
                source: "not-a-date".parse::<chrono::NaiveDate>().unwrap_err(),
            },
        }
    }

    /// The bucket every cohort fixture fires on.
    fn fired_at() -> DateTime<Utc> {
        NaiveDate::from_ymd_opt(2026, 7, 4)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap()
            .and_utc()
    }

    fn cohort_key() -> CohortKey {
        CohortKey {
            measure: "x.y".into(),
            time_dimension: "x.t".into(),
            granularity: "day".into(),
            period_start: fired_at(),
            is_decrease: true,
        }
    }

    fn seg_key(i: usize) -> String {
        MonitorFilter::key_for(&[MonitorFilter {
            member: "x.store".into(),
            values: vec![i.to_string()],
        }])
    }

    /// One anomaly at [`fired_at`], `observed = 1000 * ratio` against an
    /// expectation of 1000. Below 1.0 is a drop, above it a spike.
    fn anomaly_at(ratio: f64) -> DetectedAnomaly {
        let expected = 1000.0;
        let observed = expected * ratio;
        DetectedAnomaly {
            timestamp: fired_at(),
            observed,
            expected,
            lower: 900.0,
            upper: 1100.0,
            residual: observed - expected,
            z_score: -4.0,
            severity: Severity::High,
        }
    }

    fn cohort_outcome(
        i: usize,
        anomalies: Vec<DetectedAnomaly>,
        measured: usize,
    ) -> crate::service::MonitorOutcome {
        crate::service::MonitorOutcome {
            entry: segment(Granularity::Day, "x.store", &i.to_string()),
            anomalies,
            coverage: crate::service::Coverage {
                measured,
                required: 56,
            },
        }
    }

    /// `scanned` scored segments of one measure; the first `fired` of them carry
    /// one anomaly whose observed/expected ratio is `ratio(i)`.
    fn scan_with_segments(
        scanned: usize,
        fired: usize,
        ratio: impl Fn(usize) -> f64,
    ) -> ScanResult {
        ScanResult {
            outcomes: (0..scanned)
                .map(|i| {
                    let anomalies = if i < fired {
                        vec![anomaly_at(ratio(i))]
                    } else {
                        vec![]
                    };
                    cohort_outcome(i, anomalies, 100)
                })
                .collect(),
            failures: vec![],
            calendar: None,
            notify: None,
        }
    }

    /// As [`scan_with_segments`], plus `warming_up` segments that were never
    /// scored — `measured` short of `required`.
    fn scan_with_warming_up(scanned: usize, fired: usize, warming_up: usize) -> ScanResult {
        let mut scan = scan_with_segments(scanned, fired, |_| 0.60);
        scan.outcomes
            .extend((scanned..scanned + warming_up).map(|i| cohort_outcome(i, vec![], 10)));
        scan
    }

    #[test]
    fn a_universal_simultaneous_drop_becomes_one_cohort() {
        // 21 of 21 stores below their own expectation on the same bucket —
        // the 07-04 shape, which filed 21 separate rows.
        let scan = scan_with_segments(21, 21, |i| 0.60 + (i as f64) * 0.01);
        let plan = plan_cohorts(&scan);
        assert_eq!(plan.len(), 21, "every fired segment joins the cohort");
        let ids: HashSet<_> = plan.values().map(|(id, _, _)| *id).collect();
        assert_eq!(ids.len(), 1, "and they share one id");
    }

    #[test]
    fn cohort_ids_are_stable_across_rescans() {
        // Restatement re-scans the same buckets for days. Planning the same
        // scan twice must yield the same cohort id per segment, or every
        // re-scan churns the id and invalidates any reference to it.
        let scan = scan_with_segments(21, 21, |i| 0.60 + (i as f64) * 0.01);
        let first = plan_cohorts(&scan);
        let second = plan_cohorts(&scan);
        assert_eq!(first.len(), second.len());
        for (k, (id, _, _)) in &first {
            assert_eq!(
                second.get(k).map(|(id, _, _)| id),
                Some(id),
                "cohort id changed across an identical re-scan"
            );
        }
    }

    #[test]
    fn an_isolated_segment_gets_no_cohort() {
        // One store of 21. That is a store problem, not an event.
        let scan = scan_with_segments(21, 1, |_| 0.40);
        assert!(plan_cohorts(&scan).is_empty());
    }

    #[test]
    fn cohort_deviation_ranks_members_against_the_cluster() {
        // Nineteen stores near the cluster median, two far below it — the
        // rows worth acting on, currently buried among the identical ones.
        let scan = scan_with_segments(21, 21, |i| match i {
            0 => 0.18,
            1 => 0.00,
            _ => 0.72,
        });
        let plan = plan_cohorts(&scan);
        let dev = |i: usize| plan.get(&(cohort_key(), seg_key(i))).unwrap().1;
        assert!((dev(2) - 1.0).abs() < 1e-9, "a typical member sits at 1.0");
        assert!(dev(0) < 0.3, "the 0.18 store ranks well below the cluster");
        assert!(dev(1) < dev(0), "and the 0.00 store below that");
    }

    #[test]
    fn a_warming_up_segment_is_not_counted_in_the_denominator() {
        // A segment that was never scored cannot have failed to fire, so
        // counting it would drag the share below the threshold and hide the
        // event. `coverage.measured < coverage.required` marks these.
        let scan = scan_with_warming_up(10, 10, 40);
        assert!(!plan_cohorts(&scan).is_empty());
    }

    /// A segment that fell and one that rose are not the same event, however
    /// simultaneous. Splitting on direction is what keeps a cohort meaning
    /// "one thing happened to all of these".
    #[test]
    fn opposite_directions_are_not_one_cohort() {
        let scan = scan_with_segments(10, 10, |i| if i < 5 { 0.60 } else { 1.40 });
        let plan = plan_cohorts(&scan);
        // Five of ten in each direction is below COHORT_SHARE_MIN, so neither
        // half forms a cohort — which is the point: the shares are counted
        // separately rather than summing to 10/10.
        assert!(plan.is_empty(), "directions must not pool their share");
    }

    /// Ten segments in `tz`, all dropping on `bucket`, against `calendar`.
    fn scan_on(bucket: DateTime<Utc>, tz: &str, calendar: &[(NaiveDate, &str)]) -> ScanResult {
        let outcomes = (0..10)
            .map(|i| {
                let mut entry = segment(Granularity::Day, "x.store", &i.to_string());
                entry.timezone = Some(tz.into());
                let mut anomaly = anomaly_at(0.60);
                anomaly.timestamp = bucket;
                crate::service::MonitorOutcome {
                    entry,
                    anomalies: vec![anomaly],
                    coverage: crate::service::Coverage {
                        measured: 100,
                        required: 56,
                    },
                }
            })
            .collect();
        ScanResult {
            outcomes,
            failures: vec![],
            calendar: Some(
                calendar
                    .iter()
                    .map(|(d, l)| (*d, (*l).to_string()))
                    .collect(),
            ),
            notify: None,
        }
    }

    fn labels_of(scan: &ScanResult) -> HashSet<Option<String>> {
        plan_cohorts(scan)
            .into_values()
            .map(|(_, _, label)| label)
            .collect()
    }

    /// The calendar is keyed by the monitor's *local* calendar date. Looking
    /// the bucket up by its UTC date is wrong for any timezone ahead of UTC —
    /// Tokyo's 2025-07-04 opens at 2025-07-03T15:00Z — and the label would
    /// silently land on the wrong day, or on no day at all.
    #[test]
    fn a_cohort_takes_its_label_from_the_local_calendar_date() {
        let holiday = NaiveDate::from_ymd_opt(2025, 7, 4).unwrap();
        let cal = [(holiday, "Independence Day")];

        for (tz, on_the_day, the_day_after) in [
            // Behind UTC: local midnight is the same UTC date.
            (
                "America/Los_Angeles",
                "2025-07-04T07:00:00Z",
                "2025-07-05T07:00:00Z",
            ),
            // Ahead of UTC: local midnight is the *previous* UTC date, which
            // is what a UTC-keyed lookup gets wrong.
            ("Asia/Tokyo", "2025-07-03T15:00:00Z", "2025-07-04T15:00:00Z"),
        ] {
            let parse = |s: &str| s.parse::<DateTime<Utc>>().unwrap();
            assert_eq!(
                labels_of(&scan_on(parse(on_the_day), tz, &cal)),
                HashSet::from([Some("Independence Day".to_string())]),
                "{tz} missed its own holiday"
            );
            assert_eq!(
                labels_of(&scan_on(parse(the_day_after), tz, &cal)),
                HashSet::from([None]),
                "{tz} labelled the day after the holiday"
            );
        }
    }

    /// A segment that errored has not vanished — the scan just can't speak to
    /// it. Deleting its coverage row on a warehouse hiccup flips a "Warming up"
    /// badge to "—" for a cycle, which reads as "this monitor is fine".
    #[test]
    fn a_failed_segment_is_kept_not_pruned() {
        let scan = ScanResult {
            outcomes: vec![outcome(segment(Granularity::Day, "x.store", "1"))],
            failures: vec![failure(segment(Granularity::Day, "x.store", "2"))],
            calendar: None,
            notify: None,
        };

        let keep = prune_keep_lists(&scan);
        let keys = keep
            .get(&("x.y".into(), "x.t".into(), "day".into()))
            .expect("the scanned triple is pruned");
        assert!(
            keys.contains("x.store=2"),
            "the errored segment was left to be pruned: {keys:?}"
        );
        assert!(keys.contains("x.store=1"));
    }

    /// When segment discovery itself fails there is one failure for the parent
    /// entry and no outcomes. Pruning that triple against a single key would
    /// delete every segment's coverage row, so the triple must not be pruned.
    #[test]
    fn a_triple_with_only_failures_is_not_pruned_at_all() {
        let scan = ScanResult {
            outcomes: vec![],
            failures: vec![failure(entry_at(Granularity::Day, "UTC"))],
            calendar: None,
            notify: None,
        };

        assert!(
            prune_keep_lists(&scan).is_empty(),
            "a triple the scan never succeeded on must not be pruned"
        );
    }

    /// The prune is the only code here that deletes, and its scoping is what
    /// keeps a granularity-filtered scan from wiping another grain's coverage:
    /// the `NOT IN (seen keys)` is only safe because the triple pins it to the
    /// exact (measure, time-dim, grain) this scan actually covered.
    #[test]
    fn prune_is_scoped_to_the_scanned_triple() {
        let seen: HashSet<String> = ["store=1".to_string(), "store=2".to_string()]
            .into_iter()
            .collect();
        let sql = sql_of(
            prune_query(Uuid::nil(), "x.y", "x.t", "day", &seen)
                .build(sea_orm::DbBackend::Postgres),
        );

        for predicate in [
            r#""measure" = 'x.y'"#,
            r#""time_dimension" = 'x.t'"#,
            r#""granularity" = 'day'"#,
            r#""dimension_key" NOT IN ('store=1', 'store=2')"#,
        ] {
            assert!(
                sql.contains(predicate),
                "prune lost its {predicate} scope: {sql}"
            );
        }
    }

    #[test]
    fn period_end_for_week_survives_a_spring_forward_gap_in_santiago() {
        // Sunday 2026-08-30 00:00 -04:00 (Chile Standard Time) = 04:00 UTC.
        // `advance_period` steps a Week monitor 7 days to 2026-09-06, which
        // has no local midnight in America/Santiago (it springs forward
        // 00:00 -> 01:00 -03:00 that day). The granularity-blind
        // `start + 1 day` fallback this replaced would report period_end
        // ~1 day after start instead of ~7 — silently corrupting the stored
        // anomaly row for every Week/Month monitor that hits a DST gap.
        let start = NaiveDate::from_ymd_opt(2026, 8, 30)
            .unwrap()
            .and_hms_opt(4, 0, 0)
            .unwrap()
            .and_utc();
        let entry = entry_at(Granularity::Week, "America/Santiago");
        // Chile's 2026 spring-forward gap is 2026-09-06 00:00 -> 01:00, at the
        // instant 04:00 UTC, so `resolve_local_midnight` lands on the first
        // valid local time after midnight: 01:00 -03:00 = 04:00 UTC. Asserted
        // exactly — a `>= 6 days` bound would also pass for a 6-day answer and
        // so could not catch the off-by-one-day class this test guards.
        let end = period_end_for(start, &entry);
        assert_eq!(
            end,
            NaiveDate::from_ymd_opt(2026, 9, 6)
                .unwrap()
                .and_hms_opt(4, 0, 0)
                .unwrap()
                .and_utc(),
            "expected the week boundary at Chile's post-gap local midnight"
        );
    }
}
