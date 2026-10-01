//! Does a branch row name production? The resolver's last refusal before a
//! staging caller holds a connection.
//!
//! By id, or by endpoint. An endpoint is compared as the server it reaches
//! ([`crate::host::server`]), not as the string that was stored: `localhost`,
//! `localhost:5432`, `127.0.0.1` and `[::1]` are one server, so are a Neon
//! endpoint and its `-pooler` door, and host names are case-insensitive. The
//! port is ignored on purpose — two spellings that differ only there are
//! refused, which is the side to err on. Where the provider shares one cluster
//! across databases (`LocalProvider`) every branch sits beside production on
//! the same server, so there the database name alone decides, whatever the
//! host says.

use crate::entity::branches as oltp_branches;
use crate::entity::tenants as oltp_tenants;
use crate::host::server;
use crate::schema;

/// Whether `row` points a staging caller at `tenant`'s production database.
pub(super) fn names_production(tenant: &oltp_tenants::Model, row: &oltp_branches::Model) -> bool {
    same_database(
        schema::shares_role_namespace(&tenant.provider),
        Target {
            branch_id: &tenant.branch_id,
            host: &tenant.host,
            database: &tenant.database_name,
        },
        Target {
            branch_id: &row.provider_branch_id,
            host: &row.host,
            database: &row.database_name,
        },
    )
}

/// Where a connection lands, as a row records it.
struct Target<'a> {
    branch_id: &'a str,
    host: &'a str,
    database: &'a str,
}

fn same_database(shared_cluster: bool, production: Target<'_>, branch: Target<'_>) -> bool {
    if branch.branch_id == production.branch_id {
        return true;
    }
    if branch.database != production.database {
        return false;
    }
    shared_cluster || server(branch.host) == server(production.host)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NEON: &str = "ep-cool-sky-123.us-east-2.aws.neon.tech";
    const DB: &str = "oxy_org_0b1c";

    fn at<'a>(branch_id: &'a str, host: &'a str, database: &'a str) -> Target<'a> {
        Target {
            branch_id,
            host,
            database,
        }
    }

    /// Neon: a branch has the production database's name on its own endpoint —
    /// the normal case — but production's endpoint, however spelled, is refused.
    #[test]
    fn neon_refuses_production_by_id_or_by_endpoint_in_any_spelling() {
        let prod = || at("br-main", NEON, DB);
        let own_endpoint = "ep-warm-sea-456.us-east-2.aws.neon.tech";
        assert!(!same_database(
            false,
            prod(),
            at("br-stg", own_endpoint, DB)
        ));
        assert!(same_database(
            false,
            prod(),
            at("br-main", own_endpoint, DB)
        ));
        for spelling in [
            NEON,
            "EP-COOL-SKY-123.us-east-2.aws.neon.tech",
            "ep-cool-sky-123-pooler.us-east-2.aws.neon.tech",
            "ep-cool-sky-123.us-east-2.aws.neon.tech:5432",
        ] {
            assert!(
                same_database(false, prod(), at("br-stg", spelling, DB)),
                "{spelling:?}"
            );
        }
    }

    /// A shared cluster: every branch is a database beside production's, so
    /// production's database NAME is refused whatever host the row carries.
    #[test]
    fn a_shared_cluster_refuses_the_production_database_on_any_host() {
        let prod = || at("local", "localhost:15432", DB);
        let branch_db = "oxy_org_0b1c_5ad1e2f3";
        assert!(!same_database(
            true,
            prod(),
            at(branch_db, "localhost:15432", branch_db)
        ));
        for host in ["localhost:15432", "127.0.0.1", "postgres.internal", ""] {
            assert!(
                same_database(true, prod(), at(branch_db, host, DB)),
                "{host:?}"
            );
        }
    }
}
