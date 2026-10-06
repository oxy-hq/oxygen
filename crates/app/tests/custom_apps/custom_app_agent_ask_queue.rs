//! A queued custom-app ask (`TaskSpec::Custom { kind: "custom_app_agent_ask" }`),
//! driven by production's `AgentAskExecutor`.
//!
//! **Nothing in the product enqueues this kind yet**, so every test here seeds
//! the run the way the start handler will once it does — the run row with its
//! caller record, the analytics extension row, one `queued` Global entry — and
//! either hands the claim to the executor directly, or lets a latency-worker
//! pass (`recover_pending_global_runs`, with the resolver `router::recovery`
//! injects) find it.
//!
//! No LLM is reachable from a test, and the fixture's workspace configures no
//! model, so no ask here ever answers: its pipeline refuses to build. That is
//! enough to see everything the executor itself decides — who the ask runs as,
//! that its pipeline is started at most once, and what a claim that must not
//! start it reports — and it is the path a queued ask's bundle would otherwise
//! hang on. What it cannot show is an answer or a clarification coming back;
//! those are the handler's own pipeline (`agent_ask::pipeline`), shared by
//! construction rather than exercised here.
//!
//! **Needs** Postgres only.

use std::sync::Arc;
use std::time::{Duration, Instant};

use agentic_core::delegation::{TaskAssignment, TaskOutcome};
use agentic_core::human_input::SuspendedRunData;
use agentic_pipeline::platform::{IdentityResolver, PlatformContext};
use agentic_pipeline::recovery::recover_pending_global_runs;
use agentic_pipeline::run_execution::{begin_execution, get_run_execution};
use agentic_runtime::crud;
use agentic_runtime::router::noop_router;
use agentic_runtime::state::RuntimeState;
use agentic_runtime::worker::{CustomTaskRegistry, ExecutingTask, TaskExecutor};
use oxy_app::server::api::projects::agent_ask::caller::{
    CallerRunResolver, RUN_CALLER_KEY, RunCaller,
};
use oxy_app::server::api::projects::agent_ask::executor::{
    AgentAskExecutor, INTERRUPTED_MESSAGE, prepare,
};
use oxy_app::server::api::projects::agent_ask::task::{self, AGENT_ASK_KIND};
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::custom_app_procedure_run_fixture::{Fixture, driver_platform, fixture};

const AGENT: &str = "ask";

/// The fixture's workspace, plus an agent a bundle can ask. It names no usable
/// model, so its pipeline refuses to build.
async fn workspace_with_an_agent() -> Fixture {
    let f = fixture().await;
    std::fs::write(
        f._workspace_dir.path().join("ask.agentic.yml"),
        "llm:\n  ref: none\n",
    )
    .expect("write the agent");
    f
}

async fn set_metadata(db: &DatabaseConnection, run_id: &str, patch: Value) {
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "UPDATE agentic_runs SET metadata = metadata || $2::jsonb WHERE id = $1",
        [run_id.into(), patch.to_string().into()],
    ))
    .await
    .expect("patch run metadata");
}

/// A run as the start leaves it once it enqueues: `running`, its extension
/// row, and one `queued` Global entry carrying the ask's (empty) spec.
/// `caller` is recorded on the run when given.
async fn queued_ask(f: &Fixture, caller: Option<&RunCaller>) -> String {
    let db = &f.t.db;
    let run_id = Uuid::new_v4().to_string();
    agentic_pipeline::insert_run(
        db,
        &run_id,
        AGENT,
        "how many orders?",
        None,
        None,
        f.workspace_id,
    )
    .await
    .expect("insert the run and its extension row");
    if let Some(caller) = caller {
        set_metadata(db, &run_id, json!({ RUN_CALLER_KEY: caller.to_metadata() })).await;
    }
    crud::enqueue_task(
        db,
        &run_id,
        &run_id,
        None,
        &task::spec(),
        None,
        crud::TaskScope::Global,
    )
    .await
    .expect("queue the ask");
    run_id
}

fn caller() -> RunCaller {
    RunCaller {
        user_id: Uuid::new_v4(),
        staging_pin: None,
    }
}

fn executor(db: &DatabaseConnection) -> AgentAskExecutor {
    AgentAskExecutor {
        db: db.clone(),
        schema_cache: None,
    }
}

/// Hand `run_id`'s claim to the executor, as a worker that claimed it would.
async fn claim(db: &DatabaseConnection, run_id: &str) -> ExecutingTask {
    executor(db)
        .execute(TaskAssignment {
            task_id: run_id.to_string(),
            parent_task_id: None,
            run_id: run_id.to_string(),
            spec: task::spec(),
            policy: None,
        })
        .await
        .expect("the executor accepts its own kind")
}

/// Everything a claimed attempt reported: its events, then its one outcome.
async fn report(mut task: ExecutingTask) -> (Vec<(String, Value)>, TaskOutcome) {
    let outcome = tokio::time::timeout(Duration::from_secs(30), task.outcomes.recv())
        .await
        .expect("the attempt reports within the deadline")
        .expect("an attempt always reports an outcome");
    let mut events = Vec::new();
    while let Ok(event) = task.events.try_recv() {
        events.push(event);
    }
    (events, outcome)
}

async fn began(db: &DatabaseConnection, run_id: &str) -> bool {
    get_run_execution(db, run_id)
        .await
        .unwrap()
        .expect("extension row")
        .started_at
        .is_some()
}

/// The identity half. The context the executor builds for a queued ask has the
/// recorded caller as its subject and no role — the handler's context — where
/// a driver tick's own has no subject at all, which `airhouse_managed` mints a
/// system Admin for.
#[tokio::test(flavor = "multi_thread")]
async fn a_queued_ask_is_prepared_as_the_caller_who_asked() {
    let f = workspace_with_an_agent().await;
    let who = caller();
    let run_id = queued_ask(&f, Some(&who)).await;
    let run = crud::get_run(&f.t.db, &run_id).await.unwrap().unwrap();

    let prepared = prepare(&f.t.db, &run).await.expect("the caller's context");

    assert_eq!(prepared.context.subject(), Some(who.user_id));
    assert!(
        prepared.context.role().is_none(),
        "a subject with no role is what mints the caller's Reader"
    );
    assert_eq!(prepared.ask.agent_id, AGENT);
    assert_eq!(prepared.ask.workspace_id, f.workspace_id);

    let (driver, _dir) = driver_platform().await;
    assert_eq!(
        driver.subject(),
        None,
        "the control: the platform a driver tick would otherwise have used"
    );
}

/// No record, no drive. The ask is failed on its own log rather than run on
/// the only other context there is, and nothing is begun.
#[tokio::test(flavor = "multi_thread")]
async fn an_ask_with_no_caller_record_is_failed_not_driven_as_the_driver() {
    let f = workspace_with_an_agent().await;
    let run_id = queued_ask(&f, None).await;

    let (events, outcome) = report(claim(&f.t.db, &run_id).await).await;

    assert!(
        matches!(&outcome, TaskOutcome::Failed(m) if m.contains("no caller")),
        "{outcome:?}"
    );
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0].0, "error", "what ends the bundle's stream");
    assert!(
        !began(&f.t.db, &run_id).await,
        "an ask nobody may run is never taken"
    );
}

/// Through the queue, end to end: a latency-worker pass finds the queued ask,
/// production's executor takes it and starts its pipeline once, and when the
/// pipeline cannot be built the failure reaches both places a client reads —
/// the run's row and, as a terminal `error`, its event log.
#[tokio::test(flavor = "multi_thread")]
async fn a_latency_pass_drives_a_queued_ask_to_a_terminal_event() {
    let f = workspace_with_an_agent().await;
    let db = &f.t.db;
    let run_id = queued_ask(&f, Some(&caller())).await;
    let (driver, _dir) = driver_platform().await;
    let mut registry = CustomTaskRegistry::new();
    registry.register(AGENT_ASK_KIND, Arc::new(executor(db)));
    let registry = Arc::new(registry);
    let state = Arc::new(RuntimeState::new());

    let deadline = Instant::now() + Duration::from_secs(60);
    let run = loop {
        let platform: Arc<dyn PlatformContext> = driver.clone();
        recover_pending_global_runs(
            db.clone(),
            state.clone(),
            platform,
            // The resolver `router::recovery` hands every recovery entry
            // point, over the identity resolver in place of the preview one.
            Arc::new(CallerRunResolver::new(
                db.clone(),
                Arc::new(IdentityResolver),
            )),
            None,
            None,
            None,
            None,
            noop_router(),
            Some(f.workspace_id),
            Some(registry.clone()),
            agentic_pipeline::recovery::DrivePolicy::ALL,
        )
        .await;
        let run = crud::get_run(db, &run_id).await.unwrap().unwrap();
        if run.task_status.as_deref() != Some("running") {
            break run;
        }
        assert!(Instant::now() < deadline, "the queued ask was never driven");
        tokio::time::sleep(Duration::from_millis(200)).await;
    };

    assert_eq!(run.task_status.as_deref(), Some("failed"));
    let message = run.error_message.clone().expect("a failed run says why");
    assert_ne!(
        message, INTERRUPTED_MESSAGE,
        "the first attempt starts the pipeline; it is not an interruption"
    );
    assert!(began(db, &run_id).await, "the attempt took the run first");

    let events = crud::get_all_events(db, &run_id).await.unwrap();
    let logged: Vec<&str> = events.iter().map(|e| e.event_type.as_str()).collect();
    let error = events
        .iter()
        .find(|e| e.event_type == "error")
        .unwrap_or_else(|| panic!("no terminal event on the log: {logged:?}"));
    assert_eq!(error.payload["message"], json!(message));
    // The bundle's stream reads this log through the analytics processor and
    // closes on `error`.
    let ui = agentic_pipeline::build_event_registry()
        .stream_processor("analytics")
        .process(&error.event_type, &error.payload);
    assert_eq!(ui.len(), 1, "{ui:?}");
    assert_eq!(ui[0].0, "error");
    assert_eq!(ui[0].1["message"], json!(message));
}

/// The rule's "never re-run", with the first attempt alive: a claim handed on
/// under a driver that is still executing writes nothing and comes back later.
#[tokio::test(flavor = "multi_thread")]
async fn a_second_claim_steps_aside_while_the_first_attempt_is_beating() {
    let f = workspace_with_an_agent().await;
    let run_id = queued_ask(&f, Some(&caller())).await;
    assert!(begin_execution(&f.t.db, &run_id).await.unwrap());

    let (events, outcome) = report(claim(&f.t.db, &run_id).await).await;

    assert!(
        matches!(outcome, TaskOutcome::Deferred { .. }),
        "{outcome:?}"
    );
    assert!(events.is_empty(), "{events:?}");
    let run = crud::get_run(&f.t.db, &run_id).await.unwrap().unwrap();
    assert_eq!(run.task_status.as_deref(), Some("running"));
    assert_eq!(run.error_message, None);
}

/// The rule's "close as interrupted": the first attempt began the ask and went
/// quiet, and there is no checkpoint. The ask is ended with a sentence its user
/// can act on, on the log its stream reads — and its pipeline is not started.
#[tokio::test(flavor = "multi_thread")]
async fn a_second_claim_closes_an_ask_whose_attempt_went_quiet() {
    let f = workspace_with_an_agent().await;
    let db = &f.t.db;
    let run_id = queued_ask(&f, Some(&caller())).await;
    assert!(begin_execution(db, &run_id).await.unwrap());
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "UPDATE analytics_run_extensions \
         SET execution_heartbeat_at = now() - interval '5 minutes' WHERE run_id = $1",
        [run_id.clone().into()],
    ))
    .await
    .expect("age the heartbeat");
    let before = get_run_execution(db, &run_id).await.unwrap().unwrap();

    let (events, outcome) = report(claim(db, &run_id).await).await;

    assert!(
        matches!(&outcome, TaskOutcome::Failed(m) if m == INTERRUPTED_MESSAGE),
        "{outcome:?}"
    );
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0].0, "error");
    assert_eq!(events[0].1["message"], json!(INTERRUPTED_MESSAGE));
    assert_eq!(
        get_run_execution(db, &run_id).await.unwrap().unwrap(),
        before,
        "closing an interrupted ask takes no stamp and beats nothing: nothing ran"
    );
}

/// The rule's "resume from a suspension": an ask parked on a question is
/// continued from there by recovery. An attempt holding its start spec starts
/// nothing and closes nothing, whatever the first attempt's heartbeat says.
#[tokio::test(flavor = "multi_thread")]
async fn a_claim_on_an_ask_at_a_suspension_starts_nothing() {
    let f = workspace_with_an_agent().await;
    let db = &f.t.db;
    let run_id = queued_ask(&f, Some(&caller())).await;
    crud::suspend_with_data_txn(
        db,
        &run_id,
        "awaiting_input",
        None,
        "which store?",
        &[],
        &SuspendedRunData {
            from_state: "clarifying".into(),
            original_input: "how many orders?".into(),
            trace_id: "trace-1".into(),
            stage_data: json!({}),
            question: "which store?".into(),
            suggestions: vec![],
        },
    )
    .await
    .expect("park the ask");

    let (events, outcome) = report(claim(db, &run_id).await).await;

    assert!(
        matches!(outcome, TaskOutcome::Deferred { .. }),
        "{outcome:?}"
    );
    assert!(events.is_empty(), "{events:?}");
    assert!(!began(db, &run_id).await);
    let run = crud::get_run(db, &run_id).await.unwrap().unwrap();
    assert_eq!(run.task_status.as_deref(), Some("awaiting_input"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_claim_on_a_cancelled_ask_starts_nothing() {
    let f = workspace_with_an_agent().await;
    let run_id = queued_ask(&f, Some(&caller())).await;
    crud::request_cancel(&f.t.db, &run_id).await.unwrap();

    let (events, outcome) = report(claim(&f.t.db, &run_id).await).await;

    assert!(matches!(outcome, TaskOutcome::Cancelled), "{outcome:?}");
    assert!(events.is_empty(), "{events:?}");
    assert!(!began(&f.t.db, &run_id).await);
}

/// A claim that arrives after the ask ended reports how it ended, with the
/// answer its user got: the coordinator writes an outcome onto the run it
/// belongs to, so anything else would rewrite it.
#[tokio::test(flavor = "multi_thread")]
async fn a_claim_on_an_ended_ask_reports_it_as_it_ended() {
    let f = workspace_with_an_agent().await;
    let run_id = queued_ask(&f, Some(&caller())).await;
    crud::update_run_done(&f.t.db, &run_id, "412 orders", None)
        .await
        .unwrap();

    let (events, outcome) = report(claim(&f.t.db, &run_id).await).await;

    assert!(
        matches!(&outcome, TaskOutcome::Done { answer, .. } if answer == "412 orders"),
        "{outcome:?}"
    );
    assert!(events.is_empty(), "{events:?}");
    assert!(!began(&f.t.db, &run_id).await);
}

/// Two claims on one fresh ask at once — what a handed-on claim is. One takes
/// the stamp and starts the pipeline; the other is told the run is taken.
#[tokio::test(flavor = "multi_thread")]
async fn two_claims_at_once_start_the_pipeline_once() {
    let f = workspace_with_an_agent().await;
    let db = &f.t.db;
    let run_id = queued_ask(&f, Some(&caller())).await;

    let (first, second) = tokio::join!(claim(db, &run_id), claim(db, &run_id));
    let (_, first) = report(first).await;
    let (_, second) = report(second).await;

    let stepped_aside = [&first, &second]
        .iter()
        .filter(|o| matches!(o, TaskOutcome::Deferred { .. }))
        .count();
    assert_eq!(
        stepped_aside, 1,
        "exactly one claim may start the pipeline: {first:?} / {second:?}"
    );
    assert!(began(db, &run_id).await);
}

/// A worker hands the executor whatever kind the registry routed to it; a spec
/// of another kind is refused rather than read as an ask.
#[tokio::test(flavor = "multi_thread")]
async fn a_spec_of_another_kind_is_refused() {
    let f = workspace_with_an_agent().await;
    let run_id = queued_ask(&f, Some(&caller())).await;

    let refused = executor(&f.t.db)
        .execute(TaskAssignment {
            task_id: run_id.clone(),
            parent_task_id: None,
            run_id: run_id.clone(),
            spec: agentic_core::delegation::TaskSpec::Custom {
                kind: "custom_app_procedure_run".into(),
                payload: json!({}),
            },
            policy: None,
        })
        .await;

    assert!(refused.is_err_and(|e| e.contains("unknown agent-ask kind")));
    assert!(!began(&f.t.db, &run_id).await);
}
