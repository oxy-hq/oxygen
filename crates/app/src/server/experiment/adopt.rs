//! Adopting a proposal: pricing's output keys are written onto the draft it
//! priced, in place. It replaces pasting the snippet into a file — the snippet
//! (`snippet.rs`) stays, for display only.
//!
//! Refused unless the proposal is `done` and priced the draft **as it is now**
//! (its `design_hash`): adopting a proposal priced against an earlier draft
//! would silently drop every edit made since, so it must be priced again.

use entity::experiment_proposals;
use oxy_experiment::{
    ExperimentConfig, PeriodConfig, PriorCoefficient, SwitchbackConfig, WaveConfig,
};
use sea_orm::{DatabaseConnection, TransactionTrait};
use uuid::Uuid;

use super::candidate::Candidate;
use super::design_store::{self, Design, DesignError};
use super::proposal_store::{self, ProposalStatus};
use super::snippet::{PeriodPlan, WavePlan};

#[derive(Debug, thiserror::Error)]
pub enum AdoptError {
    #[error("experiment `{name}` has no proposal {id}")]
    NoSuchProposal { name: String, id: Uuid },
    #[error("proposal {id} is {status}; only a done proposal carries a design to adopt")]
    NotDone { id: Uuid, status: String },
    #[error(
        "proposal {id} priced an earlier draft: the design was edited since, so price it again"
    )]
    Stale { id: Uuid },
    #[error("proposal {id}'s stored output cannot be read: {reason}")]
    Unreadable { id: Uuid, reason: String },
    #[error(transparent)]
    Design(#[from] DesignError),
}

impl From<sea_orm::DbErr> for AdoptError {
    fn from(e: sea_orm::DbErr) -> Self {
        Self::Design(DesignError::Db(e))
    }
}

/// Write `proposal`'s output keys onto `name`'s draft, which it must have priced.
pub async fn adopt_proposal(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    name: &str,
    proposal_id: Uuid,
    user: Uuid,
) -> Result<Design, AdoptError> {
    let proposal = proposal_store::find_proposal(db, proposal_id)
        .await?
        .filter(|p| p.workspace_id == workspace_id && p.name == name)
        .ok_or_else(|| AdoptError::NoSuchProposal {
            name: name.to_string(),
            id: proposal_id,
        })?;
    if proposal.status != ProposalStatus::Done.as_str() {
        return Err(AdoptError::NotDone {
            id: proposal.id,
            status: proposal.status,
        });
    }
    let txn = db.begin().await?;
    let (_, row) = design_store::lock_design(&txn, workspace_id, name).await?;
    let row = design_store::editable(row, name, None)?;
    if proposal.design_id != Some(row.id)
        || proposal.design_hash.as_ref() != Some(&row.content_hash)
    {
        return Err(AdoptError::Stale { id: proposal.id });
    }
    let unreadable = |reason: String| AdoptError::Unreadable {
        id: proposal.id,
        reason,
    };
    let base = design_store::decode(row.clone()).map_err(unreadable)?;
    let config = with_output_keys(&base.config, &proposal).map_err(unreadable)?;
    let design = design_store::write_draft(&txn, row, config, user, Some(proposal.id)).await?;
    txn.commit().await?;
    Ok(design)
}

fn json<T: serde::de::DeserializeOwned>(
    v: &Option<serde_json::Value>,
    what: &str,
) -> Result<Option<T>, String> {
    v.clone()
        .map(serde_json::from_value)
        .transpose()
        .map_err(|e| format!("{what}: {e}"))
}

/// Every output key of pricing, written onto the priced design: `assign_by`,
/// `design.waves | design.switchback`, `design.strata`, `design.seed`,
/// `outcome.alpha` (only when the candidate overrode it),
/// `outcome.mde_at_proposal` and `outcome.prior_coefficient`.
pub fn with_output_keys(
    base: &ExperimentConfig,
    p: &experiment_proposals::Model,
) -> Result<ExperimentConfig, String> {
    let candidate: Candidate =
        serde_json::from_value(p.request.clone()).map_err(|e| format!("request: {e}"))?;
    let seed = p.seed.ok_or("no seed")?;
    let mde = p
        .power
        .as_ref()
        .and_then(|v| v.get("mde"))
        .and_then(serde_json::Value::as_f64)
        .ok_or("no mde")?;
    let mut c = base.clone();
    match &candidate {
        Candidate::Waves(_) => {
            let waves: Vec<WavePlan> = json(&p.waves, "waves")?.ok_or("no waves")?;
            c.design.waves = waves
                .into_iter()
                .map(|w| WaveConfig {
                    switch_on: w.switch_on,
                    units: w.units,
                })
                .collect();
            c.design.switchback = None;
        }
        Candidate::Switchback(s) => {
            let periods: Vec<PeriodPlan> = json(&p.periods, "periods")?.ok_or("no periods")?;
            c.design.switchback = Some(SwitchbackConfig {
                period_days: s.period_days,
                periods: periods
                    .into_iter()
                    .map(|p| PeriodConfig {
                        from: p.from,
                        state: p.state,
                    })
                    .collect(),
            });
            c.design.waves = Vec::new();
        }
    }
    c.assign_by = p.assign_by.clone();
    c.design.strata = json(&p.strata, "strata")?;
    c.design.seed = Some(seed as u64);
    if let Some(alpha) = candidate.alpha() {
        c.outcome.alpha = alpha;
    }
    c.outcome.mde_at_proposal = Some(mde);
    c.outcome.prior_coefficient = json::<PriorCoefficient>(&p.prior_coefficient, "prior")?;
    Ok(c)
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;
    use serde_json::json;

    use super::*;
    use crate::server::experiment::test_design::design_with;

    fn proposal() -> experiment_proposals::Model {
        let at = chrono::Utc
            .with_ymd_and_hms(2026, 9, 30, 12, 0, 0)
            .unwrap()
            .fixed_offset();
        experiment_proposals::Model {
            id: Uuid::from_u128(1),
            created_at: at,
            workspace_id: Uuid::from_u128(2),
            name: "beverage_promo_pilot".into(),
            design_id: Some(Uuid::from_u128(3)),
            design_hash: Some("h".into()),
            request_id: Uuid::from_u128(4),
            status: "done".into(),
            request: json!({"shape": "waves", "wave_sizes": [1, 1],
                "first_switch_on": "2026-11-02", "spacing_days": 14, "alpha": 0.1}),
            alpha: 0.1,
            seed: Some(42),
            waves: Some(json!([{"switch_on": "2026-11-02", "units": ["bondi"]},
                               {"switch_on": "2026-11-16", "units": ["manly"]}])),
            periods: None,
            strata: Some(json!([["bondi", "manly"]])),
            assign_by: None,
            power: Some(json!({"mde": 12.5})),
            mde_unit_day: Some(0.5),
            prior_coefficient: Some(json!({"value": 0.3, "source": "fitted"})),
            warnings: json!([]),
            refusal: None,
            requested_by: Uuid::from_u128(5),
            finished_at: Some(at),
        }
    }

    #[test]
    fn adopting_writes_every_output_key_and_nothing_else() {
        let draft = design_with("  seed: 8841207\n", "");
        let c = with_output_keys(&draft, &proposal()).unwrap();
        assert_eq!(c.design.seed, Some(42));
        assert_eq!(c.design.waves.len(), 2);
        assert_eq!(c.design.waves[1].units, vec!["manly".to_string()]);
        assert_eq!(
            c.design.strata,
            Some(vec![vec!["bondi".into(), "manly".into()]])
        );
        assert_eq!(c.outcome.alpha, 0.1, "the candidate's override");
        assert_eq!(c.outcome.mde_at_proposal, Some(12.5));
        assert_eq!(c.outcome.prior_coefficient.as_ref().unwrap().value, 0.3);
        assert_eq!(c.hypothesis, draft.hypothesis);
        assert_eq!(c.lever, draft.lever);
        assert!(oxy_experiment::is_registrable(&c).is_ok());
    }
}
