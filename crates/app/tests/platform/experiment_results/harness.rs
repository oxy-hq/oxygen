//! The estimate job's harness: a registered design, a synthetic panel with a known
//! injected effect, a seeded run, and the executor driven the way the worker drives it.

use std::sync::Arc;

use agentic_core::delegation::{TaskAssignment, TaskOutcome, TaskSpec};
use agentic_runtime::worker::TaskExecutor;
use async_trait::async_trait;
use chrono::NaiveDate;
use entity::experiment_results;
use oxy_airlayer_compat::engine::experiment::{PanelMatrix, day_ordinal};
use oxy_app::server::experiment::estimate::{EstimateSettings, ExperimentEstimateExecutor};
use oxy_app::server::experiment::jobs::{ESTIMATE_KIND, EstimatePayload};
use oxy_app::server::experiment::panel::{PanelContext, PanelOutcome, PanelRequest};
use oxy_app::server::experiment::source::PanelSource;
use oxy_experiment::config::parse;
use oxy_experiment::report::ReportView;
use oxy_shared::errors::OxyError;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use uuid::Uuid;

use super::fixtures::*;

pub const NAME: &str = "beverage_promo_pilot";
pub const TAU: f64 = 25.0; // the injected effect on the target
pub const COEFFICIENT: f64 = 5.0; // target lift per driver unit
/// If the injected effect falls outside its 95% interval, the noise draw is unlucky
/// (it misses 5% of the time): change this seed, never the assertion.
pub const NOISE_SEED: u64 = 11;

pub const CONFIG_YAML: &str = r#"
experiment: beverage_promo_pilot
hypothesis: "A beverage add-on promo lifts beverage revenue per store-day"
owner: ops
for_each: store
lever:
  on:  { via: promo_on }
  off: { via: promo_off }
  read_back: { when_on: "{{store_days.promo_active}} > 0", when_off: "{{store_days.promo_active}} == 0", grace_days: 2 }
  on_close: off
design:
  timezone: Australia/Sydney
  waves:
    - { switch_on: 2026-03-02, units: [s01, s02, s03, s04, s05, s06] }
  seed: 8841207
  pre_days: 56
  post_days: 56
  anticipation_days: 3
  washout_days: 7
outcome:
  target: store_days.beverages_revenue
  driver: store_days.promo_redemptions
  direction: increase
  mde_at_proposal: 17.5
substitution_check: { sibling: store_days.entree_revenue }
"#;

pub const VIEW: &str = "\
name: store_days
measures:
  - name: beverages_revenue
    type: sum
    expr: beverages_revenue
    drivers:
      - measure: store_days.promo_redemptions
        direction: positive
        strength: strong
";

pub struct Rng(pub u64);

impl Rng {
    fn unit(&mut self) -> f64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        (z >> 11) as f64 / (1u64 << 53) as f64
    }
    pub fn noise(&mut self, sd: f64) -> f64 {
        ((0..12).map(|_| self.unit()).sum::<f64>() - 6.0) * sd
    }
}

pub fn switch_day() -> i64 {
    day_ordinal(NaiveDate::from_ymd_opt(2026, 3, 2).unwrap())
}

/// 300 days before the switch to 100 after; the first `lifted` of `units` gain `tau`
/// from the switch day on.
pub fn synthetic(
    units: &[String],
    lifted: usize,
    base: f64,
    tau: f64,
    sd: f64,
    seed: u64,
) -> PanelMatrix {
    let switch = switch_day();
    let mut rng = Rng(seed);
    let mut rows = Vec::new();
    for (u, name) in units.iter().enumerate() {
        let level = base + 3.0 * u as f64;
        for d in (switch - 300)..=(switch + 100) {
            let weekly = 2.0 * (((d % 7) as f64) - 3.0);
            let lift = if u < lifted && d >= switch { tau } else { 0.0 };
            rows.push((name.clone(), d, level + weekly + lift + rng.noise(sd)));
        }
    }
    PanelMatrix::from_triples(rows)
}

/// `s01` … `s12`.
pub fn stores() -> Vec<String> {
    (1..=12).map(|u| format!("s{u:02}")).collect()
}

/// 12 stores; s01..s06 gain the injected effect.
pub struct FixturePanels;

#[async_trait]
impl PanelSource for FixturePanels {
    async fn load(&self, _c: &PanelContext, req: &PanelRequest) -> Result<PanelOutcome, OxyError> {
        let s = stores();
        Ok(match req.measure.as_str() {
            "store_days.beverages_revenue" => {
                PanelOutcome::Panel(synthetic(&s, 6, 155.0, TAU, 1.0, NOISE_SEED))
            }
            "store_days.promo_redemptions" => PanelOutcome::Panel(synthetic(
                &s,
                6,
                40.0,
                TAU / COEFFICIENT,
                0.5,
                NOISE_SEED + 1,
            )),
            "store_days.entree_revenue" => {
                PanelOutcome::Panel(synthetic(&s, 6, 1136.0, 0.0, 4.0, NOISE_SEED + 2))
            }
            other => PanelOutcome::NoHistory {
                measure: other.into(),
                entity: req.entity.clone(),
            },
        })
    }
}

pub const LEVER_REVISION: u128 = 77;

pub fn pinned_pool() -> serde_json::Value {
    serde_json::json!(["s07", "s08", "s09", "s10", "s11", "s12"])
}

/// Everything one seeded run varies.
#[derive(Default)]
pub struct Plan<'a> {
    pub cfg_yaml: &'a str,
    pub view_text: &'a str,
    pub pool: Option<serde_json::Value>,
    /// Under `assign_by`: group → members, as the run's first check pinned it.
    pub membership: Option<serde_json::Value>,
    /// A switchback: the unit set the run's first check pinned.
    pub fleet: Option<serde_json::Value>,
    pub abort_reason: Option<&'a str>,
    /// The one treated unit whose `on` flip failed, if any.
    pub failed_unit: Option<&'a str>,
    /// Seed an `on` flip for every unit of the config's waves (a switchback has none).
    pub flips: bool,
    /// A retrospective run: created from past data, never checked, no lever moved.
    pub retrospective: bool,
}

/// A `state` run of the given design on a workspace whose seeded revision is promoted:
/// the design is the run's PINNED version, the pool its pinned `control_pool`, the
/// membership its pinned `group_membership`. Proposals, flips and a compiled view are seeded.
pub async fn seed_plan(db: &DatabaseConnection, state: &str, p: Plan<'_>) -> Uuid {
    let (ws, rev) = seed_workspace(db, "estimate").await;
    promote(db, ws, rev).await;
    let cfg = parse(p.cfg_yaml).expect("config parses");
    let definition: serde_json::Value = serde_yaml::from_str(p.view_text).unwrap();
    seed_view(
        db,
        rev,
        "store_days",
        "views/store_days.view.yml",
        definition,
    )
    .await;
    let run = seed_run(
        db,
        RunSeed {
            config: serde_json::to_value(&cfg).unwrap(),
            control_pool: p.pool,
            group_membership: p.membership,
            fleet: p.fleet,
            observations: Some(
                serde_json::json!({ "controls_checked_on": "2026-04-20", "control_leaks": {} }),
            ),
            abort_reason: p.abort_reason.map(str::to_string),
            retrospective: p.retrospective,
            ..RunSeed::new(ws, &cfg.experiment, state) // registered 200 days ago
        },
    )
    .await;
    // Two "done" proposals before registration count; a refused one and a later one do not.
    for (days_ago, status) in [(300, "done"), (250, "done"), (260, "refused"), (10, "done")] {
        seed_proposal(db, ws, &cfg.experiment, status, days_ago).await;
    }
    if p.flips {
        let switch = NaiveDate::from_ymd_opt(2026, 3, 2).unwrap();
        for unit in cfg.design.waves.iter().flat_map(|w| w.units.iter()) {
            let flip = if p.failed_unit == Some(unit.as_str()) {
                on_flip(run, unit, "failed", None, Some("automation missing"))
            } else {
                on_flip(run, unit, "confirmed", Some(switch), None)
            };
            seed_flip(db, flip).await;
        }
    }
    run
}

/// The unit design of `CONFIG_YAML`: six treated stores, one of whose flips failed.
pub async fn seed_with(
    db: &DatabaseConnection,
    state: &str,
    view_text: &str,
    pool: Option<serde_json::Value>,
    abort_reason: Option<&str>,
) -> Uuid {
    let plan = Plan {
        cfg_yaml: CONFIG_YAML,
        view_text,
        pool,
        abort_reason,
        failed_unit: Some("s06"),
        flips: true,
        ..Default::default()
    };
    seed_plan(db, state, plan).await
}

pub async fn seed(db: &DatabaseConnection, state: &str, view_text: &str) -> Uuid {
    seed_with(db, state, view_text, Some(pinned_pool()), None).await
}

fn on_flip<'a>(
    run: Uuid,
    unit: &'a str,
    status: &'a str,
    confirmed_on: Option<NaiveDate>,
    reason: Option<&'a str>,
) -> FlipSeed<'a> {
    FlipSeed {
        run,
        unit,
        state: "on",
        status,
        confirmed_on,
        reason,
        lever_revision: Some(Uuid::from_u128(LEVER_REVISION)),
    }
}

/// The estimate kind's spec, exactly as `jobs::enqueue_estimate` queues it.
pub fn estimate_spec(run_id: Uuid) -> TaskSpec {
    TaskSpec::Custom {
        kind: ESTIMATE_KIND.to_string(),
        payload: serde_json::to_value(EstimatePayload { run_id }).unwrap(),
    }
}

/// Hand `spec` to the executor as the worker would, and wait for its one outcome.
pub async fn execute(
    db: &DatabaseConnection,
    task_id: &str,
    spec: TaskSpec,
    panels: Arc<dyn PanelSource>,
) -> TaskOutcome {
    let executor = ExperimentEstimateExecutor {
        db: db.clone(),
        panels,
        settings: EstimateSettings {
            placebo_iterations: 60,
        },
    };
    let mut task = executor
        .execute(TaskAssignment {
            task_id: task_id.to_string(),
            parent_task_id: None,
            run_id: task_id.to_string(),
            spec,
            policy: None,
            claim_epoch: agentic_core::NO_CLAIM,
        })
        .await
        .expect("the executor accepted the assignment");
    task.outcomes
        .recv()
        .await
        .expect("the executor has to report an outcome")
}

pub async fn run_estimate_with(
    db: &DatabaseConnection,
    run_id: Uuid,
    panels: Arc<dyn PanelSource>,
) -> TaskOutcome {
    execute(db, &run_id.to_string(), estimate_spec(run_id), panels).await
}

pub async fn run_estimate(db: &DatabaseConnection, run_id: Uuid) -> TaskOutcome {
    run_estimate_with(db, run_id, Arc::new(FixturePanels)).await
}

pub async fn result_of(db: &DatabaseConnection, run: Uuid) -> Option<experiment_results::Model> {
    experiment_results::Entity::find()
        .filter(experiment_results::Column::RunId.eq(run))
        .one(db)
        .await
        .unwrap()
}

pub fn report_of(row: &experiment_results::Model) -> ReportView {
    serde_json::from_value(row.report.clone()).expect("the stored report reads back")
}

pub fn is_done(o: &TaskOutcome) -> bool {
    matches!(o, TaskOutcome::Done { .. })
}
