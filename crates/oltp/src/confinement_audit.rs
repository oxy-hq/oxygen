//! The confinement audit: what every role in an org's OLTP database can
//! actually do, read from the database itself.
//!
//! The local `oltp_roles` table records what Oxy *intended*. This reads
//! `pg_roles` and `pg_auth_members` for what is true — the two diverged on
//! Neon, where API-created roles silently carried `neon_superuser`, and nothing
//! in Oxy's own tables would ever have shown it. Then it runs
//! [`crate::grant_audit`] for what each role can reach outside its own schema.
//!
//! Moved from the Rust `oxy oltp audit` (deleted 2026-11-21), which ran this on
//! an operator's laptop with the deployment's database URL and sealing key. It
//! is now `POST /api/admin/orgs/{org_id}/oltp/audit` (`oxyc oltp audit`), and
//! the verdict ([`ConfinementAudit::clean`]) is computed here so no client has
//! to re-derive the rule.
//!
//! Confinement checks what each role is a member OF, not what is a member of
//! IT — so a lingering owner→writer membership left by a failed writer
//! deprovision is invisible here (harmless: the owner already dominates the
//! writer).

use serde::Serialize;
use uuid::Uuid;

use crate::entity::tenants::Model as Tenant;
use crate::grant_audit::{GrantAuditError, RoleFindings};

/// One role, as `pg_roles` says it is.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct RoleConfinement {
    pub role: String,
    pub login: bool,
    /// `admin` for the tenant's owner (it is meant to be powerful: it runs
    /// migrations), `none` for a confined role, and otherwise the attributes
    /// it carries joined by `+` — empty when only a membership makes it
    /// unconfined. The column the CLI printed.
    pub risk: String,
    /// Every attribute from [`crate::roles::CONFINEMENT_ATTRIBUTES`] the role
    /// carries, the owner's included.
    pub attributes: Vec<String>,
    /// The roles this one is a member of.
    pub memberships: Vec<String>,
    /// Whether the role holds no authority it should not. Always true for the
    /// owner.
    pub confined: bool,
}

/// The whole audit for one org.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ConfinementAudit {
    pub org_id: Uuid,
    pub database: String,
    pub owner_role: String,
    pub roles: Vec<RoleConfinement>,
    /// Grants outside each role's own schema, per role ([`crate::grant_audit`]).
    pub grants: Vec<RoleFindings>,
    /// How many roles are not confined.
    pub unconfined: usize,
    /// How many roles hold privileges outside their scope.
    pub drifted: usize,
    /// No unconfined role and no drifted grant: the check passed.
    pub clean: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum AuditError {
    #[error("org {0} has no OLTP database; provision it first")]
    NotProvisioned(Uuid),
    /// The owner credential could not be read or unsealed.
    #[error("{0}")]
    Owner(String),
    /// The tenant database refused the session, or could not be reached. The
    /// text is `pg_detail`'s, never the DSN.
    #[error("connect: {0}")]
    Connect(String),
    #[error("query: {0}")]
    Query(String),
    #[error(transparent)]
    Grants(#[from] GrantAuditError),
}

/// Classify one `pg_roles` row. Pure, so the owner exemption and the
/// attribute and membership rules are testable without a database.
///
/// `flags` must cover the same list `roles::assert_confined_sql` checks. This
/// audit exists to vouch for containment, so an attribute the check rejects
/// and the audit ignores means `risk: none` for a credential provisioning
/// would refuse to hand out. `replication` was exactly that gap: it streams the
/// whole WAL, every other writer's rows included, reading past table ACLs
/// rather than through them.
pub fn classify(
    name: &str,
    flags: &[(&str, bool)],
    login: bool,
    memberships: &str,
    owner_role: &str,
) -> RoleConfinement {
    let attributes: Vec<String> = flags
        .iter()
        .filter(|(_, v)| *v)
        .map(|(k, _)| (*k).to_string())
        .collect();
    let memberships: Vec<String> = memberships
        .split(',')
        .filter(|m| !m.is_empty())
        .map(str::to_string)
        .collect();
    let is_owner = name == owner_role;
    let confined = is_owner || (attributes.is_empty() && memberships.is_empty());
    let risk = if is_owner {
        "admin".to_string()
    } else if confined {
        "none".to_string()
    } else {
        attributes.join("+")
    };
    RoleConfinement {
        role: name.to_string(),
        login,
        risk,
        attributes,
        memberships,
        confined,
    }
}

/// Which tenant an audit is of, for the report's header.
#[derive(Debug, Clone, Copy)]
pub struct AuditedTenant<'a> {
    pub org_id: Uuid,
    pub database: &'a str,
    pub owner_role: &'a str,
}

impl<'a> From<&'a Tenant> for AuditedTenant<'a> {
    fn from(t: &'a Tenant) -> Self {
        Self {
            org_id: t.org_id,
            database: &t.database_name,
            owner_role: &t.owner_role,
        }
    }
}

/// Assemble the verdict from the two halves.
pub fn verdict(
    tenant: AuditedTenant<'_>,
    roles: Vec<RoleConfinement>,
    grants: Vec<RoleFindings>,
) -> ConfinementAudit {
    let unconfined = roles.iter().filter(|r| !r.confined).count();
    let drifted = grants.iter().filter(|g| !g.findings.is_empty()).count();
    ConfinementAudit {
        org_id: tenant.org_id,
        database: tenant.database.to_string(),
        owner_role: tenant.owner_role.to_string(),
        roles,
        grants,
        unconfined,
        drifted,
        clean: unconfined == 0 && drifted == 0,
    }
}

/// Run the audit against `org_id`'s database, as its owner.
pub async fn audit_org(
    db: &sea_orm::DatabaseConnection,
    org_id: Uuid,
) -> Result<ConfinementAudit, AuditError> {
    let tenant = crate::migrator::tenant_for_org(db, org_id)
        .await
        .map_err(|e| match e {
            crate::migrator::MigrateError::NotProvisioned(id) => AuditError::NotProvisioned(id),
            other => AuditError::Owner(other.to_string()),
        })?;
    let owner_dsn =
        crate::migrator::owner_dsn(&tenant).map_err(|e| AuditError::Owner(e.to_string()))?;

    // `pg_detail`, not `{e}`: a refused session displays as the literal "db
    // error", which hid Neon's `exceeded the quota` for two days.
    let client = crate::connect::connect(&owner_dsn, "oltp audit")
        .await
        .map_err(|e| AuditError::Connect(crate::connect::pg_detail(&e)))?;

    let roles = role_rows(&client, &tenant).await?;
    // What each role can reach, not just what it is: a grant outside its own
    // schema is how one writer reads another's rows (`grant_audit`).
    let grants = crate::grant_audit::tenant_findings(db, &client, &tenant).await?;
    Ok(verdict((&tenant).into(), roles, grants))
}

/// Every role this tenant owns, classified.
async fn role_rows(
    client: &tokio_postgres::Client,
    tenant: &Tenant,
) -> Result<Vec<RoleConfinement>, AuditError> {
    // Columns generated from `roles::CONFINEMENT_ATTRIBUTES`, so this audit
    // and `assert_confined_sql` can never check different things again — the
    // divergence that let a REPLICATION-carrying role print `risk: none`.
    let attrs = crate::roles::CONFINEMENT_ATTRIBUTES;
    let attr_columns = attrs
        .iter()
        .map(|(_, column)| format!("r.{column}"))
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "SELECT r.rolname, {attr_columns}, r.rolcanlogin, \
           coalesce((SELECT string_agg(g.rolname, ',') FROM pg_auth_members m \
                     JOIN pg_roles g ON g.oid = m.roleid \
                     WHERE m.member = r.oid), '') AS memberships \
         FROM pg_roles r \
         WHERE r.rolname = $1 OR r.rolname LIKE 'app\\_%' OR r.rolname LIKE 'raw\\_%' \
            OR r.rolname = $2 \
         ORDER BY r.rolname"
    );
    // The tenant's OWN analyst, not the bare constant. On a cluster where
    // tenants share a role namespace the real login is `oxy_analyst_ro_<tag>`,
    // so auditing the constant reported on a decoy — and reported it clean,
    // which is the worst way to be wrong about a privilege audit.
    let analyst = crate::schema::analyst_role_for(&tenant.provider, &tenant.database_name);
    let rows = client
        .query(&sql, &[&analyst, &tenant.owner_role])
        .await
        .map_err(|e| AuditError::Query(crate::connect::pg_detail(&e)))?;

    Ok(rows
        .iter()
        .map(|row| {
            let flags: Vec<(&str, bool)> = attrs
                .iter()
                .enumerate()
                .map(|(i, (label, _))| (*label, row.get::<_, bool>(i + 1)))
                .collect();
            let login: bool = row.get(attrs.len() + 1);
            let memberships: String = row.get(attrs.len() + 2);
            classify(
                &row.get::<_, String>(0),
                &flags,
                login,
                &memberships,
                &tenant.owner_role,
            )
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    const OWNER: &str = "oxy_owner";

    fn none() -> Vec<(&'static str, bool)> {
        crate::roles::CONFINEMENT_ATTRIBUTES
            .iter()
            .map(|(label, _)| (*label, false))
            .collect()
    }

    fn with(attr: &str) -> Vec<(&'static str, bool)> {
        none()
            .into_iter()
            .map(|(label, _)| (label, label == attr))
            .collect()
    }

    #[test]
    fn a_role_with_nothing_is_confined() {
        let r = classify("app_bookings_rw", &none(), true, "", OWNER);
        assert!(r.confined);
        assert_eq!(r.risk, "none");
        assert!(r.attributes.is_empty() && r.memberships.is_empty());
    }

    /// The owner is meant to be powerful, so its authority is never a finding.
    #[test]
    fn the_owner_is_admin_and_never_a_finding() {
        let r = classify(OWNER, &with("superuser"), true, "neon_superuser", OWNER);
        assert!(r.confined, "the owner runs migrations; it is not a finding");
        assert_eq!(r.risk, "admin");
        assert_eq!(r.attributes, vec!["superuser"]);
        assert_eq!(r.memberships, vec!["neon_superuser"]);
    }

    /// Every attribute the provisioning check rejects is one the audit flags —
    /// `replication` was the one that once printed `risk: none`.
    #[test]
    fn every_confinement_attribute_is_a_finding() {
        for (label, _) in crate::roles::CONFINEMENT_ATTRIBUTES {
            let r = classify("app_x_rw", &with(label), true, "", OWNER);
            assert!(!r.confined, "{label} must make a writer unconfined");
            assert_eq!(r.risk, *label);
        }
    }

    /// The Neon failure was a membership, not an attribute.
    #[test]
    fn a_membership_alone_is_a_finding() {
        let r = classify(
            "oxy_analyst_ro",
            &none(),
            true,
            "neon_superuser,pg_read",
            OWNER,
        );
        assert!(!r.confined);
        assert_eq!(r.memberships, vec!["neon_superuser", "pg_read"]);
        assert_eq!(
            r.risk, "",
            "no attribute to name; the membership is the finding"
        );
    }

    fn tenant() -> AuditedTenant<'static> {
        AuditedTenant {
            org_id: Uuid::nil(),
            database: "oxy_org_x",
            owner_role: OWNER,
        }
    }

    #[test]
    fn the_verdict_counts_both_halves() {
        let t = tenant();
        let roles = vec![
            classify(OWNER, &with("superuser"), true, "", OWNER),
            classify("app_a_rw", &none(), true, "", OWNER),
            classify("app_b_rw", &with("createrole"), true, "", OWNER),
        ];
        let grants = vec![
            RoleFindings {
                role: "app_a_rw".into(),
                findings: vec![],
            },
            RoleFindings {
                role: "PUBLIC".into(),
                findings: vec!["USAGE on schema app_b".into()],
            },
        ];
        let v = verdict(t, roles.clone(), grants.clone());
        assert_eq!((v.unconfined, v.drifted, v.clean), (1, 1, false));

        let clean = verdict(t, roles[..2].to_vec(), grants[..1].to_vec());
        assert_eq!((clean.unconfined, clean.drifted, clean.clean), (0, 0, true));
    }

    /// The JSON `oxyc oltp audit` parses. A rename here breaks every caller.
    #[test]
    fn the_wire_shape_is_pinned() {
        let v = verdict(
            tenant(),
            vec![classify(
                "app_a_rw",
                &with("replication"),
                false,
                "x",
                OWNER,
            )],
            vec![RoleFindings {
                role: "app_a_rw".into(),
                findings: vec!["f".into()],
            }],
        );
        let json = serde_json::to_value(&v).expect("serializes");
        let mut keys: Vec<&str> = json
            .as_object()
            .expect("an object")
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "clean",
                "database",
                "drifted",
                "grants",
                "org_id",
                "owner_role",
                "roles",
                "unconfined"
            ]
        );
        assert_eq!(
            json["roles"][0],
            serde_json::json!({
                "role": "app_a_rw",
                "login": false,
                "risk": "replication",
                "attributes": ["replication"],
                "memberships": ["x"],
                "confined": false,
            })
        );
        assert_eq!(
            json["grants"][0],
            serde_json::json!({ "role": "app_a_rw", "findings": ["f"] })
        );
    }
}
