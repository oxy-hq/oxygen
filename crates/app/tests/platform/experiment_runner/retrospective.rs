//! A retrospective run: a design whose dates are all past, analysed from the warehouse.
//! It is created `closing`, moves no lever, and is reported labelled as not
//! pre-registered. `DESIGN`'s last post window ends (exclusive) on 2026-05-18 in
//! Sydney: the second wave's 2026-03-16 switch + 7 washout + 56 post days.

use oxy_app::server::experiment::jobs::{CHECK_KIND, ESTIMATE_KIND};
use oxy_app::server::experiment::register::RegisterError;
use oxy_app::server::experiment::retrospective::{Retrospective, judge};

use super::fixture::*;

#[tokio::test]
async fn refuses_while_the_last_window_is_not_past() {
    let fx = Fx::seed(DESIGN).await;
    // 2026-05-17 in Sydney: the window's last day has not ended.
    let Err(RegisterError::Refused(reason)) =
        fx.retrospective_at(utc("2026-05-17T00:00:00Z")).await
    else {
        panic!("a window still open is a registration, not a retrospective");
    };
    assert!(reason.contains("2026-05-18"), "{reason}");
    assert!(fx.run_opt().await.is_none(), "a refusal is never a run");
}

#[tokio::test]
async fn creates_a_closing_run_once_the_window_is_past_without_resolving_a_lever() {
    // The levers were renamed since: a retrospective sends nothing, so it never asks.
    let fx = Fx::seed_with_levers(DESIGN, &[]).await;
    let at = utc("2026-06-01T00:00:00Z");
    let run = fx
        .retrospective_at(at)
        .await
        .expect("past, so retrospective");
    assert_eq!(run.state, "closing");
    assert!(run.retrospective);
    assert_eq!(run.registered_by, fx.owner);
    assert_eq!(run.registered_at.with_timezone(&chrono::Utc), at);
    assert_eq!(
        run.control_pool, None,
        "the estimate pins it, from the design's dates"
    );
}

#[tokio::test]
async fn the_tick_enqueues_the_estimate_and_never_a_flip_or_a_check() {
    let fx = Fx::seed(DESIGN).await;
    let at = utc("2026-06-01T00:00:00Z");
    fx.retrospective_at(at).await.expect("created");
    fx.tick(at).await;
    fx.tick(at + chrono::Duration::hours(2)).await;
    assert!(
        fx.flips().await.is_empty(),
        "a retrospective run moves no lever"
    );
    assert_eq!(fx.lever_runs("bondi", "off").await, 0);
    assert_eq!(fx.queued_kind(CHECK_KIND).await, 0);
    assert_eq!(fx.queued_kind(ESTIMATE_KIND).await, 1);
    assert_eq!(fx.run().await.state, "closing");
}

#[tokio::test]
async fn one_run_per_experiment_whichever_way_it_was_made() {
    let fx = Fx::seed(DESIGN).await;
    fx.retrospective_at(utc("2026-06-01T00:00:00Z"))
        .await
        .expect("created");
    assert!(matches!(
        fx.retrospective_at(utc("2026-06-02T00:00:00Z")).await,
        Err(RegisterError::AlreadyRegistered(_))
    ));

    let registered = Fx::seed(DESIGN).await;
    registered.register().await;
    assert!(matches!(
        registered
            .retrospective_at(utc("2026-06-01T00:00:00Z"))
            .await,
        Err(RegisterError::AlreadyRegistered(_))
    ));
}

#[tokio::test]
async fn a_draft_is_refused_as_a_draft() {
    let fx = Fx::seed(&DESIGN.replace("  seed: 8841207\n", "")).await;
    assert!(matches!(
        fx.retrospective_at(utc("2026-06-01T00:00:00Z")).await,
        Err(RegisterError::Draft(_))
    ));
}

#[tokio::test]
async fn the_status_offers_it_only_once_the_window_is_past() {
    let fx = Fx::seed(DESIGN).await;
    let c = fx.latest_config().await;
    assert!(matches!(
        judge(&c, utc("2026-05-17T00:00:00Z")),
        Retrospective::Refused { .. }
    ));
    assert_eq!(judge(&c, utc("2026-06-01T00:00:00Z")), Retrospective::Ready);

    let open = fx.status_at(utc("2026-05-17T00:00:00Z")).await;
    assert!(open.retrospective_refusal.is_some());
    let past = fx.status_at(utc("2026-06-01T00:00:00Z")).await;
    assert!(past.retrospective_ready);

    fx.retrospective_at(utc("2026-06-01T00:00:00Z"))
        .await
        .expect("created");
    let s = fx.status_at(utc("2026-06-01T01:00:00Z")).await;
    let run = s.run.expect("a run");
    assert!(run.retrospective);
    assert!(
        !run.problems.iter().any(|p| p.contains("not pinned yet")),
        "the estimate pins a retrospective run, no check: {:?}",
        run.problems
    );
}

/// Seal `fx`'s run as the estimate would: a results row whose report carries
/// `refusal` (or none), and the run `reported`.
async fn seal(fx: &Fx, refusal: Option<&str>) {
    let report = serde_json::json!({ "refusal": refusal });
    fx.exec(
        "INSERT INTO experiment_results (id, run_id, effect, compliance, report, aborted, created_at) \
         SELECT gen_random_uuid(), id, '{}'::jsonb, '{}'::jsonb, $1, false, now() \
         FROM experiment_runs WHERE workspace_id = $2",
        [report.into(), fx.ws.into()],
    )
    .await;
    fx.exec(
        "UPDATE experiment_runs SET state = 'reported' WHERE workspace_id = $1",
        [fx.ws.into()],
    )
    .await;
}

async fn edit(fx: &Fx) -> Result<(), oxy_app::server::experiment::design_store::DesignError> {
    edit_as(fx, true).await
}

/// An edit by someone who may (`ExperimentRegister`) or may not reopen.
async fn edit_as(
    fx: &Fx,
    may_reopen: bool,
) -> Result<(), oxy_app::server::experiment::design_store::DesignError> {
    use oxy_app::server::experiment::design_store::{Edit, update_draft};
    let mut config = fx.latest_config().await;
    config.hypothesis = "edited after the refusal".into();
    update_draft(
        &fx.db,
        fx.ws,
        NAME,
        Edit {
            config,
            base_hash: None,
            updated_by: fx.owner,
            may_reopen,
        },
    )
    .await
    .map(|_| ())
}

#[tokio::test]
async fn a_refused_retrospective_reopens_its_design_for_an_edit_and_a_new_run() {
    let fx = Fx::seed(DESIGN).await;
    let first = fx
        .retrospective_at(utc("2026-06-01T00:00:00Z"))
        .await
        .expect("created");
    seal(
        &fx,
        Some("assignment names unit 'x', which is not in the panel"),
    )
    .await;

    edit(&fx)
        .await
        .expect("a refused retrospective leaves the design editable");
    assert!(fx.run_opt().await.is_none(), "the refused run is gone");

    let second = fx
        .retrospective_at(utc("2026-06-02T00:00:00Z"))
        .await
        .expect("analysed again");
    assert_ne!(first.id, second.id);
    assert_eq!(
        fx.latest_config().await.hypothesis,
        "edited after the refusal"
    );
}

#[tokio::test]
async fn an_editor_who_may_not_register_cannot_reopen_an_admins_refused_retrospective() {
    // PUT design is the editor ring; the run it would delete took the admin ring.
    let fx = Fx::seed(DESIGN).await;
    let run = fx
        .retrospective_at(utc("2026-06-01T00:00:00Z"))
        .await
        .expect("created");
    seal(&fx, Some("the window layout does not fit")).await;

    let err = edit_as(&fx, false)
        .await
        .expect_err("an editor below the admin ring meets a frozen design");
    assert!(
        matches!(
            err,
            oxy_app::server::experiment::design_store::DesignError::Frozen(_)
        ),
        "{err:?}"
    );
    assert_eq!(
        fx.run_opt().await.map(|r| r.id),
        Some(run.id),
        "the refused run and its report survive"
    );
}

#[tokio::test]
async fn a_refused_retrospective_can_be_run_again_without_an_edit() {
    let fx = Fx::seed(DESIGN).await;
    fx.retrospective_at(utc("2026-06-01T00:00:00Z"))
        .await
        .expect("created");
    seal(&fx, Some("the window layout does not fit")).await;
    fx.retrospective_at(utc("2026-06-02T00:00:00Z"))
        .await
        .expect("the data was fixed, so it is read again");
}

#[tokio::test]
async fn a_reported_retrospective_stays_frozen() {
    let fx = Fx::seed(DESIGN).await;
    fx.retrospective_at(utc("2026-06-01T00:00:00Z"))
        .await
        .expect("created");
    seal(&fx, None).await;
    assert!(edit(&fx).await.is_err(), "a result is final");
    assert!(matches!(
        fx.retrospective_at(utc("2026-06-02T00:00:00Z")).await,
        Err(RegisterError::AlreadyRegistered(_))
    ));
}

#[tokio::test]
async fn a_refused_registered_run_is_not_reopened() {
    let fx = Fx::seed(DESIGN).await;
    fx.register().await;
    seal(&fx, Some("a registered refusal is part of the record")).await;
    assert!(edit(&fx).await.is_err());
}

#[tokio::test]
async fn only_a_refused_retrospective_is_offered_for_reopening() {
    use oxy_app::server::experiment::reopen::is_reopenable;
    let fx = Fx::seed(DESIGN).await;
    let id = fx.experiment().await.id;
    assert!(
        !is_reopenable(&fx.db, id).await.unwrap(),
        "a draft is just editable"
    );
    fx.retrospective_at(utc("2026-06-01T00:00:00Z"))
        .await
        .expect("created");
    seal(
        &fx,
        Some("assignment names unit 'x', which is not in the panel"),
    )
    .await;
    assert!(is_reopenable(&fx.db, id).await.unwrap());

    let reported = Fx::seed(DESIGN).await;
    reported
        .retrospective_at(utc("2026-06-01T00:00:00Z"))
        .await
        .expect("created");
    seal(&reported, None).await;
    let rid = reported.experiment().await.id;
    assert!(!is_reopenable(&reported.db, rid).await.unwrap());
}
