//! Apply-then-promote — the step that turns a deferred compile into a live
//! revision.
//!
//! `oxy-compile` withholds promotion for any revision carrying
//! `schemas/*.sql` ([`Promotion::Deferred`]). Pointing
//! `workspaces.current_revision_id` at such a revision before its DDL has
//! reached the org's database serves an app against tables that do not
//! exist yet — the failure lands on an end user as a raw
//! `relation does not exist`, several seconds after a deploy that reported
//! success.
//!
//! The compiler cannot close that gap itself for two reasons, and both are
//! deliberate. Applying is a network round-trip to a tenant database, and
//! promotion happens inside the finalise transaction — which holds a
//! unique-index lock on `revisions`, so a remote call there would serialise
//! every concurrent compile behind one tenant's DDL. And `oxy-compile` does
//! not depend on `oxy-oltp`: they are siblings over `entity`, which is what
//! keeps the compiler ignorant of where a tenant's Postgres lives.
//!
//! So the two meet here, in the app layer, which may depend on both. Both
//! compile entry points — the worker and `oxy compile` — call
//! [`settle_deferred_promotion`] with the outcome they got back.

use std::time::Duration;

use oxy_compile::{CompileOutcome, Promotion, promote_existing};
use oxy_oltp::migrator::{self, MigrateError};
use sea_orm::{DatabaseConnection, EntityTrait};
use tracing::{info, instrument, warn};
use uuid::Uuid;

/// How many times to re-attempt an apply that lost the tenant's advisory lock.
///
/// The lock is held for the length of one apply — seconds, not minutes — so a
/// contended apply is almost always a concurrent compile of the same workspace
/// that is about to finish. Three tries a second apart covers that without
/// turning a genuinely stuck tenant into a worker that waits forever.
const APPLY_RETRIES: u32 = 3;
const APPLY_RETRY_BACKOFF: Duration = Duration::from_secs(1);

/// What [`settle_deferred_promotion`] did.
///
/// Every variant except [`Self::Failed`] and [`Self::Busy`] leaves the
/// workspace pointing at a usable revision. The two that don't are kept
/// apart on purpose: `Busy` is somebody else mid-apply and the right
/// response is to try again, while `Failed` is DDL that will not apply
/// until a human changes it. Collapsing them would turn a retryable
/// condition into a page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Settled {
    /// The compile did not defer — it promoted, declined, or lost the
    /// causality race on its own. Nothing to do.
    NotDeferred(Promotion),
    /// The revision carries DDL, but there is nowhere to apply it: the
    /// workspace has no org, or the org has no provisioned OLTP database.
    ///
    /// **Promoted anyway.** A workspace whose OLTP database does not exist
    /// is a workspace where nothing can call `ctx.oltp` in the first place,
    /// so withholding here would wedge the entire revision — semantic
    /// views, agents, apps, everything — over an optional feature nobody
    /// on that workspace can reach. The compile is not the place to
    /// discover you meant to provision a database.
    NoTenant {
        reason: &'static str,
        /// What promoting it actually did. **Not assumed.** `promote_existing`
        /// returns [`Promotion::Skipped`] when the `started_at` causality
        /// clause rejects the UPDATE because a newer revision is already
        /// current — and hardcoding `Promoted` here reported a revision as live
        /// that `current_revision_id` does not point at, which is the exact
        /// distinction `Promotion::Skipped` was introduced to preserve.
        promotion: Promotion,
    },
    /// DDL applied, revision promoted.
    Applied {
        applied: usize,
        already_applied: usize,
        promotion: Promotion,
    },
    /// Another apply still held the tenant's advisory lock after
    /// [`APPLY_RETRIES`] attempts. The revision stays unpromoted and the
    /// previous one keeps serving.
    ///
    /// This **fails the compile task**. It used to be reported as a success on
    /// the grounds that it was "retryable", but nothing retried it: two workers
    /// compiling the same workspace could leave the newer revision permanently
    /// unpromoted while the task went green. A visible failure that says
    /// "re-run" beats a green task hiding a stale pointer.
    Busy,
    /// The DDL failed. The revision stays unpromoted, so the workspace goes
    /// on serving the last revision whose tables really exist.
    Failed { message: String },
}

impl Settled {
    /// Whether the caller should treat this as a successful compile.
    ///
    /// `Busy` is a failure. The rows are written and a re-run will promote
    /// them, but nothing re-runs on its own, so reporting success would leave
    /// `current_revision_id` on the old revision with a green task above it.
    pub fn compile_succeeded(&self) -> bool {
        !matches!(self, Settled::Failed { .. } | Settled::Busy)
    }

    /// What `current_revision_id` reflects **after** this step.
    ///
    /// `CompileOutcome.promotion` is what the compiler decided, which for a
    /// deferred revision is `Deferred` — true when it was returned and stale a
    /// moment later. A caller that serializes the outcome (`oxy compile
    /// --json`) has to fold this in, or it reports "deferred" for a revision
    /// that is already live.
    pub fn effective_promotion(&self, before: &Promotion) -> Promotion {
        match self {
            Settled::NotDeferred(p) => p.clone(),
            // Nothing to apply, but the promote still had to be attempted —
            // and it can still lose the causality race.
            Settled::NoTenant { promotion, .. } => promotion.clone(),
            Settled::Applied { promotion, .. } => promotion.clone(),
            // Still not promoted, so the compiler's answer still holds.
            Settled::Busy | Settled::Failed { .. } => before.clone(),
        }
    }

    /// One line for the task outcome / CLI. Deliberately names the tenant
    /// database as the thing that failed: the first instinct on a failed
    /// compile is to go re-read the YAML, which is the wrong file.
    pub fn summary(&self) -> String {
        match self {
            Settled::NotDeferred(_) => String::new(),
            Settled::NoTenant { reason, .. } => {
                format!("schema migrations skipped — {reason}")
            }
            Settled::Applied {
                applied,
                already_applied,
                ..
            } => {
                format!("{applied} schema migration(s) applied, {already_applied} already present")
            }
            Settled::Busy => format!(
                "schema migrations could not run — another apply held the tenant lock \
                 through {APPLY_RETRIES} attempts; re-run the compile"
            ),
            Settled::Failed { message } => {
                format!("schema migration failed against the org database: {message}")
            }
        }
    }
}

/// Apply this revision's `schemas/*.sql` to the workspace's org database,
/// then promote it.
///
/// A no-op for every compile that did not defer, which is every compile in
/// a workspace with no `schemas/` directory.
#[instrument(skip(db, outcome), fields(workspace_id = %workspace_id, revision_id = %outcome.revision_id))]
pub async fn settle_deferred_promotion(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    outcome: &CompileOutcome,
) -> Settled {
    let Some(count) = outcome.promotion.deferred_count() else {
        return Settled::NotDeferred(outcome.promotion.clone());
    };

    let org_id = match org_for_workspace(db, workspace_id).await {
        Ok(Some(id)) => id,
        Ok(None) => {
            return promote_without_ddl(
                db,
                workspace_id,
                outcome,
                "the workspace does not belong to an org",
            )
            .await;
        }
        Err(e) => {
            return Settled::Failed {
                message: format!("could not read the workspace's org: {e}"),
            };
        }
    };

    let tenant = match migrator::tenant_for_org(db, org_id).await {
        Ok(t) => t,
        // Not an error. Most orgs will never provision one.
        Err(MigrateError::NotProvisioned(_)) => {
            return promote_without_ddl(
                db,
                workspace_id,
                outcome,
                "the org has no OLTP database provisioned",
            )
            .await;
        }
        Err(e) => {
            return Settled::Failed {
                message: e.to_string(),
            };
        }
    };

    let dsn = match migrator::owner_dsn(&tenant) {
        Ok(d) => d,
        Err(e) => {
            return Settled::Failed {
                message: e.to_string(),
            };
        }
    };

    info!(%org_id, migrations = count, "applying schema migrations before promoting");

    // Retry a contended lock rather than merely reporting one. `apply_to_org`
    // is idempotent — an already-applied file costs one ledger read — so
    // re-entering it is cheap and cannot double-apply.
    let mut attempt = 0;
    let applied = loop {
        attempt += 1;
        match migrator::apply_to_org(
            db,
            org_id,
            outcome.revision_id,
            &dsn,
            tenant.id,
            &tenant.owner_role,
        )
        .await
        {
            Ok(o) => break o,
            Err(MigrateError::Locked { .. }) if attempt < APPLY_RETRIES => {
                warn!(%org_id, attempt, "another apply holds the tenant lock — retrying");
                tokio::time::sleep(APPLY_RETRY_BACKOFF).await;
            }
            Err(MigrateError::Locked { .. }) => {
                warn!(
                    %org_id,
                    attempts = attempt,
                    "tenant lock still held — leaving the revision unpromoted"
                );
                return Settled::Busy;
            }
            Err(e) => {
                warn!(%org_id, error = %e, "schema migration failed — revision NOT promoted");
                return Settled::Failed {
                    message: e.to_string(),
                };
            }
        }
    };

    match promote_existing(db, workspace_id, outcome.revision_id).await {
        Ok(promotion) => Settled::Applied {
            applied: applied.applied.len(),
            already_applied: applied.already_applied,
            promotion,
        },
        Err(e) => Settled::Failed {
            message: format!(
                "schema migrations applied but promotion failed; \
                 the workspace may still serve the previous revision: {e}"
            ),
        },
    }
}

/// Promote a deferred revision that has no database to apply DDL to.
///
/// Split out because the two ways of getting here read very differently
/// at the call site but must both end the same way — promoted, with the
/// reason logged once.
async fn promote_without_ddl(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    outcome: &CompileOutcome,
    reason: &'static str,
) -> Settled {
    warn!(
        revision_id = %outcome.revision_id,
        reason,
        "revision carries schemas/*.sql but has nowhere to apply them — promoting without DDL"
    );
    match promote_existing(db, workspace_id, outcome.revision_id).await {
        Ok(promotion) => Settled::NoTenant { reason, promotion },
        Err(e) => Settled::Failed {
            message: format!("promotion failed: {e}"),
        },
    }
}

async fn org_for_workspace(
    db: &DatabaseConnection,
    workspace_id: Uuid,
) -> Result<Option<Uuid>, sea_orm::DbErr> {
    Ok(entity::workspaces::Entity::find_by_id(workspace_id)
        .one(db)
        .await?
        .and_then(|w| w.org_id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn busy_fails_the_compile_rather_than_hiding_a_stale_pointer() {
        // This assertion was inverted. `Busy` leaves the revision unpromoted
        // and nothing re-runs the compile on its own, so reporting success
        // meant a green task above a workspace still serving the OLD revision.
        // Retrying first (APPLY_RETRIES) is what keeps this from being noisy.
        assert!(!Settled::Busy.compile_succeeded());
        assert!(
            !Settled::Failed {
                message: "boom".into()
            }
            .compile_succeeded()
        );
        assert!(
            Settled::NoTenant {
                reason: "x",
                promotion: Promotion::Promoted,
            }
            .compile_succeeded()
        );
    }

    #[test]
    fn no_tenant_reports_the_promotion_it_got_rather_than_assuming_success() {
        // The regression this variant's field exists for. `promote_existing`
        // returns `Skipped` when the started_at causality clause rejects the
        // UPDATE — a newer revision is already current. Hardcoding `Promoted`
        // here made `oxy compile --json` print "promoted", and
        // `Promotion::is_live()` return true, for a revision
        // `current_revision_id` does not point at. That is precisely the
        // distinction `Promotion::Skipped` was introduced to preserve.
        let deferred = Promotion::Deferred {
            schema_migration_count: 1,
        };

        let lost_the_race = Settled::NoTenant {
            reason: "the org has no OLTP database provisioned",
            promotion: Promotion::Skipped,
        };
        assert_eq!(
            lost_the_race.effective_promotion(&deferred),
            Promotion::Skipped
        );
        assert!(!lost_the_race.effective_promotion(&deferred).is_live());

        let won = Settled::NoTenant {
            reason: "the workspace does not belong to an org",
            promotion: Promotion::Promoted,
        };
        assert!(won.effective_promotion(&deferred).is_live());
    }

    #[test]
    fn a_revision_that_never_promoted_keeps_reporting_deferred() {
        // Busy and Failed both leave the pointer where it was, so the
        // compiler's own answer is still the true one — `--json` must not
        // claim otherwise.
        let deferred = Promotion::Deferred {
            schema_migration_count: 2,
        };
        assert_eq!(Settled::Busy.effective_promotion(&deferred), deferred);
        assert_eq!(
            Settled::Failed {
                message: "boom".into()
            }
            .effective_promotion(&deferred),
            deferred
        );
    }

    #[test]
    fn summary_points_at_the_database_not_the_yaml() {
        let s = Settled::Failed {
            message: "syntax error at or near \"CREAT\"".into(),
        }
        .summary();
        assert!(s.contains("org database"), "got {s}");
    }

    #[test]
    fn not_deferred_summarises_to_nothing() {
        // Appended to a task outcome line, so it must be empty rather than
        // "nothing to do" for the overwhelmingly common case.
        assert!(
            Settled::NotDeferred(Promotion::Promoted)
                .summary()
                .is_empty()
        );
    }
}
