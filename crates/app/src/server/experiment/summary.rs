//! Read models for `GET /experiments` and `GET /experiments/{name}/proposals`
//! — index C5's `ExperimentListItem` and `Proposal`.

use std::collections::HashMap;

use chrono::{DateTime, FixedOffset};
use entity::experiment_proposals;
use oxy_experiment::Moment;
use oxy_experiment::{ExperimentConfig, PriorCoefficient};
use sea_orm::{DatabaseBackend, DatabaseConnection, DbErr, FromQueryResult, Statement};
use serde::Serialize;
use serde_json::Value;
use uuid::Uuid;

use super::candidate::Candidate;
use super::design_store;
use super::proposal_store::{self, ProposalStatus};
use super::snippet::{PeriodPlan, SnippetKeys, SnippetSchedule, WavePlan, render_snippet};

/// C5's `ExperimentStatus`, for the list and `GET /experiments/{name}/run`.
/// `draft` is a latest version that is not registrable; `registrable` is one
/// that could be registered now, with no run yet; `registration_refused` carries `registration_refusal`;
/// the last five are the run's `RunState`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExperimentStatus {
    Draft,
    Registrable,
    RegistrationRefused,
    Registered,
    Live,
    Closing,
    Reported,
    Aborted,
}

#[derive(Debug, Serialize)]
pub struct ProposalSummary {
    pub id: Uuid,
    pub request_id: Uuid,
    pub status: String,
    pub created_at: DateTime<FixedOffset>,
    pub mde: Option<f64>,
    pub mde_unit_day: Option<f64>,
    pub refusal: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ExperimentListItem {
    pub name: String,
    pub owner: String,
    pub hypothesis: String,
    /// The experiment's one design: its id, its `content_hash` (what Register and
    /// an edit send back) and, once registered, when it was frozen.
    pub design_id: Uuid,
    pub design_hash: String,
    pub design_frozen_at: Option<DateTime<FixedOffset>>,
    pub status: ExperimentStatus,
    /// Set only while `status` is `registration_refused`.
    pub registration_refusal: Option<String>,
    /// `YYYY-MM-DD` in `design.timezone`: the earliest wave (or switchback period) on or after today.
    pub next_switch_on: Option<String>,
    /// Proposals with status `done` — every candidate of every request.
    pub proposals_count: u64,
    /// The newest proposal of any status.
    pub latest_proposal: Option<ProposalSummary>,
    /// The shown version's `ExperimentConfig` as JSON. Lets the UI show the
    /// whole design.
    pub definition: Value,
    /// The run was created from past data: not pre-registered, no lever moved.
    pub retrospective: bool,
    /// No run yet, and the latest version's dates are all past: it can be analysed
    /// retrospectively.
    pub retrospective_ready: bool,
    /// A `reported` run's headline, picked from its stored report: what the row
    /// shows at a glance. `None` before the report is sealed (see [`super::glance`]).
    pub report_glance: Option<Value>,
}

/// Index C5's `Proposal`, key for key.
#[derive(Debug, Serialize)]
pub struct Proposal {
    pub id: Uuid,
    pub request_id: Uuid,
    pub status: String,
    pub created_at: DateTime<FixedOffset>,
    pub finished_at: Option<DateTime<FixedOffset>>,
    /// The one `Candidate`, as received.
    pub request: Value,
    pub seed: Option<u64>,
    pub waves: Option<Value>,
    pub periods: Option<Value>,
    pub strata: Option<Value>,
    pub assign_by: Option<String>,
    pub mde: Option<f64>,
    pub mde_relative: Option<f64>,
    pub baseline: Option<f64>,
    /// $ per unit-day: comparable across candidates (index C2b).
    pub mde_unit_day: Option<f64>,
    /// The α this candidate was priced at.
    pub alpha: f64,
    pub prior_coefficient: Option<Value>,
    pub warnings: Vec<String>,
    pub refusal: Option<String>,
    /// The design priced; `None` on a row logged before designs were records.
    pub design_id: Option<Uuid>,
    /// The draft was edited after this was priced: it must be priced again
    /// before it can be used.
    pub stale: bool,
    /// Server-rendered YAML of every output key — display only; adopting the
    /// proposal (`POST …/proposals/{id}/use`) is what writes them.
    pub snippet: Option<String>,
}

fn power_number(power: &Option<Value>, key: &str) -> Option<f64> {
    power.as_ref()?.get(key)?.as_f64()
}

fn from_json<T: serde::de::DeserializeOwned>(v: &Option<Value>) -> Option<T> {
    v.clone().and_then(|v| serde_json::from_value(v).ok())
}

/// The YAML the user commits, for a `done` row; `None` for any other status or
/// a row whose stored pieces no longer parse.
fn snippet(m: &experiment_proposals::Model, mde: Option<f64>) -> Option<String> {
    if m.status != "done" {
        return None;
    }
    let (seed, mde) = (u64::try_from(m.seed?).ok()?, mde?);
    let candidate: Candidate = serde_json::from_value(m.request.clone()).ok()?;
    let waves: Option<Vec<WavePlan>> = from_json(&m.waves);
    let periods: Option<Vec<PeriodPlan>> = from_json(&m.periods);
    let strata: Option<Vec<Vec<String>>> = from_json(&m.strata);
    let prior: Option<PriorCoefficient> = from_json(&m.prior_coefficient);
    let schedule = match (&candidate, &waves, &periods) {
        (_, Some(w), _) => SnippetSchedule::Waves(w),
        (Candidate::Switchback(s), _, Some(p)) => SnippetSchedule::Switchback {
            period_days: s.period_days,
            periods: p,
        },
        _ => return None,
    };
    render_snippet(&SnippetKeys {
        assign_by: m.assign_by.as_deref(),
        schedule,
        strata: strata.as_deref(),
        seed,
        alpha: candidate.alpha(),
        mde,
        prior: prior.as_ref(),
    })
    .ok()
}

impl Proposal {
    /// `current_hash` is the design's `content_hash` now; `None` reads as not stale.
    pub fn from_row(m: experiment_proposals::Model, current_hash: Option<&str>) -> Self {
        let mde = power_number(&m.power, "mde");
        let stale = current_hash.is_some_and(|h| m.design_hash.as_deref() != Some(h));
        let snippet = snippet(&m, mde);
        Self {
            id: m.id,
            request_id: m.request_id,
            status: m.status,
            created_at: m.created_at,
            finished_at: m.finished_at,
            request: m.request,
            seed: m.seed.map(|s| s as u64),
            waves: m.waves,
            periods: m.periods,
            strata: m.strata,
            assign_by: m.assign_by,
            mde,
            mde_relative: power_number(&m.power, "mde_relative"),
            baseline: power_number(&m.power, "baseline"),
            mde_unit_day: m.mde_unit_day,
            alpha: m.alpha,
            prior_coefficient: m.prior_coefficient,
            warnings: serde_json::from_value(m.warnings).unwrap_or_default(),
            refusal: m.refusal,
            design_id: m.design_id,
            stale,
            snippet,
        }
    }
}

#[derive(Debug, FromQueryResult)]
pub(crate) struct LatestRow {
    name: String,
    id: Uuid,
    request_id: Uuid,
    status: String,
    created_at: DateTime<FixedOffset>,
    refusal: Option<String>,
    mde: Option<f64>,
    mde_unit_day: Option<f64>,
    proposals_count: i64,
}

/// Newest proposal per experiment, and how many are `done`, in one pass.
/// `DISTINCT ON` runs after the window, so `proposals_count` is the per-name total.
const LATEST_SQL: &str = "SELECT DISTINCT ON (name) \
        name, id, request_id, status, created_at, refusal, \
        (power->>'mde')::float8 AS mde, mde_unit_day, \
        COUNT(*) FILTER (WHERE status = 'done') OVER (PARTITION BY name) AS proposals_count \
    FROM experiment_proposals WHERE workspace_id = $1 \
    ORDER BY name, created_at DESC";

/// Every experiment's newest proposal and `done` count, by name.
pub(crate) async fn latest_proposals(
    db: &DatabaseConnection,
    workspace_id: Uuid,
) -> Result<HashMap<String, LatestRow>, DbErr> {
    let stopped = proposal_store::stopped_jobs(db, workspace_id).await?;
    Ok(LatestRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        LATEST_SQL,
        [workspace_id.into()],
    ))
    .all(db)
    .await?
    .into_iter()
    .map(|mut r| {
        overlay_stopped(&stopped, r.id, &mut r.status, &mut r.refusal);
        (r.name.clone(), r)
    })
    .collect())
}

impl LatestRow {
    pub(crate) fn count(&self) -> u64 {
        self.proposals_count as u64
    }

    pub(crate) fn summary(&self) -> ProposalSummary {
        ProposalSummary {
            id: self.id,
            request_id: self.request_id,
            status: self.status.clone(),
            created_at: self.created_at,
            mde: self.mde,
            mde_unit_day: self.mde_unit_day,
            refusal: self.refusal.clone(),
        }
    }
}

/// The earliest wave switch (or switchback period start) at or after `now` (local).
pub(crate) fn next_switch(cfg: &ExperimentConfig, now: Moment) -> Option<Moment> {
    let waves = cfg.design.waves.iter().map(|w| w.switch_on);
    let periods = cfg
        .design
        .switchback
        .iter()
        .flat_map(|s| s.periods.iter().map(|p| p.from));
    waves.chain(periods).filter(|m| *m >= now).min()
}

/// Report a row whose price job stopped without settling it
/// ([`proposal_store::stopped_jobs`]) as `failed`, with the reason. Only a row
/// still `pending` as read: a settle can land between the two queries, and the
/// row's own settled status then wins.
fn overlay_stopped(
    stopped: &HashMap<Uuid, String>,
    id: Uuid,
    status: &mut String,
    refusal: &mut Option<String>,
) {
    if status != ProposalStatus::Pending.as_str() {
        return;
    }
    if let Some(reason) = stopped.get(&id) {
        *status = ProposalStatus::Failed.as_str().to_string();
        *refusal = Some(reason.clone());
    }
}

/// Every proposal for one experiment, newest first, capped at [`PAGE`]. A
/// `pending` row whose price job stopped without settling it reads `failed`,
/// with the reason ([`proposal_store::stopped_jobs`]).
pub async fn list_proposal_views(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    name: &str,
) -> Result<Vec<Proposal>, DbErr> {
    let stopped = proposal_store::stopped_jobs(db, workspace_id).await?;
    let current = match design_store::experiment_by_name(db, workspace_id, name).await? {
        Some(e) => design_store::design_of(db, e.id)
            .await?
            .map(|d| d.content_hash),
        None => None,
    };
    Ok(proposal_store::list_proposals(db, workspace_id, name, PAGE)
        .await?
        .into_iter()
        .map(|mut m| {
            overlay_stopped(&stopped, m.id, &mut m.status, &mut m.refusal);
            Proposal::from_row(m, current.as_deref())
        })
        .collect())
}

/// The most proposals one response carries. `proposals_count` on the list is
/// the full `done` total.
const PAGE: u64 = 200;

#[cfg(test)]
mod tests {
    use chrono::TimeZone;
    use entity::experiment_proposals;
    use serde_json::json;
    use uuid::Uuid;

    use super::*;

    fn row(status: &str) -> experiment_proposals::Model {
        let at = chrono::Utc
            .with_ymd_and_hms(2026, 9, 30, 12, 0, 0)
            .unwrap()
            .fixed_offset();
        experiment_proposals::Model {
            id: Uuid::from_u128(1),
            created_at: at,
            workspace_id: Uuid::from_u128(2),
            name: "promo".to_string(),
            design_id: Some(Uuid::from_u128(3)),
            design_hash: Some("h".into()),
            request_id: Uuid::from_u128(5),
            status: status.to_string(),
            request: json!({"shape": "waves", "wave_sizes": [4], "first_switch_on": "2026-09-07", "spacing_days": 0}),
            alpha: 0.05,
            seed: None,
            waves: None,
            periods: None,
            strata: None,
            assign_by: None,
            power: None,
            mde_unit_day: None,
            prior_coefficient: None,
            warnings: json!([]),
            refusal: None,
            requested_by: Uuid::from_u128(4),
            finished_at: None,
        }
    }

    fn done() -> experiment_proposals::Model {
        experiment_proposals::Model {
            seed: Some(8841207),
            waves: Some(json!([{"switch_on": "2026-09-07", "units": ["bondi", "newtown"]}])),
            // `mde_relative` is null: an infinite value is stored as null (index C2b).
            power: Some(json!({"mde": 17.5, "mde_relative": null, "baseline": 155.0})),
            mde_unit_day: Some(17.5),
            warnings: json!(["washout_days is 0"]),
            finished_at: Some(row("done").created_at),
            ..row("done")
        }
    }

    fn parsed(snippet: &str) -> serde_yaml::Value {
        serde_yaml::from_str(snippet).expect("the snippet is YAML")
    }

    #[test]
    fn the_stopped_task_overlay_applies_only_to_a_row_still_pending() {
        // A settle can land between the stopped-jobs query and the row read;
        // the row's own settled status then wins.
        let stopped = HashMap::from([(Uuid::from_u128(1), "stopped".to_string())]);
        let (mut status, mut refusal) = ("done".to_string(), None);
        overlay_stopped(&stopped, Uuid::from_u128(1), &mut status, &mut refusal);
        assert_eq!((status.as_str(), refusal), ("done", None));

        let (mut status, mut refusal) = ("pending".to_string(), None);
        overlay_stopped(&stopped, Uuid::from_u128(2), &mut status, &mut refusal);
        assert_eq!((status.as_str(), refusal), ("pending", None), "not stopped");

        let (mut status, mut refusal) = ("pending".to_string(), None);
        overlay_stopped(&stopped, Uuid::from_u128(1), &mut status, &mut refusal);
        assert_eq!(
            (status.as_str(), refusal.as_deref()),
            ("failed", Some("stopped"))
        );
    }

    #[test]
    fn a_done_row_renders_its_numbers_and_a_snippet() {
        let p = Proposal::from_row(done(), None);
        assert_eq!(
            (p.seed, p.mde, p.baseline, p.mde_unit_day),
            (Some(8841207), Some(17.5), Some(155.0), Some(17.5))
        );
        assert_eq!(p.mde_relative, None, "a null stays null, never zero");
        assert_eq!((p.alpha, p.request_id), (0.05, Uuid::from_u128(5)));
        assert_eq!(p.warnings, ["washout_days is 0"]);
        let snippet = p.snippet.expect("a done row has a snippet");
        assert!(
            snippet.contains("seed: 8841207") && snippet.contains("mde_at_proposal: 17.5"),
            "{snippet}"
        );
        assert!(snippet.contains("switch_on: 2026-09-07"), "{snippet}");
    }

    #[test]
    fn a_pending_or_refused_row_has_no_numbers_and_no_snippet() {
        let mut refused = row("refused");
        refused.refusal = Some("no history for m over e".to_string());
        for r in [row("pending"), refused] {
            let p = Proposal::from_row(r, None);
            assert_eq!(
                (
                    p.seed,
                    p.mde,
                    p.mde_relative,
                    p.baseline,
                    p.mde_unit_day,
                    p.snippet
                ),
                (None, None, None, None, None, None)
            );
            assert!(p.waves.is_none() && p.periods.is_none());
            assert_eq!(
                p.alpha, 0.05,
                "the alpha is known from the moment the row is inserted"
            );
        }
    }

    #[test]
    fn the_snippet_renders_every_output_key_a_group_proposal_has() {
        let m = experiment_proposals::Model {
            request: json!({"shape": "waves", "wave_sizes": [1], "first_switch_on": "2026-09-07", "spacing_days": 0,
                            "blocks": 2, "assign_by": "sku.category", "alpha": 0.2}),
            alpha: 0.2,
            assign_by: Some("sku.category".to_string()),
            strata: Some(json!([["books", "toys"], ["games", "tools"]])),
            prior_coefficient: Some(json!({"value": 4.2, "source": "fitted"})),
            ..done()
        };
        let v = parsed(&Proposal::from_row(m, None).snippet.expect("snippet"));
        assert_eq!(v["assign_by"].as_str(), Some("sku.category"));
        assert_eq!(
            v["design"]["waves"][0]["switch_on"].as_str(),
            Some("2026-09-07")
        );
        assert_eq!(v["design"]["strata"][1][1].as_str(), Some("tools"));
        assert_eq!(v["design"]["seed"].as_u64(), Some(8841207));
        assert_eq!(v["outcome"]["alpha"].as_f64(), Some(0.2));
        assert_eq!(v["outcome"]["mde_at_proposal"].as_f64(), Some(17.5));
        assert_eq!(
            v["outcome"]["prior_coefficient"]["source"].as_str(),
            Some("fitted")
        );
    }

    #[test]
    fn alpha_is_written_only_when_the_request_overrode_it() {
        let v = parsed(&Proposal::from_row(done(), None).snippet.expect("snippet"));
        assert!(
            v["outcome"].get("alpha").is_none(),
            "the file's own alpha stands"
        );
        assert!(v.get("assign_by").is_none() && v["design"].get("strata").is_none());
        assert!(v["outcome"].get("prior_coefficient").is_none());
    }

    #[test]
    fn a_switchback_row_renders_periods_with_the_requests_period_days() {
        let m = experiment_proposals::Model {
            request: json!({"shape": "switchback", "first_period_on": "2026-09-07", "period_days": 14, "pairs": 1}),
            waves: None,
            periods: Some(
                json!([{"from": "2026-09-07", "state": "off"}, {"from": "2026-09-21", "state": "on"}]),
            ),
            ..done()
        };
        let p = Proposal::from_row(m, None);
        assert_eq!(p.periods.as_ref().unwrap()[1]["state"], "on");
        let v = parsed(&p.snippet.expect("snippet"));
        assert_eq!(v["design"]["switchback"]["period_days"].as_u64(), Some(14));
        assert_eq!(
            v["design"]["switchback"]["periods"][0]["state"].as_str(),
            Some("off")
        );
        assert!(v["design"].get("waves").is_none());
    }

    #[test]
    fn the_proposal_json_has_exactly_the_c5_keys() {
        let v = serde_json::to_value(Proposal::from_row(done(), None)).unwrap();
        let mut keys: Vec<_> = v.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        assert_eq!(
            keys,
            [
                "alpha",
                "assign_by",
                "baseline",
                "created_at",
                "design_id",
                "finished_at",
                "id",
                "mde",
                "mde_relative",
                "mde_unit_day",
                "periods",
                "prior_coefficient",
                "refusal",
                "request",
                "request_id",
                "seed",
                "snippet",
                "stale",
                "status",
                "strata",
                "warnings",
                "waves"
            ]
        );
        assert!(v["mde_relative"].is_null());
        assert_eq!(v["request"]["spacing_days"], 0);
    }

    #[test]
    fn the_list_item_json_has_exactly_the_c5_keys() {
        let item = ExperimentListItem {
            name: "promo".into(),
            owner: "ops".into(),
            hypothesis: "h".into(),
            design_id: Uuid::from_u128(9),
            design_hash: "h".into(),
            design_frozen_at: None,
            status: ExperimentStatus::Draft,
            registration_refusal: None,
            next_switch_on: None,
            proposals_count: 0,
            latest_proposal: None,
            definition: json!({}),
            retrospective: false,
            retrospective_ready: false,
            report_glance: None,
        };
        let v = serde_json::to_value(item).unwrap();
        let mut keys: Vec<_> = v.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        assert_eq!(
            keys,
            [
                "definition",
                "design_frozen_at",
                "design_hash",
                "design_id",
                "hypothesis",
                "latest_proposal",
                "name",
                "next_switch_on",
                "owner",
                "proposals_count",
                "registration_refusal",
                "report_glance",
                "retrospective",
                "retrospective_ready",
                "status"
            ]
        );
        assert_eq!(v["status"], "draft");
    }
}
