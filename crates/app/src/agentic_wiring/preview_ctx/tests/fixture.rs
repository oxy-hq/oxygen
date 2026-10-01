//! Seeds for the preview-platform tests: a workspace, revisions with compiled
//! configs, one of each compiled artifact per revision, and a platform that
//! needs no database.

use std::collections::HashSet;

use agentic_pipeline::platform::{AirwayStepMode, PreviewScope};
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};
use serde_json::{Value, json};
use uuid::Uuid;

use super::super::PreviewPlatformContext;

pub(super) const RUN: &str = "11111111-2222-4333-8444-555555555555";

pub(super) async fn exec(db: &DatabaseConnection, sql: &str, values: Vec<sea_orm::Value>) {
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        sql,
        values,
    ))
    .await
    .expect(sql);
}

pub(super) async fn seed_workspace(db: &DatabaseConnection) -> Uuid {
    use sea_orm::{ActiveModelTrait, Set};
    let id = Uuid::new_v4();
    entity::workspaces::ActiveModel {
        id: Set(id),
        name: Set(format!("preview-ctx-{id}")),
        status: Set(entity::workspaces::WorkspaceStatus::Ready),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed workspace");
    id
}

pub(super) async fn seed_revision(
    db: &DatabaseConnection,
    ws: Uuid,
    kind: &str,
    databases: Value,
) -> Uuid {
    use sea_orm::{ActiveModelTrait, Set};
    let rev = Uuid::new_v4();
    let now = chrono::Utc::now().fixed_offset();
    entity::revisions::ActiveModel {
        revision_id: Set(rev),
        workspace_id: Set(ws),
        git_sha: Set(format!("sha-{rev}")),
        branch: Set(Some(if kind == "main" { "main" } else { "feat/x" }.into())),
        schema_version: Set(oxy_compile::CURRENT_SCHEMA_VERSION),
        status: Set("ready".into()),
        kind: Set(kind.into()),
        owner_user_id: Set(None),
        compiler_version: Set("test".into()),
        started_at: Set(now),
        finished_at: Set(Some(now)),
        file_count_seen: Set(0),
        file_count_compiled: Set(0),
        file_count_failed: Set(0),
        error_summary: Set(None),
    }
    .insert(db)
    .await
    .expect("seed revision");
    exec(
        db,
        "INSERT INTO workspace_compiled_configs (revision_id, databases) VALUES ($1, $2)",
        vec![rev.into(), databases.into()],
    )
    .await;
    rev
}

/// One automation, verified query, pipeline and agent per revision, each
/// carrying `marker` so a read says which revision answered it.
pub(super) async fn seed_artifacts(db: &DatabaseConnection, rev: Uuid, marker: &str) {
    let automation = json!({ "name": "je", "tasks": [
        { "name": "w", "type": "execute_sql", "database": "clickhouse",
          "sql_query": format!("INSERT INTO journal SELECT '{marker}'") }
    ]});
    exec(
        db,
        "INSERT INTO automation_definitions (revision_id, file_path, name, extension, definition) \
         VALUES ($1, 'workflows/je.procedure.yml', 'je', 'procedure', $2)",
        vec![rev.into(), automation.into()],
    )
    .await;
    exec(
        db,
        "INSERT INTO verified_queries (revision_id, file_path, content_sha256, content) \
         VALUES ($1, 'sql/x.sql', 'x', $2)",
        vec![rev.into(), format!("SELECT '{marker}'").into()],
    )
    .await;
    let pipeline = json!({ "name": format!("p_{marker}"),
        "source": { "kind": "rest_api", "config": {} },
        "destination": { "database": "airhouse", "dataset_name": "raw" } });
    exec(
        db,
        "INSERT INTO airway_pipelines (revision_id, name, file_path, definition) \
         VALUES ($1, $2, 'airway/x.airway.yml', $3)",
        vec![rev.into(), format!("p_{marker}").into(), pipeline.into()],
    )
    .await;
    exec(
        db,
        "INSERT INTO agent_definitions (revision_id, name, file_path, definition) \
         VALUES ($1, 'analyst', 'agents/analyst.agentic.yml', $2)",
        vec![
            rev.into(),
            json!({ "name": "analyst", "description": marker }).into(),
        ],
    )
    .await;
}

pub(super) fn row(ws: Uuid, rev: Uuid) -> entity::workspace_preview_runs::Model {
    let now = chrono::Utc::now().fixed_offset();
    entity::workspace_preview_runs::Model {
        run_id: RUN.into(),
        workspace_id: ws,
        branch: "feat/x".into(),
        preview_key: "feat_x_abc123".into(),
        revision_id: rev,
        kind: "procedure".into(),
        target_ref: Some("workflows/je.procedure.yml".into()),
        parent_run_id: None,
        options: json!({}),
        state: "running".into(),
        requested_by: None,
        created_at: now,
        started_at: Some(now),
        finished_at: None,
    }
}

pub(super) fn clickhouse_and_airhouse() -> Value {
    json!([
        { "name": "clickhouse", "type": "clickhouse", "host": "http://127.0.0.1:1",
          "user": "default", "database": "default" },
        { "name": "airhouse", "type": "airhouse_managed" }
    ])
}

/// A platform over a working copy with a `config.yml`, no database needed.
pub(super) async fn offline_ctx(withheld: HashSet<String>) -> PreviewPlatformContext {
    let root = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        root.path().join("config.yml"),
        "databases: []\nmodels: []\n",
    )
    .unwrap();
    let manager = oxy::adapters::workspace::builder::WorkspaceBuilder::new(Uuid::new_v4())
        .with_working_copy(root.path(), None, oxy::config::OnMissing::Fail)
        .await
        .expect("config")
        .build()
        .await
        .expect("manager");
    let scope = PreviewScope {
        run_id: RUN.into(),
        preview_key: "k".into(),
        revision_id: Uuid::nil(),
        airway_steps: AirwayStepMode::Hold,
    };
    PreviewPlatformContext::from_parts(
        crate::agentic_wiring::OxyProjectContext::new(manager),
        scope,
        withheld,
    )
}
