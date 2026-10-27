//! Registration is an explicit action that freezes the draft design. The deadline for
//! `DESIGN` is 2026-02-26T13:00Z (the start of 2026-02-27 in Sydney: the first
//! switch, 2026-03-02, minus three anticipation days).

use oxy_app::server::experiment::design_store;
use oxy_app::server::experiment::register::{RegisterError, Registration, decide};

use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};

use super::fixture::*;

#[tokio::test]
async fn registers_in_time_freezing_the_design_and_pinning_who_registered() {
    let fx = Fx::seed(DESIGN).await;
    let at = utc("2026-02-10T00:00:00Z");
    let run = fx.register_at(at).await.expect("in time");
    assert_eq!(run.state, "registered");
    assert_eq!(run.registered_at.with_timezone(&chrono::Utc), at);
    assert_eq!(run.registered_by, fx.owner);
    let design = design_store::design_of(&fx.db, fx.experiment().await.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(run.design_id, design.id);
    assert_eq!(run.experiment_id, design.experiment_id);
    assert_eq!(
        design.frozen_at,
        Some(run.registered_at),
        "frozen at registration"
    );
}

#[tokio::test]
async fn refuses_at_the_deadline() {
    let fx = Fx::seed(DESIGN).await;
    let deadline = utc("2026-02-26T13:00:00Z");
    let Err(RegisterError::Refused(reason)) = fx.register_at(deadline).await else {
        panic!("strictly before, so the deadline itself is late");
    };
    assert!(
        reason.contains("not before the registration deadline"),
        "{reason}"
    );
    assert!(fx.run_opt().await.is_none(), "a refusal is never a run");
    // One second earlier is in time.
    fx.register_at(deadline - chrono::Duration::seconds(1))
        .await
        .expect("just in time");
}

#[tokio::test]
async fn a_lever_that_does_not_resolve_refuses_until_fixed() {
    // `promo_on` was renamed between pricing and registration.
    let fx = Fx::seed_with_levers(DESIGN, &["promo_start", "promo_off"]).await;
    let Err(RegisterError::Refused(reason)) = fx.register_at(utc("2026-02-10T00:00:00Z")).await
    else {
        panic!("expected a refusal");
    };
    assert!(
        reason.starts_with("lever.on: ") && reason.contains("promo_on"),
        "{reason}"
    );
    assert!(fx.run_opt().await.is_none());
    let c = fx.latest_config().await;
    assert!(matches!(
        decide(&fx.db, fx.ws, &c, utc("2026-02-10T00:00:00Z"))
            .await
            .unwrap(),
        Registration::Refused { .. }
    ));

    // The lever is back before the deadline: the same version now registers.
    fx.promote_bare(utc("2026-02-12T00:00:00Z"), LEVERS).await;
    fx.register_at(utc("2026-02-12T01:00:00Z"))
        .await
        .expect("the lever resolves");
}

#[tokio::test]
async fn a_lever_that_never_reads_the_unit_is_refused() {
    // Each per-unit flip would run an automation that switches every store.
    let fx = Fx::seed(DESIGN).await;
    let mut ignores_unit = lever("promo_on");
    ignores_unit["tasks"][0]["sql_query"] = "UPDATE promo SET state = '{{ state }}'".into();
    fx.db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "UPDATE automation_definitions SET definition = $1 \
             WHERE revision_id = $2 AND name = 'promo_on'",
            [ignores_unit.into(), fx.head().await.into()],
        ))
        .await
        .expect("edit the lever");
    let Err(RegisterError::Refused(reason)) = fx.register_at(utc("2026-02-10T00:00:00Z")).await
    else {
        panic!("expected a refusal");
    };
    assert!(
        reason.starts_with("lever.on: ")
            && reason.contains("`promo_on`")
            && reason.contains("`{{ unit }}`"),
        "{reason}"
    );
    assert!(fx.run_opt().await.is_none());
}

#[tokio::test]
async fn a_draft_is_refused_as_a_draft() {
    let fx = Fx::seed(&DESIGN.replace("  seed: 8841207\n", "")).await;
    let Err(RegisterError::Draft(problems)) = fx.register_at(utc("2026-02-10T00:00:00Z")).await
    else {
        panic!("a draft is not registrable");
    };
    assert!(problems.iter().any(|p| p.contains("seed")), "{problems:?}");
    assert!(fx.run_opt().await.is_none());
}

#[tokio::test]
async fn a_second_registration_is_refused_and_leaves_the_run_alone() {
    let fx = Fx::seed(DESIGN).await;
    let first = fx.register().await;
    let Err(RegisterError::AlreadyRegistered(name)) =
        fx.register_at(utc("2026-02-11T00:00:00Z")).await
    else {
        panic!("one run per experiment, ever");
    };
    assert_eq!(name, NAME);
    assert_eq!(fx.run().await, first);
}

#[tokio::test]
async fn a_design_promoted_after_registration_never_reaches_the_run() {
    let fx = Fx::seed(DESIGN).await;
    let run = fx.register().await;
    let edited = DESIGN.replace("pre_days: 56", "pre_days: 49");
    fx.promote(utc("2026-02-12T00:00:00Z"), &edited, LEVERS)
        .await;
    let after = fx.run().await;
    assert_eq!(after.design_id, run.design_id);
    assert_eq!(
        fx.design().await.design.pre_days,
        Some(56),
        "the design is frozen"
    );
}
