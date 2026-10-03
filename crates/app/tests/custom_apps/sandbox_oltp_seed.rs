//! A sandbox's own schema on the org's OLTP staging branch
//! (`oxy_oltp::sandbox_schema`): created by the branch owner, seeded from
//! staging's `app_<writer>` as the app's writer, dropped alone.
//!
//! The branch is real — `LocalProvider` copies the tenant's database into a
//! sibling on the test cluster — so every assertion reads the catalog or
//! counts rows in the two schemas.
//!
//! - **the seed**: every table's structure, the rows of the ones under the
//!   cap, sequences at staging's position with the copied defaults reading
//!   the copies, and foreign keys pointing at the copies; staging's schema
//!   and production's database are untouched;
//! - **the drop** removes that sandbox's schema and no other;
//! - **a name that is a writer's own schema** is never created over or
//!   dropped;
//! - **no branch**: nothing to create, connect to or drop.

use oxy_oltp::OltpBranch::Staging;
use oxy_oltp::WriterRef;
use oxy_oltp::sandbox_schema::seed::{SeedCaps, seed};
use oxy_oltp::sandbox_schema::{
    Dropped, SandboxSchema, SandboxSchemaError, create_on_branch, drop_on_branch, exists_on_branch,
    resolve_branch_sandbox_writer_for_org,
};
use tokio_postgres::Client;

use crate::staging_functions_oltp::{OltpApp, run_then_cleanup};

/// Staging's schema, as production had it when the branch was cut: a serial
/// key, an identity key with a generated column and a foreign key, a
/// standalone sequence behind a default, and a table over the row cap.
const STAGING_DDL: &[&str] = &[
    "create table customers (id serial primary key, name text not null unique)",
    "insert into customers (name) values ('ana'), ('bo'), ('cy')",
    "create table order_items (\
        id bigint generated always as identity primary key, \
        order_id int not null references orders(id) on delete cascade, \
        qty int not null check (qty > 0), \
        doubled int generated always as (qty * 2) stored)",
    "insert into order_items (order_id, qty) values (1, 2), (1, 5)",
    "create sequence ticket_seq start with 100 increment by 10",
    "create table tickets (\
        code int not null default nextval('ticket_seq'), \
        customer_id int references customers(id))",
    "insert into tickets (customer_id) values (1), (2)",
    "create table audit_log (id int primary key, customer_id int references customers(id))",
    "insert into audit_log select g, 1 from generate_series(1, 20) g",
    "create table notes (audit_id int references audit_log(id), body text)",
    "insert into notes values (3, 'kept')",
    "create type status as enum ('open', 'done')",
    "create function next_code() returns int language sql as $$ select 7 $$",
    "create table labels (id int, state status default 'open', code int default next_code())",
    "insert into labels (id) values (1)",
];

/// Rows: `audit_log`'s 20 are over it, everything else under.
const CAPS: SeedCaps = SeedCaps {
    max_table_rows: 10,
    max_table_bytes: 64 * 1024 * 1024,
    max_total_bytes: 256 * 1024 * 1024,
};

fn schema_of(app: &OltpApp, label: &str) -> SandboxSchema {
    SandboxSchema::for_writer(&app.store.writer, label).expect("a sandbox schema name")
}

/// The app's writer on the branch, `search_path` the sandbox's schema alone.
async fn sandbox_client(app: &OltpApp, schema: &SandboxSchema) -> Client {
    let writer = resolve_branch_sandbox_writer_for_org(
        &app.t.db,
        app.t.org_id,
        Staging,
        &app.store.writer,
        schema,
    )
    .await
    .expect("resolve the sandbox's writer")
    .expect("the org has a branch");
    assert_eq!(writer.connection.schema, schema.name());
    oxy_oltp::connect::connect(&writer.connection.dsn, "sandbox seed test")
        .await
        .expect("connect to the branch")
}

/// The app's writer on the branch, in staging's own schema.
async fn staging_client(app: &OltpApp) -> Client {
    let writer = oxy_oltp::resolver::resolve_branch_writer_for_org(
        &app.t.db,
        app.t.org_id,
        Staging,
        &app.store.writer,
    )
    .await
    .expect("resolve staging's writer")
    .expect("the org has a branch");
    oxy_oltp::connect::connect(&writer.connection.dsn, "sandbox seed test")
        .await
        .expect("connect to the branch")
}

/// The app's writer on production's database.
async fn production_client(app: &OltpApp) -> Client {
    let conn = oxy_oltp::resolver::resolve_writer_connection_for_org(
        &app.t.db,
        app.t.org_id,
        &app.store.writer,
    )
    .await
    .expect("resolve production's writer");
    oxy_oltp::connect::connect(&conn.dsn, "sandbox seed test")
        .await
        .expect("connect to production")
}

async fn count(client: &Client, sql: &str) -> i64 {
    client.query_one(sql, &[]).await.expect(sql).get(0)
}

async fn text(client: &Client, sql: &str) -> String {
    client.query_one(sql, &[]).await.expect(sql).get(0)
}

async fn create_and_seed(app: &OltpApp, label: &str) -> SandboxSchema {
    let schema = schema_of(app, label);
    create_on_branch(&app.t.db, app.t.org_id, Staging, &app.store.writer, &schema)
        .await
        .expect("create the sandbox schema")
        .expect("the org has a branch");
    let mut client = sandbox_client(app, &schema).await;
    seed(&mut client, &schema, &CAPS).await.expect("seed");
    schema
}

async fn provisioned() -> OltpApp {
    let app = OltpApp::provision(&[]).await;
    let production = app.writer().await;
    for sql in STAGING_DDL {
        use agentic_connector::DatabaseConnector as _;
        production
            .execute_statement(sql)
            .await
            .unwrap_or_else(|e| panic!("{sql}: {e}"));
    }
    app
}

#[tokio::test]
async fn a_sandbox_schema_is_a_copy_of_stagings_structure_and_capped_rows() {
    let app = provisioned().await;
    run_then_cleanup(&app, async {
        app.store.provision_branch().await;
        let schema = schema_of(&app, "dev_a1");
        let cut = create_on_branch(&app.t.db, app.t.org_id, Staging, &app.store.writer, &schema)
            .await
            .expect("create")
            .expect("a branch");
        assert!(!cut.provider_branch_id.is_empty());
        let mut sandbox = sandbox_client(&app, &schema).await;
        let report = seed(&mut sandbox, &schema, &CAPS).await.expect("seed");

        assert_eq!(
            report.tables,
            [
                "audit_log",
                "customers",
                "labels",
                "notes",
                "order_items",
                "orders",
                "tickets"
            ]
        );
        assert_eq!(report.structure_only, ["audit_log"], "over the row cap");
        assert_eq!(report.rows_copied, 3 + 1 + 1 + 2 + 1 + 2, "{report:?}");
        // A copied column of staging's type, and a default calling staging's
        // function, still depend on staging's schema: listed, because while
        // the sandbox exists a staging migration cannot drop them.
        let uses = &report.staging_dependencies;
        assert!(
            uses.iter()
                .any(|d| d.contains("type") && d.contains(".status")),
            "{uses:?}"
        );
        assert!(
            uses.iter()
                .any(|d| d.contains("function") && d.contains("next_code")),
            "{uses:?}"
        );
        assert!(
            uses.iter()
                .all(|d| !d.contains("_seq") && !d.contains("orders")),
            "sequences and tables are re-pointed, not shared: {uses:?}"
        );
        assert_eq!(report.sequences, 2, "customers_id_seq and ticket_seq");
        assert_eq!(report.foreign_keys, 4, "{report:?}");
        assert_eq!(
            report.foreign_keys_not_valid,
            ["notes.notes_audit_id_fkey"],
            "notes kept its row; the table it references was left empty"
        );

        // Unqualified names resolve in the sandbox's schema, and hold the copy.
        assert_eq!(count(&sandbox, "select count(*) from customers").await, 3);
        assert_eq!(count(&sandbox, "select count(*) from audit_log").await, 0);
        assert_eq!(
            text(&sandbox, "select current_schema()::text").await,
            schema.name()
        );
        assert_eq!(
            count(
                &sandbox,
                "select doubled::bigint from order_items where qty = 5"
            )
            .await,
            10,
            "a generated column computes itself in the copy"
        );

        // A serial key continues after the copied rows, from the COPY of the
        // sequence: staging's is not advanced.
        let staging = staging_client(&app).await;
        let before = count(&staging, "select last_value from customers_id_seq").await;
        let id: i32 = sandbox
            .query_one(
                "insert into customers (name) values ('dee') returning id",
                &[],
            )
            .await
            .expect("insert in the sandbox")
            .get(0);
        assert_eq!(id, 4);
        assert_eq!(
            count(&staging, "select last_value from customers_id_seq").await,
            before
        );
        assert_eq!(count(&staging, "select count(*) from customers").await, 3);
        // An identity key and a standalone sequence behind a default, likewise.
        let item: i64 = sandbox
            .query_one(
                "insert into order_items (order_id, qty) values (1, 1) returning id",
                &[],
            )
            .await
            .expect("insert an item")
            .get(0);
        assert_eq!(item, 3);
        let code: i32 = sandbox
            .query_one(
                "insert into tickets (customer_id) values (3) returning code",
                &[],
            )
            .await
            .expect("insert a ticket")
            .get(0);
        assert_eq!(code, 120, "100 and 110 were staging's");
        assert_eq!(
            text(
                &sandbox,
                "select n.nspname::text from pg_attrdef ad \
                   join pg_depend d on d.classid = 'pg_attrdef'::regclass and d.objid = ad.oid \
                        and d.refclassid = 'pg_class'::regclass \
                   join pg_class s on s.oid = d.refobjid and s.relkind = 'S' \
                   join pg_namespace n on n.oid = s.relnamespace \
                  where ad.adrelid = 'tickets'::regclass"
            )
            .await,
            schema.name(),
            "the copied default reads the copied sequence"
        );

        // Foreign keys point at the copies, and are enforced there.
        assert_eq!(
            count(
                &sandbox,
                "select count(*) from pg_constraint c join pg_class p on p.oid = c.confrelid \
                  where c.contype = 'f' and c.connamespace = current_schema()::regnamespace \
                    and p.relnamespace <> c.connamespace"
            )
            .await,
            0,
            "no copied foreign key references staging's tables"
        );
        let refused = sandbox
            .execute(
                "insert into order_items (order_id, qty) values (999, 1)",
                &[],
            )
            .await
            .expect_err("no such order in the sandbox");
        assert!(format!("{refused:?}").contains("order_items_order_id_fkey"));
        // Checked for new rows though the table it references was left empty.
        sandbox
            .execute("insert into notes values (77, 'new')", &[])
            .await
            .expect_err("a NOT VALID key still checks new rows");

        // The writer owns the copies, so a migration can alter them.
        sandbox
            .batch_execute("alter table customers add column tier text")
            .await
            .expect("alter a seeded table");
        let missing = staging
            .query_one("select tier from customers limit 1", &[])
            .await
            .expect_err("staging has no such column");
        assert!(format!("{missing:?}").contains("tier"));

        // Production's database holds no sandbox schema, and its rows stand.
        let production = production_client(&app).await;
        assert_eq!(
            count(
                &production,
                "select count(*) from pg_namespace where nspname like '%\\_\\_dev\\_%'"
            )
            .await,
            0
        );
        assert_eq!(
            count(&production, "select count(*) from customers").await,
            3
        );
    })
    .await;
}

#[tokio::test]
async fn dropping_a_sandbox_schema_leaves_staging_and_the_other_sandbox() {
    let app = provisioned().await;
    run_then_cleanup(&app, async {
        app.store.provision_branch().await;
        let a1 = create_and_seed(&app, "dev_a1").await;
        let b2 = create_and_seed(&app, "dev_b2").await;
        let (db, org) = (&app.t.db, app.t.org_id);

        assert_eq!(
            drop_on_branch(db, org, Staging, &a1).await.expect("drop"),
            Dropped::Confirmed
        );
        assert_eq!(
            exists_on_branch(db, org, Staging, &a1).await.expect("read"),
            Some(false)
        );
        assert_eq!(
            exists_on_branch(db, org, Staging, &b2).await.expect("read"),
            Some(true),
            "the other sandbox keeps its schema"
        );
        let other = sandbox_client(&app, &b2).await;
        assert_eq!(count(&other, "select count(*) from customers").await, 3);
        let staging = staging_client(&app).await;
        assert_eq!(count(&staging, "select count(*) from customers").await, 3);
        assert_eq!(
            drop_on_branch(db, org, Staging, &a1).await.expect("again"),
            Dropped::Confirmed,
            "dropping what is already gone is confirmed, not an error"
        );
    })
    .await;
}

/// A legacy slug holding `--` derives a writer whose own schema reads as a
/// sandbox's. That name is the writer's: never created over, never dropped.
#[tokio::test]
async fn a_name_that_is_a_writers_own_schema_is_never_created_or_dropped() {
    let app = provisioned().await;
    run_then_cleanup(&app, async {
        let schema = schema_of(&app, "dev_a1");
        let legacy = WriterRef::app(schema.name().trim_start_matches("app_")).expect("a writer");
        assert_eq!(legacy.schema_name(), schema.name());
        app.store.ensure_extra_writer(&legacy, app.workspace).await;
        app.store.provision_branch().await;
        let (db, org) = (&app.t.db, app.t.org_id);

        let created = create_on_branch(db, org, Staging, &app.store.writer, &schema).await;
        assert!(
            matches!(created, Err(SandboxSchemaError::IsAWriters { .. })),
            "{created:?}"
        );
        let dropped = drop_on_branch(db, org, Staging, &schema).await;
        assert!(
            matches!(dropped, Err(SandboxSchemaError::IsAWriters { .. })),
            "{dropped:?}"
        );
        let resolved =
            resolve_branch_sandbox_writer_for_org(db, org, Staging, &app.store.writer, &schema)
                .await;
        assert!(
            matches!(resolved, Err(SandboxSchemaError::IsAWriters { .. })),
            "a sandbox never connects into another writer's schema"
        );
        assert_eq!(
            exists_on_branch(db, org, Staging, &schema)
                .await
                .expect("read"),
            Some(true),
            "the legacy writer's schema is still there"
        );
    })
    .await;
}

#[tokio::test]
async fn an_org_with_no_branch_has_no_sandbox_schema_to_create_or_drop() {
    let app = provisioned().await;
    run_then_cleanup(&app, async {
        let schema = schema_of(&app, "dev_a1");
        let (db, org) = (&app.t.db, app.t.org_id);
        let created = create_on_branch(db, org, Staging, &app.store.writer, &schema)
            .await
            .expect("no branch is not an error");
        assert!(created.is_none());
        let resolved =
            resolve_branch_sandbox_writer_for_org(db, org, Staging, &app.store.writer, &schema)
                .await
                .expect("no branch is not an error");
        assert!(resolved.is_none());
        assert_eq!(
            drop_on_branch(db, org, Staging, &schema)
                .await
                .expect("drop"),
            Dropped::NoBranch
        );
        assert_eq!(app.order_count().await, 1, "production is untouched");
    })
    .await;
}
