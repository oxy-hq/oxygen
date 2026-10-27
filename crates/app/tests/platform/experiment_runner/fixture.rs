//! Seeds: an org with an owner member, a workspace whose promoted revision
//! carries the two lever automations, the experiment and its design as version
//! records; and readers for what the runner wrote. Shared by every module of this group, so a helper one module
//! does not use is not dead code.

use std::collections::BTreeMap;

use chrono::{DateTime, NaiveDate, Utc};
use entity::{
    automation_definitions, experiment_flips, experiment_runs, notifications, org_members,
    organizations, revisions, users, workspaces,
};
use sea_orm::sea_query::Expr;
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, ConnectionTrait, DatabaseBackend,
    DatabaseConnection, EntityTrait, FromQueryResult, QueryFilter, QueryOrder, Statement,
};
use serde_json::json;
use uuid::Uuid;

pub(crate) const OWNER_EMAIL: &str = "owner@acme.test";
pub(crate) const NAME: &str = "beverage_promo_pilot";

pub(crate) const DESIGN: &str = r#"
experiment: beverage_promo_pilot
hypothesis: "A beverage add-on promo lifts beverage revenue per store-day"
owner: owner@acme.test
for_each: store
lever:
  on: { via: promo_on }
  off: { via: promo_off }
  read_back:
    when_on: "{{store_days.promo_active}} > 0"
    when_off: "{{store_days.promo_active}} == 0"
    grace_days: 2
  on_close: off
design:
  timezone: Australia/Sydney
  waves:
    - { switch_on: 2026-03-02, units: [bondi, newtown] }
    - { switch_on: 2026-03-16, units: [manly, surry_hills] }
  seed: 8841207
  pre_days: 56
  post_days: 56
  anticipation_days: 3
  washout_days: 7
outcome:
  target: store_days.beverages_revenue
  family: 1
  direction: increase
  mde_at_proposal: 17.5
guardrails:
  - criteria: { type: trigger, when: "{{store_days.labor_cost_pct}} > 0.34" }
    action: abort
"#;

pub(crate) const LEVERS: &[&str] = &["promo_on", "promo_off"];

pub(crate) fn utc(s: &str) -> DateTime<Utc> {
    s.parse().expect("an RFC 3339 instant")
}

pub(crate) fn d(s: &str) -> NaiveDate {
    s.parse().expect("a YYYY-MM-DD date")
}

/// The first local instant of `day` in Sydney — PR 2's `day_start_utc`, never re-derived.
pub(crate) fn switch_at(day: &str) -> DateTime<Utc> {
    oxy_experiment::timeline::day_start_utc(d(day), chrono_tz::Australia::Sydney)
}

/// A lever automation: one `execute_sql` task that records the requested state.
/// It reads `unit` and, in a comment, `units`, so both `per:` shapes register.
pub(crate) fn lever(name: &str) -> serde_json::Value {
    json!({
        "name": name,
        "variables": { "unit": "", "state": "", "experiment": "", "wave": 0 },
        "tasks": [{
            "name": "flip",
            "type": "execute_sql",
            "database": "local",
            "sql_query": "INSERT INTO promo_events (store, state) \
                          VALUES ('{{ unit }}', '{{ state }}') RETURNING store, state \
                          /* units: {{ units }} */",
        }],
    })
}

pub(crate) struct Fx {
    pub db: DatabaseConnection,
    pub org: Uuid,
    pub owner: Uuid,
    pub ws: Uuid,
}

#[derive(FromQueryResult)]
struct Count {
    n: i64,
}

impl Fx {
    /// The design written as version 1 on 2026-02-01 — well before the
    /// 2026-02-26T13:00Z deadline (start of 2026-02-27 in Sydney).
    pub(crate) async fn seed(design: &str) -> Fx {
        Fx::seed_full(utc("2026-02-01T00:00:00Z"), design, LEVERS).await
    }

    /// In time, but with only `levers` in the promoted revision.
    pub(crate) async fn seed_with_levers(design: &str, levers: &[&str]) -> Fx {
        Fx::seed_full(utc("2026-02-01T00:00:00Z"), design, levers).await
    }

    async fn seed_full(compiled_at: DateTime<Utc>, design: &str, levers: &[&str]) -> Fx {
        let (db, _url) = crate::common::fresh_db(crate::common::Schema::All).await;
        let org = seed_org(&db).await;
        let owner = seed_member(&db, org, OWNER_EMAIL, org_members::OrgRole::Owner).await;
        let ws = seed_workspace(&db, org).await;
        let fx = Fx { db, org, owner, ws };
        fx.promote(compiled_at, design, levers).await;
        fx
    }

    /// A new ready main revision carrying `levers`, promoted, and `design`
    /// written as the experiment's next version.
    pub(crate) async fn promote(&self, at: DateTime<Utc>, design: &str, levers: &[&str]) -> Uuid {
        let rev = seed_revision(&self.db, self.ws, at).await;
        for name in levers {
            seed_lever(&self.db, rev, name).await;
        }
        seed_design(&self.db, self.ws, self.owner, design).await;
        workspaces::Entity::update_many()
            .col_expr(workspaces::Column::CurrentRevisionId, Expr::value(rev))
            .filter(workspaces::Column::Id.eq(self.ws))
            .exec(&self.db)
            .await
            .expect("promote revision");
        rev
    }

    /// A promoted revision carrying only `levers`; the design is untouched.
    pub(crate) async fn promote_bare(&self, at: DateTime<Utc>, levers: &[&str]) -> Uuid {
        let rev = seed_revision(&self.db, self.ws, at).await;
        for name in levers {
            seed_lever(&self.db, rev, name).await;
        }
        workspaces::Entity::update_many()
            .col_expr(workspaces::Column::CurrentRevisionId, Expr::value(rev))
            .filter(workspaces::Column::Id.eq(self.ws))
            .exec(&self.db)
            .await
            .expect("promote revision");
        rev
    }

    pub(crate) async fn run_opt(&self) -> Option<experiment_runs::Model> {
        experiment_runs::Entity::find()
            .filter(experiment_runs::Column::WorkspaceId.eq(self.ws))
            .one(&self.db)
            .await
            .expect("read run")
    }

    pub(crate) async fn run(&self) -> experiment_runs::Model {
        self.run_opt().await.expect("a run row")
    }

    /// The workspace's promoted revision.
    pub(crate) async fn head(&self) -> Uuid {
        workspaces::Entity::find_by_id(self.ws)
            .one(&self.db)
            .await
            .expect("read workspace")
            .and_then(|w| w.current_revision_id)
            .expect("a promoted revision")
    }

    pub(crate) async fn flips(&self) -> Vec<experiment_flips::Model> {
        let run = self.run().await;
        experiment_flips::Entity::find()
            .filter(experiment_flips::Column::RunId.eq(run.id))
            .order_by_asc(experiment_flips::Column::Unit)
            .order_by_asc(experiment_flips::Column::TargetState)
            .all(&self.db)
            .await
            .expect("read flips")
    }

    pub(crate) async fn flip(&self, unit: &str, state: &str) -> experiment_flips::Model {
        self.flips()
            .await
            .into_iter()
            .find(|f| f.unit == unit && f.target_state == state)
            .unwrap_or_else(|| panic!("no {state} flip for {unit}"))
    }

    /// Lever runs the runner seeded for (unit, state) — `agentic_runs` rows whose
    /// metadata says `trigger = experiment`.
    pub(crate) async fn lever_runs(&self, unit: &str, state: &str) -> i64 {
        Count::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT COUNT(*) AS n FROM agentic_runs WHERE workspace_id = $1 \
               AND metadata->>'trigger' = 'experiment' \
               AND metadata->'variables'->>'unit' = $2 \
               AND metadata->'variables'->>'state' = $3",
            [self.ws.into(), unit.into(), state.into()],
        ))
        .one(&self.db)
        .await
        .expect("count lever runs")
        .map_or(0, |c| c.n)
    }

    pub(crate) async fn queued_kind(&self, kind: &str) -> i64 {
        Count::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT COUNT(*) AS n FROM agentic_task_queue q JOIN agentic_runs r ON r.id = q.run_id \
             WHERE r.workspace_id = $1 AND q.spec->>'type' = 'custom' AND q.spec->>'kind' = $2",
            [self.ws.into(), kind.into()],
        ))
        .one(&self.db)
        .await
        .expect("count queued jobs")
        .map_or(0, |c| c.n)
    }

    /// Register the latest version (well before its deadline) and return the run.
    pub(crate) async fn register(&self) -> experiment_runs::Model {
        self.register_at(utc("2026-02-10T00:00:00Z"))
            .await
            .expect("register")
    }

    /// Register the draft at `at`, as the owner.
    pub(crate) async fn register_at(
        &self,
        at: DateTime<Utc>,
    ) -> Result<experiment_runs::Model, oxy_app::server::experiment::register::RegisterError> {
        use oxy_app::server::experiment::register;
        let experiment = self.experiment().await;
        register::register(&self.db, &experiment, None, self.owner, at).await
    }

    /// A retrospective run of the draft, created at `at` by the owner.
    pub(crate) async fn retrospective_at(
        &self,
        at: DateTime<Utc>,
    ) -> Result<experiment_runs::Model, oxy_app::server::experiment::register::RegisterError> {
        use oxy_app::server::experiment::retrospective;
        let experiment = self.experiment().await;
        retrospective::create(&self.db, &experiment, None, self.owner, at).await
    }

    /// The experiment's design, decoded.
    pub(crate) async fn latest_config(&self) -> oxy_experiment::ExperimentConfig {
        use oxy_app::server::experiment::design_store;
        let row = design_store::design_of(&self.db, self.experiment().await.id)
            .await
            .expect("read the design")
            .expect("a design");
        design_store::decode(row).expect("decode").config
    }

    /// The workspace's first experiment by name — the one the fixture seeded,
    /// whatever its design calls it.
    pub(crate) async fn experiment(&self) -> entity::experiments::Model {
        oxy_app::server::experiment::design_store::list_experiments(&self.db, self.ws)
            .await
            .expect("read experiments")
            .into_iter()
            .next()
            .expect("the experiment")
    }

    pub(crate) async fn notices(&self) -> Vec<notifications::Model> {
        notifications::Entity::find()
            .filter(notifications::Column::Kind.eq("experiment"))
            .order_by_asc(notifications::Column::CreatedAt)
            .all(&self.db)
            .await
            .expect("read notices")
    }

    /// `GET /run`'s body at `at`.
    pub(crate) async fn status_at(
        &self,
        at: DateTime<Utc>,
    ) -> oxy_app::server::experiment::status::RunStatusBody {
        oxy_app::server::experiment::status::run_status(&self.db, self.ws, NAME, at)
            .await
            .expect("status")
            .expect("a status")
    }

    /// Make the run due at `at` (as an abort or a breach would).
    pub(crate) async fn make_due(&self, at: DateTime<Utc>) {
        self.exec(
            "UPDATE experiment_runs SET next_check_at = $1 WHERE workspace_id = $2",
            [at.fixed_offset().into(), self.ws.into()],
        )
        .await;
    }

    /// Mark every running lever run as `status` (what the worker would do).
    pub(crate) async fn finish_levers(&self, status: &str) {
        self.exec(
            "UPDATE agentic_runs SET task_status = $1 WHERE workspace_id = $2 \
               AND metadata->>'trigger' = 'experiment' AND task_status = 'running'",
            [status.into(), self.ws.into()],
        )
        .await;
    }

    /// Make the run due at `at`, then tick at `at`.
    pub(crate) async fn tick(&self, at: DateTime<Utc>) -> usize {
        self.make_due(at).await;
        oxy_app::server::experiment::tick::tick_experiments(&self.db, at).await
    }

    /// The `agentic_runs.metadata` of the lever run for (unit, state).
    pub(crate) async fn lever_metadata(&self, unit: &str, state: &str) -> serde_json::Value {
        #[derive(FromQueryResult)]
        struct Meta {
            metadata: serde_json::Value,
        }
        Meta::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT metadata FROM agentic_runs WHERE workspace_id = $1 \
               AND metadata->>'trigger' = 'experiment' \
               AND metadata->'variables'->>'unit' = $2 \
               AND metadata->'variables'->>'state' = $3",
            [self.ws.into(), unit.into(), state.into()],
        ))
        .one(&self.db)
        .await
        .expect("read lever metadata")
        .expect("a lever run")
        .metadata
    }

    /// Pin the control pool (and under `assign_by` every group's members) as
    /// the first check would.
    pub(crate) async fn pin(&self, pool: &[&str], membership: Option<&[(&str, &[&str])]>) {
        let membership = membership.map(|m| {
            let m: BTreeMap<&str, &[&str]> = m.iter().copied().collect();
            json!(m)
        });
        self.exec(
            "UPDATE experiment_runs SET control_pool = $1, group_membership = $2 WHERE workspace_id = $3",
            [json!(pool).into(), membership.into(), self.ws.into()],
        )
        .await;
    }

    /// Pin a switchback's fleet as the first check would (`experiment_runs.fleet`).
    pub(crate) async fn pin_fleet(&self, units: &[&str]) {
        self.exec(
            "UPDATE experiment_runs SET fleet = $1 WHERE workspace_id = $2",
            [json!(units).into(), self.ws.into()],
        )
        .await;
    }

    /// The variables of every `per: wave` lever run for `state`, oldest first.
    pub(crate) async fn wave_runs(&self, state: &str) -> Vec<serde_json::Value> {
        #[derive(FromQueryResult)]
        struct Vars {
            variables: serde_json::Value,
        }
        Vars::find_by_statement(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT metadata->'variables' AS variables FROM agentic_runs WHERE workspace_id = $1 \
               AND metadata->>'trigger' = 'experiment' \
               AND metadata->'variables'->>'state' = $2 \
               AND jsonb_exists(metadata->'variables', 'units') ORDER BY created_at",
            [self.ws.into(), state.into()],
        ))
        .all(&self.db)
        .await
        .expect("read wave runs")
        .into_iter()
        .map(|v| v.variables)
        .collect()
    }

    /// Insert unsent rows as a crashed replica would have left them.
    pub(crate) async fn orphan(&self, units: &[&str], state: &str, wave: i32, due: &str) {
        let run = self.run().await;
        for unit in units {
            self.exec(
                "INSERT INTO experiment_flips \
                   (id, run_id, unit, target_state, due_at, wave, status, created_at) \
                 VALUES ($1, $2, $3, $4, $5, $6, 'pending', $7)",
                [
                    Uuid::new_v4().into(),
                    run.id.into(),
                    (*unit).into(),
                    state.into(),
                    d(due).into(),
                    wave.into(),
                    switch_at(due).fixed_offset().into(),
                ],
            )
            .await;
        }
    }

    /// The design the run pinned.
    pub(crate) async fn design(&self) -> oxy_experiment::ExperimentConfig {
        oxy_app::server::experiment::store::pinned_config(&self.db, &self.run().await)
            .await
            .expect("pinned design")
    }
    pub(crate) async fn exec<const N: usize>(&self, sql: &str, values: [sea_orm::Value; N]) {
        self.db
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                sql,
                values,
            ))
            .await
            .unwrap_or_else(|e| panic!("{sql}: {e}"));
    }
}

pub(crate) const SWITCHBACK_NAME: &str = "beverage_promo_switchback";

/// The spec's switchback — on, off, off, on in weekly periods — one task per
/// period boundary for the whole fleet.
pub(crate) const SWITCHBACK: &str = r#"
experiment: beverage_promo_switchback
hypothesis: "A beverage add-on promo lifts beverage revenue per store-day"
owner: owner@acme.test
for_each: store
lever:
  on: { via: promo_on }
  off: { via: promo_off }
  per: wave
  read_back:
    when_on: "{{store_days.promo_active}} > 0"
    when_off: "{{store_days.promo_active}} == 0"
    grace_days: 2
  on_close: off
design:
  timezone: Australia/Sydney
  switchback:
    period_days: 7
    periods:
      - { from: 2026-03-02, state: on }
      - { from: 2026-03-09, state: off }
      - { from: 2026-03-16, state: off }
      - { from: 2026-03-23, state: on }
  seed: 8841207
  washout_days: 2
outcome:
  target: store_days.beverages_revenue
  direction: increase
  alpha: 0.2
  mde_at_proposal: 9.0
"#;

pub(crate) const GROUPED_NAME: &str = "beverage_promo_regions";

/// Regions on or off together (`assign_by`), one task per wave.
pub(crate) const GROUPED: &str = r#"
experiment: beverage_promo_regions
hypothesis: "A beverage add-on promo lifts beverage revenue per region-day"
owner: owner@acme.test
for_each: store
assign_by: store.region
lever:
  on: { via: promo_on }
  off: { via: promo_off }
  per: wave
  read_back:
    when_on: "{{store_days.promo_active}} > 0"
    when_off: "{{store_days.promo_active}} == 0"
    grace_days: 2
  on_close: off
design:
  timezone: Australia/Sydney
  waves:
    - { switch_on: 2026-03-02, units: [east, west] }
  seed: 8841207
  pre_days: 56
  post_days: 56
  anticipation_days: 3
  washout_days: 7
outcome:
  target: store_days.beverages_revenue
  family: 1
  direction: increase
  mde_at_proposal: 40.0
"#;

async fn seed_org(db: &DatabaseConnection) -> Uuid {
    let id = Uuid::new_v4();
    organizations::ActiveModel {
        id: Set(id),
        name: Set(format!("exp-{}", id.simple())),
        slug: Set(format!("exp-{}", id.simple())),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed org");
    id
}

pub(crate) async fn seed_member(
    db: &DatabaseConnection,
    org: Uuid,
    email: &str,
    role: org_members::OrgRole,
) -> Uuid {
    let user = users::ActiveModel {
        id: Set(Uuid::new_v4()),
        email: Set(Some(email.to_string())),
        name: Set(email.to_string()),
        picture: Set(None),
        email_verified: Set(true),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed user");
    org_members::ActiveModel {
        id: Set(Uuid::new_v4()),
        org_id: Set(org),
        user_id: Set(user.id),
        role: Set(role),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed membership");
    user.id
}

async fn seed_workspace(db: &DatabaseConnection, org: Uuid) -> Uuid {
    let now = Utc::now().fixed_offset();
    let id = Uuid::new_v4();
    workspaces::ActiveModel {
        id: Set(id),
        name: Set("experiments".into()),
        git_namespace_id: Set(None),
        git_remote_url: Set(None),
        created_at: Set(now),
        updated_at: Set(now),
        path: Set(None),
        last_opened_at: Set(None),
        created_by: Set(None),
        org_id: Set(Some(org)),
        status: Set(workspaces::WorkspaceStatus::Ready),
        error: Set(None),
        monthly_vlm_budget_micros: Set(None),
        current_revision_id: Set(None),
        default_branch: Set(None),
        repo_subdir: Set(None),
    }
    .insert(db)
    .await
    .expect("seed workspace");
    id
}

async fn seed_revision(db: &DatabaseConnection, ws: Uuid, at: DateTime<Utc>) -> Uuid {
    let rev = Uuid::new_v4();
    revisions::ActiveModel {
        revision_id: Set(rev),
        workspace_id: Set(ws),
        git_sha: Set(format!("sha-{}", rev.simple())),
        branch: Set(Some("main".into())),
        schema_version: Set(1),
        status: Set("ready".into()),
        kind: Set("main".into()),
        owner_user_id: Set(None),
        compiler_version: Set("test".into()),
        started_at: Set(at.fixed_offset()),
        finished_at: Set(Some(at.fixed_offset())),
        file_count_seen: Set(1),
        file_count_compiled: Set(1),
        file_count_failed: Set(0),
        error_summary: Set(None),
    }
    .insert(db)
    .await
    .expect("seed revision");
    rev
}

async fn seed_lever(db: &DatabaseConnection, rev: Uuid, name: &str) {
    automation_definitions::ActiveModel {
        revision_id: Set(rev),
        file_path: Set(format!("automations/{name}.automation.yml")),
        name: Set(name.to_string()),
        extension: Set("automation.yml".into()),
        definition: Set(lever(name)),
    }
    .insert(db)
    .await
    .expect("seed lever automation");
}

/// The design as the experiment's next version, inserted directly (the
/// experiment row first, when it is new): fixtures do not go through the store.
pub(crate) async fn seed_design(db: &DatabaseConnection, ws: Uuid, by: Uuid, yaml: &str) -> Uuid {
    let design = oxy_experiment::config::parse(yaml).expect("the design parses");
    let config = serde_json::to_value(&design).expect("the design serializes");
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "INSERT INTO experiments (workspace_id, name, hypothesis, owner, created_by) \
         VALUES ($1, $2, $3, $4, $5) ON CONFLICT (workspace_id, name) DO NOTHING",
        [
            ws.into(),
            design.experiment.clone().into(),
            design.hypothesis.clone().into(),
            design.owner.clone().into(),
            by.into(),
        ],
    ))
    .await
    .expect("seed experiment");
    // The experiment's one design: inserted, or the draft overwritten. A frozen
    // design is left as it is — a later promote never reaches a registered run.
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "INSERT INTO experiment_designs (experiment_id, config, content_hash, created_by, updated_by) \
         SELECT e.id, $1, md5($1::text), $2, $2 \
         FROM experiments e WHERE e.workspace_id = $3 AND e.name = $4 \
         ON CONFLICT (experiment_id) DO UPDATE \
           SET config = EXCLUDED.config, content_hash = EXCLUDED.content_hash, \
               updated_by = EXCLUDED.updated_by, updated_at = now() \
           WHERE experiment_designs.frozen_at IS NULL",
        [
            config.into(),
            by.into(),
            ws.into(),
            design.experiment.clone().into(),
        ],
    ))
    .await
    .expect("seed experiment design");
    design_id(db, ws, &design.experiment).await
}

async fn design_id(db: &DatabaseConnection, ws: Uuid, name: &str) -> Uuid {
    #[derive(FromQueryResult)]
    struct Id {
        id: Uuid,
    }
    Id::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "SELECT d.id FROM experiment_designs d JOIN experiments e ON e.id = d.experiment_id \
         WHERE e.workspace_id = $1 AND e.name = $2",
        [ws.into(), name.into()],
    ))
    .one(db)
    .await
    .expect("read the design id")
    .expect("the design")
    .id
}
