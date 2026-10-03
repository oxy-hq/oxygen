//! A sandbox's own app database: with the org's OLTP staging branch, each
//! sandbox's `ctx.oltp` runs in a schema of its own inside that branch
//! (`internal-docs/per-org-oltp-postgres.md` → Sandbox schemas on the staging
//! branch).
//!
//! Through the real publish, the real queued task run by the executor
//! production registers, and the real serve route — against a branch
//! `LocalProvider` cut for real, so every assertion counts rows or reads the
//! catalog in the schemas themselves.
//!
//! - **two sandboxes of one app** write the same table and see neither each
//!   other's rows nor staging's; production is untouched, and a
//!   production-admitted call still writes production;
//! - **a sandbox's migration** adds a column staging and the other sandbox do
//!   not have, under a ledger target of its own; a file naming staging's
//!   schema is refused;
//! - **a statement naming another schema**, or changing the search path, is
//!   refused and listed in the held row;
//! - **before the schema is ready** `ctx.oltp` is refused — never run in
//!   staging's schema;
//! - **no branch**: held, exactly as before;
//! - **a branch reset** is refused with what to do, and the next publish
//!   seeds and migrates a new copy.
//!
//! The teardown is `sandbox_oltp_teardown`.

use agentic_core::delegation::{TaskAssignment, TaskOutcome, TaskSpec};
use agentic_runtime::worker::TaskExecutor;
use axum::http::StatusCode;
use entity::apps;
use oxy_app::server::api::custom_apps_functions::env_policy::NO_BRANCH_NOTE;
use oxy_app::server::api::custom_apps_migrations::{MigrationTarget, read_ledger};
use oxy_app::server::api::custom_apps_publish::{PublishResult, PublishTarget, publish_to};
use oxy_app::server::api::custom_apps_sandboxes::oltp_state::{self, OltpSchemaStatus};
use oxy_app::server::api::custom_apps_sandboxes::oltp_task::{
    SANDBOX_OLTP_KIND, SandboxOltpExecutor,
};
use oxy_app::server::api::custom_apps_sandboxes::ops;
use oxy_oltp::OltpBranch::Staging;
use oxy_oltp::sandbox_schema::{SandboxSchema, exists_on_branch};
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, EntityTrait, Statement};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::custom_app_functions_fixture::{FunctionSpec, call_function_with};
use crate::sandbox_publish::{app_row, bundle, input, sandbox};
use crate::staging_functions::{data, held_rows};
use crate::staging_functions_oltp::{OltpApp, oltp_manifest, run_then_cleanup};

/// Runs each `[key, kind, sql]` step of the request through `ctx.oltp` —
/// `query`, `exec`, or `tx` (one statement in a `ctx.oltp.tx`) — keeping its
/// result or its error under the key.
const SQL_JS: &str = r#"
export default async (req, ctx) => {
  const { steps } = JSON.parse(req.body);
  const out = { channel: ctx.channel };
  for (const [key, kind, sql] of steps) {
    try {
      if (kind === "exec") out[key] = { ok: await ctx.oltp.exec(sql) };
      else if (kind === "tx") out[key] = { ok: await ctx.oltp.tx((tx) => tx.exec(sql)) };
      else out[key] = { ok: await ctx.oltp.query(sql) };
    } catch (e) {
      out[key] = { error: String(e && e.message ? e.message : e) };
    }
  }
  return Response.json(out);
};
"#;

pub(crate) fn functions() -> Vec<FunctionSpec> {
    vec![FunctionSpec {
        name: "sql",
        manifest: oltp_manifest(),
        js: SQL_JS,
    }]
}

const COUNT: [&str; 3] = ["n", "query", "select count(*)::int as n from orders"];

/// The app's row, with sandboxes `dev-a1` and `dev-b2` created.
pub(crate) async fn with_two_sandboxes(app: &OltpApp) -> apps::Model {
    // SAFETY: nextest runs each test in its own process.
    unsafe { std::env::set_var("OXY_API_URL", "https://app-dev.oxygen-hq.com") };
    let row = apps::Entity::find()
        .all(&app.t.db)
        .await
        .expect("read apps")
        .into_iter()
        .find(|a| a.slug == app.slug && a.org_id == app.t.org_id)
        .expect("the app");
    for handle in ["a1", "b2"] {
        ops::create(&app.t.db, &row, &sandbox(handle), app.t.guest_id)
            .await
            .expect("create the sandbox");
    }
    app_row(&app.t.db, row.id).await
}

/// Publish a build to sandbox `dev-<handle>`, with `files` as its OLTP
/// migrations when there are any.
pub(crate) async fn publish(
    app: &OltpApp,
    handle: &str,
    build_id: &str,
    files: &[(&str, &[u8])],
) -> PublishResult {
    let declares = match files {
        [] => json!({}),
        _ => json!({ "migrations": { "dir": "migrations" } }),
    };
    let tarball = bundle(&app.slug, &functions(), declares, files);
    let mut publishing = input(&app.t, &app.slug, build_id, tarball);
    publishing.project_id = app.workspace;
    publish_to(publishing, PublishTarget::Sandbox(sandbox(handle)))
        .await
        .expect("publish to the sandbox")
}

/// The app's queued sandbox OLTP tasks, oldest first.
pub(crate) async fn queued_oltp(db: &DatabaseConnection, app_id: Uuid) -> Vec<(String, TaskSpec)> {
    let rows = db
        .query_all_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT task_id, spec FROM agentic_task_queue \
             WHERE queue_status = 'queued' AND spec->>'kind' = $1 \
               AND spec->'payload'->>'app_id' = $2 \
             ORDER BY created_at, task_id",
            [SANDBOX_OLTP_KIND.into(), app_id.to_string().into()],
        ))
        .await
        .expect("read the queue");
    rows.iter()
        .map(|row| {
            let task_id: String = row.try_get("", "task_id").expect("task_id");
            let spec: Value = row.try_get("", "spec").expect("spec");
            (task_id, serde_json::from_value(spec).expect("a TaskSpec"))
        })
        .collect()
}

/// Run every queued sandbox OLTP task of the app through the executor the
/// worker fleet registers, and take each off the queue: what each answered.
pub(crate) async fn run_oltp_tasks(db: &DatabaseConnection, app_id: Uuid) -> Vec<TaskOutcome> {
    let mut outcomes = Vec::new();
    for (task_id, spec) in queued_oltp(db, app_id).await {
        let executor = SandboxOltpExecutor { db: db.clone() };
        let mut running = executor
            .execute(TaskAssignment {
                task_id: task_id.clone(),
                parent_task_id: None,
                run_id: task_id.clone(),
                spec,
                policy: None,
            })
            .await
            .expect("the executor takes a sandbox OLTP task");
        outcomes.push(running.outcomes.recv().await.expect("an outcome"));
        db.execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "UPDATE agentic_task_queue SET queue_status = 'completed' WHERE task_id = $1",
            [task_id.into()],
        ))
        .await
        .expect("complete the task");
    }
    outcomes
}

/// What every queued task answered, each of which must have succeeded.
pub(crate) async fn run_oltp_tasks_done(db: &DatabaseConnection, app_id: Uuid) -> Vec<String> {
    run_oltp_tasks(db, app_id)
        .await
        .into_iter()
        .map(|outcome| match outcome {
            TaskOutcome::Done { answer, .. } => answer,
            other => panic!("the sandbox OLTP task must succeed: {other:?}"),
        })
        .collect()
}

/// `steps` run by the `sql` function in `environment` (`production`, or the
/// environment named by `X-Oxy-App-Env` on a bearer request).
pub(crate) async fn run_in(app: &OltpApp, environment: &str, steps: &[[&str; 3]]) -> Value {
    let named = [
        ("authorization", "Bearer t"),
        ("x-oxy-app-env", environment),
    ];
    let headers: &[(&str, &str)] = if environment == "production" {
        &[]
    } else {
        &named
    };
    let body = json!({ "steps": steps });
    let call = call_function_with(&app.t.org_slug, &app.slug, "sql", body, headers).await;
    assert_eq!(call.status, StatusCode::OK, "{environment}: {}", call.raw);
    data(&call).clone()
}

pub(crate) fn schema_of(app: &OltpApp, handle: &str) -> SandboxSchema {
    let label = sandbox(handle).schema_label().expect("a label");
    SandboxSchema::for_writer(&app.store.writer, &label).expect("a sandbox schema")
}

async fn state_of(
    app: &OltpApp,
    app_id: Uuid,
    handle: &str,
) -> Option<oltp_state::OltpSchemaState> {
    oltp_state::read(&app.t.db, app_id, &sandbox(handle))
        .await
        .expect("read the sandbox's state")
}

fn error_of(got: &Value, key: &str) -> String {
    got[key]["error"].as_str().unwrap_or_default().to_string()
}

fn ids(got: &Value, key: &str) -> Vec<i64> {
    let rows = got[key]["ok"]
        .as_array()
        .unwrap_or_else(|| panic!("{key}: {got}"));
    rows.iter()
        .map(|row| row["id"].as_i64().expect("an id"))
        .collect()
}

const IDS: [&str; 3] = ["ids", "query", "select id from orders order by id"];

#[tokio::test]
async fn two_sandboxes_and_staging_each_keep_their_own_rows() {
    let app = OltpApp::provision(&functions()).await;
    run_then_cleanup(&app, async {
        app.store.provision_branch().await;
        let row = with_two_sandboxes(&app).await;
        let db = &app.t.db;

        // Published to, and not yet seeded: refused — not run in staging's.
        publish(&app, "a1", "a-1", &[]).await;
        let early = run_in(&app, "dev-a1", &[COUNT]).await;
        assert!(
            error_of(&early, "n").contains("EnvironmentRefused: ctx.oltp.query")
                && error_of(&early, "n").contains("no schema of its own yet"),
            "{early}"
        );
        publish(&app, "b2", "b-1", &[]).await;
        assert_eq!(queued_oltp(db, row.id).await.len(), 2, "one task a publish");
        let done = run_oltp_tasks_done(db, row.id).await;
        assert!(
            done.iter().all(|a| a.contains("seeded from staging's")),
            "{done:?}"
        );
        for handle in ["a1", "b2"] {
            let state = state_of(&app, row.id, handle).await.expect("a state");
            assert_eq!(state.status, OltpSchemaStatus::Ready, "{handle}");
            assert_eq!(state.schema, schema_of(&app, handle).name());
            assert_eq!(state.tables, 1, "orders");
        }

        // Each starts as a copy of staging's (one row), and keeps its own.
        let insert = |id: i64| format!("insert into orders (id) values ({id})");
        let (in_a1, in_b2, in_staging, in_production) =
            (insert(100), insert(200), insert(300), insert(400));
        let a1 = run_in(&app, "dev-a1", &[["w", "exec", &in_a1], IDS]).await;
        let b2 = run_in(&app, "dev-b2", &[["w", "tx", &in_b2], IDS]).await;
        let staging = run_in(&app, "staging", &[["w", "exec", &in_staging], IDS]).await;
        let production = run_in(&app, "production", &[["w", "exec", &in_production], IDS]).await;
        assert_eq!(a1["channel"], "dev-a1");
        assert_eq!(ids(&a1, "ids"), [1, 100], "{a1}");
        assert_eq!(ids(&b2, "ids"), [1, 200], "{b2}");
        assert_eq!(ids(&staging, "ids"), [1, 300], "{staging}");
        assert_eq!(ids(&production, "ids"), [1, 400], "{production}");

        // Read again from each: nobody saw anybody else's write.
        assert_eq!(ids(&run_in(&app, "dev-a1", &[IDS]).await, "ids"), [1, 100]);
        assert_eq!(ids(&run_in(&app, "dev-b2", &[IDS]).await, "ids"), [1, 200]);
        assert_eq!(ids(&run_in(&app, "staging", &[IDS]).await, "ids"), [1, 300]);
        assert_eq!(app.order_count().await, 2, "production holds its own two");
        assert!(
            held_rows(&app.t).await.iter().all(|held| {
                let writes = held.metadata["writes"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default();
                writes.iter().all(|w| w["verb"] == "STATEMENT")
            }),
            "only the early, refused read is in a held row"
        );
    })
    .await;
}

const ADD_TIER: &[u8] = b"alter table orders add column tier text default 'gold';";

#[tokio::test]
async fn a_sandbox_migration_changes_only_that_sandboxs_schema() {
    let app = OltpApp::provision(&functions()).await;
    run_then_cleanup(&app, async {
        let branch = app.store.provision_branch().await;
        let row = with_two_sandboxes(&app).await;
        let db = &app.t.db;
        let published = publish(&app, "a1", "a-1", &[("migrations/0001_tier.sql", ADD_TIER)]).await;
        assert!(
            published.warnings.is_empty(),
            "with a branch the files are applied, not warned about: {:?}",
            published.warnings
        );
        publish(&app, "b2", "b-1", &[]).await;
        let done = run_oltp_tasks_done(db, row.id).await;
        assert!(
            done[0].contains("1 schema migration(s) applied"),
            "{done:?}"
        );

        let tier: [&str; 3] = ["tier", "query", "select tier from orders where id = 1"];
        let a1 = run_in(&app, "dev-a1", &[tier]).await;
        assert_eq!(a1["tier"]["ok"][0]["tier"], "gold", "{a1}");
        for other in ["dev-b2", "staging", "production"] {
            let got = run_in(&app, other, &[tier]).await;
            assert!(
                error_of(&got, "tier").contains("tier"),
                "{other} has no such column: {got}"
            );
        }

        // Recorded under the sandbox's own target, and nowhere else.
        let a1_target = MigrationTarget::Schema(schema_of(&app, "a1").name().to_string());
        let applied = |target: MigrationTarget| async move {
            read_ledger(db, row.id, "oltp", &target)
                .await
                .expect("ledger")
        };
        assert!(applied(a1_target).await.contains_key("0001_tier.sql"));
        for target in [
            MigrationTarget::Production,
            MigrationTarget::Branch(branch.provider_branch_id.clone()),
            MigrationTarget::Schema(schema_of(&app, "b2").name().to_string()),
        ] {
            assert!(applied(target.clone()).await.is_empty(), "{target:?}");
        }

        // The same file again is already applied; nothing is seeded twice.
        publish(&app, "a1", "a-2", &[("migrations/0001_tier.sql", ADD_TIER)]).await;
        let again = run_oltp_tasks_done(db, row.id).await;
        assert!(again[0].contains("was already seeded"), "{again:?}");
        assert!(again[0].contains("0 schema migration(s) applied, 1 already present"));

        // A file that names staging's schema is the author's to fix: the task
        // fails, nothing runs, and the sandbox keeps the schema it had.
        let names_staging = format!(
            "alter table {}.orders add column x int;",
            app.store.writer.schema_name()
        );
        publish(
            &app,
            "a1",
            "a-3",
            &[
                ("migrations/0001_tier.sql", ADD_TIER),
                ("migrations/0002_bad.sql", names_staging.as_bytes()),
            ],
        )
        .await;
        let outcomes = run_oltp_tasks(db, row.id).await;
        let TaskOutcome::Failed(why) = &outcomes[0] else {
            panic!("a file naming staging's schema fails the task: {outcomes:?}");
        };
        assert!(
            why.contains("0002_bad.sql") && why.contains("names schema"),
            "{why}"
        );
        let staging = run_in(&app, "staging", &[["x", "query", "select x from orders"]]).await;
        assert!(
            error_of(&staging, "x").contains("x"),
            "staging was not altered: {staging}"
        );
        let a1 = run_in(&app, "dev-a1", &[tier]).await;
        assert_eq!(a1["tier"]["ok"][0]["tier"], "gold", "still ready: {a1}");
    })
    .await;
}

#[tokio::test]
async fn a_statement_that_leaves_the_sandboxs_schema_is_refused_and_listed() {
    let app = OltpApp::provision(&functions()).await;
    run_then_cleanup(&app, async {
        app.store.provision_branch().await;
        let row = with_two_sandboxes(&app).await;
        publish(&app, "a1", "a-1", &[]).await;
        publish(&app, "b2", "b-1", &[]).await;
        run_oltp_tasks_done(&app.t.db, row.id).await;

        let staging_schema = app.store.writer.schema_name();
        let other = schema_of(&app, "b2");
        let read_staging = format!("select id from {staging_schema}.orders");
        let write_staging = format!("insert into {staging_schema}.orders (id) values (7)");
        let write_other = format!("insert into {other}.orders (id) values (8)");
        let by_string = format!("select '{staging_schema}.orders'::regclass::text as t");
        let steps: Vec<[&str; 3]> = vec![
            ["readStaging", "query", &read_staging],
            ["writeStaging", "exec", &write_staging],
            ["writeOther", "tx", &write_other],
            ["byString", "query", &by_string],
            ["searchPath", "tx", "set search_path = public"],
            [
                "setConfig",
                "query",
                "select set_config('search_path', 'public', false)",
            ],
            ["public", "query", "select * from public.orders"],
            ["unparsed", "exec", "insert orders values 9 9"],
        ];
        let got = run_in(&app, "dev-a1", &steps).await;
        for [key, _, _] in &steps {
            let error = error_of(&got, key);
            assert!(
                error.contains("EnvironmentRefused: ctx.") && error.contains("This statement"),
                "{key} must be refused: {}",
                got[*key]
            );
        }
        assert!(
            error_of(&got, "readStaging").contains("not this sandbox's own"),
            "{got}"
        );
        assert!(
            error_of(&got, "searchPath").contains("how names resolve"),
            "{got}"
        );
        assert!(
            error_of(&got, "unparsed").contains("does not parse"),
            "{got}"
        );

        // Nothing reached staging's schema or the other sandbox's.
        assert_eq!(ids(&run_in(&app, "staging", &[IDS]).await, "ids"), [1]);
        assert_eq!(ids(&run_in(&app, "dev-b2", &[IDS]).await, "ids"), [1]);
        assert_eq!(ids(&run_in(&app, "dev-a1", &[IDS]).await, "ids"), [1]);
        let listed: Vec<String> = held_rows(&app.t)
            .await
            .iter()
            .flat_map(|held| {
                held.metadata["writes"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
            })
            .filter_map(|w| w["verb"].as_str().map(str::to_string))
            .collect();
        for verb in ["NAME", "SET SEARCH_PATH", "SET_CONFIG", "UNCLASSIFIED"] {
            assert!(
                listed.iter().any(|v| v == verb),
                "the held row lists {verb}: {listed:?}"
            );
        }
    })
    .await;
}

#[tokio::test]
async fn with_no_branch_a_sandboxs_oltp_is_held_as_before() {
    let app = OltpApp::provision(&functions()).await;
    run_then_cleanup(&app, async {
        let row = with_two_sandboxes(&app).await;
        let published = publish(&app, "a1", "a-1", &[("migrations/0001_tier.sql", ADD_TIER)]).await;
        assert!(
            published
                .warnings
                .iter()
                .any(|w| w.starts_with("1 OLTP migration file(s) were not applied")),
            "{:?}",
            published.warnings
        );
        assert!(
            queued_oltp(&app.t.db, row.id).await.is_empty(),
            "nothing is queued"
        );
        assert_eq!(state_of(&app, row.id, "a1").await, None);

        let got = run_in(
            &app,
            "dev-a1",
            &[COUNT, ["w", "exec", "insert into orders (id) values (100)"]],
        )
        .await;
        assert_eq!(
            got["n"]["ok"][0]["n"], 1,
            "a single read reaches production: {got}"
        );
        let held = error_of(&got, "w");
        assert!(held.contains("HeldInStaging: ctx.oltp.exec"), "{got}");
        assert!(held.contains(NO_BRANCH_NOTE), "{got}");
        assert_eq!(app.order_count().await, 1, "production is untouched");
    })
    .await;
}

#[tokio::test]
async fn after_a_branch_reset_ctx_oltp_is_refused_until_a_publish_reseeds() {
    let app = OltpApp::provision(&functions()).await;
    run_then_cleanup(&app, async {
        app.store.provision_branch().await;
        let row = with_two_sandboxes(&app).await;
        let db = &app.t.db;
        let files: &[(&str, &[u8])] = &[("migrations/0001_tier.sql", ADD_TIER)];
        publish(&app, "a1", "a-1", files).await;
        run_oltp_tasks_done(db, row.id).await;
        let insert = ["w", "exec", "insert into orders (id) values (100)"];
        assert_eq!(
            ids(&run_in(&app, "dev-a1", &[insert, IDS]).await, "ids"),
            [1, 100]
        );
        let schema = schema_of(&app, "a1");
        let on_branch = || exists_on_branch(db, app.t.org_id, Staging, &schema);
        assert_eq!(on_branch().await.expect("read"), Some(true));

        // The reset replaces the database every sandbox schema is in.
        app.store.reset_branch().await;
        assert_eq!(on_branch().await.expect("read"), Some(false));
        let state = state_of(&app, row.id, "a1").await.expect("still recorded");
        assert_eq!(
            state.status,
            OltpSchemaStatus::Ready,
            "the row has not heard"
        );
        let shown = ops::get(db, &row, &app.t.org_slug, &sandbox("a1"))
            .await
            .expect("shown");
        assert_eq!(shown.oltp_schema.expect("shown").status, "stale");

        let refused = run_in(&app, "dev-a1", &[IDS, insert]).await;
        for key in ["ids", "w"] {
            let error = error_of(&refused, key);
            assert!(
                error.contains("EnvironmentRefused")
                    && error.contains("was reset")
                    && error.contains("publish to the sandbox again"),
                "{key}: {refused}"
            );
        }
        // Not run in staging's schema instead, which the reset left as production's.
        assert_eq!(ids(&run_in(&app, "staging", &[IDS]).await, "ids"), [1]);

        // The next publish seeds a new copy and applies the build's files.
        publish(&app, "a1", "a-2", files).await;
        let done = run_oltp_tasks_done(db, row.id).await;
        assert!(done[0].contains("seeded from staging's"), "{done:?}");
        assert!(
            done[0].contains("1 schema migration(s) applied"),
            "{done:?}"
        );
        assert_eq!(on_branch().await.expect("read"), Some(true));
        let again = run_in(
            &app,
            "dev-a1",
            &[
                IDS,
                ["tier", "query", "select tier from orders where id = 1"],
            ],
        )
        .await;
        assert_eq!(ids(&again, "ids"), [1], "a fresh copy, without the old row");
        assert_eq!(again["tier"]["ok"][0]["tier"], "gold", "{again}");
        let shown = ops::get(db, &row, &app.t.org_slug, &sandbox("a1"))
            .await
            .expect("shown");
        assert_eq!(shown.oltp_schema.expect("shown").status, "ready");
    })
    .await;
}
