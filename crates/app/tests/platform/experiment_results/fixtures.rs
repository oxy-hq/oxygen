//! Seeds. Column types follow C3 and the PR 3 / PR 4 entities; if one of their
//! `Set(..)` arguments does not type-check, match it to the entity's field type.

use chrono::{DateTime, Duration, FixedOffset, NaiveDate, Utc};
use entity::workspaces::WorkspaceStatus;
use entity::{
    experiment_flips, experiment_proposals, experiment_results, experiment_runs, organizations,
    revisions, semantic_views, workspaces,
};
use sea_orm::sea_query::Expr;
use sea_orm::sea_query::OnConflict;
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, ConnectionTrait, DatabaseBackend,
    DatabaseConnection, EntityTrait, FromQueryResult, QueryFilter, Statement, TryInsertResult,
};
use serde_json::Value;
use uuid::Uuid;

pub const GIT_SHA: &str = "abc1234";

pub async fn setup_db() -> DatabaseConnection {
    let (db, _url) = crate::common::fresh_db(crate::common::Schema::Central).await;
    db
}

pub fn now() -> DateTime<FixedOffset> {
    Utc::now().fixed_offset()
}

/// An org, a workspace and one `ready` revision at [`GIT_SHA`]: `(workspace_id, revision_id)`.
pub async fn seed_workspace(db: &DatabaseConnection, label: &str) -> (Uuid, Uuid) {
    let at = now();
    let org_id = Uuid::new_v4();
    organizations::ActiveModel {
        id: Set(org_id),
        name: Set(format!("{label}-org")),
        slug: Set(format!("{label}-{}", org_id.simple())),
        logo: sea_orm::ActiveValue::NotSet,
        logo_content_type: sea_orm::ActiveValue::NotSet,
        created_at: Set(at),
        updated_at: Set(at),
    }
    .insert(db)
    .await
    .expect("seed org");

    let ws_id = Uuid::new_v4();
    workspaces::ActiveModel {
        id: Set(ws_id),
        name: Set(format!("{label}-ws")),
        git_namespace_id: Set(None),
        git_remote_url: Set(None),
        created_at: Set(at),
        updated_at: Set(at),
        path: Set(None),
        last_opened_at: Set(None),
        created_by: Set(None),
        org_id: Set(Some(org_id)),
        status: Set(WorkspaceStatus::Ready),
        error: Set(None),
        monthly_vlm_budget_micros: Set(None),
        current_revision_id: Set(None),
        default_branch: Set(None),
        repo_subdir: Set(None),
    }
    .insert(db)
    .await
    .expect("seed workspace");

    let rev_id = Uuid::new_v4();
    revisions::ActiveModel {
        revision_id: Set(rev_id),
        workspace_id: Set(ws_id),
        git_sha: Set(GIT_SHA.into()),
        branch: Set(Some("main".into())),
        schema_version: Set(1),
        status: Set("ready".into()),
        kind: Set("main".into()),
        owner_user_id: Set(None),
        compiler_version: Set("test".into()),
        started_at: Set(at),
        finished_at: Set(Some(at)),
        file_count_seen: Set(1),
        file_count_compiled: Set(1),
        file_count_failed: Set(0),
        error_summary: Set(None),
    }
    .insert(db)
    .await
    .expect("seed revision");
    (ws_id, rev_id)
}

pub struct RunSeed {
    pub workspace_id: Uuid,
    pub name: String,
    pub state: String,
    pub registered_at: DateTime<FixedOffset>,
    /// The pinned design version's config. `{}` for tests that never estimate.
    pub config: Value,
    pub control_pool: Option<Value>,
    /// Under `assign_by` (C3): group → members, pinned by the run's first check.
    pub group_membership: Option<Value>,
    /// A switchback (C3): the unit set pinned by the run's first check.
    pub fleet: Option<Value>,
    pub observations: Option<Value>,
    pub abort_reason: Option<String>,
    pub retrospective: bool,
}

impl RunSeed {
    pub fn new(workspace_id: Uuid, name: &str, state: &str) -> Self {
        RunSeed {
            workspace_id,
            name: name.into(),
            state: state.into(),
            registered_at: now() - Duration::days(200),
            config: serde_json::json!({}),
            control_pool: None,
            group_membership: None,
            fleet: None,
            observations: None,
            abort_reason: None,
            retrospective: false,
        }
    }
}

/// The experiment (created if new) and `config` as its design, frozen as a run
/// pins it, inserted directly: `(experiment_id, design_id)`. No validation — a fixture
/// that never estimates pins `{}`.
pub async fn seed_design(
    db: &DatabaseConnection,
    ws: Uuid,
    name: &str,
    config: Value,
) -> (Uuid, Uuid) {
    let by = Uuid::new_v4();
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "INSERT INTO experiments (workspace_id, name, hypothesis, owner, created_by) \
         VALUES ($1, $2, 'h', 'ops', $3) ON CONFLICT (workspace_id, name) DO NOTHING",
        [ws.into(), name.into(), by.into()],
    ))
    .await
    .expect("seed experiment");
    let experiment = entity::experiments::Entity::find()
        .filter(entity::experiments::Column::WorkspaceId.eq(ws))
        .filter(entity::experiments::Column::Name.eq(name))
        .one(db)
        .await
        .expect("read experiment")
        .expect("the experiment");
    let design = Uuid::new_v4();
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "INSERT INTO experiment_designs (id, experiment_id, config, content_hash, created_by, \
           updated_by, frozen_at) \
         VALUES ($1, $2, $3, md5($3::text), $4, $4, now())",
        [
            design.into(),
            experiment.id.into(),
            config.into(),
            by.into(),
        ],
    ))
    .await
    .expect("seed design");
    (experiment.id, design)
}

pub async fn seed_run(db: &DatabaseConnection, s: RunSeed) -> Uuid {
    let id = Uuid::new_v4();
    let at = now();
    let (experiment_id, design_id) = seed_design(db, s.workspace_id, &s.name, s.config).await;
    experiment_runs::ActiveModel {
        id: Set(id),
        created_at: Set(at),
        workspace_id: Set(s.workspace_id),
        experiment_id: Set(experiment_id),
        design_id: Set(design_id),
        registered_by: Set(Uuid::new_v4()),
        registered_at: Set(s.registered_at),
        control_pool: Set(s.control_pool),
        group_membership: Set(s.group_membership),
        fleet: Set(s.fleet),
        state: Set(s.state),
        next_check_at: Set(at),
        check_enqueued_at: Set(None),
        estimate_task_id: Set(None),
        closing_since: Set(None),
        abort_reason: Set(s.abort_reason),
        abort_levers: Set(None),
        aborted_on: Set(None),
        guardrail_breach: Set(None),
        observations: Set(s.observations),
        retrospective: Set(s.retrospective),
        updated_at: Set(at),
    }
    .insert(db)
    .await
    .expect("seed run");
    id
}

/// Insert a results row; `false` when the unique `run_id` made it a conflict.
pub async fn insert_result_row(
    db: &DatabaseConnection,
    run_id: Uuid,
    aborted: bool,
    report: Value,
    coefficient_patch: Option<String>,
) -> bool {
    insert_result_row_with(db, run_id, aborted, report, coefficient_patch, None).await
}

/// As [`insert_result_row`], with the `prediction` column.
pub async fn insert_result_row_with(
    db: &DatabaseConnection,
    run_id: Uuid,
    aborted: bool,
    report: Value,
    coefficient_patch: Option<String>,
    prediction: Option<Value>,
) -> bool {
    let row = experiment_results::ActiveModel {
        id: Set(Uuid::new_v4()),
        run_id: Set(run_id),
        effect: Set(serde_json::json!({})),
        ratio: Set(None),
        prediction: Set(prediction),
        // A row as sealed before the day-by-day series existed.
        series: Set(None),
        compliance: Set(serde_json::json!({})),
        report: Set(report),
        coefficient_patch: Set(coefficient_patch),
        aborted: Set(aborted),
        created_at: Set(now()),
    };
    let res = experiment_results::Entity::insert(row)
        .on_conflict(
            OnConflict::column(experiment_results::Column::RunId)
                .do_nothing()
                .to_owned(),
        )
        .try_insert()
        .exec(db)
        .await
        .expect("insert result row");
    !matches!(res, TryInsertResult::Conflicted)
}

// ---- the estimate job (Task 7). The design comes from the run's pinned version.

/// A second (or later) `main` revision of a workspace.
pub async fn seed_revision(db: &DatabaseConnection, ws: Uuid, git_sha: &str) -> Uuid {
    let at = now();
    let id = Uuid::new_v4();
    revisions::ActiveModel {
        revision_id: Set(id),
        workspace_id: Set(ws),
        git_sha: Set(git_sha.into()),
        branch: Set(Some("main".into())),
        schema_version: Set(1),
        status: Set("ready".into()),
        kind: Set("main".into()),
        owner_user_id: Set(None),
        compiler_version: Set("test".into()),
        started_at: Set(at),
        finished_at: Set(Some(at)),
        file_count_seen: Set(1),
        file_count_compiled: Set(1),
        file_count_failed: Set(0),
        error_summary: Set(None),
    }
    .insert(db)
    .await
    .expect("seed revision");
    id
}

/// Make `rev` the workspace's promoted revision.
pub async fn promote(db: &DatabaseConnection, ws: Uuid, rev: Uuid) {
    workspaces::Entity::update_many()
        .col_expr(workspaces::Column::CurrentRevisionId, Expr::value(rev))
        .filter(workspaces::Column::Id.eq(ws))
        .exec(db)
        .await
        .expect("promote revision");
}

/// The compiled `.view.yml` row: the PARSED definition and its path — no source text, as in production.
pub async fn seed_view(
    db: &DatabaseConnection,
    rev: Uuid,
    name: &str,
    file_path: &str,
    definition: Value,
) {
    semantic_views::ActiveModel {
        revision_id: Set(rev),
        name: Set(name.into()),
        file_path: Set(file_path.into()),
        definition: Set(definition),
        compiled_sql_blob_key: Set(None),
    }
    .insert(db)
    .await
    .expect("seed view");
}

pub struct FlipSeed<'a> {
    pub run: Uuid,
    pub unit: &'a str,
    pub state: &'a str,
    /// C3: `pending | dispatched | confirmed | unconfirmed | failed | cancelled`.
    pub status: &'a str,
    pub confirmed_on: Option<NaiveDate>,
    pub reason: Option<&'a str>,
    pub lever_revision: Option<Uuid>,
}

pub async fn seed_flip(db: &DatabaseConnection, f: FlipSeed<'_>) {
    experiment_flips::ActiveModel {
        id: Set(Uuid::new_v4()),
        created_at: Set(now()),
        run_id: Set(f.run),
        unit: Set(f.unit.into()),
        target_state: Set(f.state.into()),
        due_at: Set(NaiveDate::from_ymd_opt(2026, 3, 2)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap()),
        wave: Set(0),
        task_id: Set(None),
        agentic_run_id: Set(None),
        lever_revision_id: Set(f.lever_revision),
        status: Set(f.status.into()),
        confirmed_on: Set(f.confirmed_on.map(|d| d.and_hms_opt(0, 0, 0).unwrap())),
        reason: Set(f.reason.map(str::to_string)),
    }
    .insert(db)
    .await
    .expect("seed flip");
}

/// A proposal row in `status`, created `days_before_now` days ago.
/// It priced the experiment's design, when one exists.
pub async fn seed_proposal(
    db: &DatabaseConnection,
    ws: Uuid,
    name: &str,
    status: &str,
    days_before_now: i64,
) {
    let at = now() - Duration::days(days_before_now);
    let design_id = latest_design(db, ws, name).await;
    experiment_proposals::ActiveModel {
        id: Set(Uuid::new_v4()),
        created_at: Set(at),
        workspace_id: Set(ws),
        name: Set(name.into()),
        design_id: Set(design_id),
        design_hash: Set(None),
        request_id: Set(Uuid::new_v4()),
        status: Set(status.into()),
        request: Set(serde_json::json!({})),
        alpha: Set(0.05),
        seed: Set(Some(1)),
        waves: Set(None),
        periods: Set(None),
        strata: Set(None),
        assign_by: Set(None),
        power: Set(None),
        mde_unit_day: Set(None),
        prior_coefficient: Set(None),
        warnings: Set(serde_json::json!([])),
        refusal: Set(None),
        requested_by: Set(Uuid::new_v4()),
        finished_at: Set(Some(at)),
    }
    .insert(db)
    .await
    .expect("seed proposal");
}

async fn latest_design(db: &DatabaseConnection, ws: Uuid, name: &str) -> Option<Uuid> {
    #[derive(sea_orm::FromQueryResult)]
    struct Row {
        id: Uuid,
    }
    Row::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "SELECT d.id FROM experiment_designs d JOIN experiments e ON e.id = d.experiment_id \
         WHERE e.workspace_id = $1 AND e.name = $2",
        [ws.into(), name.into()],
    ))
    .one(db)
    .await
    .expect("read latest design")
    .map(|r| r.id)
}
