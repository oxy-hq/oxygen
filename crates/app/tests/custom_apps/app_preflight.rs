//! The deploy preflight reads prod's live apps with raw SQL, so its queries are
//! tested against Postgres: a wrong column there would not fail the build — it
//! would make `oxy migrate` log "could not run; continuing" on every release
//! and the preflight would never see an app.

use std::collections::HashMap;

use chrono::{Duration, Utc};
use entity::{
    app_builds, app_function_invocations, app_functions, apps, organizations, workspaces,
};
use oxy::config::model::Database;
use oxy_app::server::api::custom_apps_functions::preflight::{
    Findings, LiveFunction, WORKING_WITHIN_DAYS, evaluate, judge, ledger, live_functions,
    worked_recently,
};
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ConnectionTrait, DatabaseBackend, DatabaseConnection,
    IntoActiveModel, Statement,
};
use uuid::Uuid;

use crate::common::test_db;

struct Seeded {
    app_id: Uuid,
    workspace_id: Uuid,
    live_build: Uuid,
}

async fn build(db: &DatabaseConnection, app_id: Uuid, name: &str) -> Uuid {
    let id = Uuid::new_v4();
    app_builds::ActiveModel {
        id: Set(id),
        app_id: Set(app_id),
        build_id: Set(name.into()),
        s3_prefix: Set(format!("customer-apps/{app_id}/builds/{name}/")),
        created_at: Set(Utc::now().into()),
        validation_status: Set("passed".into()),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed build");
    id
}

async fn function(
    db: &DatabaseConnection,
    app_id: Uuid,
    build_id: Uuid,
    name: &str,
    manifest: serde_json::Value,
) {
    app_functions::ActiveModel {
        id: Set(Uuid::new_v4()),
        app_id: Set(app_id),
        build_id: Set(build_id),
        name: Set(name.into()),
        manifest_json: Set(Some(manifest)),
        artifact_key: Set(format!("customer-apps/{app_id}/functions/{name}.js")),
        created_at: Set(Utc::now().into()),
    }
    .insert(db)
    .await
    .expect("seed function");
}

/// An app whose live build has the incident's function and a declared one,
/// and whose older (not live) build has a function the preflight must ignore.
async fn seed(db: &DatabaseConnection) -> Seeded {
    let org_id = Uuid::new_v4();
    organizations::ActiveModel {
        id: Set(org_id),
        name: Set("Preflight Org".into()),
        slug: Set(format!("preflight-{org_id}")),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed org");
    let workspace_id = Uuid::new_v4();
    workspaces::ActiveModel {
        id: Set(workspace_id),
        name: Set("Preflight Workspace".into()),
        org_id: Set(Some(org_id)),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed workspace");
    let app_id = Uuid::new_v4();
    let app = apps::ActiveModel {
        id: Set(app_id),
        slug: Set(format!("warehouse-{app_id}")),
        name: Set("Warehouse".into()),
        org_id: Set(org_id),
        project_id: Set(workspace_id),
        branch: Set("main".into()),
        source_repo: Set("preflight/test".into()),
        status: Set("active".into()),
        source_type: Set("s3".into()),
        source_config: Set(serde_json::json!({})),
        published_at: Set(Some(Utc::now().into())),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed app");

    let old = build(db, app_id, "b0").await;
    function(
        db,
        app_id,
        old,
        "retired",
        serde_json::json!({ "destinations": ["clickhouse"] }),
    )
    .await;
    let live = build(db, app_id, "b1").await;
    function(
        db,
        app_id,
        live,
        "submit-receiving",
        serde_json::json!({ "destinations": ["clickhouse"] }),
    )
    .await;
    function(
        db,
        app_id,
        live,
        "report",
        serde_json::json!({
            "destinations": ["clickhouse"],
            "customerWarehouseWrites": { "clickhouse": "receiving reports land here" }
        }),
    )
    .await;

    let mut app = app.into_active_model();
    app.published_build_id = Set(Some(live));
    app.update(db).await.expect("publish b1");
    Seeded {
        app_id,
        workspace_id,
        live_build: live,
    }
}

async fn invocation(
    db: &DatabaseConnection,
    s: &Seeded,
    status: &str,
    fingerprint: Option<&str>,
    ago: Duration,
) {
    app_function_invocations::ActiveModel {
        id: Set(Uuid::new_v4()),
        app_id: Set(s.app_id),
        build_id: Set(s.live_build),
        function_name: Set("submit-receiving".into()),
        mode: Set("route".into()),
        user_id: Set(None),
        status: Set(status.into()),
        duration_ms: Set(Some(12)),
        error: Set(None),
        cancel_requested_at: Set(None),
        created_at: Set((Utc::now() - ago).into()),
        idempotency_key: Set(None),
        result_body: Set(None),
        result_status: Set(None),
        request_hash: Set(None),
        failure_fingerprint: Set(fingerprint.map(str::to_string)),
    }
    .insert(db)
    .await
    .expect("seed invocation");
}

/// The Poke House shape, as the compiled config's JSON carries it.
fn poke_house() -> Vec<Database> {
    serde_json::from_value(serde_json::json!([
        { "name": "clickhouse", "type": "clickhouse", "host_var": "CLICKHOUSE_HOST",
          "user_var": "CLICKHOUSE_USERNAME", "password_var": "CLICKHOUSE_PASSWORD",
          "database_var": "CLICKHOUSE_DATABASE" },
        { "name": "pokehouse", "type": "airhouse_managed" }
    ]))
    .expect("database config")
}

#[tokio::test]
async fn only_the_live_build_is_read_and_the_incident_function_is_refused() {
    let db = test_db().await;
    let s = seed(&db).await;

    let live: Vec<_> = live_functions(&db)
        .await
        .expect("live functions")
        .into_iter()
        .filter(|f| f.app_id == s.app_id)
        .collect();
    let names: Vec<&str> = live.iter().map(|f| f.function.as_str()).collect();
    assert_eq!(
        names,
        ["report", "submit-receiving"],
        "the live build's functions, and not the retired build's"
    );
    assert!(
        live.iter()
            .all(|f| f.project_id == s.workspace_id && f.manifest.is_some())
    );
    assert!(live[0].app.starts_with("preflight-") && live[0].app.contains("/warehouse-"));

    let refusals = evaluate(&live, &HashMap::from([(s.workspace_id, poke_house())])).refusals;
    assert_eq!(refusals.len(), 1, "{refusals:?}");
    assert_eq!(refusals[0].function, "submit-receiving");
    assert!(refusals[0].reason.contains("customerWarehouseWrites"));
}

#[tokio::test]
async fn working_means_a_clean_success_this_week() {
    let db = test_db().await;
    let s = seed(&db).await;
    let worked = || worked_recently(&db, s.app_id, "submit-receiving");

    assert!(!worked().await.unwrap(), "never called");

    invocation(
        &db,
        &s,
        "error",
        Some("ddd84e90d9c8c2ed"),
        Duration::hours(1),
    )
    .await;
    invocation(
        &db,
        &s,
        "success",
        Some("ddd84e90d9c8c2ed"),
        Duration::hours(1),
    )
    .await;
    assert!(
        !worked().await.unwrap(),
        "a success that answered 5xx or caught a failed ctx call is not working"
    );

    invocation(
        &db,
        &s,
        "success",
        None,
        Duration::days(WORKING_WITHIN_DAYS + 1),
    )
    .await;
    assert!(
        !worked().await.unwrap(),
        "working last month is not working this week"
    );

    invocation(&db, &s, "success", None, Duration::days(1)).await;
    assert!(worked().await.unwrap());
}

async fn live(db: &DatabaseConnection, s: &Seeded) -> Vec<LiveFunction> {
    live_functions(db)
        .await
        .expect("live functions")
        .into_iter()
        .filter(|f| f.app_id == s.app_id)
        .collect()
}

async fn ledger_rows(db: &DatabaseConnection, app_id: Uuid) -> i64 {
    db.query_one_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "SELECT count(*) AS n FROM app_preflight_refusals WHERE app_id = $1",
        [app_id.into()],
    ))
    .await
    .unwrap()
    .unwrap()
    .try_get::<i64>("", "n")
    .unwrap()
}

fn breaking(findings: &Findings) -> Vec<(bool, bool)> {
    findings
        .judged
        .iter()
        .map(|j| (j.new, j.breaks_working))
        .collect()
}

/// The release blocks on what IT changes. A function the running binary
/// already refuses on one path, while it answers on others, looks "working" —
/// only the ledger can say the refusal is not this release's doing.
#[tokio::test]
async fn a_refusal_the_last_rollout_recorded_is_carried_over_not_blocking() {
    let db = test_db().await;
    let s = seed(&db).await;
    invocation(&db, &s, "success", None, Duration::days(1)).await;
    let functions = live(&db, &s).await;
    let dbs = HashMap::from([(s.workspace_id, poke_house())]);

    let first = judge(&db, &functions, &dbs).await.unwrap();
    assert_eq!(
        breaking(&first),
        [(true, true)],
        "first sight of a refusal on a function that answers: new, and it breaks"
    );

    // A rollout the preflight let through records what it saw…
    ledger::record(&db, &first).await.unwrap();
    assert_eq!(ledger_rows(&db, s.app_id).await, 1);

    // …so the next release carries it over, though the function still answers.
    let next = judge(&db, &functions, &dbs).await.unwrap();
    assert_eq!(breaking(&next), [(false, false)]);

    // Republished with the declaration: the row goes, so a regression is new again.
    let fixed: Vec<LiveFunction> = functions
        .iter()
        .cloned()
        .map(|mut f| {
            if f.function == "submit-receiving" {
                f.manifest = Some(serde_json::json!({
                    "destinations": ["clickhouse"],
                    "customerWarehouseWrites": { "clickhouse": "receiving reports land here" }
                }));
            }
            f
        })
        .collect();
    let after_fix = judge(&db, &fixed, &dbs).await.unwrap();
    assert!(after_fix.judged.is_empty());
    ledger::record(&db, &after_fix).await.unwrap();
    assert_eq!(ledger_rows(&db, s.app_id).await, 0);
}
