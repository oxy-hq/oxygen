//! A cut branch's change stream, seeded so it captures nothing (OLTP change
//! stream design §8 phase 1, "Branches don't capture yet").
//!
//! A branch is a copy of production, so production's capture reaches it
//! whole: each declared table's functions and triggers, its `tables` row (so
//! the DDL guard holds on the branch too, and a migration that would break
//! capture fails on staging before it reaches production), the `stream` row,
//! and every log row not yet shipped. Until phase 2 gives a branch a
//! generation of its own, each cut and reset replaces the branch's `stream`
//! row with one that does not capture and empties the log it inherited, so a
//! branch neither fails its writes nor grows a log no relay drains. A
//! schema-only branch arrives with no `stream` row at all, which would fail
//! every write to a captured table; this is what seeds it.
//!
//! [`SEED_BRANCH_STREAM_SQL`] runs as the branch's owner, who owns
//! `oxy_changes` there as on production, once the cut has recorded that owner
//! and before any credential lets a caller in. It does nothing on a branch of
//! a store whose platform schema predates step 8. A failure fails the cut, as
//! the ledger copy's does, and the next attempt seeds again.

use tracing::info;

use crate::OltpBranch;
use crate::changes::SEED_BRANCH_STREAM_SQL;
use crate::entity::tenants as oltp_tenants;
use crate::provisioner::branch_site::{BranchSite, RoleStore};
use crate::provisioner::{OltpProvisioner, ProvisionerError};

impl OltpProvisioner {
    /// Seed `site`'s change stream as not capturing, as the branch's owner.
    pub(in crate::provisioner) async fn seed_branch_change_stream(
        &self,
        tenant: &oltp_tenants::Model,
        site: &BranchSite,
    ) -> Result<(), ProvisionerError> {
        refuse_production_site(tenant, site)?;
        let owner = self.branch_owner_dsn(tenant, site)?;
        self.sql
            .execute_batch(&owner, &[SEED_BRANCH_STREAM_SQL.to_string()])
            .await?;
        info!(branch = %site.label, "seeded the branch's change stream as not capturing");
        Ok(())
    }
}

/// Refuse a site on production's endpoint and database. The seed switches
/// capture off and empties the log wherever it runs, so it must never reach
/// production, whatever a row or a provider's answer says. Decided by where
/// the owner's DSN lands, the one thing the seed acts on.
fn refuse_production_site(
    tenant: &oltp_tenants::Model,
    site: &BranchSite,
) -> Result<(), ProvisionerError> {
    if !crate::resolver::names_production(tenant, "", &site.host, &site.database_name) {
        return Ok(());
    }
    let branch_id = format!("{}/{}", site.host, site.database_name);
    Err(match (site.roles, OltpBranch::parse(&site.label)) {
        (RoleStore::OrgBranch(_), Some(branch)) => ProvisionerError::BranchIsProduction {
            org_id: tenant.org_id,
            branch,
            branch_id,
        },
        _ => ProvisionerError::EnvBranchIsProduction {
            org_id: tenant.org_id,
            env_name: site.label.clone(),
            branch_id,
        },
    })
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use super::*;
    use crate::platform::{CHANGES_LOG_TABLE, CHANGES_STREAM_TABLE};

    #[test]
    fn the_seed_is_guarded_and_stops_capture() {
        let sql = SEED_BRANCH_STREAM_SQL;
        assert!(
            sql.starts_with(&format!(
                "DO $$ BEGIN IF to_regclass('{CHANGES_STREAM_TABLE}') IS NOT NULL THEN "
            )),
            "nothing runs on a store without step 8: {sql}"
        );
        assert!(
            sql.ends_with(" END IF; END $$"),
            "everything runs inside the guard: {sql}"
        );
        let delete = sql
            .find(&format!("DELETE FROM {CHANGES_STREAM_TABLE};"))
            .expect("the inherited stream row goes");
        let insert = sql
            .find(&format!(
                "INSERT INTO {CHANGES_STREAM_TABLE} (generation, capturing) VALUES (0, false);"
            ))
            .expect("a stream that does not capture takes its place");
        assert!(delete < insert, "{sql}");
        assert!(
            sql.contains(&format!("TRUNCATE {CHANGES_LOG_TABLE};")),
            "the inherited log is emptied: {sql}"
        );
        assert!(
            !sql[5..sql.len() - 2].contains("$$"),
            "the body must not close the quote: {sql}"
        );
    }

    fn tenant(provider: &str, host: &str, database: &str) -> oltp_tenants::Model {
        serde_json::from_value(serde_json::json!({
            "id": Uuid::nil(),
            "org_id": Uuid::nil(),
            "provider": provider,
            "project_id": "p",
            "branch_id": "br-main",
            "project_name": "oxy-org-x",
            "region": "aws-us-east-2",
            "pg_version": 17,
            "host": host,
            "database_name": database,
            "owner_role": "oxy_owner",
            "owner_password_ciphertext": null,
            "analyst_password_ciphertext": null,
            "status": "active",
            "platform_schema_version": 8,
            "created_at": "2026-11-24T00:00:00Z",
            "updated_at": "2026-11-24T00:00:00Z"
        }))
        .expect("a tenant row")
    }

    fn site(label: &str, roles: RoleStore, host: &str, database: &str) -> BranchSite {
        BranchSite {
            label: label.to_string(),
            row_id: Uuid::nil(),
            host: host.to_string(),
            database_name: database.to_string(),
            owner_role: "oxy_owner".to_string(),
            owner_password_ciphertext: None,
            roles,
        }
    }

    /// The seed switches capture off wherever it lands, so a site on
    /// production's endpoint and database is refused, whichever table the
    /// branch is recorded in.
    #[test]
    fn the_seed_never_reaches_production() {
        const PROD: &str = "ep-prod-1.us-east-2.aws.neon.tech";
        let neon = tenant("neon", PROD, "neondb");
        let org = RoleStore::OrgBranch(Uuid::nil());
        let env = RoleStore::Environment(Uuid::nil());

        let staging = site("staging", org, "ep-stg-2.us-east-2.aws.neon.tech", "neondb");
        assert!(refuse_production_site(&neon, &staging).is_ok());
        assert!(matches!(
            refuse_production_site(&neon, &site("staging", org, PROD, "neondb")),
            Err(ProvisionerError::BranchIsProduction { .. })
        ));
        assert!(matches!(
            refuse_production_site(&neon, &site("preview", env, PROD, "neondb")),
            Err(ProvisionerError::EnvBranchIsProduction { .. })
        ));

        // A shared cluster: the database name alone decides.
        let local = tenant("local", "localhost:15432", "oxy_org_x");
        let branch = site("preview", env, "localhost:15432", "oxy_org_x_5ad1e2f3");
        assert!(refuse_production_site(&local, &branch).is_ok());
        assert!(
            refuse_production_site(&local, &site("preview", env, "127.0.0.1", "oxy_org_x"))
                .is_err()
        );
    }
}
