//! Org-level storage quotas.
//!
//! ## Why org-level
//!
//! A tenant thinks in organisations. A per-app limit just relocates the question
//! to "which app do I raise?", and every app under an org shares one invoice.
//! Supabase reaches the same conclusion — their quota is org-wide regardless of
//! how many projects sit under it.
//!
//! ## Soft, then hard
//!
//! Crossing the soft limit **breaks nothing**: it notifies and starts a grace
//! period (`org_billing.grace_period_ends_at`, which already exists for exactly
//! this shape of problem). Only past the hard limit are new writes refused.
//!
//! ## Three things that keep working no matter how far over you are
//!
//! * **Reads.** Blocking them bricks a live customer-facing app for a billing
//!   reason.
//! * **Deletes.** Blocking them traps the tenant in the state you want them to
//!   leave — the one action that fixes the problem must never be the one you
//!   take away.
//!
//! Note this list does **not** include overwrites. An earlier version exempted
//! `allowOverwrite`, reasoning that refusing an overwrite pushes the caller to a
//! new name and grows the silo — but the flag does not assert the key exists, so
//! a function passing it with fresh pathnames was never checked at all. Past the
//! hard limit every write is refused, so there is no cheaper name to escape to.
//!
//! ## This is a cost guardrail, not a security boundary
//!
//! It reads the rollup, which lags by up to one sweep interval, so a tenant can
//! overshoot by whatever they can write in that window. That is acceptable and
//! deliberate: the boundary that must be exact is the tenant silo, and that one
//! is enforced per key on every call. Never reuse this for isolation.

use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use uuid::Uuid;

use entity::app_storage_usage;
use entity::prelude::AppStorageUsage;

use super::StorageError;
use super::environment_limits::production_bytes;

/// Default soft limit per org. Generous on purpose — the first version of a
/// quota exists to catch runaway growth, not to ration normal use, and a limit
/// that pages people during ordinary work gets raised until it means nothing.
pub(crate) const DEFAULT_SOFT_LIMIT_BYTES: i64 = 50 * 1024 * 1024 * 1024; // 50 GiB

/// Hard limit as a multiple of the soft limit. Writes stop here.
const HARD_LIMIT_MULTIPLIER: i64 = 2;

fn env_bytes(key: &str) -> Option<i64> {
    std::env::var(key)
        .ok()
        .and_then(|v| v.trim().parse::<i64>().ok())
        .filter(|&v| v > 0)
}

/// Soft limit in bytes, or `None` when quotas are disabled entirely.
///
/// `OXY_CUSTOMER_APPS_STORAGE_SOFT_LIMIT_BYTES=0` is the documented off switch:
/// a self-hosted operator paying their own S3 bill has no reason to be policed
/// by ours, and an off switch beats them discovering the limit mid-incident.
pub fn soft_limit_bytes() -> Option<i64> {
    match std::env::var("OXY_CUSTOMER_APPS_STORAGE_SOFT_LIMIT_BYTES") {
        Ok(raw) if raw.trim() == "0" => None,
        _ => Some(
            env_bytes("OXY_CUSTOMER_APPS_STORAGE_SOFT_LIMIT_BYTES")
                .unwrap_or(DEFAULT_SOFT_LIMIT_BYTES),
        ),
    }
}

pub fn hard_limit_bytes() -> Option<i64> {
    env_bytes("OXY_CUSTOMER_APPS_STORAGE_HARD_LIMIT_BYTES")
        .or_else(|| soft_limit_bytes().map(|s| s.saturating_mul(HARD_LIMIT_MULTIPLIER)))
}

/// Where an org sits against its limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuotaState {
    /// Under the soft limit, or quotas disabled.
    Ok,
    /// Over soft, under hard: notify and start a grace period; writes continue.
    OverSoft,
    /// Over hard: new writes are refused.
    OverHard,
}

/// An org's usage against its limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuotaStatus {
    /// Production's bytes: environment silos are metered but not counted.
    pub used_bytes: i64,
    pub soft_limit_bytes: Option<i64>,
    pub hard_limit_bytes: Option<i64>,
    pub state: QuotaState,
    /// True when any app in the org last measured `partial`/`failed`, so
    /// `used_bytes` is a floor rather than a total.
    pub measurement_incomplete: bool,
}

impl QuotaStatus {
    /// Percentage of the soft limit consumed, for the UI meter. `None` when
    /// quotas are off — a meter with no denominator is worse than no meter.
    pub fn percent_of_soft(&self) -> Option<f64> {
        let limit = self.soft_limit_bytes?;
        if limit <= 0 {
            return None;
        }
        Some((self.used_bytes as f64 / limit as f64) * 100.0)
    }

    pub fn blocks_writes(&self) -> bool {
        matches!(self.state, QuotaState::OverHard)
    }
}

/// Classify a usage total against the configured limits.
///
/// Pure, so the thresholds are testable without a database — which matters
/// because an off-by-one here either fails to protect anything or refuses a
/// paying tenant's writes.
pub fn classify(used_bytes: i64, soft: Option<i64>, hard: Option<i64>) -> QuotaState {
    let Some(soft) = soft else {
        return QuotaState::Ok;
    };
    if let Some(hard) = hard
        && used_bytes >= hard
    {
        return QuotaState::OverHard;
    }
    if used_bytes >= soft {
        return QuotaState::OverSoft;
    }
    QuotaState::Ok
}

/// Sum an org's measured usage across its apps.
pub async fn status_for_org(
    db: &DatabaseConnection,
    org_id: Uuid,
) -> Result<QuotaStatus, StorageError> {
    let rows = AppStorageUsage::find()
        .filter(app_storage_usage::Column::OrgId.eq(org_id))
        .all(db)
        .await
        .map_err(|e| StorageError::S3(format!("storage usage lookup for org {org_id}: {e}")))?;

    // Production's bytes only: an environment silo is metered with its app but
    // capped on its own (`environment_limits`), so staging never pauses
    // production's writes.
    let used_bytes = rows
        .iter()
        .map(|r| production_bytes(r.bytes, r.prefix_breakdown.as_ref()))
        .sum();
    let measurement_incomplete = rows
        .iter()
        .any(|r| r.measure_status != entity::app_storage_usage::measure_status::OK);

    let soft = soft_limit_bytes();
    let hard = hard_limit_bytes();
    Ok(QuotaStatus {
        used_bytes,
        soft_limit_bytes: soft,
        hard_limit_bytes: hard,
        state: classify(used_bytes, soft, hard),
        measurement_incomplete,
    })
}

/// Gate a write. `Ok(())` to proceed.
///
/// Fails **open** on a lookup error: a database blip must not stop a paying
/// tenant's uploads. The quota is a cost guardrail, and the cost of wrongly
/// refusing writes is higher than the cost of a few extra gigabytes.
pub async fn check_write_allowed(
    db: &DatabaseConnection,
    org_id: Uuid,
    incoming_bytes: u64,
) -> Result<(), StorageError> {
    let status = match status_for_org(db, org_id).await {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(
                %org_id,
                "storage quota check failed ({e}); allowing the write rather than \
                 blocking a tenant on a lookup error"
            );
            return Ok(());
        }
    };
    if !status.blocks_writes() {
        return Ok(());
    }
    let hard = status.hard_limit_bytes.unwrap_or_default();
    Err(StorageError::TooLarge(format!(
        "storage quota exceeded: this organization is using {} of its {} hard limit, \
         so new uploads are paused (this {} write was refused). Delete unused assets \
         from the app's Storage tab, add a `storage.retention` rule to oxy-app.json, \
         or contact support to raise the limit. Reads and deletes are unaffected.",
        human_bytes(status.used_bytes),
        human_bytes(hard),
        human_bytes(incoming_bytes as i64),
    )))
}

/// Byte count for humans. Binary units, matching how object stores and the rest
/// of this crate's size ceilings are stated.
pub fn human_bytes(bytes: i64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let negative = bytes < 0;
    let mut value = bytes.unsigned_abs() as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    let sign = if negative { "-" } else { "" };
    if unit == 0 {
        format!("{sign}{} {}", value as u64, UNITS[unit])
    } else {
        format!("{sign}{value:.1} {}", UNITS[unit])
    }
}

/// Orgs at or over their soft limit, for the notifier and the admin view.
///
/// Aggregated in Rust rather than SQL. `SUM(bigint)` returns **`numeric`** in
/// Postgres, which will not decode into an `i64` — a `GROUP BY` version of this
/// compiles fine and fails on its first real call, at runtime, in the notifier.
/// Summing here also keeps this consistent with [`status_for_org`], which
/// already reads whole rows.
///
/// The scan is one row per **app**, not per object, so it stays in the hundreds
/// at the scale this table is designed for. If the app count ever makes that
/// untrue, cast the aggregate to `bigint` in SQL rather than reaching for
/// `i64` again.
pub async fn orgs_over_soft_limit(
    db: &DatabaseConnection,
) -> Result<Vec<(Uuid, i64)>, StorageError> {
    let Some(soft) = soft_limit_bytes() else {
        return Ok(Vec::new());
    };
    let rows = AppStorageUsage::find()
        .all(db)
        .await
        .map_err(|e| StorageError::S3(format!("org usage aggregate: {e}")))?;

    let per_app: Vec<(Uuid, i64)> = rows
        .into_iter()
        .map(|r| {
            (
                r.org_id,
                production_bytes(r.bytes, r.prefix_breakdown.as_ref()),
            )
        })
        .collect();
    // Fold + sort live in `totals_by_org` so the unit tests exercise THIS path
    // rather than a copy of it.
    Ok(totals_by_org(&per_app)
        .into_iter()
        .filter(|(_, total)| *total >= soft)
        .collect())
}

/// Sum per org, split out so the threshold logic is testable without a database.
pub fn totals_by_org(rows: &[(Uuid, i64)]) -> Vec<(Uuid, i64)> {
    let mut totals: std::collections::HashMap<Uuid, i64> = std::collections::HashMap::new();
    for (org_id, bytes) in rows {
        *totals.entry(*org_id).or_default() += bytes;
    }
    let mut out: Vec<(Uuid, i64)> = totals.into_iter().collect();
    out.sort_by_key(|(_, t)| std::cmp::Reverse(*t));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const GIB: i64 = 1024 * 1024 * 1024;

    #[test]
    fn classify_walks_ok_soft_hard() {
        let (soft, hard) = (Some(10 * GIB), Some(20 * GIB));
        assert_eq!(classify(0, soft, hard), QuotaState::Ok);
        assert_eq!(classify(9 * GIB, soft, hard), QuotaState::Ok);
        // At the limit counts as over — "10 of 10 GiB used" is not headroom.
        assert_eq!(classify(10 * GIB, soft, hard), QuotaState::OverSoft);
        assert_eq!(classify(19 * GIB, soft, hard), QuotaState::OverSoft);
        assert_eq!(classify(20 * GIB, soft, hard), QuotaState::OverHard);
        assert_eq!(classify(999 * GIB, soft, hard), QuotaState::OverHard);
    }

    #[test]
    fn no_soft_limit_disables_quotas_entirely() {
        // The self-hosted off switch: never block, whatever the usage.
        assert_eq!(classify(i64::MAX, None, None), QuotaState::Ok);
        assert_eq!(classify(i64::MAX, None, Some(GIB)), QuotaState::Ok);
    }

    #[test]
    fn only_over_hard_blocks_writes() {
        let status = |state| QuotaStatus {
            used_bytes: 0,
            soft_limit_bytes: Some(GIB),
            hard_limit_bytes: Some(2 * GIB),
            state,
            measurement_incomplete: false,
        };
        assert!(!status(QuotaState::Ok).blocks_writes());
        // Crossing soft must NOT break anything — it notifies.
        assert!(!status(QuotaState::OverSoft).blocks_writes());
        assert!(status(QuotaState::OverHard).blocks_writes());
    }

    #[test]
    fn percent_of_soft_is_none_without_a_denominator() {
        let mut s = QuotaStatus {
            used_bytes: GIB,
            soft_limit_bytes: None,
            hard_limit_bytes: None,
            state: QuotaState::Ok,
            measurement_incomplete: false,
        };
        assert_eq!(s.percent_of_soft(), None);
        s.soft_limit_bytes = Some(0);
        assert_eq!(s.percent_of_soft(), None, "must not divide by zero");
        s.soft_limit_bytes = Some(4 * GIB);
        assert_eq!(s.percent_of_soft(), Some(25.0));
    }

    #[test]
    fn human_bytes_reads_like_a_person_wrote_it() {
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(1024), "1.0 KiB");
        assert_eq!(human_bytes(1536), "1.5 KiB");
        assert_eq!(human_bytes(5 * GIB), "5.0 GiB");
        assert_eq!(human_bytes(1024 * GIB), "1.0 TiB");
        // Clamped at the largest unit rather than inventing one.
        assert_eq!(human_bytes(4096 * GIB), "4.0 TiB");
    }

    #[test]
    fn hard_limit_defaults_to_a_multiple_of_soft() {
        // Guards the relationship, not the env plumbing: hard must never land
        // below soft, or OverSoft would be unreachable.
        let soft = 10 * GIB;
        let hard = soft.saturating_mul(HARD_LIMIT_MULTIPLIER);
        assert!(hard > soft);
        assert_eq!(classify(soft, Some(soft), Some(hard)), QuotaState::OverSoft);
    }

    #[test]
    fn totals_by_org_sums_apps_and_ranks_worst_first() {
        let a = Uuid::from_u128(1);
        let b = Uuid::from_u128(2);
        let totals = totals_by_org(&[(a, 3 * GIB), (b, 10 * GIB), (a, 2 * GIB)]);
        // Org `a` has two apps; both must count toward its one quota.
        assert_eq!(totals, vec![(b, 10 * GIB), (a, 5 * GIB)]);
    }

    #[test]
    fn totals_by_org_is_deterministic_across_runs() {
        // A HashMap's iteration order is arbitrary, so the sort is what makes
        // "worst offender first" mean anything to a notifier or an operator.
        let a = Uuid::from_u128(1);
        let b = Uuid::from_u128(2);
        let c = Uuid::from_u128(3);
        let rows = [(a, GIB), (b, 3 * GIB), (c, 2 * GIB)];
        let first = totals_by_org(&rows);
        for _ in 0..25 {
            assert_eq!(totals_by_org(&rows), first);
        }
        assert_eq!(first[0].0, b);
    }
}
