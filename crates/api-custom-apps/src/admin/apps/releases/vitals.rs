//! The release timeline's Web Vitals, per production release and screen.
//!
//! The read itself is `oxy_custom_apps::usage_reads::release_vitals`, which
//! answers from the app's workspace's usage store (its Airhouse once its usage
//! history is copied there, Postgres until then). This file only picks the
//! releases whose numbers can be compared and bounds the read in time.

use std::time::Duration;

use oxy_custom_apps::usage_reads::{self, VitalsStretch};
use sea_orm::DatabaseConnection;
use uuid::Uuid;

pub use oxy_custom_apps::usage_reads::{MAX_SCREENS, VitalsRow};

use super::metrics::{BUDGET, STATEMENT_TIMEOUT_MS};
use super::timeline::Stretch;

/// The Airhouse attempt's share of the read. Running out of it falls back to
/// Postgres, which then still has the whole [`BUDGET`].
const AIRHOUSE_SHARE: Duration = Duration::from_secs(2);

/// Every release's screens, up to [`MAX_SCREENS`] each, plus one more where
/// there are more (`rank > MAX_SCREENS`), which the caller reports as a count.
/// `None`: the read failed or ran out of time.
pub async fn release_vitals(
    db: &DatabaseConnection,
    app_id: Uuid,
    stretches: &[Stretch],
) -> Option<Vec<VitalsRow>> {
    let stretches: Vec<VitalsStretch> = stretches
        .iter()
        .filter(|s| s.comparable())
        .filter_map(|s| {
            Some(VitalsStretch {
                idx: s.index as i64,
                build_id: s.build_id?,
                starts_at: s.starts_at,
                ends_at: s.ends_at,
            })
        })
        .collect();
    if stretches.is_empty() {
        return Some(Vec::new());
    }
    let read =
        usage_reads::release_vitals(db, app_id, &stretches, AIRHOUSE_SHARE, STATEMENT_TIMEOUT_MS);
    match tokio::time::timeout(AIRHOUSE_SHARE + BUDGET, read).await {
        Ok(Ok(rows)) => Some(rows),
        Ok(Err(e)) => {
            tracing::warn!(part = "vitals", error = %e, "release timeline read failed");
            None
        }
        Err(_) => {
            tracing::warn!(part = "vitals", "release timeline read timed out");
            None
        }
    }
}
