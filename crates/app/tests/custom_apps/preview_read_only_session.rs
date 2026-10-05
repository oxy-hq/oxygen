//! A holding connector over Postgres runs its session read-only
//! (`previews::hold::pg`), so a write the classifier cannot see — inside a
//! function a `SELECT` calls — is refused by the database, and `set_config`,
//! the way back, is held.
//!
//! The connector is the one a staging ask and a workspace preview both get;
//! it is built here directly over a real Postgres (`common::fresh_db`).

use std::sync::{Arc, Mutex};

use agentic_connector::{DatabaseConnector, PostgresConnector};
use agentic_core::result::CellValue;
use async_trait::async_trait;
use oxy_app::server::previews::hold::HoldingConnector;
use oxy_app::server::previews::request_hold::{HeldSink, HeldStatement, HoldScope};
use oxy_app::server::previews::sql_kind::StatementKind;
use uuid::Uuid;

use crate::common::{Schema, fresh_db};

#[derive(Default)]
struct Verbs(Mutex<Vec<String>>);

#[async_trait]
impl HeldSink for Verbs {
    async fn held(&self, statement: HeldStatement<'_>) {
        let verb = match statement.kind {
            StatementKind::Write { verb, .. } => verb.clone(),
            other => format!("{other:?}"),
        };
        self.0.lock().unwrap().push(verb);
    }
}

async fn count(c: &dyn DatabaseConnector) -> f64 {
    let r = c
        .execute_query("SELECT count(*) AS n FROM smuggled", 1)
        .await
        .expect("count");
    match &r.result.rows[0].0[0] {
        CellValue::Number(n) => *n,
        other => panic!("unexpected count {other:?}"),
    }
}

#[tokio::test]
async fn preview_read_only_session_refuses_a_write_smuggled_in_a_function() {
    let (_db, url) = fresh_db(Schema::Central).await;
    let owner = PostgresConnector::from_dsn(&url, false).expect("owner connector");
    owner
        .execute_statement("CREATE TABLE smuggled (a int)")
        .await
        .expect("create the target");
    owner
        .execute_statement(
            "CREATE FUNCTION smuggle() RETURNS int LANGUAGE plpgsql AS \
             $$ BEGIN INSERT INTO smuggled VALUES (1); RETURN 1; END $$",
        )
        .await
        .expect("create the function");

    let verbs = Arc::new(Verbs::default());
    let inner = PostgresConnector::from_dsn(&url, false).expect("held connector");
    let held = HoldingConnector::under(
        Arc::new(inner),
        "pg",
        &HoldScope::staging(Uuid::new_v4(), verbs.clone()),
    );

    // A plain read still returns rows — through the sampler that needs no
    // temp table, which a read-only session would refuse.
    let rows = held
        .execute_query(
            "SELECT g AS n, 'x' || g AS label FROM generate_series(1, 3) g",
            2,
        )
        .await
        .expect("a read runs");
    assert_eq!(rows.result.columns, vec!["n", "label"]);
    assert_eq!(rows.result.rows.len(), 2);
    assert_eq!(rows.result.total_row_count, 3);
    assert!(matches!(rows.result.rows[1].0[0], CellValue::Number(n) if n == 2.0));
    assert!(matches!(&rows.result.rows[1].0[1], CellValue::Text(s) if s == "x2"));
    assert_eq!(count(&held).await, 0.0);

    let e = held
        .execute_query("SELECT smuggle()", 10)
        .await
        .expect_err("the session refuses the function's INSERT");
    assert!(e.to_string().contains("read-only"), "{e}");

    let e = held
        .execute_statement("SELECT set_config('default_transaction_read_only', 'off', false)")
        .await
        .expect_err("set_config is held");
    assert!(e.to_string().contains("held"), "{e}");

    assert!(
        held.execute_statement("SELECT smuggle()").await.is_err(),
        "still read-only after the attempt to switch it off"
    );
    assert_eq!(count(&owner).await, 0.0, "nothing was written");
    assert_eq!(*verbs.0.lock().unwrap(), vec!["SET_CONFIG".to_string()]);
}

/// A function the classifier cannot see into switches the session back
/// (`set_config` from its body): the connector re-asserts the read-only `SET`
/// before the next statement, so a write after it is still refused.
#[tokio::test]
async fn preview_read_only_session_survives_a_function_that_switches_it_off() {
    let (_db, url) = fresh_db(Schema::Central).await;
    let owner = PostgresConnector::from_dsn(&url, false).expect("owner connector");
    for ddl in [
        "CREATE TABLE smuggled (a int)",
        "CREATE FUNCTION smuggle() RETURNS int LANGUAGE plpgsql AS \
         $$ BEGIN INSERT INTO smuggled VALUES (1); RETURN 1; END $$",
        "CREATE FUNCTION flip() RETURNS text LANGUAGE plpgsql AS \
         $$ BEGIN RETURN set_config('default_transaction_read_only', 'off', false); END $$",
    ] {
        owner.execute_statement(ddl).await.expect(ddl);
    }
    let inner = PostgresConnector::from_dsn(&url, false).expect("held connector");
    let held = HoldingConnector::new(Arc::new(inner), "pg");
    held.execute_query("SELECT flip()", 10)
        .await
        .expect("a read, to the classifier and to Postgres");
    held.execute_query("SELECT smuggle()", 10)
        .await
        .expect_err("the session is read-only again");
    assert_eq!(count(&owner).await, 0.0, "nothing was written");
}
