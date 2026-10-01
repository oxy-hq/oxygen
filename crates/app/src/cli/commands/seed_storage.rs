//! Seed custom-app storage usage, so the admin Storage tab has something to
//! show on a fresh clone.
//!
//! ## What it is for
//!
//! The Storage surface is mostly *states*: an app growing fast, an app whose
//! bytes nothing will ever reclaim, a measurement that only got a floor, an app
//! never measured at all. Each renders differently and each is a different
//! operator decision — and with an empty database every one of them is
//! indistinguishable from "no data". This seeds one app per state so the UI can
//! be looked at, and so a change to it can be reviewed by eye.
//!
//! ## Deterministic, not random
//!
//! Every number here derives from the app's UUID and the day offset. Re-running
//! the seed produces the same fleet, which is what makes a screenshot diff or a
//! browser test meaningful — a random walk would make both useless.
//!
//! ## Why it writes real objects too
//!
//! Half the surface is the per-app file browser, and that reads S3/the local
//! filesystem live rather than any table. Seeding rows alone would leave it
//! permanently empty, so a handful of real objects are written per app under the
//! same prefixes the rollup claims.

use std::collections::HashMap;

use chrono::{DateTime, Duration, FixedOffset, Utc};
use sea_orm::ActiveValue::Set;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use uuid::Uuid;

use entity::app_storage_usage::measure_status;
use entity::prelude::{AppStorageUsage, AppStorageUsageSamples};
use entity::{app_storage_usage, app_storage_usage_samples};
use oxy::theme::StyledText;
use oxy_shared::errors::OxyError;

use crate::server::api::custom_apps_storage::usage::PrefixUsage;

const MIB: i64 = 1024 * 1024;
const GIB: i64 = 1024 * MIB;

/// Days of history seeded. Enough for the 30- and 90-day chart ranges and for a
/// 7-day growth figure to be real rather than absent.
const HISTORY_DAYS: i64 = 90;

/// The byte-shape of one seeded app's history. Each exists because it renders a
/// different state in the UI and implies a different operator response.
///
/// **Exactly as many variants as `oxy seed` can create apps.** The seed builds at
/// most three targets (the demo app, plus Acme's open and restricted pair when
/// the partner seed ran — `seed::deploy_example_apps`), so a fourth variant would
/// be assigned to an app that does not exist and would simply never render.
/// `three_apps_cover_every_byte_shape` fails the build if that drifts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Profile {
    /// Large and compounding, almost none of it covered by a retention rule.
    /// This is the surprise-bill case the fleet view exists to surface — it
    /// should sort to the top of both "size" and "7d growth".
    RunawayUntagged,
    /// Steady, fully covered by retention. The control: proves a healthy app
    /// looks visibly different from an unhealthy one.
    HealthyFlat,
    /// A big export written then deleted. Exercises the metering argument — its
    /// GB-month is modest despite a large peak, and its growth figure is negative.
    SpikeThenDelete,
}

/// Every shape, in assignment order — the first app is always the interesting one.
const PROFILES: [Profile; 3] = [
    Profile::RunawayUntagged,
    Profile::HealthyFlat,
    Profile::SpikeThenDelete,
];

/// Which app reports an incomplete walk.
///
/// `measure_status` is **orthogonal to the byte shape** — a walk can truncate on
/// any app, whatever its history looks like — so it is assigned by position
/// rather than folded into `Profile`. Making it a fourth variant is what put it
/// out of reach: with three apps, a fourth profile is never assigned and the
/// "totals are a floor" banner never fires.
///
/// The control app takes it, so the floor state is visible without muddying the
/// runaway app that leads the table or the spike that carries the metering point.
const PARTIAL_AT_INDEX: usize = 1;

/// The measurement outcome for the app at `index`, given a fleet of `total`.
///
/// A one-app fleet stays `ok`: a single seeded app should read as a clean
/// headline rather than as a broken measurement.
fn status_for(index: usize, total: usize) -> (&'static str, Option<String>) {
    if total >= 2 && index == PARTIAL_AT_INDEX {
        return (
            measure_status::PARTIAL,
            Some(
                "walk stopped at the 5000-page ceiling (5000000 objects); this app should \
                 move to an S3 Inventory-based measure"
                    .to_string(),
            ),
        );
    }
    (measure_status::OK, None)
}

impl Profile {
    /// Cycle through the shapes, so a fleet of any size covers as many as it has
    /// room for and the first app is always the interesting one.
    fn for_index(i: usize) -> Self {
        PROFILES[i % PROFILES.len()]
    }

    /// Bytes held `days_ago` days back. Day 0 is today.
    ///
    /// Returns the *level*, not a delta — storage is a level, and building the
    /// series this way means the chart and the growth column agree by
    /// construction instead of by two separate calculations.
    fn bytes_at(self, days_ago: i64) -> i64 {
        // 1 at the oldest seeded point (`days_ago` maxes at HISTORY_DAYS - 1),
        // HISTORY_DAYS today. Only the span matters here, not the origin.
        let elapsed = HISTORY_DAYS - days_ago;
        match self {
            // Compounding ~4%/day from 900 MiB, so the trailing week is visibly
            // steeper than the month — which is the whole point of the growth column.
            Profile::RunawayUntagged => {
                let growth = 1.04f64.powi(elapsed as i32);
                (900.0 * MIB as f64 * growth) as i64
            }
            // A gentle sawtooth around 6 GiB: real enough not to look synthetic,
            // flat enough that its 7d growth reads ~0.
            Profile::HealthyFlat => {
                let wobble = ((elapsed % 7) as f64 - 3.0) * 40.0;
                6 * GIB + (wobble * MIB as f64) as i64
            }
            // Baseline 2 GiB; a 40 GiB export lands 10 days ago and is deleted 3
            // days ago. Peak is 20x the resting size, GB-month barely moves.
            Profile::SpikeThenDelete => {
                let base = 2 * GIB;
                if (3..=10).contains(&days_ago) {
                    base + 40 * GIB
                } else {
                    base
                }
            }
        }
    }

    /// Object count tracks bytes at a plausible average object size, so the
    /// "N objects" column never contradicts the size column.
    fn objects_at(self, days_ago: i64) -> i64 {
        let avg = match self {
            // Many small uploads.
            Profile::RunawayUntagged => 300 * 1024,
            Profile::HealthyFlat => 2 * MIB,
            // Few, enormous exports.
            Profile::SpikeThenDelete => 60 * MIB,
        };
        (self.bytes_at(days_ago) / avg).max(1)
    }

    /// How the current size splits across prefixes, and which of them a
    /// retention rule covers. `expire_after: None` is what the UI renders as
    /// "keeps forever" and counts toward `untagged_bytes`.
    fn breakdown(self, total: i64) -> HashMap<String, PrefixUsage> {
        let mut out = HashMap::new();
        let mut add = |name: &str, share: f64, objects: i64, class: Option<&str>| {
            out.insert(
                name.to_string(),
                PrefixUsage {
                    bytes: (total as f64 * share) as i64,
                    objects,
                    expire_after: class.map(str::to_string),
                },
            );
        };
        match self {
            Profile::RunawayUntagged => {
                // 92% of a growing silo with no rule at all — the actionable gap.
                add("uploads/", 0.92, 11_400, None);
                add("generated/", 0.08, 900, Some("30d"));
            }
            Profile::HealthyFlat => {
                add("generated/", 0.70, 2_100, Some("90d"));
                add("uploads/", 0.28, 700, Some("365d"));
                add("tmp/", 0.02, 120, Some("1d"));
            }
            Profile::SpikeThenDelete => {
                add("exports/", 0.95, 34, Some("7d"));
                add("uploads/", 0.05, 12, Some("365d"));
            }
        }
        out
    }

    /// Bytes no rule covers — the leading indicator the fleet view ranks on.
    fn untagged(self, total: i64) -> (i64, i64) {
        self.breakdown(total)
            .values()
            .filter(|p| p.expire_after.is_none())
            .fold((0, 0), |(b, o), p| (b + p.bytes, o + p.objects))
    }

    /// A few representative object keys, so the per-app browser is not empty.
    /// Names are stable across runs for the same reason the numbers are.
    fn sample_objects(self) -> Vec<(&'static str, usize)> {
        match self {
            Profile::RunawayUntagged => vec![
                ("uploads/scan-8f21.png", 4096),
                ("uploads/scan-b0c4.png", 6144),
                ("uploads/customer-attachment-19a2.pdf", 8192),
                ("generated/weekly-rollup.csv", 2048),
            ],
            Profile::HealthyFlat => vec![
                ("generated/2026/q1-report.pdf", 8192),
                ("generated/2026/q2-report.pdf", 8192),
                ("uploads/logo.png", 1024),
                ("tmp/scratch.csv", 512),
            ],
            Profile::SpikeThenDelete => vec![
                ("exports/orders-2026-06.parquet", 16384),
                ("uploads/mapping.csv", 1024),
            ],
        }
    }
}

/// Midday rather than midnight, so a sample never lands on a day boundary
/// where a reader has to reason about which day it belongs to.
fn noon_utc_days_ago(days_ago: i64) -> DateTime<FixedOffset> {
    let d = Utc::now() - Duration::days(days_ago);
    d.date_naive()
        .and_hms_opt(12, 0, 0)
        .map(|n| n.and_utc())
        .unwrap_or(d)
        .into()
}

/// Seed storage usage for every app that exists.
///
/// Idempotent: the rollup is upserted and samples collide on their
/// `(app_id, measured_at)` primary key, so re-running the seed refreshes rather
/// than accumulating a second history.
///
/// **Warns instead of failing**, matching `seed_example_apps` — a developer can
/// work without demo storage numbers, and a failure here should never take down
/// the rest of the seed.
pub(crate) async fn seed_storage_usage(db: &DatabaseConnection) -> Result<(), OxyError> {
    // Skip (don't error) on a non-local DB, matching `seed_partner_tenants` — this
    // is folded into `oxy seed`, so erroring would take down the whole seed over
    // an optional fixture.
    if !super::seed_partners::is_local_db() {
        println!(
            "{} skipping storage seed — OXY_DATABASE_URL is not local",
            "⚠️".warning()
        );
        return Ok(());
    }

    // ONLY apps this seed created. `Apps::find().all()` would reach every app in
    // the database, and both writes below are destructive to a real one: the
    // rollup upsert replaces measured usage that feeds the org soft-limit and the
    // GB-month meter, and `seed_objects` overwrites fixed keys like
    // `uploads/logo.png` with filler bytes.
    let apps = super::seed_apps::seeded_apps(db).await?;

    if apps.is_empty() {
        println!(
            "{} no seeded apps to attach storage usage to",
            "⚠️".warning()
        );
        return Ok(());
    }

    // Leave one app unmeasured only when the fleet has more apps than shapes, so
    // the "N apps have never been measured" banner has something to report
    // without costing a distinct state.
    //
    // `oxy seed` tops out at three apps, so in practice this is zero and that
    // banner is NOT exercised by the seed — delete a usage row by hand to see it.
    // Sacrificing one of three apps to an empty state is the worse trade.
    let unmeasured_tail = usize::from(apps.len() > PROFILES.len());
    let measured = &apps[..apps.len() - unmeasured_tail];

    for (i, app) in measured.iter().enumerate() {
        let profile = Profile::for_index(i);
        if let Err(e) = seed_one(db, app.id, app.org_id, profile, i, measured.len()).await {
            tracing::warn!("seed_storage: {} ({}): {e}", app.slug, app.id);
        }
        if let Err(e) = seed_objects(app.id, profile).await {
            // The browser being empty is cosmetic; the rows are the useful part.
            tracing::warn!("seed_storage: objects for {}: {e}", app.slug);
        }
    }

    tracing::info!(
        "seed_storage: seeded {} app(s) with {HISTORY_DAYS}d of usage history{}",
        measured.len(),
        if unmeasured_tail > 0 {
            ", leaving 1 unmeasured"
        } else {
            ""
        }
    );
    Ok(())
}

async fn seed_one(
    db: &DatabaseConnection,
    app_id: Uuid,
    org_id: Uuid,
    profile: Profile,
    index: usize,
    total: usize,
) -> Result<(), sea_orm::DbErr> {
    let (status, detail) = status_for(index, total);
    let now_bytes = profile.bytes_at(0);
    let now_objects = profile.objects_at(0);
    let (untagged_bytes, untagged_objects) = profile.untagged(now_bytes);

    let rollup = app_storage_usage::ActiveModel {
        app_id: Set(app_id),
        org_id: Set(org_id),
        bytes: Set(now_bytes),
        object_count: Set(now_objects),
        untagged_bytes: Set(untagged_bytes),
        untagged_object_count: Set(untagged_objects),
        prefix_breakdown: Set(Some(
            serde_json::to_value(profile.breakdown(now_bytes)).unwrap_or(serde_json::Value::Null),
        )),
        measured_at: Set(noon_utc_days_ago(0)),
        measure_status: Set(status.to_string()),
        measure_detail: Set(detail),
    };
    AppStorageUsage::insert(rollup)
        .on_conflict(
            sea_orm::sea_query::OnConflict::column(app_storage_usage::Column::AppId)
                .update_columns([
                    app_storage_usage::Column::OrgId,
                    app_storage_usage::Column::Bytes,
                    app_storage_usage::Column::ObjectCount,
                    app_storage_usage::Column::UntaggedBytes,
                    app_storage_usage::Column::UntaggedObjectCount,
                    app_storage_usage::Column::PrefixBreakdown,
                    app_storage_usage::Column::MeasuredAt,
                    app_storage_usage::Column::MeasureStatus,
                    app_storage_usage::Column::MeasureDetail,
                ])
                .to_owned(),
        )
        .exec(db)
        .await?;

    // One insert for the whole history rather than 90 round-trips.
    let samples: Vec<app_storage_usage_samples::ActiveModel> = (0..HISTORY_DAYS)
        .map(|d| app_storage_usage_samples::ActiveModel {
            app_id: Set(app_id),
            measured_at: Set(noon_utc_days_ago(d)),
            bytes: Set(profile.bytes_at(d)),
            object_count: Set(profile.objects_at(d)),
        })
        .collect();
    AppStorageUsageSamples::insert_many(samples)
        .on_conflict(
            sea_orm::sea_query::OnConflict::columns([
                app_storage_usage_samples::Column::AppId,
                app_storage_usage_samples::Column::MeasuredAt,
            ])
            .update_columns([
                app_storage_usage_samples::Column::Bytes,
                app_storage_usage_samples::Column::ObjectCount,
            ])
            .to_owned(),
        )
        .exec(db)
        .await?;
    Ok(())
}

/// Write a few real objects so the per-app browser lists something.
///
/// Contents are filler bytes at the declared length — the browser shows key,
/// size, and age, none of which care what is inside. Deliberately tiny: this
/// runs on a laptop, and the rollup's headline numbers come from the seeded rows
/// rather than from what is actually on disk.
async fn seed_objects(app_id: Uuid, profile: Profile) -> Result<(), OxyError> {
    use crate::server::api::custom_apps_storage as storage;

    for (path, size) in profile.sample_objects() {
        let body = vec![b'x'; size];
        let opts = storage::PutOptions {
            allow_overwrite: true,
            ..Default::default()
        };
        // No retention policy: the seeded objects are for browsing, and tagging
        // them would make a local run's lifecycle behaviour differ from what the
        // app's own manifest declares.
        storage::put(
            &storage::Silo::production(app_id),
            path,
            body,
            opts,
            &storage::RetentionPolicy::default(),
        )
        .await
        .map_err(|e| OxyError::RuntimeError(format!("seeding {path}: {e}")))?;
    }
    Ok(())
}

/// Remove seeded storage rows. Called by `oxy seed --clear`.
pub(crate) async fn clear_storage_usage(db: &DatabaseConnection) -> Result<(), OxyError> {
    // Scoped to seeded apps, not a truncate. The FK is ON DELETE CASCADE so
    // dropping the apps would take these with them anyway — but `--clear` also
    // runs where the apps survive, and the reason to clean up is *orphans*, which
    // argues for scoping rather than for deleting everything. The
    // `refuse_if_not_local` gate upstream is bypassable with
    // `OXY_SEED_ALLOW_REMOTE=1`, so this must not lean on it.
    let ids: Vec<Uuid> = super::seed_apps::seeded_apps(db)
        .await?
        .into_iter()
        .map(|a| a.id)
        .collect();
    if ids.is_empty() {
        return Ok(());
    }
    AppStorageUsageSamples::delete_many()
        .filter(app_storage_usage_samples::Column::AppId.is_in(ids.clone()))
        .exec(db)
        .await
        .map_err(|e| OxyError::DBError(format!("clear_storage: samples: {e}")))?;
    AppStorageUsage::delete_many()
        .filter(app_storage_usage::Column::AppId.is_in(ids))
        .exec(db)
        .await
        .map_err(|e| OxyError::DBError(format!("clear_storage: rollup: {e}")))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The most apps `oxy seed` can create: the demo app, plus Acme's open and
    /// restricted pair when the partner seed ran (`seed::deploy_example_apps`).
    /// Fewer on a fleet where the partner seed skipped.
    const MAX_SEEDED_APPS: usize = 3;

    #[test]
    fn every_profile_is_reachable_at_the_real_app_count() {
        // Pinned to the app count the seed ACTUALLY produces, not to
        // `0..PROFILES.len()`. An earlier version cycled four profiles and was
        // tested over `0..4`, which passed while the fourth was never assigned to
        // anything — the seed only ever creates three apps, so that state simply
        // never rendered. Testing the range the caller uses is the difference.
        let reachable: std::collections::HashSet<Profile> =
            (0..MAX_SEEDED_APPS).map(Profile::for_index).collect();
        assert_eq!(
            reachable.len(),
            PROFILES.len(),
            "a profile is unreachable at {MAX_SEEDED_APPS} apps: add a seeded app or drop the shape"
        );
    }

    #[test]
    fn the_floor_banner_fires_at_the_real_app_count() {
        // `measure_status` is orthogonal to the byte shape precisely so this
        // state survives the fleet being small.
        let statuses: Vec<&str> = (0..MAX_SEEDED_APPS)
            .map(|i| status_for(i, MAX_SEEDED_APPS).0)
            .collect();
        assert!(
            statuses.contains(&measure_status::PARTIAL),
            "no seeded app reports an incomplete walk: {statuses:?}"
        );
    }

    #[test]
    fn a_single_app_fleet_reads_as_clean() {
        // One seeded app should be the headline, not a broken measurement.
        assert_eq!(status_for(0, 1).0, measure_status::OK);
        assert!(status_for(0, 1).1.is_none());
    }

    #[test]
    fn the_runaway_app_actually_grows_over_the_window() {
        // If this ever flattens, the growth column and the usage chart both go
        // blank and the seed stops demonstrating the thing it exists for.
        let p = Profile::RunawayUntagged;
        assert!(p.bytes_at(0) > p.bytes_at(7), "no 7-day growth");
        assert!(
            p.bytes_at(0) > p.bytes_at(HISTORY_DAYS - 1) * 4,
            "growth too shallow to see"
        );
    }

    #[test]
    fn the_flat_app_is_visibly_flat_week_over_week() {
        let p = Profile::HealthyFlat;
        let delta = (p.bytes_at(0) - p.bytes_at(7)).abs();
        assert!(delta < GIB / 2, "flat profile drifted by {delta} bytes");
    }

    #[test]
    fn the_spike_is_over_before_today_and_shows_as_a_drop() {
        let p = Profile::SpikeThenDelete;
        assert!(
            p.bytes_at(5) > p.bytes_at(0) * 10,
            "spike not visible mid-window"
        );
        // Deleted three days ago, so the 7-day growth is negative — the one
        // profile that exercises the down-arrow.
        assert!(
            p.bytes_at(0) < p.bytes_at(7),
            "spike should read as shrinkage"
        );
    }

    #[test]
    fn untagged_is_the_sum_of_prefixes_with_no_rule() {
        let p = Profile::RunawayUntagged;
        let total = p.bytes_at(0);
        let (untagged, _) = p.untagged(total);
        // `uploads/` is 92% of this profile and carries no rule.
        assert!(untagged > total / 2, "expected most of it untagged");
        // And the healthy app has none at all.
        let healthy = Profile::HealthyFlat;
        assert_eq!(healthy.untagged(healthy.bytes_at(0)).0, 0);
    }

    #[test]
    fn breakdown_shares_account_for_the_whole_app() {
        for i in 0..PROFILES.len() {
            let p = Profile::for_index(i);
            let total = p.bytes_at(0);
            let summed: i64 = p.breakdown(total).values().map(|u| u.bytes).sum();
            // Float shares, so allow rounding slack — but the prefix rows must
            // not visibly disagree with the headline number above them.
            let drift = (total - summed).abs();
            assert!(
                drift < total / 100,
                "{p:?} prefixes drift {drift} from {total}"
            );
        }
    }

    #[test]
    fn exactly_one_app_reports_a_floor() {
        let partial: Vec<usize> = (0..MAX_SEEDED_APPS)
            .filter(|&i| status_for(i, MAX_SEEDED_APPS).0 != measure_status::OK)
            .collect();
        assert_eq!(partial.len(), 1, "exactly one app should be a floor");
        assert!(status_for(partial[0], MAX_SEEDED_APPS).1.is_some());
    }

    #[test]
    fn the_whole_seeded_fleet_fits_under_the_default_soft_quota() {
        // The rollup rows this seed writes are exactly what `quota::status_for_org`
        // sums, so an oversized fixture puts a local org over its own limit and
        // every `ctx.storage` write starts failing closed — a quota error that
        // reads as a product bug rather than as a seed artifact.
        //
        // Worst case is all seeded apps landing in one org, so sum every shape.
        // This is what bounds HISTORY_DAYS: at 120 the runaway app alone is
        // ~96 GiB and blows past the hard limit on its own.
        // Sum the assignment the seed actually makes, not the shape list. The two
        // coincide only while MAX_SEEDED_APPS == PROFILES.len(); a fourth seeded
        // app would land on RunawayUntagged a second time and the shape-list sum
        // would understate the org total it is supposed to bound.
        let total: i64 = (0..MAX_SEEDED_APPS)
            .map(|i| Profile::for_index(i).bytes_at(0))
            .sum();
        let soft = crate::server::api::custom_apps_storage::quota::DEFAULT_SOFT_LIMIT_BYTES;
        assert!(
            total < soft,
            "seeded fleet is {} GiB against a {} GiB soft limit — lower HISTORY_DAYS \
             or the growth rate",
            total / GIB,
            soft / GIB
        );
    }

    #[test]
    fn object_counts_never_collapse_to_zero() {
        // A zero would render "0 objects" beside a non-zero size.
        for i in 0..PROFILES.len() {
            let p = Profile::for_index(i);
            for d in [0, 7, HISTORY_DAYS - 1] {
                assert!(p.objects_at(d) >= 1, "{p:?} day {d}");
            }
        }
    }
}
