//! An experiment has one design: a draft edited in place until Register
//! freezes it. The database refuses an UPDATE of a frozen design, two editors
//! working from the same draft cannot both save, and adopting a proposal writes
//! pricing's output keys onto the draft it priced.

use oxy_app::server::experiment::adopt::{AdoptError, adopt_proposal};
use oxy_app::server::experiment::design_store::{self, DesignError, Edit};
use oxy_app::server::experiment::register::RegisterError;
use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};
use serde_json::json;
use uuid::Uuid;

use super::fixture::*;

fn edited(pre_days: u32) -> oxy_experiment::ExperimentConfig {
    oxy_experiment::config::parse(&DESIGN.replace("pre_days: 56", &format!("pre_days: {pre_days}")))
        .unwrap()
}

fn edit(config: oxy_experiment::ExperimentConfig, by: Uuid) -> Edit {
    Edit {
        config,
        base_hash: None,
        updated_by: by,
        may_reopen: true,
    }
}

async fn draft(fx: &Fx) -> entity::experiment_designs::Model {
    design_store::design_of(&fx.db, fx.experiment().await.id)
        .await
        .unwrap()
        .expect("a design")
}

async fn update_raises(fx: &Fx, sql: &str) -> String {
    fx.db
        .execute_raw(Statement::from_string(DatabaseBackend::Postgres, sql))
        .await
        .expect_err("the update is refused")
        .to_string()
}

#[tokio::test]
async fn a_draft_updates_but_a_frozen_design_or_a_result_raises() {
    let fx = Fx::seed(DESIGN).await;
    fx.exec(
        "UPDATE experiment_designs SET content_hash = 'x'",
        [] as [sea_orm::Value; 0],
    )
    .await;
    let run = fx.register().await;
    let err = update_raises(&fx, "UPDATE experiment_designs SET content_hash = 'y'").await;
    assert!(err.contains("frozen"), "{err}");
    // The trigger is per row: a sealed result needs one to refuse.
    fx.exec(
        "INSERT INTO experiment_results (id, run_id, aborted, report, effect, compliance) \
         VALUES (gen_random_uuid(), $1, false, '{}'::jsonb, '{}'::jsonb, '{}'::jsonb)",
        [run.id.into()],
    )
    .await;
    let err = update_raises(&fx, "UPDATE experiment_results SET aborted = true").await;
    assert!(err.contains("insert-only"), "{err}");
}

#[tokio::test]
async fn an_edit_overwrites_the_draft_in_place() {
    let fx = Fx::seed(DESIGN).await;
    let before = draft(&fx).await;
    let after = design_store::update_draft(&fx.db, fx.ws, NAME, edit(edited(49), fx.owner))
        .await
        .unwrap();
    assert_eq!(after.row.id, before.id, "the same row, not a new one");
    assert_ne!(after.row.content_hash, before.content_hash);
    assert_eq!(after.config.design.pre_days, Some(49));
    assert_eq!(after.row.updated_by, fx.owner);
    assert!(after.row.frozen_at.is_none());
    assert_eq!(draft(&fx).await.config, after.row.config);
}

#[tokio::test]
async fn two_editors_from_the_same_draft_cannot_both_save() {
    let fx = Fx::seed(DESIGN).await;
    let base = draft(&fx).await.content_hash;
    let writes = (0..8u32).map(|i| {
        let db = fx.db.clone();
        let (ws, by, base) = (fx.ws, fx.owner, base.clone());
        tokio::spawn(async move {
            let e = Edit {
                base_hash: Some(base),
                ..edit(edited(40 + i), by)
            };
            design_store::update_draft(&db, ws, NAME, e).await
        })
    });
    let results: Vec<_> = futures::future::join_all(writes)
        .await
        .into_iter()
        .map(Result::unwrap)
        .collect();
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    assert!(
        results
            .iter()
            .filter_map(|r| r.as_ref().err())
            .all(|e| matches!(e, DesignError::Stale { .. }))
    );
}

#[tokio::test]
async fn register_freezes_the_draft_and_a_frozen_design_is_duplicated_not_edited() {
    let fx = Fx::seed(DESIGN).await;
    let run = fx.register().await;
    let frozen = draft(&fx).await;
    assert_eq!(run.design_id, frozen.id);
    assert_eq!(frozen.frozen_at, Some(run.registered_at));
    assert!(matches!(
        design_store::update_draft(&fx.db, fx.ws, NAME, edit(edited(49), fx.owner)).await,
        Err(DesignError::Frozen(_))
    ));
    let (copy, design) = design_store::duplicate(&fx.db, fx.ws, NAME, "promo_again", fx.owner)
        .await
        .unwrap();
    assert_eq!(copy.name, "promo_again");
    assert_eq!(design.config.experiment, "promo_again");
    assert!(design.row.frozen_at.is_none());
    assert_eq!(design.config.design, frozen_config(&frozen).design);
    assert!(matches!(
        design_store::duplicate(&fx.db, fx.ws, NAME, "promo_again", fx.owner).await,
        Err(DesignError::Duplicate(_))
    ));
}

fn frozen_config(row: &entity::experiment_designs::Model) -> oxy_experiment::ExperimentConfig {
    serde_json::from_value(row.config.clone()).unwrap()
}

#[tokio::test]
async fn register_refuses_a_draft_that_changed_since_it_was_read() {
    let fx = Fx::seed(DESIGN).await;
    let read = draft(&fx).await.content_hash;
    design_store::update_draft(&fx.db, fx.ws, NAME, edit(edited(49), fx.owner))
        .await
        .unwrap();
    let experiment = fx.experiment().await;
    let err = oxy_app::server::experiment::register::register(
        &fx.db,
        &experiment,
        Some(&read),
        fx.owner,
        utc("2026-02-10T00:00:00Z"),
    )
    .await
    .expect_err("the draft moved");
    assert!(matches!(err, RegisterError::Changed { .. }), "{err}");
    assert!(draft(&fx).await.frozen_at.is_none(), "nothing was frozen");
}

#[tokio::test]
async fn an_invalid_design_or_a_renamed_one_is_refused() {
    let fx = Fx::seed(DESIGN).await;
    let renamed = oxy_experiment::config::parse(
        &DESIGN.replace("experiment: beverage_promo_pilot", "experiment: other"),
    )
    .unwrap();
    assert!(matches!(
        design_store::update_draft(&fx.db, fx.ws, NAME, edit(renamed, fx.owner)).await,
        Err(DesignError::Invalid(_))
    ));
    assert!(matches!(
        design_store::create_experiment(&fx.db, fx.ws, edited(56), fx.owner).await,
        Err(DesignError::Duplicate(_))
    ));
}

/// A proposal that priced the draft as it is now, with the output keys pricing writes.
async fn seed_proposal(fx: &Fx, status: &str) -> Uuid {
    let d = draft(fx).await;
    let id = Uuid::new_v4();
    fx.exec(
        "INSERT INTO experiment_proposals (id, workspace_id, name, design_id, design_hash, \
           request_id, status, request, alpha, seed, waves, power, mde_unit_day, requested_by) \
         VALUES ($1, $2, $3, $4, $5, gen_random_uuid(), $6, $7, 0.05, 42, $8, $9, 0.5, $10)",
        [
            id.into(),
            fx.ws.into(),
            NAME.into(),
            d.id.into(),
            d.content_hash.into(),
            status.into(),
            json!({"shape": "waves", "wave_sizes": [2, 2], "first_switch_on": "2026-03-09",
                   "spacing_days": 14})
            .into(),
            json!([{"switch_on": "2026-03-09", "units": ["bondi", "manly"]},
                   {"switch_on": "2026-03-23", "units": ["newtown", "surry_hills"]}])
            .into(),
            json!({"mde": 12.5}).into(),
            fx.owner.into(),
        ],
    )
    .await;
    id
}

#[tokio::test]
async fn adopting_a_proposal_writes_its_output_keys_onto_the_draft() {
    let fx = Fx::seed(&DESIGN.replace("  seed: 8841207\n", "")).await;
    let before = draft(&fx).await;
    let proposal = seed_proposal(&fx, "done").await;
    let adopted = adopt_proposal(&fx.db, fx.ws, NAME, proposal, fx.owner)
        .await
        .expect("adopted");
    assert_eq!(adopted.row.id, before.id, "in place");
    assert_eq!(adopted.row.from_proposal_id, Some(proposal));
    assert_eq!(adopted.config.design.seed, Some(42));
    assert_eq!(
        adopted.config.design.waves[0].switch_on.to_string(),
        "2026-03-09"
    );
    assert_eq!(adopted.config.outcome.mde_at_proposal, Some(12.5));
    assert!(oxy_experiment::is_registrable(&adopted.config).is_ok());
}

#[tokio::test]
async fn adopting_refuses_a_proposal_not_done_or_priced_before_an_edit() {
    let fx = Fx::seed(&DESIGN.replace("  seed: 8841207\n", "")).await;
    let pending = seed_proposal(&fx, "pending").await;
    assert!(matches!(
        adopt_proposal(&fx.db, fx.ws, NAME, pending, fx.owner).await,
        Err(AdoptError::NotDone { .. })
    ));
    let done = seed_proposal(&fx, "done").await;
    design_store::update_draft(&fx.db, fx.ws, NAME, edit(edited(49), fx.owner))
        .await
        .unwrap();
    assert!(matches!(
        adopt_proposal(&fx.db, fx.ws, NAME, done, fx.owner).await,
        Err(AdoptError::Stale { .. })
    ));
    assert!(matches!(
        adopt_proposal(&fx.db, fx.ws, NAME, Uuid::new_v4(), fx.owner).await,
        Err(AdoptError::NoSuchProposal { .. })
    ));
}
