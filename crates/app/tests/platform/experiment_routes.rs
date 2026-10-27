//! What surrounds the price job: registration, and (Task 9) enqueue, list and
//! proposals.

use std::path::PathBuf;

use chrono::TimeZone;
use oxy_app::server::experiment::candidate::Candidate;
use oxy_app::server::experiment::enqueue::{StartError, StartInput, start_price};
use oxy_app::server::experiment::list::list_items;
use oxy_app::server::experiment::proposal_store;
use oxy_app::server::experiment::summary::{ExperimentStatus, Proposal, list_proposal_views};
use oxy_app::server::experiment::{EXPERIMENT_PRICE_KIND, ExperimentPricePayload};
use sea_orm::{ConnectionTrait, Statement};
use uuid::Uuid;

use crate::experiment_pricing::{DRAFT, Fixture, now, seed_design};

#[test]
fn the_production_registry_registers_the_price_executor() {
    // `build_custom_task_registry` is private, so, like
    // `custom_app_functions_manual_run_guards.rs`, this reads the source: a task
    // whose kind is not registered stays queued forever, and no test that builds
    // its own registry would notice.
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/server/router/recovery.rs");
    let src = std::fs::read_to_string(path).expect("recovery.rs");
    assert!(
        src.contains("crate::server::experiment::EXPERIMENT_PRICE_KIND"),
        "kind not registered"
    );
    assert!(
        src.contains("crate::server::experiment::ExperimentPriceExecutor { db: db.clone() }"),
        "executor not registered"
    );
}

fn input(f: &Fixture, name: &str, candidates: Vec<Candidate>) -> StartInput {
    StartInput {
        workspace_id: f.ws,
        name: name.to_string(),
        requested_by: Uuid::new_v4(),
        candidates,
    }
}

async fn count(f: &Fixture, table: &str) -> i64 {
    f.db.query_one_raw(Statement::from_string(
        f.db.get_database_backend(),
        format!("SELECT count(*) AS n FROM {table}"),
    ))
    .await
    .unwrap()
    .unwrap()
    .try_get::<i64>("", "n")
    .unwrap()
}

#[tokio::test]
#[serial_test::serial]
async fn three_candidates_are_three_pending_rows_sharing_a_request_id() {
    // Review Focus 12. A unit design, a blocked one and a looser alpha, side by side.
    let f = Fixture::new("enqueue").await;
    let candidates = vec![
        f.waves(&[8]),
        f.waves_with(&[8], Some(3), None, None),
        f.waves_with(&[8], None, None, Some(0.2)),
    ];
    let input = input(&f, "promo_pilot", candidates.clone());
    let requested_by = input.requested_by;
    let started = start_price(&f.db, input).await.expect("enqueue");
    assert_eq!(started.proposal_ids.len(), 3);

    // Every row exists the instant the handler would return, in candidate order,
    // sharing the request id, holding its own candidate and its own alpha.
    let rows = proposal_store::list_request(&f.db, f.ws, started.request_id)
        .await
        .unwrap();
    assert_eq!(
        rows.iter().map(|r| r.id).collect::<Vec<_>>(),
        started.proposal_ids
    );
    for (row, candidate) in rows.iter().zip(&candidates) {
        assert_eq!(row.status, "pending");
        assert_eq!(
            (row.workspace_id, row.design_id, row.requested_by),
            (f.ws, Some(f.design), requested_by),
            "a proposal pins the design it priced"
        );
        assert_eq!(row.request, serde_json::to_value(candidate).unwrap());
        assert!(row.finished_at.is_none());
    }
    assert_eq!(
        rows.iter().map(|r| r.alpha).collect::<Vec<_>>(),
        [0.05, 0.05, 0.2],
        "an override, else the file's alpha"
    );

    // One queue row and one run row each; the queue carries only the proposal id (index C4).
    assert_eq!(
        (
            count(&f, "agentic_task_queue").await,
            count(&f, "agentic_runs").await
        ),
        (3, 3)
    );
    for id in &started.proposal_ids {
        let queued =
            f.db.query_one_raw(Statement::from_string(
                f.db.get_database_backend(),
                format!("SELECT spec::text AS spec FROM agentic_task_queue WHERE task_id = '{id}'"),
            ))
            .await
            .expect("query queue")
            .expect("the task was not enqueued");
        let spec: serde_json::Value =
            serde_json::from_str(&queued.try_get::<String>("", "spec").expect("spec"))
                .expect("json");
        assert_eq!(spec["type"], "custom", "{spec}");
        assert_eq!(spec["kind"], EXPERIMENT_PRICE_KIND);
        let payload: ExperimentPricePayload =
            serde_json::from_value(spec["payload"].clone()).expect("round trip");
        assert_eq!(payload.proposal_id, *id);
        assert_eq!(spec["payload"].as_object().unwrap().len(), 1);
        let run = agentic_runtime::crud::get_run(&f.db, &id.to_string())
            .await
            .unwrap()
            .expect("run row");
        assert_eq!(
            (run.source_type.as_deref(), run.workspace_id),
            (Some(EXPERIMENT_PRICE_KIND), f.ws)
        );
    }
}

#[tokio::test]
#[serial_test::serial]
async fn an_unknown_experiment_inserts_and_enqueues_nothing() {
    let f = Fixture::new("enqueue-missing").await;
    let err = start_price(&f.db, input(&f, "no_such", vec![f.waves(&[8])]))
        .await
        .unwrap_err();
    assert!(matches!(err, StartError::NotFound(ref n) if n == "no_such"));
    for table in ["experiment_proposals", "agentic_task_queue", "agentic_runs"] {
        assert_eq!(
            count(&f, table).await,
            0,
            "{table} was written for a missing experiment"
        );
    }
}

#[tokio::test]
#[serial_test::serial]
async fn every_done_candidate_is_a_draw() {
    // Review Focus 12. Two assignments priced, one refused (every unit placed):
    // the report will say two were drawn — choosing among candidates is choosing
    // among draws.
    let f = Fixture::new("draws").await;
    let started = start_price(
        &f.db,
        input(
            &f,
            "promo_pilot",
            vec![f.waves(&[8]), f.waves(&[24]), f.waves(&[6, 6])],
        ),
    )
    .await
    .expect("enqueue");
    for id in &started.proposal_ids {
        f.run(*id, now()).await;
    }
    let rows = proposal_store::list_request(&f.db, f.ws, started.request_id)
        .await
        .unwrap();
    assert_eq!(
        rows.iter().map(|r| r.status.as_str()).collect::<Vec<_>>(),
        ["done", "refused", "done"]
    );
    assert_eq!(f.count_done().await, 2);
}

#[tokio::test]
#[serial_test::serial]
async fn the_experiment_list_matches_the_c5_shape() {
    let f = Fixture::new("list").await;
    let none_yet = list_items(&f.db, f.ws, now()).await.unwrap();
    assert_eq!(none_yet.len(), 1);
    assert_eq!(
        (none_yet[0].name.as_str(), none_yet[0].owner.as_str()),
        ("promo_pilot", "ops")
    );
    assert_eq!(none_yet[0].status, ExperimentStatus::Draft);
    assert_eq!(
        (
            none_yet[0].proposals_count,
            none_yet[0].next_switch_on.clone(),
            none_yet[0].registration_refusal.clone()
        ),
        (0, None, None)
    );
    assert!(none_yet[0].latest_proposal.is_none());

    // One done proposal, then a refused one (newer): the count is done-only, the
    // latest is the newest of any status.
    f.price(&f.waves(&[8]), now()).await;
    let refused = f.price(&f.waves(&[24]), now()).await;
    let listed = list_items(&f.db, f.ws, now()).await.unwrap();
    assert_eq!(listed[0].proposals_count, 1);
    let latest = listed[0].latest_proposal.as_ref().expect("latest");
    assert_eq!((latest.id, latest.status.as_str()), (refused, "refused"));
    assert!(latest.refusal.as_deref().unwrap().contains("all 24 units"));
    assert!(listed[0].design_frozen_at.is_none(), "a draft");
    assert_eq!((latest.mde, latest.mde_unit_day), (None, None));

    // Another workspace lists nothing: experiments are read by workspace.
    let (other_ws, _) = crate::simulation_routes::seed_workspace(&f.db, "other-list").await;
    assert!(list_items(&f.db, other_ws, now()).await.unwrap().is_empty());
}

#[tokio::test]
#[serial_test::serial]
async fn a_file_with_waves_seed_and_mde_lists_as_registrable_with_its_next_switch() {
    let f = Fixture::new("registrable").await;
    let registrable = DRAFT
        .replace("experiment: promo_pilot", "experiment: promo_ready")
        .replace("  washout_days: 7", "  washout_days: 7\n  seed: 1\n  waves:\n    - { switch_on: 2026-09-07, units: [\"1\", \"2\"] }\n    - { switch_on: 2026-09-21, units: [\"3\"] }")
        .replace("  direction: increase", "  direction: increase\n  mde_at_proposal: 12.5");
    seed_design(&f.db, f.ws, &registrable).await;
    let items = list_items(&f.db, f.ws, now()).await.unwrap();
    let ready = items
        .iter()
        .find(|i| i.name == "promo_ready")
        .expect("listed");
    assert_eq!(ready.status, ExperimentStatus::Registrable);
    assert_eq!(ready.next_switch_on.as_deref(), Some("2026-09-07"));
    // Once the first wave is past, the next one is the next.
    let later = chrono::Utc.with_ymd_and_hms(2026, 9, 10, 0, 0, 0).unwrap();
    let items = list_items(&f.db, f.ws, later).await.unwrap();
    assert_eq!(
        items
            .iter()
            .find(|i| i.name == "promo_ready")
            .unwrap()
            .next_switch_on
            .as_deref(),
        Some("2026-09-21")
    );
}

#[tokio::test]
#[serial_test::serial]
async fn a_switchback_draft_lists_with_its_first_period_as_the_next_switch() {
    let f = Fixture::new("switchback-list").await;
    let sb = crate::experiment_pricing::switchback_draft()
        .replace("experiment: promo_pilot", "experiment: promo_sb")
        .replace(
            "    period_days: 14",
            "    period_days: 14\n    periods:\n      - { from: 2026-09-07, state: on }\n      - { from: 2026-09-21, state: off }",
        )
        .replace("  washout_days: 2", "  washout_days: 2\n  seed: 3")
        .replace("  direction: increase", "  direction: increase\n  mde_at_proposal: 4.0");
    seed_design(&f.db, f.ws, &sb).await;
    let item = list_items(&f.db, f.ws, now())
        .await
        .unwrap()
        .into_iter()
        .find(|i| i.name == "promo_sb")
        .expect("listed");
    assert_eq!(item.status, ExperimentStatus::Registrable);
    assert_eq!(item.next_switch_on.as_deref(), Some("2026-09-07"));
}

#[tokio::test]
#[serial_test::serial]
async fn the_proposal_list_is_newest_first_and_carries_every_status() {
    let f = Fixture::new("proposals").await;
    let pending = start_price(&f.db, input(&f, "promo_pilot", vec![f.waves(&[8])]))
        .await
        .expect("pending, never run")
        .proposal_ids[0];
    let refused = f.price(&f.waves(&[24]), now()).await;
    let done = f.price(&f.waves(&[8]), now()).await;

    let views: Vec<Proposal> = list_proposal_views(&f.db, f.ws, "promo_pilot")
        .await
        .unwrap();
    assert_eq!(
        views.iter().map(|p| p.id).collect::<Vec<_>>(),
        vec![done, refused, pending]
    );
    assert_eq!(
        views.iter().map(|p| p.status.as_str()).collect::<Vec<_>>(),
        ["done", "refused", "pending"]
    );
    assert!(
        views[0]
            .snippet
            .as_deref()
            .unwrap()
            .contains("mde_at_proposal")
    );
    assert!(views[0].seed.is_some() && views[0].mde.unwrap() > 0.0);
    assert!(views[1].refusal.is_some() && views[1].snippet.is_none());
    assert!(
        views[2].finished_at.is_none(),
        "a pending request is visible before the job runs"
    );
    assert_eq!(views[2].request["wave_sizes"], serde_json::json!([8]));
    assert_eq!(views[2].alpha, 0.05);

    // Tenant scoping.
    let (other_ws, _) = crate::simulation_routes::seed_workspace(&f.db, "other").await;
    assert!(
        list_proposal_views(&f.db, other_ws, "promo_pilot")
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
#[serial_test::serial]
async fn a_done_group_proposal_renders_every_output_key_in_its_snippet() {
    // Index C5: the snippet carries assign_by, the waves, strata, seed, the
    // overridden alpha, the MDE and the frozen prior — pasted, it is the whole
    // pricing output.
    let yaml = DRAFT.replace(
        "  direction: increase",
        "  direction: increase\n  driver: store_days.marketing_spend",
    );
    let f = Fixture::with_definition("snippet-keys", &yaml).await;
    let id = f
        .price(
            &f.waves_with(&[5], Some(2), Some("location.city"), Some(0.2)),
            now(),
        )
        .await;
    let view = list_proposal_views(&f.db, f.ws, "promo_pilot")
        .await
        .unwrap()
        .into_iter()
        .find(|p| p.id == id)
        .unwrap();
    assert_eq!(view.status, "done", "{:?}", view.refusal);
    let v: serde_yaml::Value = serde_yaml::from_str(&view.snippet.expect("snippet")).expect("yaml");
    assert_eq!(v["assign_by"].as_str(), Some("location.city"));
    assert_eq!(
        v["design"]["waves"][0]["units"]
            .as_sequence()
            .unwrap()
            .len(),
        5
    );
    assert_eq!(v["design"]["strata"].as_sequence().unwrap().len(), 2);
    assert!(v["design"]["seed"].as_u64().is_some());
    assert_eq!(v["outcome"]["alpha"].as_f64(), Some(0.2));
    assert!(v["outcome"]["mde_at_proposal"].as_f64().unwrap() > 0.0);
    assert_eq!(
        v["outcome"]["prior_coefficient"]["source"].as_str(),
        Some("fitted")
    );
    assert_eq!(view.alpha, 0.2);
    assert!(view.mde_unit_day.unwrap() > 0.0);
}

#[tokio::test]
#[serial_test::serial]
async fn a_done_switchback_proposal_renders_its_periods_in_its_snippet() {
    let f = Fixture::with_definition(
        "snippet-switchback",
        &crate::experiment_pricing::switchback_draft(),
    )
    .await;
    let id = f.price(&f.switchback(6), now()).await;
    let view = list_proposal_views(&f.db, f.ws, "promo_pilot")
        .await
        .unwrap()
        .into_iter()
        .find(|p| p.id == id)
        .unwrap();
    assert_eq!(view.status, "done", "{:?}", view.refusal);
    let v: serde_yaml::Value = serde_yaml::from_str(&view.snippet.expect("snippet")).expect("yaml");
    assert_eq!(v["design"]["switchback"]["period_days"].as_u64(), Some(14));
    assert_eq!(
        v["design"]["switchback"]["periods"]
            .as_sequence()
            .unwrap()
            .len(),
        12
    );
    assert!(v["design"].get("waves").is_none() && v.get("assign_by").is_none());
}

/// Put `id`'s queue row in `status`, or delete it (`None`: the retention pass
/// purged a terminal row).
async fn set_queue_status(f: &Fixture, id: Uuid, status: Option<&str>) {
    let sql = match status {
        Some(s) => {
            format!("UPDATE agentic_task_queue SET queue_status = '{s}' WHERE task_id = '{id}'")
        }
        None => format!("DELETE FROM agentic_task_queue WHERE task_id = '{id}'"),
    };
    let done =
        f.db.execute_raw(Statement::from_string(f.db.get_database_backend(), sql))
            .await
            .unwrap();
    assert_eq!(done.rows_affected(), 1, "{id}");
}

#[tokio::test]
#[serial_test::serial]
async fn a_pending_proposal_whose_price_task_stopped_reads_back_failed() {
    // Final review I1. The queue dead-letters a task whose worker died three
    // times (an OOM `catch_unwind` cannot catch), and stamps it `failed` when
    // the payload does not decode or the job's own settling write errored. In
    // each the row was never settled; it must not read `pending` forever.
    let f = Fixture::new("stopped-task").await;
    let mut ids = Vec::new();
    for _ in 0..7 {
        let started = start_price(&f.db, input(&f, "promo_pilot", vec![f.waves(&[8])]))
            .await
            .expect("enqueue");
        ids.push(started.proposal_ids[0]);
    }
    let [queued, claimed, settled, dead, failed, cancelled, purged] = ids[..] else {
        unreachable!()
    };
    set_queue_status(&f, claimed, Some("claimed")).await;
    proposal_store::finish_refused(&f.db, settled, "no history for m over e", &[])
        .await
        .unwrap();
    set_queue_status(&f, settled, Some("completed")).await;
    set_queue_status(&f, dead, Some("dead")).await;
    set_queue_status(&f, failed, Some("failed")).await;
    set_queue_status(&f, cancelled, Some("cancelled")).await;
    set_queue_status(&f, purged, None).await;

    let views: std::collections::HashMap<Uuid, Proposal> =
        list_proposal_views(&f.db, f.ws, "promo_pilot")
            .await
            .unwrap()
            .into_iter()
            .map(|p| (p.id, p))
            .collect();
    for id in [queued, claimed] {
        assert_eq!(views[&id].status, "pending", "a live task is still pending");
        assert_eq!(views[&id].refusal, None);
    }
    assert_eq!(views[&settled].status, "refused", "a settled row stands");
    assert_eq!(
        views[&settled].refusal.as_deref(),
        Some("no history for m over e")
    );
    for (id, state) in [
        (dead, "its task is `dead`"),
        (failed, "its task is `failed`"),
        (cancelled, "its task is `cancelled`"),
        (purged, "its task is no longer in the queue"),
    ] {
        let p = &views[&id];
        assert_eq!(p.status, "failed", "{id}");
        assert_eq!(
            p.refusal.as_deref(),
            Some(format!("the price job stopped without settling this proposal: {state}").as_str())
        );
        assert!(p.seed.is_none() && p.snippet.is_none());
    }

    // The list's latest proposal reads the same, and a stopped job is no draw.
    let item = list_items(&f.db, f.ws, now())
        .await
        .unwrap()
        .into_iter()
        .find(|i| i.name == "promo_pilot")
        .expect("listed");
    let latest = item.latest_proposal.expect("latest");
    assert_eq!((latest.id, latest.status.as_str()), (purged, "failed"));
    assert_eq!(
        latest.refusal.as_deref(),
        Some(
            "the price job stopped without settling this proposal: its task is no longer in the queue"
        )
    );
    assert_eq!(item.proposals_count, 0);
}

#[tokio::test]
#[serial_test::serial]
async fn pricing_after_the_draft_is_edited_prices_the_edit_and_the_earlier_proposal_is_stale() {
    let f = Fixture::new("edited-draft").await;
    let first = f.price(&f.waves(&[8]), now()).await;
    assert_eq!(f.row(first).await.design_id, Some(f.design));

    // The draft edited in place, with a different alpha.
    let edited = DRAFT.replace(
        "  direction: increase",
        "  direction: increase\n  alpha: 0.2",
    );
    assert_eq!(
        seed_design(&f.db, f.ws, &edited).await,
        f.design,
        "same row"
    );

    let started = start_price(&f.db, input(&f, "promo_pilot", vec![f.waves(&[8])]))
        .await
        .expect("enqueue");
    let rows = proposal_store::list_request(&f.db, f.ws, started.request_id)
        .await
        .unwrap();
    assert_eq!(rows[0].alpha, 0.2, "the edit's alpha is the one priced");

    f.run(started.proposal_ids[0], now()).await;
    let views = list_proposal_views(&f.db, f.ws, "promo_pilot")
        .await
        .unwrap();
    assert_eq!(views[0].id, started.proposal_ids[0]);
    assert_eq!(views[0].status, "done", "{:?}", views[0].refusal);
    assert!(!views[0].stale);
    let earlier = views.iter().find(|v| v.id == first).expect("the first");
    assert!(earlier.stale, "it priced the draft before the edit");
}
