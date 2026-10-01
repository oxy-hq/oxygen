//! Transform detection: nested manual steps and variables, and its database
//! halves — what counts as changed, and one build per auto transform under a
//! check however often it runs. Fixtures: [`super::transform_fixtures`].

use serde_json::{Value, json};
use uuid::Uuid;

use super::tests::{seed_revision, seed_workspace};
pub(super) use super::transform_fixtures::{databases, note, pokehouse, sql, sql_file};
use super::transforms::detect;
use super::*;
use crate::server::previews::airhouse_duckdb::DuckDbAirhouse;
use crate::server::previews::runs;
use crate::server::test_support::{SKIP_MSG, test_db};

#[test]
fn an_airhouse_with_its_own_credentials_or_an_unknown_database_is_manual() {
    let dbs = databases();
    let on = |database: &str| {
        let def = json!({ "tasks": [{ "type": "execute_sql", "database": database,
                                      "sql_query": "INSERT INTO s.t SELECT 1" }] });
        classify(&def, &dbs, &sql_file)
    };
    assert!(matches!(on("legacy_lake"), Build::Manual(r) if r.contains("its own credentials")));
    assert!(matches!(on("nowhere"), Build::Manual(r) if r.contains("config.yml")));
    let templated = json!({ "tasks": [{ "type": "execute_sql", "database": "pokehouse",
                                        "sql_file": "sql/{{ region }}.sql" }] });
    assert!(
        matches!(classify(&templated, &dbs, &sql_file), Build::Manual(r) if r.contains("templated"))
    );
}

/// A manual step is found wherever it sits — a conditional's `else`, a loop
/// body — and a procedure declaring a variable with no default is not built
/// (a build passes no variables).
#[test]
fn nested_manual_steps_and_variables_without_defaults_are_manual() {
    let dbs = databases();
    let write = sql("w", "INSERT INTO toast_pos.t SELECT 1");
    let agent = json!({ "name": "a", "type": "agent", "agent_ref": "x", "prompt": "p" });
    let in_else = json!({ "tasks": [write, { "name": "c", "type": "conditional",
        "conditions": [{ "if": "true", "tasks": [note()] }], "else": [agent] }] });
    let in_loop = json!({ "tasks": [write, { "name": "l", "type": "loop_sequential",
        "values": [1], "tasks": [{ "name": "h", "type": "http_request", "url": "https://x" }] }] });
    let manual = |def: &Value| match classify(def, &dbs, &sql_file) {
        Build::Manual(reason) => reason,
        Build::Auto => panic!("built: {def}"),
    };
    assert_eq!(manual(&in_else), "calls an agent");
    assert_eq!(manual(&in_loop), "sends an HTTP request");

    let mut needs = json!({ "tasks": [write], "variables": {
        "date": { "description": "the business date" },
        "region": null,
        "label": { "default": "all", "description": "shown" },
        "plain": "value",
    }});
    let why = manual(&needs);
    assert!(why.starts_with("needs variables: date, region"), "{why}");
    needs["variables"] = json!({ "label": { "default": "all" }, "plain": "value" });
    assert_eq!(
        classify(&needs, &dbs, &sql_file),
        Build::Auto,
        "control: defaults"
    );
}

async fn seed(db: &sea_orm::DatabaseConnection, rev: Uuid, path: &str, def: &Value) {
    tests_exec(
        db,
        "INSERT INTO automation_definitions (revision_id, file_path, name, extension, definition) \
         VALUES ($1, $2, $3, 'procedure', $4)",
        vec![
            rev.into(),
            path.into(),
            def["name"].as_str().unwrap_or("p").into(),
            def.clone().into(),
        ],
    )
    .await;
}

async fn seed_sql(db: &sea_orm::DatabaseConnection, rev: Uuid, path: &str, content: &str) {
    tests_exec(
        db,
        "INSERT INTO verified_queries (revision_id, file_path, content_sha256, content) \
         VALUES ($1, $2, md5($3), $3)",
        vec![rev.into(), path.into(), content.into()],
    )
    .await;
}

/// The `transform_build` runs queued under check `check`.
async fn builds_of(db: &sea_orm::DatabaseConnection, check: &str) -> i64 {
    use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};
    db.query_one_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "SELECT count(*) AS n FROM workspace_preview_runs \
         WHERE parent_run_id = $1 AND kind = 'transform_build'",
        [check.into()],
    ))
    .await
    .unwrap()
    .unwrap()
    .try_get("", "n")
    .unwrap()
}

/// A missing compiled config is the platform's fault: the check fails (and
/// is retried), rather than calling the branch's databases unknown.
#[tokio::test]
async fn a_revision_with_no_compiled_config_fails_the_check_to_retry() {
    let Some(db) = test_db().await else {
        eprintln!("{SKIP_MSG}");
        return;
    };
    let ws = seed_workspace(&db).await;
    let staging = seed_revision(&db, ws, "staging").await;
    let err = changed_transforms(&db, staging, None)
        .await
        .expect_err("no compiled config");
    assert!(err.to_string().contains("no compiled config"), "{err}");
    tests_exec(
        &db,
        "INSERT INTO workspace_compiled_configs (revision_id, databases) VALUES ($1, '[]'::jsonb)",
        vec![staging.into()],
    )
    .await;
    assert!(
        changed_transforms(&db, staging, None)
            .await
            .unwrap()
            .is_empty()
    );
}

async fn tests_exec(db: &sea_orm::DatabaseConnection, sql: &str, values: Vec<sea_orm::Value>) {
    use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        sql,
        values,
    ))
    .await
    .expect(sql);
}

/// Changed = a different definition, a new file, or the same definition over
/// a `.sql` file whose content changed. An untouched procedure is not listed.
#[tokio::test]
async fn changed_automations_are_found_by_definition_and_by_sql_file() {
    let Some(db) = test_db().await else {
        eprintln!("{SKIP_MSG}");
        return;
    };
    let ws = seed_workspace(&db).await;
    let (main, staging) = (
        seed_revision(&db, ws, "main").await,
        seed_revision(&db, ws, "staging").await,
    );
    let same = pokehouse("weekly_report");
    let camera = pokehouse("rollups/camera_time_to_serve_rollups");
    let before = pokehouse("compute_toast_journal_entry_airhouse");
    let mut after = before.clone();
    after["tasks"][0]["sql_query"] = json!("DELETE FROM toast_pos.toast_journal_entry_lines");
    for (rev, je) in [(main, &before), (staging, &after)] {
        seed(&db, rev, "workflows/weekly.procedure.yml", &same).await;
        seed(&db, rev, "workflows/camera.procedure.yml", &camera).await;
        seed(&db, rev, "workflows/je.procedure.yml", je).await;
    }
    seed_sql(
        &db,
        main,
        "sql/camera_tts.sql",
        "INSERT INTO cameras.a SELECT 1",
    )
    .await;
    seed_sql(
        &db,
        staging,
        "sql/camera_tts.sql",
        &sql_file("sql/camera_tts.sql").unwrap(),
    )
    .await;
    seed(
        &db,
        staging,
        "workflows/new.procedure.yml",
        &pokehouse("toast_ingest_and_rollups"),
    )
    .await;

    let found = detect(&db, staging, Some(main), &databases())
        .await
        .unwrap();
    let seen: Vec<(&str, &str, &str)> = found
        .iter()
        .map(|t| (t.file_path.as_str(), t.change.as_str(), t.build.as_str()))
        .collect();
    assert_eq!(
        seen,
        vec![
            ("workflows/camera.procedure.yml", "modified", "auto"),
            ("workflows/je.procedure.yml", "modified", "auto"),
            ("workflows/new.procedure.yml", "added", "manual"),
        ]
    );
    assert_eq!(found[2].reason.as_deref(), Some("calls an airway step"));
}

/// One build per auto transform under a check, however often the check runs;
/// a manual one gets none; and with runs off, none at all, with why.
#[tokio::test]
#[serial_test::serial]
async fn a_check_queues_each_auto_build_once() {
    let Some(db) = test_db().await else {
        eprintln!("{SKIP_MSG}");
        return;
    };
    let ws = seed_workspace(&db).await;
    let staging = seed_revision(&db, ws, "staging").await;
    let check = ensure_enqueued(&db, ws, "feat/x", staging)
        .await
        .unwrap()
        .unwrap();
    let row = entity::workspace_preview_runs::Entity::find_by_id(check)
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    let transforms = || {
        vec![
            TransformReport {
                name: "je".into(),
                file_path: "workflows/je.procedure.yml".into(),
                change: "modified".into(),
                build: "auto".into(),
                reason: None,
                build_run_id: None,
            },
            TransformReport {
                name: "ingest".into(),
                file_path: "workflows/ingest.procedure.yml".into(),
                change: "added".into(),
                build: "manual".into(),
                reason: Some("calls an airway step".into()),
                build_run_id: None,
            },
        ]
    };
    let lake = || {
        std::sync::Arc::new(std::sync::Mutex::new(
            duckdb::Connection::open_in_memory().unwrap(),
        ))
    };
    let (scopes, old) = (
        DuckDbAirhouse::new(lake()).unwrap(),
        DuckDbAirhouse::old(lake()).unwrap(),
    );
    // SAFETY: serialised; nextest runs each test in its own process anyway.
    unsafe { std::env::remove_var(runs::RUNS_FLAG_ENV) };
    let mut off = transforms();
    queue_builds(&db, &scopes, &row, &mut off).await.unwrap();
    assert!(off[0].build_run_id.is_none());
    assert!(
        off[0]
            .reason
            .as_deref()
            .unwrap()
            .contains("OXY_PREVIEW_RUNS")
    );

    unsafe { std::env::set_var(runs::RUNS_FLAG_ENV, "1") };
    // An Airhouse that cannot scope preview writers: nothing is built, and
    // the check says why.
    let mut skipped = transforms();
    queue_builds(&db, &old, &row, &mut skipped).await.unwrap();
    assert!(skipped[0].build_run_id.is_none(), "{skipped:?}");
    let why = skipped[0].reason.as_deref().unwrap();
    assert!(why.starts_with(super::builds::BUILDS_SKIPPED), "{why}");
    assert_eq!(builds_of(&db, &row.run_id).await, 0, "nothing queued");

    let (mut first, mut again) = (transforms(), transforms());
    queue_builds(&db, &scopes, &row, &mut first).await.unwrap();
    queue_builds(&db, &scopes, &row, &mut again).await.unwrap();
    unsafe { std::env::remove_var(runs::RUNS_FLAG_ENV) };
    let build = first[0].build_run_id.clone().expect("queued");
    assert_eq!(
        again[0].build_run_id.as_deref(),
        Some(build.as_str()),
        "once"
    );
    assert!(
        first[1].build_run_id.is_none(),
        "a manual transform is not built"
    );
    let build_row = entity::workspace_preview_runs::Entity::find_by_id(build)
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (build_row.kind.as_str(), build_row.parent_run_id.as_deref()),
        ("transform_build", Some(row.run_id.as_str()))
    );
    assert_eq!(
        build_row.target_ref.as_deref(),
        Some("workflows/je.procedure.yml")
    );
    assert_eq!(
        build_row.state, "running",
        "the queue was free, so it started"
    );
}
