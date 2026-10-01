//! The staging branch on `LocalProvider`: a real database copied on the test
//! cluster, reached through the real resolver.
//!
//! What only a real Postgres can show: that the copy carries production's rows,
//! that a staging write stays on staging, that `CREATE DATABASE … TEMPLATE`'s
//! lost ACL and settings came back, that a reset really re-copies, and that
//! deprovisioning drops the branch database rather than stranding it.

use oxy_oltp::resolver::resolve_branch_writer_connection_for_org;
use oxy_oltp::{GrantLevel, OltpBranch};

use crate::oltp_provisioner::with_fx;

const STAGING: OltpBranch = OltpBranch::Staging;

async fn exec(dsn: &str, sql: &str) {
    let client = oxy_oltp::connect::connect(dsn, "branch test")
        .await
        .unwrap_or_else(|e| panic!("connect for {sql}: {e}"));
    client
        .batch_execute(sql)
        .await
        .unwrap_or_else(|e| panic!("{sql}: {e:?}"));
}

async fn notes(dsn: &str) -> Vec<String> {
    let client = oxy_oltp::connect::connect(dsn, "branch test")
        .await
        .expect("connect");
    client
        .query("SELECT note FROM orders ORDER BY id", &[])
        .await
        .expect("read orders")
        .iter()
        .map(|r| r.get(0))
        .collect()
}

/// One text column from a shared catalog, read on the admin database so no
/// session is left open on the tenant's (which `TEMPLATE` would trip over).
async fn catalog(sql: &str, database: &str) -> Vec<String> {
    let admin = crate::common::admin_url().await;
    let client = oxy_oltp::connect::connect(&admin, "branch test catalog")
        .await
        .expect("connect admin");
    client
        .query(sql, &[&database])
        .await
        .unwrap_or_else(|e| panic!("{sql}: {e}"))
        .iter()
        .map(|r| r.get(0))
        .collect()
}

/// Who holds what on the database, owner excepted — PUBLIC shows as `-`.
const DATABASE_GRANTS: &str = "SELECT CASE WHEN e.grantee = 0 THEN '-' \
            ELSE pg_get_userbyid(e.grantee)::text END || ':' || e.privilege_type \
     FROM pg_database d, aclexplode(d.datacl) e \
     WHERE d.datname = $1 AND e.grantee <> d.datdba ORDER BY 1";

const DATABASE_SETTINGS: &str = "SELECT array_to_string(s.setconfig, ';') \
     FROM pg_db_role_setting s JOIN pg_database d ON d.oid = s.setdatabase \
     WHERE d.datname = $1 AND s.setrole = 0";

const DATABASE_EXISTS: &str = "SELECT datname::text FROM pg_database WHERE datname = $1";

#[tokio::test]
async fn a_local_branch_is_a_copy_that_diverges_resets_and_goes_with_the_org() {
    with_fx(|fx| async move {
        fx.provisioner
            .provision(fx.org_id)
            .await
            .expect("provision");
        let writer = fx.writer("brstg");
        let production = fx
            .ensure_writer(&writer, GrantLevel::ReadWrite)
            .await
            .expect("writer")
            .dsn;
        exec(
            &production,
            "CREATE TABLE orders (id int PRIMARY KEY, note text); \
             INSERT INTO orders VALUES (1, 'production')",
        )
        .await;

        let row = fx
            .provisioner
            .provision_branch(fx.org_id, STAGING)
            .await
            .expect("branch");
        let tenant = fx.tenant_row().await.expect("tenant");
        assert_ne!(row.database_name, tenant.database_name);
        assert!(
            row.owner_password_ciphertext.is_none(),
            "a local branch shares the tenant's cluster-global roles"
        );

        let staging = resolve_branch_writer_connection_for_org(&fx.db, fx.org_id, STAGING, &writer)
            .await
            .expect("resolve")
            .expect("the org has a branch")
            .dsn;
        assert!(
            staging.contains(&format!("/{}?", row.database_name)),
            "{staging}"
        );
        assert_eq!(
            notes(&staging).await,
            ["production"],
            "a copy of production"
        );

        exec(&staging, "INSERT INTO orders VALUES (2, 'staging')").await;
        assert_eq!(
            notes(&production).await,
            ["production"],
            "a staging write must never reach production"
        );

        // What TEMPLATE drops, replayed: no PUBLIC connect, the writer's
        // grant, and the database-default search_path.
        assert_eq!(
            catalog(DATABASE_GRANTS, &row.database_name).await,
            catalog(DATABASE_GRANTS, &tenant.database_name).await,
        );
        assert!(
            !catalog(DATABASE_GRANTS, &row.database_name)
                .await
                .iter()
                .any(|g| g.starts_with("-:")),
            "PUBLIC must not reach the branch database"
        );
        assert_eq!(
            catalog(DATABASE_SETTINGS, &row.database_name).await,
            catalog(DATABASE_SETTINGS, &tenant.database_name).await,
        );

        exec(&production, "INSERT INTO orders VALUES (3, 'later')").await;
        let reset = fx
            .provisioner
            .reset_branch(fx.org_id, STAGING)
            .await
            .expect("reset");
        assert_eq!(
            reset.database_name, row.database_name,
            "same name, fresh copy"
        );
        assert_eq!(
            notes(&staging).await,
            ["production", "later"],
            "staging's write is gone and production's newer row arrived"
        );

        // Releasing the app takes its staging copy too — and on a shared
        // cluster it could not otherwise drop the role at all: the branch
        // database's grants are dependencies of it.
        fx.provisioner
            .deprovision_writer(fx.org_id, &writer)
            .await
            .expect("an app's release must work with a branch in place");
        assert!(
            !schema_exists(&row.database_name, &writer.schema_name()).await,
            "the app's staging schema goes with the app"
        );

        fx.provisioner
            .deprovision(fx.org_id)
            .await
            .expect("deprovision");
        assert!(
            catalog(DATABASE_EXISTS, &row.database_name)
                .await
                .is_empty(),
            "the branch database goes with the org"
        );
    })
    .await;
}

/// `pg_namespace` is per database, so this connects to `database` itself as
/// the cluster superuser, and hangs up before returning.
async fn schema_exists(database: &str, schema: &str) -> bool {
    let admin = crate::common::admin_url().await;
    let (base, query) = match admin.split_once('?') {
        Some((b, q)) => (b, format!("?{q}")),
        None => (admin.as_str(), String::new()),
    };
    let cut = base.rfind('/').expect("admin DSN has a /dbname path");
    let dsn = format!("{}/{database}{query}", &base[..cut]);
    let client = oxy_oltp::connect::connect(&dsn, "branch test schema probe")
        .await
        .expect("connect branch database");
    let n: i64 = client
        .query_one(
            "SELECT count(*) FROM pg_namespace WHERE nspname = $1",
            &[&schema],
        )
        .await
        .expect("probe")
        .get(0);
    n > 0
}

/// On a local cluster a branch is its own database: nothing else will drop it,
/// so deprovision is strict — and a row naming the tenant's own database is
/// refused before any `DROP DATABASE`, leaving production in place.
#[tokio::test]
async fn a_poisoned_local_branch_row_stops_deprovision_before_any_drop() {
    with_fx(|fx| async move {
        fx.provisioner
            .provision(fx.org_id)
            .await
            .expect("provision");
        let row = fx
            .provisioner
            .provision_branch(fx.org_id, STAGING)
            .await
            .expect("branch");
        let tenant = fx.tenant_row().await.expect("tenant");
        let mut poisoned: oxy_oltp::entity::branches::ActiveModel = row.into();
        poisoned.provider_branch_id = sea_orm::ActiveValue::Set(tenant.database_name.clone());
        sea_orm::ActiveModelTrait::update(poisoned, &fx.db)
            .await
            .expect("poison");

        let err = fx.provisioner.deprovision(fx.org_id).await.unwrap_err();

        assert!(
            matches!(err, oxy_oltp::ProvisionerError::BranchIsProduction { .. }),
            "{err}"
        );
        assert_eq!(
            catalog(DATABASE_EXISTS, &tenant.database_name).await,
            [tenant.database_name.as_str()],
            "production's database must survive"
        );
        // Fx cleanup drops both databases by their derived names.
    })
    .await;
}
