//! `AirwayPgStateStore` — implements [`airway::StateStore`] against
//! oxy's SeaORM-managed Postgres.
//!
//! Backs the airway engine's per-pipeline incremental state, schema and audit
//! log with the same database that holds `agentic_runs` and the rest of the
//! platform tables. One row per `(workspace_id, pipeline_name)` in
//! [`crate::extension::workspace_pipeline_state`] — the same key the
//! single-flight lease uses — and one row per load in
//! [`crate::extension::load_audit`].
//!
//! The legacy [`crate::extension::pipeline_state`] is keyed by name alone, so
//! two workspaces running a pipeline of the same name shared one cursor. It is
//! read exactly once per workspace, to adopt, and never written again.
//!
//! Optimistic concurrency: `save` writes `version + 1` only if the row's
//! current `version` still matches `expected_version`. A concurrent writer that
//! bumped it first leaves the second writer's UPDATE matching zero rows, which
//! surfaces as `AirwayError::State`. Mirrors
//! `airway::state::postgres::PostgresStateStore`'s semantics, just via SeaORM
//! instead of raw tokio-postgres.
//!
//! **The engine does not retry that.** `Pipeline::persist` is best-effort: it
//! logs the error and leaves its `state_version` where it was. So a losing
//! writer's cursor is dropped, not reconciled, and the next run re-extracts
//! that window — duplicates rather than gaps, which is the safe direction. It
//! only arises where two runs of one pipeline share a workspace, which the
//! single-flight lease prevents unless the pipeline sets
//! `allow_concurrent_runs: true`.

use std::sync::Arc;

use airway::state::{PipelineState, StateSnapshot, StateStore};
use airway::{AirwayError, Schema};
use async_trait::async_trait;
use chrono::Utc;
use sea_orm::sea_query::OnConflict;
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, ConnectionTrait, DatabaseBackend,
    DatabaseConnection, EntityTrait, QueryFilter, Statement,
};
use uuid::Uuid;

use crate::extension::load_audit::{self, Entity as LoadAuditEntity, status as load_status};
use crate::extension::pipeline_state::Entity as LegacyPipelineStateEntity;
use crate::extension::run_extension::Entity as RunExtEntity;
use crate::extension::workspace_pipeline_state::{
    self as pipeline_state, Column as PipelineStateColumn, Entity as PipelineStateEntity,
};

/// SeaORM-backed [`StateStore`] for one pipeline **in one workspace**.
///
/// Construct one per pipeline run; the worker hands this to airway via
/// [`airway::Pipeline::with_state_store`]. `workspace_id` is the id the
/// executor already used to take the single-flight lease, so a run's lease and
/// its cursor always name the same thing.
#[derive(Clone)]
pub struct AirwayPgStateStore {
    db: Arc<DatabaseConnection>,
    workspace_id: Uuid,
    pipeline_name: String,
}

impl AirwayPgStateStore {
    pub fn new(
        db: Arc<DatabaseConnection>,
        workspace_id: Uuid,
        pipeline_name: impl Into<String>,
    ) -> Self {
        Self {
            db,
            workspace_id,
            pipeline_name: pipeline_name.into(),
        }
    }

    pub fn pipeline_name(&self) -> &str {
        &self.pipeline_name
    }

    pub fn workspace_id(&self) -> Uuid {
        self.workspace_id
    }

    /// This workspace's row, adopting the legacy shared row the first time the
    /// workspace runs this pipeline.
    ///
    /// `airway_pipeline_state` is keyed by name alone, so before this change
    /// every workspace ran off one row. Copying it once — rather than starting
    /// from an empty cursor — means the first run after the deploy continues
    /// from exactly the position it would have used, so no cursor is lost.
    /// `ON CONFLICT DO NOTHING` makes a concurrent second adoption a no-op, and
    /// after this the workspaces diverge.
    ///
    /// One window can still be re-read, but only mid-deploy: if an old pod runs
    /// after a new pod has adopted, it advances the legacy row, and that
    /// progress never reaches the adopted one. Bounded to the deploy, and it
    /// yields duplicates rather than gaps — the safe direction.
    ///
    /// A **tombstone** (a row whose `schema_json` is NULL) is a real row, so a
    /// reset is never undone by re-adopting the legacy cursor.
    async fn current_row(&self) -> Result<Option<pipeline_state::Model>, AirwayError> {
        let key = (self.workspace_id, self.pipeline_name.clone());
        if let Some(row) = PipelineStateEntity::find_by_id(key.clone())
            .one(self.db.as_ref())
            .await
            .map_err(|e| AirwayError::State(format!("load pipeline_state: {e}")))?
        {
            return Ok(Some(row));
        }

        let Some(legacy) = LegacyPipelineStateEntity::find_by_id(self.pipeline_name.clone())
            .one(self.db.as_ref())
            .await
            .map_err(|e| AirwayError::State(format!("load legacy pipeline_state: {e}")))?
        else {
            return Ok(None);
        };

        tracing::info!(
            target: "airway_state",
            workspace_id = %self.workspace_id,
            pipeline = %self.pipeline_name,
            version = legacy.version,
            "adopting the legacy shared pipeline state for this workspace"
        );
        let adopted = pipeline_state::ActiveModel {
            workspace_id: ActiveValue::Set(self.workspace_id),
            pipeline_name: ActiveValue::Set(self.pipeline_name.clone()),
            state: ActiveValue::Set(legacy.state),
            schema_json: ActiveValue::Set(Some(legacy.schema_json)),
            version: ActiveValue::Set(legacy.version),
            updated_at: ActiveValue::Set(Utc::now()),
        };
        PipelineStateEntity::insert(adopted)
            .on_conflict(
                OnConflict::columns([
                    PipelineStateColumn::WorkspaceId,
                    PipelineStateColumn::PipelineName,
                ])
                .do_nothing()
                .to_owned(),
            )
            // `exec_without_returning` rather than `exec`: a conflict here is
            // the expected concurrent-adoption case, and `exec` reports zero
            // inserted rows as an error.
            .exec_without_returning(self.db.as_ref())
            .await
            .map_err(|e| AirwayError::State(format!("adopt legacy pipeline_state: {e}")))?;

        // Re-read rather than trust the copy: whoever won the insert owns the
        // row, and its version is what `save` must match.
        PipelineStateEntity::find_by_id(key)
            .one(self.db.as_ref())
            .await
            .map_err(|e| AirwayError::State(format!("load pipeline_state after adopt: {e}")))
    }
}

#[async_trait]
impl StateStore for AirwayPgStateStore {
    async fn load(&self) -> Result<StateSnapshot, AirwayError> {
        match self.current_row().await? {
            None => Ok(StateSnapshot::default()),
            Some(model) => {
                let state: PipelineState = serde_json::from_value(model.state)
                    .map_err(|e| AirwayError::State(format!("deserialize PipelineState: {e}")))?;
                // NULL schema is a tombstone from a reset, or a row that never
                // provisioned: the same "no schema" an absent row used to mean.
                let schema: Option<Schema> = model
                    .schema_json
                    .map(serde_json::from_value)
                    .transpose()
                    .map_err(|e| AirwayError::State(format!("deserialize Schema: {e}")))?;
                Ok(StateSnapshot {
                    state,
                    schema,
                    version: model.version,
                })
            }
        }
    }

    async fn save(
        &self,
        state: &PipelineState,
        schema: &Schema,
        expected_version: i64,
    ) -> Result<(), AirwayError> {
        let state_json = serde_json::to_value(state)
            .map_err(|e| AirwayError::State(format!("serialize PipelineState: {e}")))?;
        let schema_json = serde_json::to_value(schema)
            .map_err(|e| AirwayError::State(format!("serialize Schema: {e}")))?;
        let new_version = expected_version + 1;

        // Initial insert path: expected_version 0 + no row yet.
        // Use INSERT ... ON CONFLICT DO UPDATE WHERE version = expected.
        // That single round-trip handles both insert and version-checked update.
        let model = pipeline_state::ActiveModel {
            workspace_id: ActiveValue::Set(self.workspace_id),
            pipeline_name: ActiveValue::Set(self.pipeline_name.clone()),
            state: ActiveValue::Set(state_json),
            schema_json: ActiveValue::Set(Some(schema_json)),
            version: ActiveValue::Set(new_version),
            updated_at: ActiveValue::Set(Utc::now()),
        };

        // `update_columns` + `action_and_where` enforces `version =
        // expected_version` as part of the upsert — `DO UPDATE SET … WHERE
        // version = $n`. That WHERE is what makes a reset stick: a tombstone
        // bumps the version, so a run still holding the pre-reset one writes
        // nothing here instead of restoring the cursor the reset discarded.
        //
        // NOT `target_and_where`, which was here before and guarded nothing:
        // it emits `ON CONFLICT (cols) WHERE …`, a *partial-index predicate*
        // for inferring the arbiter index, which Postgres simply ignores
        // against the non-partial primary key. Every save therefore wrote
        // unconditionally, and the re-read that followed compared the version
        // against the one this same statement had just written — so it always
        // matched. Optimistic concurrency was dark on oxy's path; the
        // single-flight lease is what has actually been serializing writers.
        let written = PipelineStateEntity::insert(model)
            .on_conflict(
                OnConflict::columns([
                    PipelineStateColumn::WorkspaceId,
                    PipelineStateColumn::PipelineName,
                ])
                .update_columns([
                    PipelineStateColumn::State,
                    PipelineStateColumn::SchemaJson,
                    PipelineStateColumn::Version,
                    PipelineStateColumn::UpdatedAt,
                ])
                .action_and_where(PipelineStateColumn::Version.eq(expected_version))
                .to_owned(),
            )
            // `exec_without_returning` gives us the row count. `exec` turns the
            // unmet-WHERE case — the conflict we specifically want to report —
            // into an opaque "no records inserted" error.
            .exec_without_returning(self.db.as_ref())
            .await
            .map_err(|e| AirwayError::State(format!("save pipeline_state: {e}")))?;

        if written == 0 {
            // Someone advanced the row first. Re-read so the message names the
            // version airway must reload from. Mirrors
            // `airway::PostgresStateStore`'s defensive check.
            let current =
                PipelineStateEntity::find_by_id((self.workspace_id, self.pipeline_name.clone()))
                    .one(self.db.as_ref())
                    .await
                    .map_err(|e| AirwayError::State(format!("verify save: {e}")))?
                    .map(|row| row.version.to_string())
                    .unwrap_or_else(|| "absent".to_string());
            return Err(AirwayError::State(format!(
                "optimistic concurrency conflict for pipeline `{}` in workspace {}: \
                 expected version {expected_version}, current is {current}",
                self.pipeline_name, self.workspace_id,
            )));
        }
        Ok(())
    }

    async fn record_load_start(
        &self,
        load_id: &str,
        pipeline_name: &str,
        schema_hash: &str,
    ) -> Result<(), AirwayError> {
        // airway hands us `&str` because `Schema.version_hash` is `String`.
        // An empty value means the state store had no prior schema yet
        // (first-ever load for this pipeline) — store as NULL so the
        // audit log distinguishes that from "ran against an empty schema".
        let schema_hash = if schema_hash.is_empty() {
            None
        } else {
            Some(schema_hash.to_string())
        };
        let model = load_audit::ActiveModel {
            load_id: ActiveValue::Set(load_id.to_string()),
            workspace_id: ActiveValue::Set(Some(self.workspace_id)),
            pipeline_name: ActiveValue::Set(pipeline_name.to_string()),
            schema_hash: ActiveValue::Set(schema_hash),
            status: ActiveValue::Set(load_status::IN_PROGRESS.to_string()),
            error_message: ActiveValue::Set(None),
            partial: ActiveValue::Set(false),
            started_at: ActiveValue::Set(Utc::now()),
            finished_at: ActiveValue::Set(None),
        };
        LoadAuditEntity::insert(model)
            .exec(self.db.as_ref())
            .await
            .map_err(|e| AirwayError::State(format!("record_load_start: {e}")))?;
        Ok(())
    }

    async fn record_load_complete(
        &self,
        load_id: &str,
        _table_counts: &std::collections::HashMap<String, usize>,
    ) -> Result<(), AirwayError> {
        // Per-table counts are already on the `LoadCompleted` event;
        // the audit row just records the terminal status + finish ts.
        let existing = LoadAuditEntity::find()
            .filter(load_audit::Column::LoadId.eq(load_id.to_string()))
            .one(self.db.as_ref())
            .await
            .map_err(|e| AirwayError::State(format!("record_load_complete lookup: {e}")))?
            .ok_or_else(|| {
                AirwayError::State(format!("load_audit row not found for `{load_id}`"))
            })?;
        let mut active: load_audit::ActiveModel = existing.into();
        active.status = ActiveValue::Set(load_status::COMPLETED.to_string());
        active.finished_at = ActiveValue::Set(Some(Utc::now()));
        active
            .update(self.db.as_ref())
            .await
            .map_err(|e| AirwayError::State(format!("record_load_complete: {e}")))?;
        Ok(())
    }

    async fn record_load_partial(
        &self,
        load_id: &str,
        _table_counts: &std::collections::HashMap<String, usize>,
    ) -> Result<(), AirwayError> {
        // The run finished (status Completed) but some resources/tables
        // were skipped — `partial = true` distinguishes it from a clean
        // load. Per-table counts ride the events, as with complete.
        let existing = LoadAuditEntity::find()
            .filter(load_audit::Column::LoadId.eq(load_id.to_string()))
            .one(self.db.as_ref())
            .await
            .map_err(|e| AirwayError::State(format!("record_load_partial lookup: {e}")))?
            .ok_or_else(|| {
                AirwayError::State(format!("load_audit row not found for `{load_id}`"))
            })?;
        let mut active: load_audit::ActiveModel = existing.into();
        active.status = ActiveValue::Set(load_status::COMPLETED.to_string());
        active.partial = ActiveValue::Set(true);
        active.finished_at = ActiveValue::Set(Some(Utc::now()));
        active
            .update(self.db.as_ref())
            .await
            .map_err(|e| AirwayError::State(format!("record_load_partial: {e}")))?;
        Ok(())
    }

    async fn record_load_failed(&self, load_id: &str, error: &str) -> Result<(), AirwayError> {
        let existing = LoadAuditEntity::find()
            .filter(load_audit::Column::LoadId.eq(load_id.to_string()))
            .one(self.db.as_ref())
            .await
            .map_err(|e| AirwayError::State(format!("record_load_failed lookup: {e}")))?
            .ok_or_else(|| {
                AirwayError::State(format!("load_audit row not found for `{load_id}`"))
            })?;
        let mut active: load_audit::ActiveModel = existing.into();
        active.status = ActiveValue::Set(load_status::FAILED.to_string());
        active.error_message = ActiveValue::Set(Some(error.to_string()));
        active.finished_at = ActiveValue::Set(Some(Utc::now()));
        active
            .update(self.db.as_ref())
            .await
            .map_err(|e| AirwayError::State(format!("record_load_failed: {e}")))?;
        Ok(())
    }
}

/// Run-scoped [`StateStore`] for a backfill chunk.
///
/// Persists the incremental **cursor** to `airway_run_extensions.resume_state`
/// (keyed by `run_id`) so a reset-in-place retry resumes mid-window instead of
/// re-extracting the whole window. The **schema** and the load-audit log are
/// delegated to the pipeline-global [`AirwayPgStateStore`]: a backfill reads the
/// live schema but never writes it (so it can't clobber/race the live schema),
/// and never advances the live incremental cursor (`resume_state` is per-run).
///
/// Single writer per run, so `save` skips optimistic concurrency on
/// `resume_state` (the pipeline-global schema is never written here).
///
/// NOTE: this is the host seam for mid-window resume (P2c-1). It is inert until
/// the airway engine gains a mid-run persist hook and the Toast source stops
/// freezing the cursor during a backfill (P2c-2/3) — see
/// `docs/plans/airway-midwindow-resume.md`. Wiring it in for backfill runs is
/// safe before then: the source still emits no advanced cursor, so `resume_state`
/// just round-trips an empty cursor and the run re-extracts as it does today.
#[derive(Clone)]
pub struct AirwayRunScopedStateStore {
    db: Arc<DatabaseConnection>,
    run_id: String,
    global: AirwayPgStateStore,
}

impl AirwayRunScopedStateStore {
    pub fn new(
        db: Arc<DatabaseConnection>,
        run_id: impl Into<String>,
        workspace_id: Uuid,
        pipeline_name: impl Into<String>,
    ) -> Self {
        let global = AirwayPgStateStore::new(Arc::clone(&db), workspace_id, pipeline_name);
        Self {
            db,
            run_id: run_id.into(),
            global,
        }
    }

    pub fn run_id(&self) -> &str {
        &self.run_id
    }
}

#[async_trait]
impl StateStore for AirwayRunScopedStateStore {
    async fn load(&self) -> Result<StateSnapshot, AirwayError> {
        // Schema + pipeline identity come from the pipeline-global row.
        let global = self.global.load().await?;
        // Cursor comes from THIS run's resume_state if a prior attempt persisted
        // one; otherwise start with an empty cursor so the source runs from the
        // window start — never inheriting the live incremental position.
        let ext = RunExtEntity::find_by_id(self.run_id.clone())
            .one(self.db.as_ref())
            .await
            .map_err(|e| AirwayError::State(format!("load run_extension: {e}")))?;
        let resume: Option<PipelineState> = ext
            .and_then(|m| m.resume_state)
            .map(serde_json::from_value)
            .transpose()
            .map_err(|e| AirwayError::State(format!("deserialize resume_state: {e}")))?;
        let state = match resume {
            Some(s) => s,
            None => {
                // Keep pipeline identity + schema_version_hash from the global
                // state, but clear the cursors so we don't inherit the live one.
                let mut s = global.state.clone();
                s.resource_states.clear();
                s
            }
        };
        Ok(StateSnapshot {
            state,
            schema: global.schema,
            // Run-scoped single writer: optimistic concurrency not needed.
            version: 0,
        })
    }

    async fn save(
        &self,
        state: &PipelineState,
        _schema: &Schema,
        _expected_version: i64,
    ) -> Result<(), AirwayError> {
        // Persist ONLY the cursor, to this run's resume_state. The schema is
        // deliberately not written — a backfill must not touch the live schema.
        let state_json = serde_json::to_value(state)
            .map_err(|e| AirwayError::State(format!("serialize resume_state: {e}")))?;
        let res = self
            .db
            .as_ref()
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                "UPDATE airway_run_extensions SET resume_state = $1 WHERE run_id = $2",
                [state_json.into(), self.run_id.clone().into()],
            ))
            .await
            .map_err(|e| AirwayError::State(format!("save resume_state: {e}")))?;
        if res.rows_affected() == 0 {
            // The extension row is inserted at run start, so a miss means the run
            // is non-airway or its row was purged — the cursor is silently lost and
            // resume degrades to a full re-extract. Surface it so a future
            // regression (a store keyed off a missing run) doesn't go unnoticed.
            tracing::warn!(
                run_id = %self.run_id,
                "run-scoped resume_state UPDATE matched no airway_run_extensions row"
            );
        }
        Ok(())
    }

    async fn record_load_start(
        &self,
        load_id: &str,
        pipeline_name: &str,
        schema_hash: &str,
    ) -> Result<(), AirwayError> {
        self.global
            .record_load_start(load_id, pipeline_name, schema_hash)
            .await
    }

    async fn record_load_complete(
        &self,
        load_id: &str,
        table_counts: &std::collections::HashMap<String, usize>,
    ) -> Result<(), AirwayError> {
        self.global
            .record_load_complete(load_id, table_counts)
            .await
    }

    async fn record_load_partial(
        &self,
        load_id: &str,
        table_counts: &std::collections::HashMap<String, usize>,
    ) -> Result<(), AirwayError> {
        self.global.record_load_partial(load_id, table_counts).await
    }

    async fn record_load_failed(&self, load_id: &str, error: &str) -> Result<(), AirwayError> {
        self.global.record_load_failed(load_id, error).await
    }
}

// Unit tests for `AirwayPgStateStore` need a real Postgres (the impl
// is a thin SeaORM wrapper, mocking just shadows what SeaORM already
// tests). Real integration coverage lives in the testcontainers suite
// that will follow this in the worker slice.
