//! Previews P4b: with the org's OLTP staging branch, a staging function's
//! `ctx.oltp` and `ctx.oltp.tx` read **and write** the branch — a copy of
//! production's database (P4a) — and never production.
//!
//! The branch is real: `LocalProvider` copies the tenant's database into a
//! sibling on the test cluster (`CREATE DATABASE … TEMPLATE`), as the P4a
//! platform test does, and every assertion counts rows in the two databases.
//!
//! - **the write probe on the branch**: one step per OLTP write op, from a
//!   table checked against the policy — each lands on the branch, production
//!   keeps its one row, and nothing is held. What production's read-only hold
//!   refuses (DDL, a savepoint, an app-defined function that writes)
//!   runs too, and a read after the writes sees them;
//! - **outside the database**: `dblink`, `lo_import`, `pg_read_file`,
//!   `COPY … TO PROGRAM` and `ALTER ROLE` are refused even on the branch, and
//!   listed in the invocation's held row;
//! - **SQL decided only when it runs** — `DO` with `EXECUTE` of a concatenated
//!   or `format`ted `ALTER ROLE`, `CALL`, `CREATE FUNCTION` — is held. On this
//!   local branch the writer is production's role, so the final check that
//!   production's writer still logs in fails if a `DO` is sent;
//! - **the differential**: production's and staging's `ctx.oltp` resolve
//!   disjoint databases, through the resolver the host uses;
//! - **a branch row that names production** is refused by the branch
//!   resolver's guard, so the call fails and production is untouched; a
//!   branch re-cut under another id since admission is refused too.
//!
//! Without a branch the Phase 3 hold stands: `staging_functions_oltp` and the
//! `staging_write_probe` run against an org that has none.

use std::collections::BTreeSet;

use agentic_connector::PostgresConnector;
use oxy_app::server::api::custom_apps_functions::env_policy::{
    Decision, EnvPolicy, HostOp, OltpHome, Target,
};
use oxy_app::server::api::custom_apps_functions::host::writer_connection;
use oxy_app_core::custom_app_environment::AppEnvironment;
use sea_orm::{ActiveModelTrait, ActiveValue, ColumnTrait, EntityTrait, QueryFilter};

use crate::custom_app_functions_fixture::FunctionSpec;
use crate::staging_functions::{data, held_ops, held_rows};
use crate::staging_functions_oltp::{OltpApp, count_orders, oltp_manifest, run_then_cleanup};

/// One step per OLTP write op that staging sends to the branch, each writing
/// ids of its own; then what the read-only hold refuses. `ctx.oltp.tx` steps
/// serve `tx.begin_oltp` and `tx.commit` too.
const BRANCH_STEPS: &[(&str, &str)] = &[
    (
        "oltp.exec",
        r#"ctx.oltp.exec("insert into orders (id) values (2)")"#,
    ),
    (
        "oltp.query",
        r#"ctx.oltp.query("with x as (insert into orders (id) values (3) returning id) select id from x")"#,
    ),
    (
        "tx.begin_oltp",
        r#"ctx.oltp.tx((tx) => tx.exec("insert into orders (id) values (4)"))"#,
    ),
    (
        "tx.query",
        r#"ctx.oltp.tx((tx) => tx.query("insert into orders (id) values (5) returning id"))"#,
    ),
    (
        "tx.exec",
        r#"ctx.oltp.tx(async (tx) => { await tx.exec("insert into orders (id) values (6)"); return tx.exec("insert into orders (id) values (7)"); })"#,
    ),
    (
        "tx.commit",
        r#"ctx.oltp.tx((tx) => tx.exec("insert into orders (id) values (8)"))"#,
    ),
    (
        "ddl",
        r#"ctx.oltp.exec("create table notes (id int primary key)")"#,
    ),
    (
        "savepoint",
        r#"ctx.oltp.tx(async (tx) => { await tx.exec("SAVEPOINT s"); await tx.exec("insert into orders (id) values (9)"); await tx.exec("RELEASE SAVEPOINT s"); return tx.exec("insert into orders (id) values (10)"); })"#,
    ),
    ("bump", r#"ctx.oltp.query("select bump() as id")"#),
    (
        "readAfterWrite",
        r#"(await ctx.oltp.query("select count(*)::int as n from orders"))[0].n"#,
    ),
];

/// The ids the steps write, with production's own row (1) and `bump()`'s.
const BRANCH_ROWS: i64 = 11;

/// Each reaches outside the database it runs in.
const OUTSIDE_STEPS: &[(&str, &str)] = &[
    (
        "dblink",
        r#"ctx.oltp.query("select * from dblink('host=localhost', 'select 1') as t(a int)")"#,
    ),
    (
        "loImport",
        r#"ctx.oltp.query("select lo_import('/etc/passwd')")"#,
    ),
    (
        "readFile",
        r#"ctx.oltp.query("select pg_read_file('postgresql.conf')")"#,
    ),
    (
        "copyProgram",
        r#"ctx.oltp.exec("COPY orders TO PROGRAM 'cat'")"#,
    ),
    (
        "alterRole",
        r#"ctx.oltp.exec("ALTER ROLE CURRENT_USER PASSWORD 'staging-owned'")"#,
    ),
    (
        "txDblink",
        r#"ctx.oltp.tx((tx) => tx.query("select dblink_exec('host=x', 'delete from orders')"))"#,
    ),
];

/// Each runs SQL decided only when it runs, which nothing can check: held on
/// the branch. The first two lock production's writer out on a local branch
/// if sent — the role is production's.
const HELD_STEPS: &[(&str, &str)] = &[
    (
        "doConcat",
        r#"ctx.oltp.exec("DO $$BEGIN EXECUTE 'ALTER' || ' ROLE CURRENT_USER PASSWORD ''staging-owned'''; END$$")"#,
    ),
    (
        "doFormat",
        r#"ctx.oltp.exec("DO $$BEGIN EXECUTE format('%s ROLE CURRENT_USER PASSWORD %L', 'ALTER', 'staging-owned'); END$$")"#,
    ),
    ("call", r#"ctx.oltp.exec("CALL write_things()")"#),
    (
        "createFunction",
        r#"ctx.oltp.exec("create function staging_fn() returns int language sql as $$ select 1 $$")"#,
    ),
];

/// A function running `steps`, each result or error kept under its key.
fn steps_js(steps: &[(&str, &str)]) -> &'static str {
    let body: String = steps
        .iter()
        .map(|(key, call)| format!("  await attempt({key:?}, async () => {call});\n"))
        .collect();
    let js = format!(
        "export default async (req, ctx) => {{\n  const out = {{ channel: ctx.channel }};\n  \
         const attempt = async (key, f) => {{\n    \
         try {{ out[key] = {{ ok: await f() }}; }} \
         catch (e) {{ out[key] = {{ error: String(e && e.message ? e.message : e) }}; }}\n  }};\n\
         {body}  return Response.json(out);\n}};\n"
    );
    Box::leak(js.into_boxed_str())
}

fn functions() -> Vec<FunctionSpec> {
    vec![
        FunctionSpec {
            name: "probe",
            manifest: oltp_manifest(),
            js: steps_js(BRANCH_STEPS),
        },
        FunctionSpec {
            name: "outside",
            manifest: oltp_manifest(),
            js: steps_js(OUTSIDE_STEPS),
        },
        FunctionSpec {
            name: "unread",
            manifest: oltp_manifest(),
            js: steps_js(HELD_STEPS),
        },
    ]
}

fn writer_name(app: &OltpApp) -> String {
    oxy_oltp::schema::app_writer_name(&app.slug).expect("the slug backs a schema")
}

/// A connector on `home`, resolved exactly as the host resolves it.
async fn connector_on(app: &OltpApp, home: &OltpHome) -> PostgresConnector {
    let conn = writer_connection(&app.t.db, app.t.org_id, &writer_name(app), home)
        .await
        .expect("resolve the writer");
    PostgresConnector::from_dsn(&conn.dsn, conn.verify_tls).expect("connector")
}

/// The OLTP ops held-row entries name (`oltp.*`, `tx.*`).
async fn held_oltp_ops(app: &OltpApp) -> BTreeSet<String> {
    let mut ops = BTreeSet::new();
    for row in held_rows(&app.t).await {
        ops.extend(
            held_ops(&row)
                .into_iter()
                .filter(|op| op.starts_with("oltp.") || op.starts_with("tx.")),
        );
    }
    ops
}

/// The verbs held-row entries name.
async fn held_verbs(app: &OltpApp) -> BTreeSet<String> {
    let mut verbs = BTreeSet::new();
    for row in held_rows(&app.t).await {
        let writes = row.metadata["writes"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        verbs.extend(
            writes
                .iter()
                .filter_map(|w| w["verb"].as_str().map(str::to_string)),
        );
    }
    verbs
}

/// The table covers every op a staging run with a branch sends there — by
/// the op, or on a handle opened on it. `tx.rollback` writes nothing.
#[test]
fn every_op_isolated_to_the_oltp_branch_has_a_branch_probe_step() {
    let branch = EnvPolicy::for_environment(AppEnvironment::Staging)
        .with_oltp_home(OltpHome::StagingBranch("br".into()));
    let on_branch = Decision::Isolate(Target::OltpBranch);
    let probed: BTreeSet<&str> = BRANCH_STEPS.iter().map(|(op, _)| *op).collect();
    let unprobed: Vec<&str> = HostOp::ALL
        .iter()
        .copied()
        .filter(|op| *op != HostOp::TxRollback)
        .filter(|op| {
            branch.decide(*op) == on_branch
                || branch.decide_on_handle(*op, Some(Target::OltpBranch)) == on_branch
        })
        .map(HostOp::name)
        .filter(|name| !probed.contains(name))
        .collect();
    assert!(
        unprobed.is_empty(),
        "ops isolated to the OLTP branch with no step in BRANCH_STEPS: {unprobed:?}"
    );
}

#[tokio::test]
async fn oltp_branch_write_probe_writes_the_branch_and_refuses_what_reaches_outside_it() {
    let app = OltpApp::provision(&functions()).await;
    run_then_cleanup(&app, async {
        let row = app.store.provision_branch().await;
        let branch = connector_on(&app, &OltpHome::StagingBranch(row.provider_branch_id)).await;

        let staged = app.staged("probe").await;
        let got = data(&staged);
        assert_eq!(got["channel"], "staging");
        let failed: Vec<String> = BRANCH_STEPS
            .iter()
            .filter(|(key, _)| got[*key].get("error").is_some())
            .map(|(key, _)| format!("{key}: {}", got[*key]))
            .collect();
        assert!(failed.is_empty(), "steps that did not run: {failed:#?}");
        assert_eq!(got["readAfterWrite"]["ok"], BRANCH_ROWS, "{}", staged.raw);
        assert_eq!(
            count_orders(&branch).await,
            BRANCH_ROWS,
            "every write is on the branch"
        );
        assert_eq!(
            app.order_count().await,
            1,
            "no staging write reached production"
        );
        assert!(
            held_oltp_ops(&app).await.is_empty(),
            "nothing on the branch is held"
        );

        let outside = app.staged("outside").await;
        let got = data(&outside);
        for (key, _) in OUTSIDE_STEPS {
            let error = got[*key]["error"].as_str().unwrap_or_default();
            assert!(
                error.contains("EnvironmentRefused: ctx.") && error.contains("This statement"),
                "{key} must be refused on the branch, got {}",
                got[*key]
            );
        }
        assert_eq!(
            held_oltp_ops(&app).await,
            BTreeSet::from(["oltp.exec".into(), "oltp.query".into(), "tx.query".into()]),
            "each refusal is listed in the held row"
        );

        // SQL decided only when it runs is held; on this local branch the
        // writer is production's role, so a DO that ran would lock it out.
        let unread = app.staged("unread").await;
        let got = data(&unread);
        for (key, _) in HELD_STEPS {
            let error = got[*key]["error"].as_str().unwrap_or_default();
            assert!(
                error.contains("HeldInStaging: ctx.oltp.exec"),
                "{key} must be held on the branch, got {}",
                got[*key]
            );
        }
        let verbs = held_verbs(&app).await;
        for verb in ["DO", "CALL", "CREATE FUNCTION"] {
            assert!(verbs.contains(verb), "the held row lists {verb}: {verbs:?}");
        }
        assert_eq!(count_orders(&branch).await, BRANCH_ROWS);
        assert_eq!(
            app.order_count().await,
            1,
            "production's writer still logs in: its password was not changed from staging"
        );
    })
    .await;
}

#[tokio::test]
async fn oltp_differential_staging_resolves_the_branch_and_a_row_naming_production_is_refused() {
    let app = OltpApp::provision(&functions()).await;
    run_then_cleanup(&app, async {
        let row = app.store.provision_branch().await;
        let home = OltpHome::StagingBranch(row.provider_branch_id.clone());
        let dbname = |dsn: &str| {
            let config: tokio_postgres::Config = dsn.parse().expect("a DSN");
            config.get_dbname().map(str::to_string)
        };
        let name = writer_name(&app);
        let (t, org) = (&app.t, app.t.org_id);
        let production = writer_connection(&t.db, org, &name, &OltpHome::Production)
            .await
            .expect("production");
        let staging = writer_connection(&t.db, org, &name, &home)
            .await
            .expect("staging");
        assert_eq!(dbname(&staging.dsn), Some(row.database_name.clone()));
        assert_ne!(
            dbname(&staging.dsn),
            dbname(&production.dsn),
            "staging's ctx.oltp must reach a database production's does not"
        );

        // A row poisoned to name production's database: the resolver's guard
        // refuses it, and the call writes nothing anywhere.
        let tenant_db = dbname(&production.dsn).expect("production's database");
        set_branch_database(&app, &tenant_db).await;
        let staged = app.staged("probe").await;
        set_branch_database(&app, &row.database_name).await;
        let got = data(&staged);
        let error = got["oltp.exec"]["error"].as_str().unwrap_or_default();
        assert!(
            error.contains("recorded as production's own database"),
            "the guard refuses the row: {}",
            got["oltp.exec"]
        );
        assert_eq!(app.order_count().await, 1, "production is untouched");

        // A branch re-cut under another id since the admission chose it is
        // refused, not followed (review fix round 1).
        let stale = OltpHome::StagingBranch("br-cut-before".into());
        let err = writer_connection(&t.db, org, &name, &stale)
            .await
            .expect_err("a re-cut branch is not the admitted one");
        assert!(
            err.contains("re-cut since this invocation was admitted"),
            "{err}"
        );
    })
    .await;
}

/// Point the org's staging branch row at `database`.
async fn set_branch_database(app: &OltpApp, database: &str) {
    use oxy_oltp::entity::{branches, tenants};
    let tenant = tenants::Entity::find()
        .filter(tenants::Column::OrgId.eq(app.t.org_id))
        .one(&app.t.db)
        .await
        .expect("query tenant")
        .expect("the org's tenant");
    let row = branches::Entity::find()
        .filter(branches::Column::TenantRowId.eq(tenant.id))
        .one(&app.t.db)
        .await
        .expect("query branch")
        .expect("the org's branch");
    let mut active: branches::ActiveModel = row.into();
    active.database_name = ActiveValue::Set(database.to_string());
    active.update(&app.t.db).await.expect("repoint the branch");
}
