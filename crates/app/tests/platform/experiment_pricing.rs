//! The price job against a real database and the committed `example_new`
//! fixture: 24 locations × ~400 days of `store_days.net_sales`, read through
//! the real DuckDB path.
//!
//! Every test `set_current_dir`s (the view SQL reads its CSVs by relative
//! path) and so is `#[serial_test::serial]`, for the reason
//! `metric_tree_fit_panel.rs` states at length.

use agentic_core::delegation::TaskOutcome;
use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use entity::{automation_definitions, experiment_proposals};
use oxy_airlayer_compat::engine::experiment::{day_ordinal, propose_waves};
use oxy_airlayer_compat::engine::metric_tree_ops::QueryExecutor;
use oxy_airlayer_compat::{DatasourceDialectMap, SemanticEngine, SemanticLayer};
use oxy_app::server::experiment::candidate::{Candidate, SwitchbackCandidate, WavesCandidate};
use oxy_app::server::experiment::executor::run_guarded;
use oxy_app::server::experiment::job::{Finished, PriceJob, run_job};
use oxy_app::server::experiment::panel::{
    GroupMembership, HistoryBounds, PanelContext, PanelOutcome, PanelRequest, fetch_panel,
};
use oxy_app::server::experiment::price::Iterations;
use oxy_app::server::experiment::proposal_store::{self, NewPending, ProposalStatus};
use oxy_app::server::experiment::reads::{ReadAsk, Readings, fetch_readings};
use oxy_app::server::experiment::snippet::plan_waves;
use oxy_app::server::experiment::source::PanelSource;
use oxy_shared::errors::OxyError;
use sea_orm::{
    ActiveModelTrait, ActiveValue, ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement,
};
use uuid::Uuid;

use crate::metric_tree_fit_panel::{duckdb_executor, enter_fixture_dir, layer};
use crate::simulation_routes::{seed_workspace, setup_db};

pub(crate) const DRAFT: &str = r#"
experiment: promo_pilot
hypothesis: "A promo lifts net sales per store-day"
owner: ops
for_each: location
lever:
  on: { via: promo_on }
  off: { via: promo_off }
  read_back:
    when_on: "{{store_days.promo_redemptions}} > 0"
    when_off: "{{store_days.promo_redemptions}} == 0"
    grace_days: 2
design:
  timezone: UTC
  pre_days: 28
  post_days: 28
  anticipation_days: 0
  washout_days: 7
outcome:
  target: store_days.net_sales
  direction: increase
"#;

/// A switchback draft of the same experiment: no windows, no anticipation.
pub(crate) fn switchback_draft() -> String {
    DRAFT.replace(
        "  pre_days: 28\n  post_days: 28\n  anticipation_days: 0\n  washout_days: 7",
        "  switchback:\n    period_days: 14\n  washout_days: 2",
    )
}

/// The fixture's last row is 2026-07-19; this is after it, so `today` bounds
/// nothing and the whole history is the panel.
pub(crate) fn now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 8, 1, 0, 0, 0).unwrap()
}

pub(crate) struct FixturePanelSource {
    pub layer: SemanticLayer,
    pub executor: Box<QueryExecutor>,
}

#[async_trait::async_trait]
impl PanelSource for FixturePanelSource {
    async fn load(
        &self,
        _ctx: &PanelContext,
        req: &PanelRequest,
    ) -> Result<PanelOutcome, OxyError> {
        fetch_panel(&self.layer, &*self.executor, req)
    }

    async fn read(&self, _ctx: &PanelContext, ask: &ReadAsk) -> Result<Readings, OxyError> {
        fetch_readings(&self.layer, &*self.executor, ask)
    }
}

struct FailingSource;

#[async_trait::async_trait]
impl PanelSource for FailingSource {
    async fn load(
        &self,
        _ctx: &PanelContext,
        _req: &PanelRequest,
    ) -> Result<PanelOutcome, OxyError> {
        Err(OxyError::RuntimeError(
            "connection reset by the warehouse".to_string(),
        ))
    }
}

pub(crate) struct Fixture {
    pub db: DatabaseConnection,
    pub ws: Uuid,
    pub rev: Uuid,
    /// The design of `promo_pilot`.
    pub design: Uuid,
    pub source: FixturePanelSource,
}

impl Fixture {
    pub(crate) async fn new(label: &str) -> Self {
        Self::with_definition(label, DRAFT).await
    }

    pub(crate) async fn with_definition(label: &str, yaml: &str) -> Self {
        Self::with_levers(label, yaml, &["promo_on", "promo_off"]).await
    }

    /// `levers` are the automations the promoted revision holds, one file each.
    pub(crate) async fn with_levers(label: &str, yaml: &str, levers: &[&str]) -> Self {
        enter_fixture_dir();
        let db = setup_db().await;
        let (ws, rev) = seed_workspace(&db, label).await;
        let design = seed_design(&db, ws, yaml).await;
        for name in levers {
            seed_automation(
                &db,
                rev,
                name,
                &format!("automations/{name}.automation.yml"),
            )
            .await;
        }
        let layer = layer();
        let engine = SemanticEngine::from_semantic_layer(
            layer.clone(),
            DatasourceDialectMap::with_default(oxy_airlayer_compat::Dialect::DuckDB),
        )
        .expect("engine");
        Self {
            db,
            ws,
            rev,
            design,
            source: FixturePanelSource {
                layer,
                executor: duckdb_executor(engine),
            },
        }
    }

    /// A unit-level waves candidate with the given wave sizes.
    pub(crate) fn waves(&self, wave_sizes: &[usize]) -> Candidate {
        Candidate::Waves(WavesCandidate {
            wave_sizes: wave_sizes.to_vec(),
            first_switch_on: NaiveDate::from_ymd_opt(2026, 9, 7).unwrap().into(),
            spacing_days: 14,
            blocks: None,
            pairs: false,
            assign_by: None,
            alpha: None,
        })
    }

    /// `waves` with its grouping, blocks and α overridden.
    pub(crate) fn waves_with(
        &self,
        sizes: &[usize],
        blocks: Option<usize>,
        assign_by: Option<&str>,
        alpha: Option<f64>,
    ) -> Candidate {
        let Candidate::Waves(w) = self.waves(sizes) else {
            unreachable!()
        };
        Candidate::Waves(WavesCandidate {
            blocks,
            assign_by: assign_by.map(str::to_string),
            alpha,
            ..w
        })
    }

    pub(crate) fn switchback(&self, pairs: usize) -> Candidate {
        Candidate::Switchback(SwitchbackCandidate {
            first_period_on: NaiveDate::from_ymd_opt(2026, 9, 7).unwrap().into(),
            period_days: 14,
            pairs,
            alpha: None,
        })
    }

    /// The `pending` row the handler would have inserted for one candidate,
    /// pricing design `design_id` as it is now.
    pub(crate) async fn submit(&self, candidate: &Candidate, design_id: Uuid) -> Uuid {
        let id = Uuid::new_v4();
        let design_hash = design_hash(&self.db, design_id).await;
        proposal_store::insert_pending(
            &self.db,
            NewPending {
                id,
                request_id: Uuid::new_v4(),
                workspace_id: self.ws,
                name: "promo_pilot".to_string(),
                design_id,
                design_hash,
                request: serde_json::to_value(candidate).expect("candidate json"),
                alpha: candidate.alpha().unwrap_or(0.05),
                requested_by: Uuid::new_v4(),
            },
        )
        .await
        .expect("pending row");
        id
    }

    /// Run the job. Iteration counts are turned down: this file tests the job,
    /// not the placebo's tail.
    pub(crate) async fn run(&self, id: Uuid, at: DateTime<Utc>) -> Finished {
        let iterations = Iterations {
            common_date: 200,
            staggered: 40,
            switchback: 100,
        };
        run_job(
            PriceJob {
                db: &self.db,
                proposal_id: id,
                now: at,
                iterations,
            },
            &self.source,
        )
        .await
    }

    /// Submit and run; the id of the settled row.
    pub(crate) async fn price(&self, candidate: &Candidate, at: DateTime<Utc>) -> Uuid {
        let id = self.submit(candidate, self.design).await;
        self.run(id, at).await;
        id
    }

    pub(crate) async fn row(&self, id: Uuid) -> experiment_proposals::Model {
        proposal_store::find_proposal(&self.db, id)
            .await
            .expect("read row")
            .expect("row exists")
    }

    pub(crate) async fn count_done(&self) -> u64 {
        proposal_store::count_done(&self.db, self.ws, "promo_pilot")
            .await
            .expect("count")
    }
}

/// The experiment record (made on first use) and `yaml` as its draft, inserted
/// or overwritten in place; the design's id. Raw SQL, so the store under test is
/// not the fixture's.
pub(crate) async fn seed_design(db: &DatabaseConnection, ws: Uuid, yaml: &str) -> Uuid {
    let design = oxy_experiment::config::parse(yaml).expect("the design parses");
    let config = serde_json::to_value(&design).expect("the design serializes");
    let by = Uuid::new_v4();
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
    use sea_orm::FromQueryResult;
    #[derive(FromQueryResult)]
    struct Id {
        id: Uuid,
    }
    Id::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "INSERT INTO experiment_designs (experiment_id, config, content_hash, created_by, updated_by) \
         SELECT e.id, $1, md5($1::text), $2, $2 \
         FROM experiments e WHERE e.workspace_id = $3 AND e.name = $4 \
         ON CONFLICT (experiment_id) DO UPDATE \
           SET config = EXCLUDED.config, content_hash = EXCLUDED.content_hash, updated_at = now() \
         RETURNING id",
        [
            config.into(),
            by.into(),
            ws.into(),
            design.experiment.into(),
        ],
    ))
    .one(db)
    .await
    .expect("seed experiment design")
    .expect("the design row")
    .id
}

/// A lever automation in the promoted revision.
pub(crate) async fn seed_automation(
    db: &DatabaseConnection,
    rev: Uuid,
    name: &str,
    file_path: &str,
) {
    seed_automation_with(db, rev, name, file_path, lever_definition("{{ unit }}")).await;
}

/// [`seed_automation`] with the automation's compiled `definition`.
pub(crate) async fn seed_automation_with(
    db: &DatabaseConnection,
    rev: Uuid,
    name: &str,
    file_path: &str,
    definition: serde_json::Value,
) {
    automation_definitions::ActiveModel {
        revision_id: ActiveValue::Set(rev),
        file_path: ActiveValue::Set(file_path.to_string()),
        name: ActiveValue::Set(name.to_string()),
        extension: ActiveValue::Set("automation.yml".to_string()),
        definition: ActiveValue::Set(definition),
    }
    .insert(db)
    .await
    .expect("seed lever automation");
}

/// A lever whose one task writes `unit_expr`: the unit check reads its templates.
pub(crate) fn lever_definition(unit_expr: &str) -> serde_json::Value {
    serde_json::json!({
        "tasks": [{
            "name": "flip",
            "type": "execute_sql",
            "database": "local",
            "sql_query": format!("UPDATE promo SET state = '{{{{ state }}}}' WHERE store = '{unit_expr}'"),
        }],
    })
}

fn warnings_of(row: &experiment_proposals::Model) -> Vec<String> {
    row.warnings
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w.as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
#[serial_test::serial]
async fn price_returns_an_mde_and_waves_and_settles_the_row_done() {
    let f = Fixture::new("price-ok").await;
    let id = f.price(&f.waves(&[8]), now()).await;
    let row = f.row(id).await;
    assert_eq!(row.status, "done", "{:?}", row.refusal);
    assert!(row.finished_at.is_some());
    assert!(row.seed.is_some());
    let mde = row.power.as_ref().unwrap()["mde"].as_f64().unwrap();
    assert!(mde > 0.0);
    assert_eq!(
        row.mde_unit_day,
        Some(mde),
        "a unit design's MDE is already per unit-day"
    );
    assert_eq!(row.alpha, 0.05);
    let waves = row.waves.as_ref().unwrap().as_array().unwrap();
    assert_eq!(waves.len(), 1);
    assert_eq!(waves[0]["switch_on"], "2026-09-07");
    assert_eq!(waves[0]["units"].as_array().unwrap().len(), 8);
    assert!(row.periods.is_none() && row.strata.is_none() && row.assign_by.is_none());
    assert_eq!(f.count_done().await, 1);
}

#[tokio::test]
#[serial_test::serial]
async fn the_stored_seed_reproduces_the_stored_waves() {
    let f = Fixture::new("reproduce").await;
    let id = f.price(&f.waves(&[8]), now()).await;
    let row = f.row(id).await;

    let ctx = PanelContext {
        workspace_id: f.ws,
        revision_id: f.rev,
        requested_by: Uuid::new_v4(),
    };
    let request = PanelRequest {
        entity: "location".to_string(),
        measure: "store_days.net_sales".to_string(),
        time_dimension: None,
        grain: oxy_experiment::Grain::Day,
        timezone: None,
        history: HistoryBounds {
            from: None,
            to_exclusive: NaiveDate::from_ymd_opt(2026, 8, 1).unwrap(),
        },
        membership: None,
        restrict_to: None,
        unit_floor: None,
        filters: vec![],
    };
    let PanelOutcome::Panel(panel) = f.source.load(&ctx, &request).await.expect("panel") else {
        panic!("the fixture has history");
    };
    let first = NaiveDate::from_ymd_opt(2026, 9, 7).unwrap();
    let engine = propose_waves(
        panel.units(),
        &[8],
        day_ordinal(first),
        14,
        None,
        row.seed.unwrap() as u64,
    )
    .expect("waves");
    assert_eq!(
        row.waves.unwrap(),
        serde_json::to_value(plan_waves(oxy_experiment::Grain::Day, &engine).unwrap()).unwrap()
    );
}

#[tokio::test]
#[serial_test::serial]
async fn the_seed_a_job_draws_survives_a_javascript_client() {
    let f = Fixture::new("js-safe-seed").await;
    let id = f.price(&f.waves(&[8]), now()).await;
    let seed = f.row(id).await.seed.unwrap();
    assert!(
        (0..(1i64 << 53)).contains(&seed),
        "seed {seed} is not a JS-safe integer"
    );
}

#[tokio::test]
#[serial_test::serial]
async fn the_proposal_log_counts_two_proposals_after_two_calls() {
    let f = Fixture::new("two-calls").await;
    let (elsewhere, _) = seed_workspace(&f.db, "elsewhere").await;
    f.price(&f.waves(&[8]), now()).await;
    f.price(&f.waves(&[8]), now()).await;
    assert_eq!(f.count_done().await, 2);
    // Tenant scoping: another workspace's log is empty.
    assert_eq!(
        proposal_store::count_done(&f.db, elsewhere, "promo_pilot")
            .await
            .unwrap(),
        0
    );
    assert!(
        proposal_store::list_proposals(&f.db, elsewhere, "promo_pilot", 50)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
#[serial_test::serial]
async fn a_redelivered_task_leaves_a_finished_proposal_untouched() {
    // Review Focus 3. The queue's reaper hands a payload out again when a lease
    // expires after the row was settled.
    let f = Fixture::new("redelivered").await;
    let id = f.price(&f.waves(&[8]), now()).await;
    let first = f.row(id).await;

    let again = f.run(id, now()).await;
    assert!(again.already_finished);
    assert_eq!(again.status, ProposalStatus::Done);
    assert_eq!(again.error, None);

    let after = f.row(id).await;
    assert_eq!(
        (after.seed, after.finished_at, after.waves),
        (first.seed, first.finished_at, first.waves)
    );
    assert_eq!(
        f.count_done().await,
        1,
        "a redelivery must not add a done proposal"
    );
}

#[tokio::test]
#[serial_test::serial]
async fn price_job_refuses_empty_history_without_panicking() {
    // Review Focus 1. `now` is before the fixture's first row (2025-06-15), so
    // the panel query's `business_date < today` matches nothing and the real
    // DuckDB path answers with zero rows.
    let f = Fixture::new("empty-history").await;
    let before_any_data = Utc.with_ymd_and_hms(2020, 1, 1, 12, 0, 0).unwrap();
    let id = f.price(&f.waves(&[8]), before_any_data).await;
    let row = f.row(id).await;
    assert_eq!(row.status, "refused", "a refusal is a row, not a timeout");
    let refusal = row.refusal.expect("the refusal is recorded");
    assert!(
        refusal.contains("no history for store_days.net_sales over location"),
        "{refusal}"
    );
    assert!(row.finished_at.is_some());
    assert_eq!((row.seed, row.waves, row.power), (None, None, None));
    assert_eq!(f.count_done().await, 0, "a refusal is not a draw");
}

#[tokio::test]
#[serial_test::serial]
async fn an_unknown_unit_name_refuses_via_validate_against_units() {
    let yaml = DRAFT
        .replace("  washout_days: 7", "  washout_days: 7\n  seed: 1\n  waves:\n    - { switch_on: 2026-09-07, units: [Location_7] }")
        .replace("  direction: increase", "  direction: increase\n  mde_at_proposal: 1.0");
    let f = Fixture::with_definition("unknown-unit", &yaml).await;
    let row = f.row(f.price(&f.waves(&[8]), now()).await).await;
    assert_eq!(row.status, "refused");
    assert!(row.refusal.unwrap().contains("Location_7"));
    assert_eq!(f.count_done().await, 0);
}

#[tokio::test]
#[serial_test::serial]
async fn every_unit_in_a_wave_is_refused_as_unidentified() {
    let f = Fixture::new("all-placed").await;
    let row = f.row(f.price(&f.waves(&[24]), now()).await).await;
    assert_eq!(row.status, "refused");
    assert!(row.refusal.unwrap().contains("all 24 units of `location`"));
}

#[tokio::test]
#[serial_test::serial]
async fn zero_washout_is_a_warning_on_a_done_row() {
    let f = Fixture::with_definition(
        "no-washout",
        &DRAFT.replace("washout_days: 7", "washout_days: 0"),
    )
    .await;
    let row = f.row(f.price(&f.waves(&[8]), now()).await).await;
    assert_eq!(row.status, "done", "a warning is not a refusal");
    assert!(
        warnings_of(&row)
            .iter()
            .any(|w| w.contains("partial first day")),
        "{:?}",
        warnings_of(&row)
    );
}

#[tokio::test]
#[serial_test::serial]
async fn an_explicit_time_dimension_prices_the_same_panel() {
    let yaml = DRAFT.replace(
        "  washout_days: 7",
        "  washout_days: 7\n  time_dimension: store_days.business_date",
    );
    let f = Fixture::with_definition("time-dim-ok", &yaml).await;
    assert_eq!(
        f.row(f.price(&f.waves(&[8]), now()).await).await.status,
        "done"
    );
}

#[tokio::test]
#[serial_test::serial]
async fn a_time_dimension_that_is_not_a_date_dimension_settles_a_refused_row() {
    let yaml = DRAFT.replace(
        "  washout_days: 7",
        "  washout_days: 7\n  time_dimension: store_days.location_id",
    );
    let f = Fixture::with_definition("time-dim-bad", &yaml).await;
    let row = f.row(f.price(&f.waves(&[8]), now()).await).await;
    assert_eq!(row.status, "refused");
    let refusal = row.refusal.unwrap();
    assert!(
        refusal.contains("store_days.location_id") && refusal.contains("store_days.business_date"),
        "{refusal}"
    );
    assert!(
        refusal.contains("the measure `store_days.net_sales`"),
        "{refusal}"
    );
}

#[tokio::test]
#[serial_test::serial]
async fn a_proposal_naming_a_missing_design_is_a_refused_row() {
    let f = Fixture::new("wrong-design").await;
    let id = f.submit(&f.waves(&[8]), Uuid::new_v4()).await;
    f.run(id, now()).await;
    let row = f.row(id).await;
    assert_eq!(row.status, "refused");
    assert!(
        row.refusal
            .unwrap()
            .contains("experiment `promo_pilot` has no design")
    );
}

#[tokio::test]
#[serial_test::serial]
async fn a_proposal_whose_draft_was_edited_before_it_ran_is_a_refused_row() {
    let f = Fixture::new("edited-draft").await;
    let id = f.submit(&f.waves(&[8]), f.design).await;
    seed_design(&f.db, f.ws, &DRAFT.replace("pre_days: 28", "pre_days: 21")).await;
    f.run(id, now()).await;
    let row = f.row(id).await;
    assert_eq!(row.status, "refused");
    assert!(
        row.refusal.unwrap().contains("the draft was edited"),
        "an edited draft is not what was asked to be priced"
    );
}

/// The design's `content_hash` now, or a hash no design has.
async fn design_hash(db: &DatabaseConnection, design_id: Uuid) -> String {
    oxy_app::server::experiment::design_store::design_by_id(db, design_id)
        .await
        .expect("read the design")
        .map_or_else(|| "missing".to_string(), |d| d.content_hash)
}

#[tokio::test]
#[serial_test::serial]
async fn a_panel_fault_marks_the_proposal_failed_with_the_reason() {
    // Review Focus 4. An infrastructure fault must settle the row — a `pending`
    // row that never moves is the timeout this design exists to remove.
    let f = Fixture::new("panel-fault").await;
    let id = f.submit(&f.waves(&[8]), f.design).await;
    let iterations = Iterations {
        common_date: 20,
        staggered: 20,
        switchback: 20,
    };
    let finished = run_job(
        PriceJob {
            db: &f.db,
            proposal_id: id,
            now: now(),
            iterations,
        },
        &FailingSource,
    )
    .await;
    assert_eq!(finished.status, ProposalStatus::Failed);
    assert!(
        finished
            .error
            .as_deref()
            .unwrap()
            .contains("connection reset"),
        "{finished:?}"
    );

    let row = f.row(id).await;
    assert_eq!(row.status, "failed");
    assert!(row.refusal.unwrap().contains("connection reset"));
    assert!(row.finished_at.is_some());
    assert_eq!(f.count_done().await, 0);
}

struct PanickingSource;

#[async_trait::async_trait]
impl PanelSource for PanickingSource {
    async fn load(
        &self,
        _ctx: &PanelContext,
        _req: &PanelRequest,
    ) -> Result<PanelOutcome, OxyError> {
        panic!("the arrow decoder blew up");
    }

    async fn read(&self, _ctx: &PanelContext, _ask: &ReadAsk) -> Result<Readings, OxyError> {
        panic!("the arrow decoder blew up");
    }
}

#[tokio::test]
#[serial_test::serial]
async fn a_panicking_job_settles_its_row_failed_naming_the_panic() {
    // A panic outside the blocking thread unwinds the worker's driver task; the
    // queue sees a terminal `Failed` and never redelivers, so only the executor
    // can settle the row.
    let f = Fixture::new("job-panic").await;
    let id = f.submit(&f.waves(&[8]), f.design).await;
    let outcome = run_guarded(&f.db, id, &PanickingSource).await;
    assert!(
        matches!(&outcome, TaskOutcome::Failed(r) if r.contains("arrow decoder blew up")),
        "{outcome:?}"
    );
    let row = f.row(id).await;
    assert_eq!(row.status, "failed");
    assert!(row.refusal.unwrap().contains("arrow decoder blew up"));
    assert!(row.finished_at.is_some());
}

#[tokio::test]
#[serial_test::serial]
async fn a_job_for_a_row_that_does_not_exist_reports_a_fault() {
    let f = Fixture::new("no-row").await;
    let finished = f.run(Uuid::new_v4(), now()).await;
    assert_eq!(finished.status, ProposalStatus::Failed);
    assert!(finished.error.is_some());
}

// ── the lever check ─────────────────────────────────────────────────────────

#[tokio::test]
#[serial_test::serial]
async fn a_lever_that_does_not_resolve_refuses_the_proposal_naming_it() {
    // Review Focus 5. `promo_on` was renamed: the promoted revision holds only
    // `promo_off`. The source is one that FAILS if it is asked for anything, so
    // a `refused` row (not `failed`) proves the lever was checked first.
    let f = Fixture::with_levers("lever-missing", DRAFT, &["promo_off"]).await;
    let id = f.submit(&f.waves(&[8]), f.design).await;
    let iterations = Iterations {
        common_date: 20,
        staggered: 20,
        switchback: 20,
    };
    run_job(
        PriceJob {
            db: &f.db,
            proposal_id: id,
            now: now(),
            iterations,
        },
        &FailingSource,
    )
    .await;
    let row = f.row(id).await;
    assert_eq!(row.status, "refused", "{:?}", row.refusal);
    let refusal = row.refusal.unwrap();
    assert!(
        refusal.contains("lever.on")
            && refusal.contains("`promo_on`")
            && refusal.contains("no automation named"),
        "{refusal}"
    );
    assert!(
        !refusal.contains("lever.off"),
        "the off lever resolves: {refusal}"
    );
    assert_eq!(f.count_done().await, 0);
}

#[tokio::test]
#[serial_test::serial]
async fn an_unresolvable_off_lever_refuses_too() {
    let f = Fixture::with_levers("lever-off-missing", DRAFT, &["promo_on"]).await;
    let row = f.row(f.price(&f.waves(&[8]), now()).await).await;
    assert_eq!(row.status, "refused");
    let refusal = row.refusal.unwrap();
    assert!(
        refusal.contains("lever.off") && refusal.contains("`promo_off`"),
        "{refusal}"
    );
}

#[tokio::test]
#[serial_test::serial]
async fn both_levers_missing_are_named_in_one_refusal() {
    let f = Fixture::with_levers("lever-none", DRAFT, &[]).await;
    let refusal = f
        .row(f.price(&f.waves(&[8]), now()).await)
        .await
        .refusal
        .unwrap();
    assert!(
        refusal.contains("lever.on") && refusal.contains("lever.off"),
        "{refusal}"
    );
}

#[tokio::test]
#[serial_test::serial]
async fn a_lever_that_never_reads_the_unit_refuses_naming_the_side_and_the_variable() {
    let f = Fixture::with_levers("lever-ignores-unit", DRAFT, &["promo_on"]).await;
    seed_automation_with(
        &f.db,
        f.rev,
        "promo_off",
        "automations/promo_off.automation.yml",
        lever_definition("every store"),
    )
    .await;
    let row = f.row(f.price(&f.waves(&[8]), now()).await).await;
    assert_eq!(row.status, "refused", "{:?}", row.refusal);
    let refusal = row.refusal.unwrap();
    assert!(
        refusal.contains("lever.off")
            && refusal.contains("`promo_off`")
            && refusal.contains("`{{ unit }}`")
            && refusal.contains("whole fleet"),
        "{refusal}"
    );
    assert!(
        !refusal.contains("lever.on"),
        "the on lever reads it: {refusal}"
    );
}

#[tokio::test]
#[serial_test::serial]
async fn an_ambiguous_lever_names_both_paths() {
    let f = Fixture::new("lever-ambiguous").await;
    seed_automation(
        &f.db,
        f.rev,
        "promo_on",
        "automations/copies/promo_on.automation.yml",
    )
    .await;
    let refusal = f
        .row(f.price(&f.waves(&[8]), now()).await)
        .await
        .refusal
        .unwrap();
    assert!(
        refusal.contains("2 automations")
            && refusal.contains("automations/promo_on.automation.yml")
            && refusal.contains("automations/copies/promo_on.automation.yml"),
        "{refusal}"
    );
}

#[tokio::test]
#[serial_test::serial]
async fn a_workspace_with_no_promoted_revision_refuses_the_proposal() {
    let f = Fixture::new("no-promoted").await;
    f.db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "UPDATE workspaces SET current_revision_id = NULL WHERE id = $1",
        [f.ws.into()],
    ))
    .await
    .expect("unpromote");
    let row = f.row(f.price(&f.waves(&[8]), now()).await).await;
    assert_eq!(row.status, "refused", "{:?}", row.refusal);
}

// ── groups, strata, switchback, alpha ───────────────────────────────────────

#[tokio::test]
#[serial_test::serial]
async fn a_group_candidate_prices_group_sums_and_stores_the_scaled_mde() {
    let f = Fixture::new("groups").await;
    let id = f
        .price(
            &f.waves_with(&[5], None, Some("location.city"), None),
            now(),
        )
        .await;
    let row = f.row(id).await;
    assert_eq!(row.status, "done", "{:?}", row.refusal);
    assert_eq!(row.assign_by.as_deref(), Some("location.city"));
    let waves = row.waves.as_ref().unwrap().as_array().unwrap();
    let units: Vec<&str> = waves[0]["units"]
        .as_array()
        .unwrap()
        .iter()
        .map(|u| u.as_str().unwrap())
        .collect();
    assert_eq!(units.len(), 5);
    assert!(
        units.iter().all(|u| u.parse::<i64>().is_err()),
        "waves list cities, not location ids: {units:?}"
    );
    // 24 locations in 17 cities.
    let mde = row.power.as_ref().unwrap()["mde"].as_f64().unwrap();
    assert!(
        (row.mde_unit_day.unwrap() - mde / (24.0 / 17.0)).abs() < 1e-9,
        "{:?} vs {mde}",
        row.mde_unit_day
    );
    assert!(
        warnings_of(&row)
            .iter()
            .any(|w| w.contains("spreads evenly")),
        "{:?}",
        warnings_of(&row)
    );
}

#[tokio::test]
#[serial_test::serial]
async fn a_non_additive_target_under_assign_by_settles_a_refused_row_naming_the_measure() {
    let yaml = DRAFT.replace(
        "target: store_days.net_sales",
        "target: store_days.sales_per_check",
    );
    let f = Fixture::with_definition("groups-ratio", &yaml).await;
    let row = f
        .row(
            f.price(
                &f.waves_with(&[5], None, Some("location.city"), None),
                now(),
            )
            .await,
        )
        .await;
    assert_eq!(
        row.status, "refused",
        "a value, never a retried fault: {:?}",
        row.refusal
    );
    assert!(
        row.refusal
            .unwrap()
            .contains("`store_days.sales_per_check`")
    );
}

#[tokio::test]
#[serial_test::serial]
async fn an_assign_by_dimension_that_does_not_exist_settles_a_refused_row() {
    let f = Fixture::new("groups-typo").await;
    let row = f
        .row(
            f.price(
                &f.waves_with(&[5], None, Some("location.citty"), None),
                now(),
            )
            .await,
        )
        .await;
    assert_eq!(row.status, "refused");
    let refusal = row.refusal.unwrap();
    assert!(
        refusal.contains("`assign_by: location.citty`") && refusal.contains("city"),
        "{refusal}"
    );
}

#[tokio::test]
#[serial_test::serial]
async fn the_panel_sums_by_the_membership_it_is_handed() {
    // Index Review Focus 8, PR 3's half, against real SQL: locations 1 and 2 in
    // group A, 3 in group B — a membership that matches no dimension of the
    // fixture. The panel follows it, and issues exactly one query.
    let f = Fixture::new("handed-membership").await;
    let membership = GroupMembership(
        [
            ("A".to_string(), vec!["1".to_string(), "2".to_string()]),
            ("B".to_string(), vec!["3".to_string()]),
        ]
        .into_iter()
        .collect(),
    );
    let request = PanelRequest {
        entity: "location".to_string(),
        measure: "store_days.net_sales".to_string(),
        time_dimension: None,
        grain: oxy_experiment::Grain::Day,
        timezone: None,
        history: HistoryBounds {
            from: None,
            to_exclusive: NaiveDate::from_ymd_opt(2026, 8, 1).unwrap(),
        },
        membership: Some(membership),
        restrict_to: None,
        unit_floor: None,
        filters: vec![],
    };
    let asked = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let engine = SemanticEngine::from_semantic_layer(
        f.source.layer.clone(),
        DatasourceDialectMap::with_default(oxy_airlayer_compat::Dialect::DuckDB),
    )
    .expect("engine");
    let inner = duckdb_executor(engine);
    let seen = asked.clone();
    let counting: Box<QueryExecutor> = Box::new(
        move |r: &oxy_airlayer_compat::engine::query::QueryRequest| {
            seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            inner(r)
        },
    );
    let out = fetch_panel(&f.source.layer, &*counting, &request).expect("outcome");
    let PanelOutcome::Panel(panel) = out else {
        panic!("expected a panel")
    };
    assert_eq!(panel.units(), ["A".to_string(), "B".to_string()]);
    assert_eq!(asked.load(std::sync::atomic::Ordering::SeqCst), 1);
}

#[tokio::test]
#[serial_test::serial]
async fn a_stratified_candidate_stores_strata_that_partition_the_units() {
    let f = Fixture::new("strata").await;
    let row = f
        .row(
            f.price(&f.waves_with(&[12], Some(3), None, None), now())
                .await,
        )
        .await;
    assert_eq!(row.status, "done", "{:?}", row.refusal);
    let strata: Vec<Vec<String>> = serde_json::from_value(row.strata.expect("strata")).unwrap();
    assert_eq!(strata.len(), 3);
    let mut all: Vec<String> = strata.into_iter().flatten().collect();
    all.sort();
    all.dedup();
    assert_eq!(all.len(), 24, "every location sits in exactly one stratum");
}

#[tokio::test]
#[serial_test::serial]
async fn a_switchback_candidate_stores_periods_and_no_waves() {
    let f = Fixture::with_definition("switchback", &switchback_draft()).await;
    let row = f.row(f.price(&f.switchback(6), now()).await).await;
    assert_eq!(row.status, "done", "{:?}", row.refusal);
    assert!(row.waves.is_none() && row.strata.is_none() && row.assign_by.is_none());
    let periods = row.periods.as_ref().unwrap().as_array().unwrap();
    assert_eq!(periods.len(), 12);
    assert_eq!(periods[0]["from"], "2026-09-07");
    assert_eq!(
        periods[1]["from"], "2026-09-21",
        "periods are period_days apart"
    );
    for pair in periods.chunks(2) {
        let on = pair.iter().filter(|p| p["state"] == "on").count();
        assert_eq!(on, 1, "one half of every pair is on: {pair:?}");
    }
    assert_eq!(
        row.mde_unit_day,
        Some(row.power.as_ref().unwrap()["mde"].as_f64().unwrap())
    );
}

#[tokio::test]
#[serial_test::serial]
async fn a_candidates_alpha_override_is_the_alpha_the_row_records() {
    let f = Fixture::new("alpha").await;
    let row = f
        .row(
            f.price(&f.waves_with(&[8], None, None, Some(0.2)), now())
                .await,
        )
        .await;
    assert_eq!(row.status, "done", "{:?}", row.refusal);
    assert_eq!(row.alpha, 0.2);
}

// ── the warnings the warehouse informs, and the frozen slope ────────────────

#[tokio::test]
#[serial_test::serial]
async fn arrival_lag_warns_for_the_read_back_and_a_guardrail_measure() {
    // The fixture's last row is 2026-07-19 and `now` is 2026-08-01: the view
    // declares no freshness contract, so the lag is observed — 13 days — against
    // a grace of 2 and a guardrail allowance of 1.
    let yaml = format!(
        "{DRAFT}guardrails:\n  - criteria: {{ type: anomaly, measure: store_days.labor_cost, significance: 0.95 }}\n    action: abort\n"
    );
    let f = Fixture::with_definition("lag", &yaml).await;
    let row = f.row(f.price(&f.waves(&[8]), now()).await).await;
    assert_eq!(row.status, "done", "{:?}", row.refusal);
    let w = warnings_of(&row);
    assert!(
        w.iter().any(
            |x| x.contains("read-back measure `store_days.promo_redemptions`")
                && x.contains("13 days")
                && x.contains("observed")
        ),
        "{w:?}"
    );
    assert!(
        w.iter()
            .any(|x| x.contains("guardrail measure `store_days.labor_cost`")
                && x.contains("13 days late")),
        "{w:?}"
    );
}

#[tokio::test]
#[serial_test::serial]
async fn a_trigger_on_a_widely_spread_measure_warns_and_a_narrow_one_does_not() {
    // Per-location means over the fixture: promo_margin 323 → 911 (2.8x);
    // labor_cost 327 → 634 (1.94x, under the 2x line).
    let trigger = |m: &str| {
        format!(
            "{DRAFT}guardrails:\n  - criteria: {{ type: trigger, when: \"{{{{store_days.{m}}}}} > 100000\" }}\n    action: abort\n"
        )
    };
    let wide = Fixture::with_definition("spread-wide", &trigger("promo_margin")).await;
    let w = warnings_of(&wide.row(wide.price(&wide.waves(&[8]), now()).await).await);
    assert!(
        w.iter()
            .any(|x| x.contains("`store_days.promo_margin`") && x.contains("anomaly")),
        "{w:?}"
    );

    let narrow = Fixture::with_definition("spread-narrow", &trigger("labor_cost")).await;
    let w = warnings_of(
        &narrow
            .row(narrow.price(&narrow.waves(&[8]), now()).await)
            .await,
    );
    assert!(
        w.iter().all(|x| !x.contains("trigger guardrail on")),
        "{w:?}"
    );
}

#[tokio::test]
#[serial_test::serial]
async fn a_fitted_prior_is_frozen_from_the_fixtures_history() {
    // `marketing_spend -> net_sales` declares no coefficient on purpose: the fit
    // measures it (about 5.8 over the shipped window).
    let yaml = DRAFT.replace(
        "  direction: increase",
        "  direction: increase\n  driver: store_days.marketing_spend",
    );
    let f = Fixture::with_definition("prior-fitted", &yaml).await;
    let row = f.row(f.price(&f.waves(&[8]), now()).await).await;
    assert_eq!(row.status, "done", "{:?}", row.refusal);
    let prior = row.prior_coefficient.expect("a fitted prior");
    assert_eq!(prior["source"], "fitted");
    let value = prior["value"].as_f64().unwrap();
    assert!((3.0..9.0).contains(&value), "fitted slope {value}");
}

#[tokio::test]
#[serial_test::serial]
async fn a_declared_prior_is_frozen_as_declared() {
    let yaml = DRAFT
        .replace(
            "target: store_days.net_sales",
            "target: store_days.guest_count",
        )
        .replace(
            "  direction: increase",
            "  direction: increase\n  driver: store_days.promo_redemptions",
        );
    let f = Fixture::with_definition("prior-declared", &yaml).await;
    let row = f.row(f.price(&f.waves(&[8]), now()).await).await;
    assert_eq!(row.status, "done", "{:?}", row.refusal);
    assert_eq!(
        row.prior_coefficient,
        Some(serde_json::json!({"value": 2.3, "source": "declared"}))
    );
}

#[tokio::test]
#[serial_test::serial]
async fn a_driver_with_no_edge_freezes_nothing_and_says_so() {
    let yaml = DRAFT.replace(
        "  direction: increase",
        "  direction: increase\n  driver: store_days.food_cost",
    );
    let f = Fixture::with_definition("prior-none", &yaml).await;
    let row = f.row(f.price(&f.waves(&[8]), now()).await).await;
    assert_eq!(
        row.status, "done",
        "a missing prior is a warning, not a refusal"
    );
    assert!(row.prior_coefficient.is_none());
    assert!(
        warnings_of(&row)
            .iter()
            .any(|w| w.contains("no prior_coefficient was frozen")),
        "{:?}",
        warnings_of(&row)
    );
}
