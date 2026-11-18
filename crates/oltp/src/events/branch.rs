//! The outbox on an OLTP branch: where staging's and the sandboxes' events
//! wait (app events design §6.2, Task 17).
//!
//! [`OutboxTarget::Branch`](super::OutboxTarget::Branch) names an
//! environment, and the org's **staging branch** hosts every non-production
//! environment's outbox: staging's own, and each sandbox's whose `ctx.oltp`
//! runs in a schema of its own there (`crate::sandbox_schema`), its writer
//! reaching `oxy_events` beside that schema. A sandbox on an OLTP branch of
//! its own (`oltp_env_branches`) is not hosted here: the host refuses its
//! publishes before anything is written.
//!
//! **A branch is a copy of production taken when it was cut**, so a branch
//! cut before the outbox existed (platform step 5) has no `oxy_events`. The
//! first grant or claim on it applies the platform steps after the version
//! its row records (`oltp_branches.platform_schema_version`), as the branch's
//! owner, through the path production's tenant is brought current by
//! ([`crate::provisioner::apply_platform_steps`]). A read never reconciles:
//! a body is only ever in a store the outbox reached.
//!
//! **`None` is "this org has no such branch"**: nothing was ever published to
//! one (a publish without a branch is held), or it went with the org. Every
//! other gap, a branch mid-reset or one recorded as production's own
//! database, is an error, never a fallback to production.

use sea_orm::DatabaseConnection;
use uuid::Uuid;

use super::EventsError;
use crate::provisioner::reconcile_branch_platform_schema_as_owner;
use crate::resolver::{ResolveError, branch_hosting, branch_owner_dsn, resolve_branch_owner};
use crate::sql::PgSqlExecutor;

/// The owner DSN of the branch hosting `environment`'s outbox. With
/// `bring_current` (a publish's grant, a claim), its Oxy-owned objects are
/// brought current first. `None`: the org has no such branch, or
/// `environment` is none a branch hosts.
pub(super) async fn owner_dsn(
    control: &DatabaseConnection,
    org_id: Uuid,
    environment: &str,
    bring_current: bool,
) -> Result<Option<String>, EventsError> {
    if !bring_current {
        return branch_owner_dsn(control, org_id, environment)
            .await
            .map_err(|e| resolve_error(org_id, e));
    }
    let Some(kind) = branch_hosting(environment) else {
        return Ok(None);
    };
    let Some(owner) = resolve_branch_owner(control, org_id, kind)
        .await
        .map_err(|e| resolve_error(org_id, e))?
    else {
        return Ok(None);
    };
    reconcile_branch_platform_schema_as_owner(control, &PgSqlExecutor, &owner)
        .await
        .map_err(|e| EventsError::Store(format!("bring the org's OLTP branch up to date: {e}")))?;
    Ok(Some(owner.dsn))
}

/// What a failed branch lookup means to the outbox: no usable tenant is the
/// operator step production's store names; anything else (a branch mid-reset,
/// one naming production) is a store error, never a fallback.
fn resolve_error(org_id: Uuid, e: ResolveError) -> EventsError {
    match e {
        ResolveError::NotProvisioned(_) | ResolveError::NotActive(..) => {
            EventsError::NotProvisioned { org_id }
        }
        other => EventsError::Store(format!("open the org's OLTP branch: {other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::OltpBranch;

    #[test]
    fn no_usable_tenant_names_the_operator_step_and_the_rest_is_a_store_error() {
        let org = Uuid::nil();
        for gap in [
            ResolveError::NotProvisioned(org),
            ResolveError::NotActive(org, "failed"),
        ] {
            assert!(
                matches!(resolve_error(org, gap), EventsError::NotProvisioned { .. }),
                "no tenant to branch"
            );
        }
        for gap in [
            ResolveError::BranchNotActive(org, OltpBranch::Staging, "resetting"),
            ResolveError::BranchIsProduction(org, OltpBranch::Staging),
        ] {
            let err = resolve_error(org, gap);
            assert!(matches!(err, EventsError::Store(_)), "{err}");
            assert!(err.to_string().contains("OLTP branch"), "{err}");
        }
    }
}
