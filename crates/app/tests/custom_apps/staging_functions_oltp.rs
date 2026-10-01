//! Phase 3, the app's own OLTP store: a staging function reads production's
//! rows, and no statement it sends can write them.
//!
//! The org's tenant is provisioned for real (as
//! `custom_app_functions_shape_zoo_oltp` does), so "held" is checked against
//! the rows actually in the app's schema, not against what the function said.
//! Three layers hold a write, and each has a test that fails without it:
//!
//! - **the statement classifier** (`env_policy::admit_oltp_statement`): only
//!   a single read is sent. It is what stops the review's two escapes from a
//!   `READ ONLY` transaction — `COMMIT` then a write that autocommits (A), and
//!   `SET TRANSACTION READ WRITE` before the first snapshot (B) — and a read
//!   calling `set_config` — by name, spelled with Unicode escapes
//!   (`U&"set_confi\0067"`, which the parser reads as one harmless read), or
//!   inside SQL text handed to `query_to_xml`;
//! - **`READ ONLY`** (the transaction, and the session's
//!   `default_transaction_read_only`): what stops a read the classifier cannot
//!   see into — a function the app defined that writes. Not one that calls
//!   `pg_notify` or takes an advisory lock: `READ ONLY` allows both, so such
//!   a function passes all three layers until staging has an OLTP branch of
//!   its own (P4b);
//! - **the held commit** (`tx.commit` rolls back): pinned by the write probe
//!   (`staging_write_probe`), which lists it in the held row.

use agentic_connector::DatabaseConnector as _;
use agentic_connector::PostgresConnector;
use agentic_core::result::CellValue;
use futures::FutureExt as _;
use oxy_app::server::api::custom_apps_functions::env_policy::NO_BRANCH_NOTE;
use oxy_oltp::resolver::resolve_writer_connection_for_org;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::custom_app_functions_fixture::{
    FnCall, FunctionSpec, Tenant, publish_build, seeded_tenant, throwaway_org,
};
use crate::custom_app_functions_shape_zoo::{compile, write_config};
use crate::custom_app_functions_shape_zoo_oltp::AppStore;
use crate::staging_functions::{
    call_on, data, held_ops, held_rows, make_guest_staff, production_host, staging_host,
};

/// An app with a provisioned OLTP store holding `orders` (one row) and
/// `bump()`, a SQL function that inserts a row — a write no parser can see
/// from `SELECT bump()`. A `duck` warehouse is configured for probes that
/// need one.
pub(crate) struct OltpApp {
    pub(crate) t: Tenant,
    pub(crate) slug: String,
    /// The workspace every build of the app is published in.
    pub(crate) workspace: Uuid,
    pub(crate) store: AppStore,
    _root: tempfile::TempDir,
}

impl OltpApp {
    pub(crate) async fn provision(functions: &[FunctionSpec]) -> Self {
        let t = throwaway_org(&seeded_tenant().await).await;
        let slug = format!("stg-oltp-{}", &Uuid::new_v4().simple().to_string()[..8]);
        let root = write_config("  - name: duck\n    type: duckdb\n    path: probe.duckdb\n");
        let workspace = compile(&t, root.path()).await;
        let store = AppStore::new(&t, &slug).await;
        store.provision(workspace).await;
        let app = Self {
            t,
            slug,
            workspace,
            store,
            _root: root,
        };
        for sql in [
            "create table orders (id int primary key)",
            "insert into orders values (1)",
            "create function bump() returns int language sql as \
             $$ insert into orders (id) values (500) returning id $$",
        ] {
            app.writer()
                .await
                .execute_statement(sql)
                .await
                .expect("seed the app's production store");
        }
        publish_build(&app.t, &app.slug, workspace, "oltp-1", true, functions).await;
        make_guest_staff();
        app
    }

    pub(crate) async fn writer(&self) -> PostgresConnector {
        let conn = resolve_writer_connection_for_org(&self.t.db, self.t.org_id, &self.store.writer)
            .await
            .expect("resolve the app's writer");
        PostgresConnector::from_dsn(&conn.dsn, conn.verify_tls).expect("connector")
    }

    pub(crate) async fn order_count(&self) -> i64 {
        count_orders(&self.writer().await).await
    }

    pub(crate) async fn staged(&self, name: &str) -> FnCall {
        call_on(
            &self.t,
            &self.slug,
            name,
            &staging_host(&self.t, &self.slug),
            &[],
        )
        .await
    }

    pub(crate) async fn live(&self, name: &str) -> FnCall {
        call_on(
            &self.t,
            &self.slug,
            name,
            &production_host(&self.t, &self.slug),
            &[],
        )
        .await
    }
}

/// `orders`' row count through `connector`.
pub(crate) async fn count_orders(connector: &PostgresConnector) -> i64 {
    let counted = connector
        .execute_query("select count(*)::bigint as n from orders", 1)
        .await
        .expect("count orders");
    match counted.result.rows.first().map(|row| &row.0[0]) {
        Some(CellValue::Number(n)) => *n as i64,
        Some(CellValue::Text(s)) => s.parse().expect("a count"),
        other => panic!("unexpected count {other:?}"),
    }
}

/// Run `body` against `app`, then drop the store — database first, then the
/// role (`AppStore::cleanup`) — whatever happened.
pub(crate) async fn run_then_cleanup<F: std::future::Future<Output = ()>>(app: &OltpApp, body: F) {
    let outcome = std::panic::AssertUnwindSafe(body).catch_unwind().await;
    app.store.cleanup().await;
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
}

pub(crate) fn oltp_manifest() -> Value {
    json!({ "route": true, "timeoutSeconds": 60, "oltp": { "enabled": true } })
}

/// Reads production's rows, then tries each way out of `READ ONLY` the
/// review reproduced, and one it did not.
const ESCAPES_JS: &str = r#"
export default async (req, ctx) => {
  const out = { channel: ctx.channel };
  const attempt = async (key, f) => {
    try { out[key] = await f(); } catch (e) { out[key] = { error: String(e && e.message ? e.message : e) }; }
  };
  await attempt("read", async () => (await ctx.oltp.query("select count(*)::int as n from orders"))[0].n);
  await attempt("txRead", () => ctx.oltp.tx(async (tx) => (await tx.query("select count(*)::int as n from orders"))[0].n));
  await attempt("commitThenWrite", () => ctx.oltp.tx(async (tx) => {
    await tx.exec("COMMIT");
    return tx.exec("insert into orders (id) values (7)");
  }));
  await attempt("readWriteThenCommit", () => ctx.oltp.tx(async (tx) => {
    await tx.exec("SET TRANSACTION READ WRITE");
    await tx.exec("insert into orders (id) values (8)");
    return tx.exec("COMMIT");
  }));
  await attempt("setConfig", () => ctx.oltp.query("select set_config('default_transaction_read_only', 'off', false)"));
  await attempt("setConfigEscaped", () => ctx.oltp.query("select U&\"set_confi\\0067\"('default_transaction_read_only', 'off', false)"));
  await attempt("setConfigInText", () => ctx.oltp.query("select query_to_xml('select set_config(''default_transaction_read_only'', ''off'', false)', true, false, '')::text as x"));
  await attempt("twoStatements", () => ctx.oltp.exec("commit; insert into orders (id) values (9)"));
  return Response.json(out);
};
"#;

/// A read of a function that writes — invisible to the classifier.
const BUMP_JS: &str = r#"
export default async (req, ctx) => {
  try { return Response.json({ bumped: await ctx.oltp.query("select bump() as id") }); }
  catch (e) { return Response.json({ error: String(e && e.message ? e.message : e) }); }
};
"#;

fn error_of(got: &Value, key: &str) -> String {
    got[key]["error"].as_str().unwrap_or_default().to_string()
}

/// Escapes A and B from the review, `set_config` (by name, Unicode-escaped,
/// and inside SQL text), and several statements in one string: each held
/// unsent, the row count unchanged.
#[tokio::test]
async fn a_staging_function_reads_production_rows_and_cannot_leave_read_only() {
    let escapes = vec![FunctionSpec {
        name: "escapes",
        manifest: oltp_manifest(),
        js: ESCAPES_JS,
    }];
    let app = OltpApp::provision(&escapes).await;
    run_then_cleanup(&app, async {
        let staged = app.staged("escapes").await;
        let got = data(&staged);
        assert_eq!(got["channel"], "staging");
        assert_eq!(
            got["read"], 1,
            "staging reads production's rows: {}",
            staged.raw
        );
        assert_eq!(got["txRead"], 1, "a read inside ctx.oltp.tx is still sent");
        for key in [
            "commitThenWrite",
            "readWriteThenCommit",
            "setConfig",
            "setConfigEscaped",
            "setConfigInText",
            "twoStatements",
        ] {
            assert!(
                error_of(got, key).contains("HeldInStaging:"),
                "{key} must be held, got {}",
                got[key]
            );
        }
        assert_eq!(
            app.order_count().await,
            1,
            "no staging write reached the table"
        );

        let held = held_rows(&app.t).await;
        assert_eq!(held.len(), 1, "one held row per invocation");
        let writes = held[0].metadata["writes"]
            .as_array()
            .expect("writes")
            .clone();
        let verbs: Vec<&str> = writes.iter().filter_map(|w| w["verb"].as_str()).collect();
        for verb in ["COMMIT", "SET", "SELECT", "MULTIPLE"] {
            assert!(verbs.contains(&verb), "held log names {verb}: {verbs:?}");
        }
        // The org has no OLTP staging branch (P4b): each OLTP entry, and the
        // error the function saw, says how to get one.
        for w in writes.iter().filter(|w| w["plane"] == "oltp") {
            assert_eq!(w["note"], NO_BRANCH_NOTE, "{w}");
        }
        assert!(
            error_of(got, "twoStatements").contains("oxyc oltp provision --branch staging"),
            "{}",
            got["twoStatements"]
        );
    })
    .await;
}

/// The classifier sends `SELECT bump()` — it is one read — and `READ ONLY`
/// refuses the insert inside the function. A one-shot `ctx.oltp` statement is
/// committed by the host, so no held commit stands behind `READ ONLY` here.
#[tokio::test]
async fn read_only_refuses_a_write_inside_a_function_the_app_defined() {
    let bump = vec![FunctionSpec {
        name: "bump",
        manifest: oltp_manifest(),
        js: BUMP_JS,
    }];
    let app = OltpApp::provision(&bump).await;
    run_then_cleanup(&app, async {
        let staged = app.staged("bump").await;
        let got = data(&staged);
        let err = got["error"].as_str().unwrap_or_default();
        assert!(err.contains("HeldInStaging:"), "bump() must be held: {got}");
        assert_eq!(
            app.order_count().await,
            1,
            "the function's insert did not land"
        );
        let held = held_rows(&app.t).await;
        assert_eq!(held.len(), 1);
        assert_eq!(held_ops(&held[0]), vec!["oltp.query"]);

        // Production runs the same function for real: the hold is staging's.
        let live = app.live("bump").await;
        assert!(data(&live)["bumped"].is_array(), "{}", live.raw);
        assert_eq!(app.order_count().await, 2);
        assert_eq!(held_rows(&app.t).await.len(), 1, "production holds nothing");
    })
    .await;
}
