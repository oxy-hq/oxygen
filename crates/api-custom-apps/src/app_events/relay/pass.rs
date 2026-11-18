//! One relay pass over an environment's outbox: every store it may hold rows
//! in (`app_events::stores::relayed_stores`), a batch claimed, relayed and
//! marked in each, then the outbox's mark brought up to what is still waiting
//! across them all (the module docs of `relay`).

use chrono::{DateTime, Utc};
use oxy_oltp::events::{self as outbox, EventsError, OutboxClaim, OutboxTarget};
use sea_orm::DatabaseConnection;
use uuid::Uuid;

use super::{BATCH, RelayStats, enqueue_relay, marks, relay_rows};
use crate::app_events::stores::{is_relayed, relayed_stores};

/// Relay one batch from each store of `org_id`'s `environment` outbox
/// ([`relayed_stores`]: one for production and staging; for a sandbox, the
/// staging branch and every own branch of that name), in the order the
/// module docs give. A name that is no environment, or an org with
/// `app_events` off, is nothing to do: `Ok` with empty stats, never a failed
/// run, and no connection to a store. A store the org does not have (no
/// staging branch, a sandbox's branch gone with it) holds nothing.
///
/// Every store is relayed, whatever another one does ([`relay_stores`]): one
/// app's broken own branch never holds up the next app's events. A pass in
/// which any store failed queues its retry ([`retry_later`]) and never
/// settles the mark, since the failed store's rows still wait; it fails,
/// naming each store that did.
pub async fn relay_once(
    db: &DatabaseConnection,
    org_id: Uuid,
    environment: &str,
) -> Result<RelayStats, String> {
    if !is_relayed(environment) {
        tracing::debug!(%org_id, environment, "app events: this environment's outbox is not relayed");
        return Ok(RelayStats::default());
    }
    if !crate::app_events::enabled_for(org_id) {
        // Inert: the rows wait, the store is not touched.
        tracing::debug!(%org_id, environment, "app events: off for this org; nothing relayed");
        return Ok(RelayStats::default());
    }
    // On the platform's clock, which a publish's mark is on too.
    let started = marks::platform_now(db).await.map_err(|e| e.to_string())?;
    let stores = relayed_stores(db, org_id, environment)
        .await
        .map_err(|e| format!("find the stores of {environment}'s outbox: {e}"))?;
    let (pass, failed) = relay_stores(db, org_id, environment, &stores).await;
    if pass.full {
        // More may be waiting: drain it now rather than at the next sweep.
        if let Err(e) = enqueue_relay(db, org_id, environment).await {
            tracing::warn!(%org_id, environment, error = %e, "could not queue the next relay");
        }
    }
    if !failed.is_empty() {
        retry_later(db, org_id, environment).await;
        return Err(format!(
            "relaying {environment}'s outbox failed in {}",
            failed.join("; ")
        ));
    }
    settle(db, org_id, environment, started, pass.left).await;
    Ok(pass.stats)
}

/// Steps 1 to 5 on each of `stores`, every one of them: a store that fails
/// (its credential unreadable, its reconcile refused, its endpoint
/// unreachable) is logged and passed over. What the others made, and each
/// failure as `<store>: <error>`.
async fn relay_stores(
    db: &DatabaseConnection,
    org_id: Uuid,
    environment: &str,
    stores: &[OutboxTarget],
) -> (StorePass, Vec<String>) {
    let mut pass = StorePass::default();
    let mut failed = Vec::new();
    for target in stores {
        match relay_store(db, org_id, target).await {
            Ok(one) => pass.add(one),
            Err(error) => {
                let store = store_name(target);
                tracing::warn!(
                    %org_id,
                    environment,
                    store,
                    error = %error,
                    "app events: could not relay one store of the outbox; relaying the others"
                );
                failed.push(format!("{store}: {error}"));
            }
        }
    }
    (pass, failed)
}

/// `target` as a person reads it, in a log line and a failed pass's error.
fn store_name(target: &OutboxTarget) -> String {
    match target {
        OutboxTarget::Production => "production".to_string(),
        OutboxTarget::Branch(environment) => format!("the `{environment}` branch"),
        OutboxTarget::EnvBranch {
            app_id,
            environment,
        } => format!("app {app_id}'s own `{environment}` branch"),
    }
}

/// What a pass made of one store.
#[derive(Default)]
struct StorePass {
    stats: RelayStats,
    /// How long the oldest row the store's claim left waiting had waited.
    left: Option<std::time::Duration>,
    /// Whether the claim took a full batch: more may be waiting.
    full: bool,
}

impl StorePass {
    /// `other` folded in: the counts summed, the oldest row left of either.
    fn add(&mut self, other: StorePass) {
        self.stats.relayed += other.stats.relayed;
        self.stats.deliveries += other.stats.deliveries;
        self.stats.rejected += other.stats.rejected;
        self.left = self.left.max(other.left);
        self.full |= other.full;
    }
}

/// Steps 1 to 5 on one store: claim a batch, relay it, mark it. Nothing to
/// do for a store the org does not have.
async fn relay_store(
    db: &DatabaseConnection,
    org_id: Uuid,
    target: &OutboxTarget,
) -> Result<StorePass, String> {
    let Some(claim) = claim_store(db, org_id, target).await? else {
        return Ok(StorePass::default());
    };
    // Read from the claim's own snapshot, taken with it: nothing to fail later.
    let left = claim.oldest_left_waiting();
    if claim.rows().is_empty() {
        // Nothing waiting, or another relay holds it all.
        claim.release().await.map_err(|e| e.to_string())?;
        return Ok(StorePass {
            left,
            ..StorePass::default()
        });
    }
    let full = claim.rows().len() as i64 >= BATCH;
    let relayed = match relay_rows(db, org_id, target, claim.rows()).await {
        Ok(relayed) => relayed,
        Err(e) => {
            let _ = claim.release().await;
            return Err(e);
        }
    };
    claim
        .mark_relayed_and_commit(&relayed.discard)
        .await
        .map_err(|e| {
            format!("{e}; the events are stored, so the next relay of these rows adds nothing")
        })?;
    Ok(StorePass {
        stats: relayed.stats,
        left,
        full,
    })
}

/// Claim a batch of `target`'s waiting rows; `None` when `target` is a branch
/// the org does not have.
async fn claim_store(
    db: &DatabaseConnection,
    org_id: Uuid,
    target: &OutboxTarget,
) -> Result<Option<OutboxClaim>, String> {
    match outbox::claim(db, org_id, target, BATCH).await {
        Ok(claim) => Ok(Some(claim)),
        Err(EventsError::NoBranch { .. }) => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

/// After a pass that began claiming at `started` succeeded in every store:
/// bring the mark up to what is still waiting. Only such a pass does — one
/// in which any store failed never moves the wait (`retry_later`).
///
/// - **Its claim left nothing waiting** (`left` is `None`, read in the claim
///   statement's own snapshot): everything committed before it began is
///   relayed, so the outbox is drained as of `started` ([`marks::mark_drained`];
///   a publish since keeps the mark due, its wait moved up to `started`).
/// - **It left rows** — held by another pass (passes overlap: `enqueue_relay`
///   dedupes queued tasks only), or past a full batch: the mark stays due, and
///   its wait follows the oldest row actually waiting
///   ([`marks::move_wait_up`] to `started` less that row's age, measured in
///   the store). So overlapping passes under steady publishing never pin the
///   wait, while a hung holder's row, or a backlog the relay cannot keep up
///   with, keeps ageing and trips the stale alarm.
///
/// A failure only leaves the mark as it was: the sweep queues one more pass.
async fn settle(
    db: &DatabaseConnection,
    org_id: Uuid,
    environment: &str,
    started: DateTime<Utc>,
    left: Option<std::time::Duration>,
) {
    let marked = match left {
        None => marks::mark_drained(db, org_id, environment, started).await,
        Some(age) => {
            let since = chrono::Duration::from_std(age)
                .ok()
                .and_then(|age| started.checked_sub_signed(age))
                .unwrap_or(DateTime::<Utc>::MIN_UTC);
            marks::move_wait_up(db, org_id, environment, since).await
        }
    };
    if let Err(e) = marked {
        tracing::warn!(%org_id, environment, error = %e, "app events: could not mark the outbox after a pass");
    }
}

/// Mark `org_id`'s `environment` outbox drained as of `now`, for the sweep's
/// mark whose org has no app: nothing in its outbox could be stored. A failure
/// only leaves the mark due.
pub(super) async fn drained(
    db: &DatabaseConnection,
    org_id: Uuid,
    environment: &str,
    now: DateTime<Utc>,
) {
    if let Err(e) = marks::mark_drained(db, org_id, environment, now).await {
        tracing::warn!(%org_id, environment, error = %e, "app events: could not mark the outbox drained");
    }
}

/// A pass in which any store failed leaves that store's rows waiting: reopen
/// the mark
/// ([`marks::reopen`], published now), so the sweep queues another pass even
/// if a concurrent one (which skipped the rows this one held, and so began
/// before this failure) marks it drained. The wait the alarm measures
/// (`pending_since`) goes on.
async fn retry_later(db: &DatabaseConnection, org_id: Uuid, environment: &str) {
    if let Err(e) = marks::reopen(db, org_id, environment).await {
        tracing::warn!(%org_id, environment, error = %e, "app events: could not mark the outbox for a retry");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A failed store is named so an operator can tell which of a sandbox's
    /// stores it was: the shared branch, or which app's own.
    #[test]
    fn each_store_has_a_readable_name() {
        let app_id = Uuid::new_v4();
        assert_eq!(store_name(&OutboxTarget::Production), "production");
        assert_eq!(
            store_name(&OutboxTarget::Branch("dev-x".into())),
            "the `dev-x` branch"
        );
        assert_eq!(
            store_name(&OutboxTarget::EnvBranch {
                app_id,
                environment: "dev-x".into(),
            }),
            format!("app {app_id}'s own `dev-x` branch")
        );
    }
}
