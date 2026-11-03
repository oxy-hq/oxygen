//! A schema-only branch on `LocalProvider`, against a real Postgres.
//!
//! The truncation is a `DO` block over `pg_tables` run on the branch's own
//! database, which no unit test can exercise: what matters is that the tables
//! are still there, empty, and that production kept its rows.

use crate::Fixture;

use oxy_oltp::provider::{BranchInit, BranchRequest, OltpProvider, branch_database_name};

/// One integer from `sql` run in `database`, as the cluster superuser — the
/// fixture's DSNs name the tenant's database only.
async fn scalar(database: &str, sql: &str) -> i64 {
    let mut dsn = crate::admin_dsn();
    let at = dsn.rfind('/').expect("a DSN with a database");
    dsn.replace_range(at + 1.., database);
    let client = oxy_oltp::connect::connect(&dsn, "branch test")
        .await
        .expect("connect to the database");
    client
        .query_one(sql, &[])
        .await
        .unwrap_or_else(|e| panic!("{sql} in {database}: {e}"))
        .get(0)
}

async fn count(database: &str, table: &str) -> i64 {
    scalar(database, &format!("SELECT count(*) FROM {table}")).await
}

fn sandbox(fx: &Fixture, init: BranchInit) -> BranchRequest {
    let mut req = BranchRequest::staging(
        fx.database.as_str(),
        "local",
        "oxy-sandbox",
        fx.database.as_str(),
        fx.owner_role.as_str(),
    );
    req.init = init;
    req
}

const SEED: &str = "CREATE TABLE public.orders (id serial PRIMARY KEY, note text); \
     CREATE TABLE public.lines (order_id int REFERENCES public.orders (id)); \
     CREATE SCHEMA app_x; \
     CREATE TABLE app_x.events (id int); \
     INSERT INTO public.orders (note) VALUES ('a'), ('b'), ('c'); \
     INSERT INTO public.lines VALUES (1), (2); \
     INSERT INTO app_x.events VALUES (1), (2), (3), (4); \
     INSERT INTO oxy_meta.schema_migrations (file_path, checksum) VALUES ('m1.sql', 'abc')";

#[tokio::test]
async fn a_schema_only_copy_has_the_tables_and_no_rows() {
    let Some(fx) = Fixture::create("schemaonly").await else {
        return;
    };
    fx.as_owner(SEED).await;

    let req = sandbox(&fx, BranchInit::SchemaOnly);
    let branch = fx.provider.create_branch(&req).await;
    let database = branch_database_name(&fx.database, &req.name);

    // Read before asserting, so a failure still drops the sibling database.
    let seen = match &branch {
        Ok(b) => {
            let d = b.database.name.as_str();
            Some((
                count(d, "public.orders").await,
                count(d, "public.lines").await,
                count(d, "app_x.events").await,
                scalar(
                    d,
                    "SELECT nextval(pg_get_serial_sequence('public.orders', 'id'))",
                )
                .await,
                count(d, "oxy_meta.schema_migrations").await,
                count(&fx.database, "public.orders").await,
                count(&fx.database, "oxy_meta.schema_migrations").await,
            ))
        }
        Err(_) => None,
    };
    let _ = fx.provider.delete_branch(&req, &database).await;
    fx.cleanup(&[]).await;

    branch.expect("create a schema-only branch");
    let (orders, lines, events, next_id, ledger, prod_orders, prod_ledger) =
        seen.expect("read the branch");
    assert_eq!(orders, 0, "the table exists on the branch, with no rows");
    assert_eq!(lines, 0, "a referencing table is emptied too");
    assert_eq!(events, 0, "so is a table in a second schema");
    assert_eq!(next_id, 1, "sequences start fresh");
    assert_eq!(prod_ledger, 1, "the seed wrote one ledger row");
    assert_eq!(ledger, prod_ledger, "the migration ledger is not org data");
    assert_eq!(prod_orders, 3, "production keeps its rows");
}

/// A populated copy left under the derived name — an earlier attempt that died
/// between the copy and the emptying — must not be adopted as it is.
#[tokio::test]
async fn adopting_a_populated_copy_as_schema_only_empties_it() {
    let Some(fx) = Fixture::create("schemaonlyadopt").await else {
        return;
    };
    fx.as_owner(SEED).await;

    let data = sandbox(&fx, BranchInit::ParentData);
    let populated = fx.provider.create_branch(&data).await;
    let database = branch_database_name(&fx.database, &data.name);
    let before = match &populated {
        Ok(b) => Some(count(&b.database.name, "public.orders").await),
        Err(_) => None,
    };

    let schema_only = sandbox(&fx, BranchInit::SchemaOnly);
    let adopted = fx.provider.create_branch(&schema_only).await;
    let after = match &adopted {
        Ok(b) => Some(count(&b.database.name, "app_x.events").await),
        Err(_) => None,
    };
    let _ = fx.provider.delete_branch(&data, &database).await;
    fx.cleanup(&[]).await;

    populated.expect("create the populated copy");
    adopted.expect("adopt it as schema-only");
    assert_eq!(before, Some(3), "the copy started populated");
    assert_eq!(after, Some(0), "adoption emptied it");
}
