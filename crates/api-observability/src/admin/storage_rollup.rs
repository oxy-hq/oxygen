//! Custom-app storage held per org over a window, in GB-months.
//!
//! The arithmetic is **not** here: `oxy_custom_apps::storage::metering::`
//! `time_weighted_average_bytes` owns what a GB-month is, and this module only
//! feeds it. That split is deliberate — the algorithm carries decisions
//! (integrate, don't peak; time-weight, don't average the samples; fetch the
//! reading *before* the window so an app's opening stretch is not lost) that a
//! second copy would get subtly wrong for a mid-period app.
//!
//! What is here is the **batching**, plus a resolution choice. Every
//! `/admin/observability` storage figure reads this module: the fleet list's
//! column over many orgs, and the per-org Resources panel over one (as
//! `fleet_storage(db, &[org_id], ..)` — it takes the same path rather than a
//! second one, so the two surfaces cannot disagree). `metering::meter_org` is
//! the **only** caller that integrates raw samples, and it is the billing path
//! behind Admin → Custom apps.
//!
//! ## One reading per day, and that is the grade
//!
//! This path differs from `meter_org` in one way, deliberately: samples are
//! **collapsed to one row per app per day** in SQL before the integral runs.
//! The sweeper appends a sample every tick (~96 per app per day), so a 30-day
//! window over a few hundred apps is otherwise hundreds of thousands of rows
//! deserialized per uncached page load. `metering::load_history` collapses the
//! same way, for the same reason, and keeps each day's **closing** reading —
//! which is what `ORDER BY … measured_at DESC` inside each `DISTINCT ON` group
//! selects.
//!
//! The collapse stays here rather than moving down into `metering`. A
//! time-weighted integral is **not** invariant under it: `load_history` can
//! collapse safely because `daily_series` reduces each day to its closing
//! reading anyway, but integrating closing readings would change what
//! `meter_org` meters, and that is a *billed* quantity. Re-grading the billing
//! path is not this module's call.
//!
//! Daily is the right grade for what reads this. The windows here are
//! operator-chosen, 7 to 365 days, with no billing period to agree with —
//! Admin → Custom apps meters month-to-date, so the periods coincide only by
//! accident and matching the arithmetic was never going to match the number.
//! Storage also moves slowly, so sub-daily resolution over a month-plus window
//! buys precision nobody reads. An earlier pass did point the per-org panel at
//! `meter_org` for exactly that imagined agreement, and paid ~39k rows per app
//! per request for it; `internal-docs/2026-11-16-admin-observability-design.md`
//! §8.5 records why that was wrong.
//!
//! ## Two statements, not one
//!
//! In-window samples alone under-count: an app sampled on the 1st and not
//! again until the 20th has no reading inside a window starting on the 10th,
//! and would meter as zero despite holding bytes the whole time. So the
//! opening reading — the last sample at or before the window start, per app —
//! is fetched separately and prepended.
//!
//! **Both reads are bounded at [`SAMPLE_LOOKBACK_DAYS`].** `meter_org` bounds
//! its own at 45 days on purpose: samples are retained for 400, so an
//! unbounded opening read walks an app's whole history, and — worse — an app
//! last sampled 300 days ago would be metered for the window off that stale
//! reading. Dropping the bound while reusing the pure function is exactly the
//! divergence the split was meant to prevent.

use std::collections::HashMap;

use chrono::{DateTime, FixedOffset};
use oxy_custom_apps::storage::metering::time_weighted_average_bytes;
use sea_orm::{DatabaseBackend, DbErr, FromQueryResult, Statement};
use uuid::Uuid;

/// Bytes per GiB, the unit `metering` reports in and the one the admin storage
/// surface already shows.
const BYTES_PER_GIB: f64 = 1024.0 * 1024.0 * 1024.0;

/// How far before the window to look for an opening reading.
///
/// Mirrors `metering::SAMPLE_LOOKBACK_DAYS`, which is private. The number is
/// the same 45 for the same two reasons: samples are kept 400 days, so an
/// unbounded look-back scans an app's whole history; and an app with no sample
/// in 45 days is pathological, so metering it off that reading would bill
/// stale data rather than report missing data.
const SAMPLE_LOOKBACK_DAYS: i64 = 45;

#[derive(Debug, FromQueryResult)]
struct AppOrgRow {
    app_id: Uuid,
    org_id: Uuid,
}

#[derive(Debug, FromQueryResult)]
struct SampleRow {
    app_id: Uuid,
    measured_at: DateTime<FixedOffset>,
    bytes: i64,
}

/// `app_id → org_id` for every app of the given orgs.
///
/// From `app_storage_usage`, which carries `org_id` directly, rather than a
/// join through `apps`: the question is "which apps hold storage", and an app
/// with no storage row has never held any.
fn app_owner_sql(org_ids: &[Uuid], values: &mut Vec<sea_orm::Value>) -> String {
    let placeholders = org_ids
        .iter()
        .map(|id| {
            values.push((*id).into());
            format!("${}", values.len())
        })
        .collect::<Vec<_>>()
        .join(",");
    format!("SELECT app_id, org_id FROM app_storage_usage WHERE org_id IN ({placeholders})")
}

/// One row per app per day inside the window — each day's closing reading.
///
/// `AT TIME ZONE 'UTC'` is load-bearing, not decoration, and the reason is
/// `metering::load_history`'s: `measured_at` is `timestamptz`, so a bare
/// `date_trunc('day', …)` buckets by the connection's `TimeZone` GUC. On a
/// deployment whose Postgres is not UTC the bucket boundaries would not line
/// up with the window's own UTC bounds, and each day would keep the wrong
/// closing sample.
///
/// `= ANY($n)` over a `uuid[]` rather than an expanded `IN (…)`: one bound
/// value, so the statement shape stays constant however many apps a fleet has
/// — which is what lets Postgres reuse a plan across page loads.
fn in_window_sql(values: &mut Vec<sea_orm::Value>, app_ids: &[Uuid]) -> String {
    values.push(app_ids.to_vec().into());
    let apps = format!("${}", values.len());
    // `$1`/`$2` are the window bounds, bound before the app list.
    format!(
        "SELECT DISTINCT ON (app_id, date_trunc('day', measured_at AT TIME ZONE 'UTC')) \
         app_id, measured_at, bytes \
         FROM app_storage_usage_samples \
         WHERE app_id = ANY({apps}) AND measured_at >= $1 AND measured_at < $2 \
         ORDER BY app_id, date_trunc('day', measured_at AT TIME ZONE 'UTC'), measured_at DESC"
    )
}

/// The last sample at or before the window start, one per app.
///
/// Without it an app that was sampled before the window and not again inside
/// it meters as zero, though it held bytes throughout. `DISTINCT ON` is the
/// same shape the weekly usage report uses for its own opening read.
fn opening_sql(values: &mut Vec<sea_orm::Value>, app_ids: &[Uuid]) -> String {
    values.push(app_ids.to_vec().into());
    let apps = format!("${}", values.len());
    // `$1` is the window start, `$2` the look-back floor.
    format!(
        "SELECT DISTINCT ON (app_id) app_id, measured_at, bytes \
         FROM app_storage_usage_samples \
         WHERE app_id = ANY({apps}) AND measured_at < $1 AND measured_at >= $2 \
         ORDER BY app_id, measured_at DESC"
    )
}

/// One org's metered storage.
#[derive(Debug, Clone, Default, PartialEq)]
pub(super) struct OrgStorage {
    /// Time-weighted mean bytes held across the window.
    pub(super) average_bytes: f64,
    /// The same, in GiB — the quantity this surface prices at S3 list. Phrased
    /// as "the billable quantity" in `metering::AppMeter`, where it is one;
    /// here it is a daily-resolution figure priced for an operator, and the
    /// billed number is `meter_org`'s.
    pub(super) gib: f64,
    /// Apps that contributed a reading. Zero means **no data**, which is not
    /// the same as no usage; the caller must not render it as a measured zero.
    pub(super) apps_with_samples: usize,
}

/// Metered storage for each of `org_ids` over `[start, end)`, keyed by org.
///
/// An org absent from the map had no app with a storage row at all. An org
/// present with `apps_with_samples == 0` has apps but no readings — the
/// distinction `metering`'s own `OrgMeter` draws with `apps_without_samples`,
/// and for the same reason: an under-count must not be shown as a figure.
pub(super) async fn fleet_storage(
    db: &sea_orm::DatabaseConnection,
    org_ids: &[Uuid],
    start: DateTime<FixedOffset>,
    end: DateTime<FixedOffset>,
) -> Result<HashMap<Uuid, OrgStorage>, DbErr> {
    if org_ids.is_empty() {
        return Ok(HashMap::new());
    }

    let mut values: Vec<sea_orm::Value> = Vec::new();
    let sql = app_owner_sql(org_ids, &mut values);
    let owners = AppOrgRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        sql,
        values,
    ))
    .all(db)
    .await?;
    if owners.is_empty() {
        return Ok(HashMap::new());
    }

    let app_ids: Vec<Uuid> = owners.iter().map(|o| o.app_id).collect();
    let owner_of: HashMap<Uuid, Uuid> = owners.iter().map(|o| (o.app_id, o.org_id)).collect();

    // In-window samples: $1 = start, $2 = end, then the app list.
    let mut values: Vec<sea_orm::Value> = vec![start.into(), end.into()];
    let sql = in_window_sql(&mut values, &app_ids);
    let mut samples_by_app: HashMap<Uuid, Vec<(DateTime<FixedOffset>, i64)>> = HashMap::new();
    for row in SampleRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        sql,
        values,
    ))
    .all(db)
    .await?
    {
        samples_by_app
            .entry(row.app_id)
            .or_default()
            .push((row.measured_at, row.bytes));
    }

    // Opening readings: $1 = start, $2 = the look-back floor, then the apps.
    let lookback = start - chrono::Duration::days(SAMPLE_LOOKBACK_DAYS);
    let mut values: Vec<sea_orm::Value> = vec![start.into(), lookback.into()];
    let sql = opening_sql(&mut values, &app_ids);
    for row in SampleRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        sql,
        values,
    ))
    .all(db)
    .await?
    {
        // Prepended, so the series stays ascending for the pure function —
        // which documents that its input must be sorted.
        samples_by_app
            .entry(row.app_id)
            .or_default()
            .insert(0, (row.measured_at, row.bytes));
    }

    let mut out: HashMap<Uuid, OrgStorage> = HashMap::new();
    for app_id in &app_ids {
        let Some(org_id) = owner_of.get(app_id) else {
            continue;
        };
        let entry = out.entry(*org_id).or_default();
        let Some(samples) = samples_by_app.get(app_id) else {
            continue;
        };
        if samples.is_empty() {
            continue;
        }
        entry.average_bytes += time_weighted_average_bytes(samples, start, end);
        entry.apps_with_samples += 1;
    }
    for v in out.values_mut() {
        v.gib = v.average_bytes / BYTES_PER_GIB;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The window bounds bind before the app list, and the opening read takes
    /// both the start and the look-back floor.
    #[test]
    fn window_bounds_bind_before_the_app_list() {
        let apps = [Uuid::nil(), Uuid::from_u128(2)];

        let mut values: Vec<sea_orm::Value> = vec![0i64.into(), 0i64.into()];
        let sql = in_window_sql(&mut values, &apps);
        assert!(sql.contains("app_id = ANY($3)"), "{sql}");
        assert!(
            sql.contains("measured_at >= $1 AND measured_at < $2"),
            "{sql}"
        );
        assert_eq!(values.len(), 3, "two bounds plus one array");

        let mut values: Vec<sea_orm::Value> = vec![0i64.into(), 0i64.into()];
        let sql = opening_sql(&mut values, &apps);
        assert!(sql.contains("app_id = ANY($3)"), "{sql}");
        assert!(
            sql.contains("measured_at < $1 AND measured_at >= $2"),
            "{sql}"
        );
    }

    /// **The opening read is bounded.** Samples are kept 400 days, so without
    /// a floor this walks an app's whole history — and worse, an app last
    /// sampled 300 days ago would be metered for the window off that stale
    /// reading. `metering::meter_org` bounds its own read at 45 days for
    /// exactly these two reasons; dropping the bound while reusing its pure
    /// function is the divergence the split exists to prevent.
    #[test]
    fn the_opening_read_is_bounded_at_the_lookback() {
        let sql = opening_sql(&mut vec![0i64.into(), 0i64.into()], &[Uuid::nil()]);
        assert!(
            sql.contains("measured_at >= $2"),
            "unbounded opening read: {sql}"
        );
        assert_eq!(SAMPLE_LOOKBACK_DAYS, 45, "must match metering's own bound");
    }

    /// Samples are collapsed to one row per app per day in SQL. The sweeper
    /// appends ~96 a day, so a 30-day window over a few hundred apps is
    /// otherwise hundreds of thousands of rows deserialized per page load.
    /// `load_history` collapses the same way.
    #[test]
    fn in_window_samples_are_collapsed_to_one_row_per_app_per_day() {
        let sql = in_window_sql(&mut vec![0i64.into(), 0i64.into()], &[Uuid::nil()]);
        assert!(
            sql.contains("DISTINCT ON (app_id, date_trunc('day', measured_at AT TIME ZONE 'UTC'))"),
            "{sql}"
        );
        // Each group keeps the day's CLOSING reading, which is what a step
        // function integrated by day needs.
        assert!(sql.contains("measured_at DESC"), "{sql}");
    }

    /// `AT TIME ZONE 'UTC'` is load-bearing. `measured_at` is `timestamptz`,
    /// so a bare `date_trunc('day', …)` buckets by the connection's `TimeZone`
    /// GUC, and on a non-UTC Postgres the buckets would not line up with the
    /// window's own UTC bounds — each day would keep the wrong closing sample.
    #[test]
    fn day_buckets_are_utc_not_the_connections_timezone() {
        let sql = in_window_sql(&mut vec![0i64.into(), 0i64.into()], &[Uuid::nil()]);
        assert_eq!(
            sql.matches("AT TIME ZONE 'UTC'").count(),
            2,
            "both the DISTINCT ON and the ORDER BY need it: {sql}"
        );
    }

    /// The app list is one bound array, not an expanded `IN (…)`. The
    /// statement shape then stays constant however many apps a fleet has,
    /// which is what lets Postgres reuse a plan across page loads — and an
    /// empty list correctly matches nothing instead of failing to parse.
    #[test]
    fn the_app_list_is_one_bound_array_not_an_expanded_in_list() {
        for n in [1usize, 2, 50] {
            let apps: Vec<Uuid> = (0..n).map(|i| Uuid::from_u128(i as u128)).collect();
            let mut values: Vec<sea_orm::Value> = vec![0i64.into(), 0i64.into()];
            let sql = in_window_sql(&mut values, &apps);
            assert!(sql.contains("= ANY($3)"), "{sql}");
            assert_eq!(values.len(), 3, "the shape must not grow with the fleet");
        }
    }

    /// An app whose only reading predates the window still meters its bytes —
    /// the case the opening read exists for, now bounded.
    #[test]
    fn an_app_sampled_only_before_the_window_still_meters() {
        let start: DateTime<FixedOffset> = "2026-11-01T00:00:00Z".parse().unwrap();
        let end: DateTime<FixedOffset> = "2026-12-01T00:00:00Z".parse().unwrap();
        let before: DateTime<FixedOffset> = "2026-10-15T00:00:00Z".parse().unwrap();
        let ten_gib = (10.0 * BYTES_PER_GIB) as i64;

        let held = time_weighted_average_bytes(&[(before, ten_gib)], start, end);
        assert!(
            (held / BYTES_PER_GIB - 10.0).abs() < 0.001,
            "a reading before the window must hold through it, got {held}"
        );
        assert_eq!(time_weighted_average_bytes(&[], start, end), 0.0);
    }

    /// An org with apps but no readings is `apps_with_samples == 0`, which the
    /// caller renders as "no data" rather than a measured zero.
    #[test]
    fn no_readings_is_distinguishable_from_no_usage() {
        let empty = OrgStorage::default();
        assert_eq!(empty.apps_with_samples, 0);
        assert_eq!(empty.gib, 0.0);
    }

    /// GiB is bytes over 1024³, matching what the admin storage surface
    /// already reports.
    #[test]
    fn a_gibibyte_is_the_binary_one() {
        assert_eq!(BYTES_PER_GIB, 1_073_741_824.0);
    }
}
