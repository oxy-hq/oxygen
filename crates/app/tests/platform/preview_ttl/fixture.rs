//! The TTL tests' fixture: a workspace in a fresh Oxy database, in-process
//! DuckDB standing in for its Airhouse, and helpers that drive the sweep and
//! the drop task the way the worker fleet would.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agentic_core::delegation::{TaskAssignment, TaskOutcome, TaskSpec};
use agentic_runtime::worker::TaskExecutor;
use airhouse::preview_sql::{PreviewNamespace, ShadowState};
use entity::workspaces::WorkspaceStatus;
use entity::{organizations, users, workspaces};
use oxy_app::server::previews::ddl::OpenSchemaDropper;
use oxy_app::server::previews::ddl_duckdb::{DuckDbDroppers, DuckDbPreviewDdl};
use oxy_app::server::previews::drop::PreviewSchemaDropExecutor;
use oxy_app::server::previews::maintenance::{DropClaim, sweep_expired};
use oxy_app::server::previews::registry;
use sea_orm::{
    ActiveModelTrait, ActiveValue, ConnectionTrait, DatabaseBackend, DatabaseConnection,
    EntityTrait, Statement,
};
use uuid::Uuid;

pub(super) const BRANCH: &str = "feat/je-v2";
pub(super) const TTL: Duration = Duration::from_secs(72 * 3600);

pub(super) struct Fx {
    pub db: DatabaseConnection,
    pub ws: Uuid,
    pub user: Uuid,
    pub ns: PreviewNamespace,
    pub duck: Arc<Mutex<duckdb::Connection>>,
}

pub(super) async fn setup() -> Fx {
    let db = crate::common::test_db_with(crate::common::Schema::All).await;
    let now = chrono::Utc::now().fixed_offset();
    let org = Uuid::new_v4();
    organizations::ActiveModel {
        id: ActiveValue::Set(org),
        name: ActiveValue::Set("acme".into()),
        slug: ActiveValue::Set(format!("acme-{}", org.simple())),
        logo: ActiveValue::NotSet,
        logo_content_type: ActiveValue::NotSet,
        created_at: ActiveValue::Set(now),
        updated_at: ActiveValue::Set(now),
    }
    .insert(&db)
    .await
    .expect("seed org");
    let ws = workspaces::ActiveModel {
        id: ActiveValue::Set(Uuid::new_v4()),
        name: ActiveValue::Set("ws".into()),
        org_id: ActiveValue::Set(Some(org)),
        status: ActiveValue::Set(WorkspaceStatus::Ready),
        ..Default::default()
    }
    .insert(&db)
    .await
    .expect("seed workspace")
    .id;
    let user = users::ActiveModel {
        id: ActiveValue::Set(Uuid::new_v4()),
        email: ActiveValue::Set(Some("staff@oxy.test".into())),
        name: ActiveValue::Set("Staff".into()),
        picture: ActiveValue::Set(None),
        email_verified: ActiveValue::Set(true),
        ..Default::default()
    }
    .insert(&db)
    .await
    .expect("seed user")
    .id;
    Fx {
        db,
        ws,
        user,
        ns: PreviewNamespace::for_branch(ws, BRANCH),
        duck: Arc::new(Mutex::new(duckdb::Connection::open_in_memory().unwrap())),
    }
}

impl Fx {
    pub fn key(&self) -> &str {
        self.ns.key()
    }

    pub fn ddl(&self) -> DuckDbPreviewDdl {
        DuckDbPreviewDdl::new(self.duck.clone(), self.ns.clone())
    }

    pub fn droppers(&self) -> Arc<dyn OpenSchemaDropper> {
        Arc::new(DuckDbDroppers {
            conn: self.duck.clone(),
        })
    }

    /// What a preview run's first write into `live` does: register, then
    /// create, `preview_<key>__<live>`.
    pub async fn ensure(&self, live: &str) -> String {
        registry::ensure_schema(&self.db, &self.ddl(), self.ws, &self.ns, live, "run-1", TTL)
            .await
            .expect("ensure the preview schema")
    }

    pub fn duck_exec(&self, sql: &str) {
        self.duck.lock().unwrap().execute_batch(sql).expect(sql);
    }

    pub fn duck_schemas(&self) -> BTreeSet<String> {
        let conn = self.duck.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT schema_name FROM information_schema.schemata")
            .unwrap();
        stmt.query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    }

    pub fn duck_count(&self, table: &str) -> i64 {
        let conn = self.duck.lock().unwrap();
        conn.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
            .unwrap()
    }

    /// One sweep pass as if the clock read `hours` from now.
    pub async fn sweep_after(&self, hours: i64) -> Vec<DropClaim> {
        self.sweep_at(chrono::Utc::now() + chrono::Duration::hours(hours))
            .await
    }

    pub async fn sweep_at(&self, now: chrono::DateTime<chrono::Utc>) -> Vec<DropClaim> {
        sweep_expired(&self.db, now).await.expect("sweep")
    }

    /// The spec the sweep queued for `run_id`, as the worker would claim it.
    pub async fn queued_spec(&self, run_id: &str) -> TaskSpec {
        let row = self
            .db
            .query_one_raw(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                "SELECT spec FROM agentic_task_queue WHERE task_id = $1 AND run_id = $1",
                [run_id.into()],
            ))
            .await
            .unwrap()
            .expect("the drop is queued");
        serde_json::from_value(row.try_get::<serde_json::Value>("", "spec").unwrap()).unwrap()
    }

    /// Run the queued drop of `claim` and record its outcome on the run row,
    /// as the coordinator does.
    pub async fn run_drop(
        &self,
        droppers: Arc<dyn OpenSchemaDropper>,
        claim: &DropClaim,
    ) -> TaskOutcome {
        let spec = self.queued_spec(&claim.run_id).await;
        self.run_task(droppers, &claim.run_id, spec).await
    }

    pub async fn run_task(
        &self,
        droppers: Arc<dyn OpenSchemaDropper>,
        run_id: &str,
        spec: TaskSpec,
    ) -> TaskOutcome {
        let exec = PreviewSchemaDropExecutor {
            db: self.db.clone(),
            droppers,
        };
        let mut task = exec
            .execute(TaskAssignment {
                task_id: run_id.into(),
                parent_task_id: None,
                run_id: run_id.into(),
                spec,
                policy: None,
            })
            .await
            .expect("the executor accepts its own kind");
        let outcome = task.outcomes.recv().await.expect("an outcome");
        match &outcome {
            TaskOutcome::Done { answer, metadata } => {
                agentic_runtime::crud::update_run_done(&self.db, run_id, answer, metadata.clone())
                    .await
                    .unwrap()
            }
            TaskOutcome::Failed(error) => {
                agentic_runtime::crud::update_run_failed(&self.db, run_id, error)
                    .await
                    .unwrap()
            }
            other => panic!("unexpected outcome {other:?}"),
        }
        outcome
    }

    pub async fn row(&self, schema: &str) -> entity::workspace_preview_schemas::Model {
        entity::workspace_preview_schemas::Entity::find_by_id((self.ws, schema.to_string()))
            .one(&self.db)
            .await
            .unwrap()
            .expect("registry row")
    }

    pub async fn exec(&self, sql: &str, values: Vec<sea_orm::Value>) {
        self.db
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                sql,
                values,
            ))
            .await
            .expect(sql);
    }

    pub async fn count(&self, sql: &str, values: Vec<sea_orm::Value>) -> i64 {
        self.db
            .query_one_raw(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                sql,
                values,
            ))
            .await
            .unwrap()
            .unwrap()
            .try_get("", "n")
            .unwrap()
    }

    /// The text column `s` of a one-row query (NULL reads as `NULL`).
    pub async fn text(&self, sql: &str, values: Vec<sea_orm::Value>) -> String {
        let row = self
            .db
            .query_one_raw(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                sql,
                values,
            ))
            .await
            .unwrap()
            .unwrap();
        row.try_get::<Option<String>>("", "s")
            .unwrap()
            .unwrap_or_else(|| "NULL".into())
    }

    /// A pipeline's Airway state row and lease, the lease lapsed or not.
    pub async fn seed_airway(&self, pipeline: &str, lease_expired: bool) {
        self.exec(
            "INSERT INTO airway_workspace_pipeline_state (workspace_id, pipeline_name, state) \
             VALUES ($1, $2, '{}'::jsonb)",
            vec![self.ws.into(), pipeline.into()],
        )
        .await;
        let expires = if lease_expired { "-1" } else { "1" };
        self.exec(
            &format!(
                "INSERT INTO airway_pipeline_leases \
                     (workspace_id, pipeline_name, run_id, expires_at) \
                 VALUES ($1, $2, 'some-run', now() + interval '{expires} hour')"
            ),
            vec![self.ws.into(), pipeline.into()],
        )
        .await;
    }

    /// Rows of an Airway table (`airway_workspace_pipeline_state`,
    /// `airway_pipeline_leases`) for these pipeline names.
    pub async fn airway_rows(&self, table: &str, names: &[&str]) -> i64 {
        let names: Vec<String> = names.iter().map(|s| s.to_string()).collect();
        self.count(
            &format!(
                "SELECT count(*)::bigint AS n FROM {table} \
                 WHERE workspace_id = $1 AND pipeline_name = ANY($2)"
            ),
            vec![self.ws.into(), names.into()],
        )
        .await
    }

    /// Record `tables` of `live` in the preview's shadow map, as a step that
    /// created them would.
    pub async fn record(&self, live: &str, tables: &[&str]) {
        let updates: Vec<_> = tables
            .iter()
            .map(|t| ((live.to_string(), t.to_string()), ShadowState::Shadow))
            .collect();
        registry::upsert_shadow(&self.db, self.ws, self.key(), "run-1", &updates)
            .await
            .unwrap();
    }

    /// A preview run of this key, shaped as S9 makes one
    /// (`previews::runs::{submit, advance}`): its `workspace_preview_runs` row
    /// in `state`, and — once the queue started it — its `agentic_runs` row in
    /// `task_status`. A run still `queued` has no `agentic_runs` row: pass
    /// `None`.
    pub async fn preview_run(
        &self,
        run_id: &str,
        kind: &str,
        state: &str,
        task_status: Option<&str>,
    ) {
        self.exec(
            "INSERT INTO workspace_preview_runs \
                 (run_id, workspace_id, branch, preview_key, revision_id, kind, state) \
             VALUES ($1, $2, $3, $4, $5, $6, $7) \
             ON CONFLICT (run_id) DO UPDATE SET state = EXCLUDED.state",
            vec![
                run_id.into(),
                self.ws.into(),
                BRANCH.into(),
                self.key().into(),
                Uuid::new_v4().into(),
                kind.into(),
                state.into(),
            ],
        )
        .await;
        let Some(task_status) = task_status else {
            return;
        };
        let exists = self
            .count(
                "SELECT count(*)::bigint AS n FROM agentic_runs WHERE id = $1",
                vec![run_id.into()],
            )
            .await;
        if exists == 0 {
            agentic_runtime::crud::insert_run(
                &self.db,
                run_id,
                "preview run",
                None,
                kind,
                None,
                self.ws,
            )
            .await
            .unwrap();
        }
        self.exec(
            "UPDATE agentic_runs SET task_status = $2 WHERE id = $1",
            vec![run_id.into(), task_status.into()],
        )
        .await;
    }
}
