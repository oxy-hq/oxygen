//! Letting a writer publish into a store's outbox: `USAGE` on the events
//! schema and `EXECUTE` on its definer function, granted lazily, **per
//! store** (ruling R24). Production's store and a branch are separate
//! databases with separate catalogs, so a grant in one is no grant in the
//! other, and each is granted on the first publish there.
//!
//! The grant audit (`crate::grant_audit`, R10) accepts exactly these two
//! privileges for a writer, on a branch as on production.

use std::collections::HashSet;
use std::sync::{Mutex, MutexGuard, OnceLock};

use sea_orm::DatabaseConnection;
use uuid::Uuid;

use super::{EventsError, OutboxTarget, store, store::required_client};
use crate::platform::{EVENTS_PUBLISH_FUNCTION, EVENTS_SCHEMA};
use crate::schema::{quote_ident, validate_name};

/// One store's grant to one writer: the org, which of its stores, and the
/// writer role.
type Grant = (Uuid, OutboxTarget, String);

/// The grants already made in this process.
fn publishable() -> MutexGuard<'static, HashSet<Grant>> {
    static PUBLISHABLE: OnceLock<Mutex<HashSet<Grant>>> = OnceLock::new();
    PUBLISHABLE
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Let `writer_role` — the role's real name, qualified on a shared-cluster
/// provider — call `oxy_events.publish` in `target`, the org's store a
/// publish is written to. Asked before an app's first publish there, the
/// same lazy way `ctx.notify` prepares its inbox: brings the store's
/// Oxy-owned objects current (a branch cut before the outbox gets it now),
/// then grants `USAGE` on the schema and `EXECUTE` on the function, through
/// the store's owner.
///
/// Remembered per `(org, store, role)` for the life of the process; a failure
/// is not, so the next publish asks again.
pub async fn ensure_publishable(
    control: &DatabaseConnection,
    org_id: Uuid,
    target: &OutboxTarget,
    writer_role: &str,
) -> Result<(), EventsError> {
    let key = (org_id, target.clone(), writer_role.to_string());
    let known = publishable().contains(&key);
    if known {
        return Ok(());
    }
    let grant = grant_publish_sql(writer_role)?;
    let client = required_client(control, org_id, target, true).await?;
    client
        .batch_execute(&grant)
        .await
        .map_err(|e| store(&format!("let {writer_role} publish events"), &e))?;
    publishable().insert(key);
    Ok(())
}

/// Drop what [`ensure_publishable`] remembers for `writer_role` in `target`,
/// so the next call grants again there, bringing the store current first. For
/// a publish refused though the grant was remembered: `42501` (insufficient
/// privilege), the grant gone with the role (a re-mint's `DROP OWNED`), the
/// tenant or the branch; `42883` or `3F000`, a reset copied production from
/// before the current definer or the outbox. This process cannot tell.
pub fn forget_publishable(org_id: Uuid, target: &OutboxTarget, writer_role: &str) {
    publishable().remove(&(org_id, target.clone(), writer_role.to_string()));
}

/// The grants [`ensure_publishable`] makes, as one implicit transaction.
///
/// Serialised on an advisory lock: two sessions granting on one catalog row at
/// once can fail with `tuple concurrently updated`, and every replica's first
/// publish for an app grants.
fn grant_publish_sql(writer_role: &str) -> Result<String, EventsError> {
    validate_name(writer_role).map_err(|e| EventsError::Store(e.to_string()))?;
    let role = quote_ident(writer_role);
    Ok(format!(
        "SELECT pg_advisory_xact_lock(hashtext('{EVENTS_PUBLISH_FUNCTION}')); \
         GRANT USAGE ON SCHEMA {EVENTS_SCHEMA} TO {role}; \
         GRANT EXECUTE ON FUNCTION {EVENTS_PUBLISH_FUNCTION} TO {role}"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// R24: the cache is per store, so a grant on production says nothing
    /// about a branch, and forgetting one store's grant keeps the others'.
    #[test]
    fn forgetting_drops_that_stores_grant_alone() {
        let org = Uuid::new_v4();
        let other_org = Uuid::new_v4();
        let staging = OutboxTarget::Branch("staging".into());
        let grants = [
            (org, OutboxTarget::Production, "app_a_rw".to_string()),
            (org, staging.clone(), "app_a_rw".to_string()),
            (org, OutboxTarget::Production, "app_b_rw".to_string()),
            (other_org, OutboxTarget::Production, "app_a_rw".to_string()),
        ];
        for grant in grants.clone() {
            publishable().insert(grant);
        }

        forget_publishable(org, &OutboxTarget::Production, "app_a_rw");

        let cache = publishable();
        assert!(!cache.contains(&grants[0]), "forgotten");
        for kept in &grants[1..] {
            assert!(cache.contains(kept), "{kept:?} is kept");
        }
        assert!(
            !cache.contains(&(org, OutboxTarget::Branch("dev-x".into()), "app_a_rw".into())),
            "one branch environment's grant is not another's"
        );
        let own = OutboxTarget::EnvBranch {
            app_id: Uuid::new_v4(),
            environment: "staging".into(),
        };
        assert!(
            !cache.contains(&(org, own, "app_a_rw".into())),
            "a sandbox's own branch is a store of its own: the staging branch's grant is not its"
        );
    }

    #[test]
    fn the_grant_reaches_the_function_and_the_schema_only() {
        let sql = grant_publish_sql("app_bookings_rw_1a2b3c4d").unwrap();
        assert!(
            sql.contains(r#"GRANT USAGE ON SCHEMA oxy_events TO "app_bookings_rw_1a2b3c4d""#),
            "{sql}"
        );
        assert!(
            sql.contains(&format!(
                r#"GRANT EXECUTE ON FUNCTION {EVENTS_PUBLISH_FUNCTION} TO "app_bookings_rw_1a2b3c4d""#
            )),
            "{sql}"
        );
        assert!(!sql.contains("ON TABLE"), "never the table: {sql}");
        assert!(
            !sql.contains(" ALL "),
            "never ALL PRIVILEGES or ON ALL: {sql}"
        );
    }

    #[test]
    fn a_role_name_is_validated_before_any_sql() {
        assert!(grant_publish_sql("app_x\"; DROP SCHEMA oxy_events; --").is_err());
        assert!(grant_publish_sql("").is_err());
    }
}
