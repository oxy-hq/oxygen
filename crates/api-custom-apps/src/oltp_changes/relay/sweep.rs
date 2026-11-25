//! The fleet jobs that queue relay passes (`oltp_changes_relay_sweep`, every
//! 5 s, and `oltp_changes_weekly_sweep`, every 7 days).
//!
//! Both read and write the platform database only: neither connects to an
//! org's store, so a scale-to-zero store with nothing waiting is left asleep.
//! The 5 s sweep queues a pass for each mark that is due (nudged and not
//! drained since, or drained too early to have seen the nudge's commit), for
//! production in an org with `oltp_changes` on. The weekly sweep first makes
//! a mark for every flagged org's active store that has none, then nudges
//! every mark, so the 5 s sweep queues a pass for each store whatever its
//! marks said, catching a write no nudge announced. Its first run is at the
//! first deploy: a fleet job with no claim row yet is due at once.

use oxy_app_core::fleet_claim::run_claimed;
use oxy_oltp::entity::tenants::{self as oltp_tenants, TenantStatus};
use sea_orm::{ColumnTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter, QuerySelect};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::{
    PRODUCTION, REDRAIN_EVERY, REDRAIN_MARGIN, STALE_AFTER, SWEEP_INTERVAL, WEEKLY_INTERVAL,
    enqueue_relay, marks,
};
use crate::app_events::marks::platform_now;
use crate::oltp_changes::owner::CHANGES_STEP;

/// The `fleet_job_claims` row the 5 s sweep claims each interval.
pub const SWEEP_JOB: &str = "oltp_changes_relay_sweep";
/// The `fleet_job_claims` row the weekly sweep claims each interval.
pub const WEEKLY_JOB: &str = "oltp_changes_weekly_sweep";

/// What one [`sweep_once`] did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SweepReport {
    /// Relays it found or queued waiting.
    pub queued: usize,
    /// The orgs it logged `oltp_changes_stale` for.
    pub stale: Vec<Uuid>,
}

/// Queue a relay pass for each production mark that is due, in an org with
/// `oltp_changes` on, and log `oltp_changes_stale` for one whose work has
/// waited past [`STALE_AFTER`] (its `pending_since`, which neither a failed
/// pass nor a nudge into a waiting mark moves): at every sweep, but only once
/// per backoff window while failed passes back the mark off, since a nudge
/// makes such a mark due at every sweep. Phase 1 relays production
/// only, so any other environment's mark is passed by. A mark whose org has
/// no app to queue a run under is marked drained, since nothing could be
/// relayed for it; one that cannot be queued is logged and left due.
pub async fn sweep_once(db: &DatabaseConnection) -> Result<SweepReport, DbErr> {
    let mut report = SweepReport::default();
    let now = platform_now(db).await?;
    for due in marks::due(db, REDRAIN_MARGIN, REDRAIN_EVERY).await? {
        let (org_id, environment) = (due.org_id, due.environment.as_str());
        if environment != PRODUCTION || !crate::oltp_changes::enabled_for(org_id) {
            continue;
        }
        let waited = (now - due.pending_since).to_std();
        if waited.is_ok_and(|waited| waited > STALE_AFTER) && alarm_stale(db, &due).await {
            alarm!(
                environment,
                %org_id,
                environment,
                pending_since = %due.pending_since,
                "oltp_changes_stale"
            );
            report.stale.push(org_id);
        }
        match enqueue_relay(db, org_id, environment).await {
            Ok(true) => report.queued += 1,
            Ok(false) => {
                if let Err(e) = marks::drained(db, org_id, environment, now, REDRAIN_MARGIN).await {
                    tracing::warn!(%org_id, error = %e, "oltp_changes sweep could not drain a mark");
                }
            }
            Err(e) => {
                tracing::warn!(%org_id, environment, error = %e, "oltp_changes sweep could not queue a relay");
            }
        }
    }
    Ok(report)
}

/// Whether to log a due, stale mark stale now ([`marks::alarm_stale`]): once
/// per backoff window, every sweep outside one. A failure to tell alarms.
async fn alarm_stale(db: &DatabaseConnection, due: &marks::Due) -> bool {
    marks::alarm_stale(db, due.org_id, &due.environment)
        .await
        .unwrap_or_else(|e| {
            tracing::warn!(org_id = %due.org_id, error = %e, "oltp_changes sweep could not rate-limit a stale alarm");
            true
        })
}

/// Run [`sweep_once`] once per [`SWEEP_INTERVAL`] across the fleet until
/// shutdown. Every replica that runs singleton work starts it; the
/// [`fleet_claim`](oxy_app_core::fleet_claim) row lets one of them sweep per
/// interval.
pub fn spawn_sweep(db: DatabaseConnection, shutdown: CancellationToken) {
    let pass_db = db.clone();
    tokio::spawn(run_claimed(
        db,
        SWEEP_JOB,
        SWEEP_INTERVAL,
        shutdown.cancelled_owned(),
        move || {
            let db = pass_db.clone();
            async move {
                if let Err(e) = sweep_once(&db).await {
                    tracing::warn!(error = %e, "oltp_changes relay sweep failed");
                }
            }
        },
    ));
}

/// The weekly sweep: first a production mark for every org with
/// `oltp_changes` on and an active OLTP store at the step that captures,
/// made if it has none ([`marks::nudge`]): such a store may have captured
/// without a nudge, under PR 1 or before its org was flagged, and nothing
/// else would ever drain it or alarm. Then every mark is nudged, so the 5 s
/// sweep queues a pass for each store. Returns how many marks it nudged.
pub async fn weekly_once(db: &DatabaseConnection) -> Result<u64, DbErr> {
    for org_id in flagged_stores(db).await? {
        if let Err(e) = marks::nudge(db, org_id, PRODUCTION).await {
            tracing::warn!(%org_id, error = %e, "oltp_changes weekly sweep could not mark a flagged store");
        }
    }
    marks::mark_all(db).await
}

/// Every org whose OLTP store is active at the step that creates
/// `oxy_changes` or later, with `oltp_changes` on. Reads the platform
/// database only; no store is opened.
async fn flagged_stores(db: &DatabaseConnection) -> Result<Vec<Uuid>, DbErr> {
    let orgs: Vec<Uuid> = oltp_tenants::Entity::find()
        .select_only()
        .column(oltp_tenants::Column::OrgId)
        .filter(oltp_tenants::Column::Status.eq(TenantStatus::Active))
        .filter(oltp_tenants::Column::PlatformSchemaVersion.gte(CHANGES_STEP))
        .into_tuple()
        .all(db)
        .await?;
    Ok(orgs
        .into_iter()
        .filter(|org_id| crate::oltp_changes::enabled_for(*org_id))
        .collect())
}

/// Run [`weekly_once`] once per [`WEEKLY_INTERVAL`] across the fleet until
/// shutdown, claimed like [`spawn_sweep`].
pub fn spawn_weekly(db: DatabaseConnection, shutdown: CancellationToken) {
    let pass_db = db.clone();
    tokio::spawn(run_claimed(
        db,
        WEEKLY_JOB,
        WEEKLY_INTERVAL,
        shutdown.cancelled_owned(),
        move || {
            let db = pass_db.clone();
            async move {
                match weekly_once(&db).await {
                    Ok(nudged) => {
                        tracing::info!(nudged, "oltp_changes weekly sweep nudged every mark")
                    }
                    Err(e) => tracing::warn!(error = %e, "oltp_changes weekly sweep failed"),
                }
            }
        },
    ));
}
