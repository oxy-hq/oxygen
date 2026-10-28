//! A queued chat run (`TaskSpec::Custom { kind: "analytics_chat_run" }`),
//! driven by production's `ChatRunExecutor`, and the requester record recovery
//! drives a chat run's root as.
//!
//! Most tests seed the run the way `create_run`'s `queue_chat` does
//! (`queued_run::seed_queued_analytics_run`, with the requester the request
//! resolved) and either hand the claim to the executor, as a worker that
//! claimed it would, or let a latency-worker pass find it; the last two call
//! the start's own `queueable_chat` and `queue_chat`.
//!
//! No LLM is reachable from a test and the workspace names no model, so no
//! chat here answers: its pipeline refuses to build. That is enough to see
//! what the executor decides — who the run is for, that its pipeline starts at
//! most once, and what a re-claim reports — which is the half that matters
//! when the pod driving a chat is killed mid-run.
//!
//! Run with:
//! `cargo nextest run -p oxy-app --test platform -E 'test(queued_chat_run)'`

use std::sync::Arc;
use std::time::{Duration, Instant};

use agentic_core::delegation::{TaskAssignment, TaskOutcome};
use agentic_pipeline::platform::{
    IdentityResolver, PlatformContext, Requester, RunPlatformResolver,
};
use agentic_pipeline::queued_run::{
    CHAT_RUN_KIND, QueuedAnalyticsRun, chat_run_spec, seed_queued_analytics_run,
};
use agentic_pipeline::run_execution::{begin_execution, get_run_execution};
use agentic_runtime::crud;
use agentic_runtime::state::RuntimeState;
use agentic_runtime::worker::{CustomTaskRegistry, ExecutingTask, TaskExecutor};
use entity::workspace_members::WorkspaceRole;
use entity::{organizations, users, workspaces};
use oxy_app::server::api::projects::agent_ask::caller::CallerRunResolver;
use oxy_app::server::queued_analytics::INTERRUPTED_MESSAGE;
use oxy_app::server::queued_analytics::chat::{ChatRunExecutor, prepare};
use sea_orm::{
    ActiveModelTrait, ActiveValue, ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement,
};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::common::{Schema, fresh_db};

const AGENT: &str = "sales";

struct Fixture {
    db: DatabaseConnection,
    workspace_id: Uuid,
    user_id: Uuid,
    _dir: tempfile::TempDir,
}

/// One org, one user, one workspace on disk with an agent that names no usable
/// model.
async fn fixture() -> Fixture {
    let (db, _url) = fresh_db(Schema::All).await;
    let user_id = Uuid::new_v4();
    users::ActiveModel {
        id: ActiveValue::Set(user_id),
        email: ActiveValue::Set(Some(format!("chat-{user_id}@example.com"))),
        name: ActiveValue::Set("Chat".into()),
        picture: ActiveValue::Set(None),
        email_verified: ActiveValue::Set(true),
        ..Default::default()
    }
    .insert(&db)
    .await
    .expect("seed user");
    let org_id = Uuid::new_v4();
    organizations::ActiveModel {
        id: ActiveValue::Set(org_id),
        name: ActiveValue::Set("Chat Org".into()),
        slug: ActiveValue::Set(format!("chat-org-{org_id}")),
        ..Default::default()
    }
    .insert(&db)
    .await
    .expect("seed org");
    let dir = tempfile::tempdir().expect("workspace dir");
    std::fs::write(dir.path().join("config.yml"), "databases: []\nmodels: []\n")
        .expect("write config.yml");
    std::fs::write(
        dir.path().join(format!("{AGENT}.agentic.yml")),
        "llm:\n  ref: none\n",
    )
    .expect("write the agent");
    let workspace_id = Uuid::new_v4();
    workspaces::ActiveModel {
        id: ActiveValue::Set(workspace_id),
        name: ActiveValue::Set(format!("chat-{workspace_id}")),
        org_id: ActiveValue::Set(Some(org_id)),
        path: ActiveValue::Set(Some(dir.path().to_string_lossy().to_string())),
        ..Default::default()
    }
    .insert(&db)
    .await
    .expect("seed workspace");
    Fixture {
        db,
        workspace_id,
        user_id,
        _dir: dir,
    }
}

fn requester(f: &Fixture, role: Option<&str>) -> Requester {
    Requester {
        user_id: f.user_id,
        role: role.map(str::to_string),
    }
}

/// A chat as the start seeds it: the run with its requester, its extension
/// row, one `queued` Global entry of the chat kind.
async fn queued_chat(f: &Fixture, who: Option<&Requester>) -> String {
    let run = QueuedAnalyticsRun {
        run_id: Uuid::new_v4().to_string(),
        workspace_id: f.workspace_id,
        agent_id: AGENT.into(),
        question: "how did the stores do?".into(),
        thread_id: None,
        thinking_mode: Default::default(),
        metadata: who
            .map(|r| (Requester::KEY.to_string(), r.to_metadata()))
            .into_iter()
            .collect(),
        spec: chat_run_spec(),
    };
    seed_queued_analytics_run(&f.db, &run)
        .await
        .expect("seed the chat");
    run.run_id
}

fn executor(db: &DatabaseConnection) -> ChatRunExecutor {
    ChatRunExecutor {
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
            spec: chat_run_spec(),
            policy: None,
            claim_epoch: agentic_core::NO_CLAIM,
        })
        .await
        .expect("the executor accepts its own kind")
}

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

/// The identity half: the context the executor builds is the requester's, at
/// the role the request resolved — where a driver tick's own has no subject
/// at all, which `airhouse_managed` mints a system Admin for.
#[tokio::test(flavor = "multi_thread")]
async fn a_queued_chat_is_prepared_as_its_requester_at_the_recorded_role() {
    let f = fixture().await;
    for (recorded, expected) in [
        (Some("member"), Some(WorkspaceRole::Member)),
        (Some("owner"), Some(WorkspaceRole::Owner)),
        (None, None),
    ] {
        let run_id = queued_chat(&f, Some(&requester(&f, recorded))).await;
        let run = crud::get_run(&f.db, &run_id).await.unwrap().unwrap();

        let prepared = prepare(&f.db, &run).await.expect("the requester's context");

        assert_eq!(prepared.context.subject(), Some(f.user_id), "{recorded:?}");
        assert_eq!(prepared.context.role(), expected, "{recorded:?}");
        assert_eq!(prepared.chat.agent_id, AGENT);
        assert_eq!(prepared.chat.workspace_id, f.workspace_id);
    }
}

/// No record, no drive: failed on its own log, nothing begun.
#[tokio::test(flavor = "multi_thread")]
async fn a_chat_with_no_requester_is_failed_not_driven_as_the_driver() {
    let f = fixture().await;
    let run_id = queued_chat(&f, None).await;

    let (events, outcome) = report(claim(&f.db, &run_id).await).await;

    assert!(
        matches!(&outcome, TaskOutcome::Failed(m) if m.contains("no requester")),
        "{outcome:?}"
    );
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0].0, "error");
    assert!(!began(&f.db, &run_id).await);
}

/// A recorded role that is not a workspace role is refused rather than read as
/// "no role" — a typo must not become a different credential.
#[tokio::test(flavor = "multi_thread")]
async fn a_chat_whose_role_does_not_read_back_is_failed() {
    let f = fixture().await;
    let run_id = queued_chat(&f, Some(&requester(&f, Some("root")))).await;

    let (_, outcome) = report(claim(&f.db, &run_id).await).await;

    assert!(
        matches!(&outcome, TaskOutcome::Failed(m) if m.contains("not a workspace role")),
        "{outcome:?}"
    );
    assert!(!began(&f.db, &run_id).await);
}

/// The re-claim, with the first attempt alive: a claim handed on under a
/// driver still executing writes nothing and comes back later.
#[tokio::test(flavor = "multi_thread")]
async fn a_second_claim_steps_aside_while_the_first_attempt_is_beating() {
    let f = fixture().await;
    let run_id = queued_chat(&f, Some(&requester(&f, Some("member")))).await;
    assert!(begin_execution(&f.db, &run_id).await.unwrap());

    let (events, outcome) = report(claim(&f.db, &run_id).await).await;

    assert!(
        matches!(outcome, TaskOutcome::Deferred { .. }),
        "{outcome:?}"
    );
    assert!(events.is_empty(), "{events:?}");
    let run = crud::get_run(&f.db, &run_id).await.unwrap().unwrap();
    assert_eq!(run.task_status.as_deref(), Some("running"));
}

/// The re-claim after the driving pod died: the first attempt began the chat
/// and went quiet, there is no checkpoint. The chat is closed with a sentence
/// its user can act on, on the log its stream reads, and not started again.
#[tokio::test(flavor = "multi_thread")]
async fn a_second_claim_closes_a_chat_whose_attempt_went_quiet() {
    let f = fixture().await;
    let run_id = queued_chat(&f, Some(&requester(&f, Some("member")))).await;
    assert!(begin_execution(&f.db, &run_id).await.unwrap());
    f.db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "UPDATE analytics_run_extensions \
         SET execution_heartbeat_at = now() - interval '5 minutes' WHERE run_id = $1",
        [run_id.clone().into()],
    ))
    .await
    .expect("age the heartbeat");

    let (events, outcome) = report(claim(&f.db, &run_id).await).await;

    assert!(
        matches!(&outcome, TaskOutcome::Failed(m) if m == INTERRUPTED_MESSAGE),
        "{outcome:?}"
    );
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0].0, "error");
    assert_eq!(events[0].1["message"], json!(INTERRUPTED_MESSAGE));
}

/// Through the queue: a latency-worker pass finds the chat, the executor takes
/// the stamp and starts its pipeline once, and the pipeline's failure reaches
/// the run's row and, as a terminal `error`, its log.
#[tokio::test(flavor = "multi_thread")]
async fn a_latency_pass_drives_a_queued_chat_to_a_terminal_event() {
    let f = fixture().await;
    let db = &f.db;
    let run_id = queued_chat(&f, Some(&requester(&f, Some("member")))).await;
    let mut registry = CustomTaskRegistry::new();
    registry.register(CHAT_RUN_KIND, Arc::new(executor(db)));
    let registry = Arc::new(registry);
    let state = Arc::new(RuntimeState::new());
    // Stands in for the tick's platform; what matters is that the resolver
    // replaces it with the requester's.
    let base: Arc<dyn PlatformContext> = Arc::new(
        oxy_app::server::queued_analytics::requester::requester_context(
            db,
            f.workspace_id,
            &requester(&f, None),
        )
        .await
        .expect("a workspace context"),
    );

    let deadline = Instant::now() + Duration::from_secs(60);
    let run = loop {
        agentic_pipeline::recovery::recover_pending_global_runs(
            db.clone(),
            state.clone(),
            base.clone(),
            Arc::new(CallerRunResolver::new(
                db.clone(),
                Arc::new(IdentityResolver),
            )),
            None,
            None,
            None,
            None,
            agentic_runtime::router::noop_router(),
            Some(f.workspace_id),
            Some(registry.clone()),
            agentic_pipeline::recovery::DrivePolicy::ALL,
        )
        .await;
        let run = crud::get_run(db, &run_id).await.unwrap().unwrap();
        if run.task_status.as_deref() != Some("running") {
            break run;
        }
        assert!(
            Instant::now() < deadline,
            "the queued chat was never driven"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    };

    assert_eq!(run.task_status.as_deref(), Some("failed"));
    assert_ne!(run.error_message.as_deref(), Some(INTERRUPTED_MESSAGE));
    assert!(began(db, &run_id).await, "the attempt took the run first");
    let events = crud::get_all_events(db, &run_id).await.unwrap();
    assert!(
        events.iter().any(|e| e.event_type == "error"),
        "no terminal event on the log: {:?}",
        events.iter().map(|e| &e.event_type).collect::<Vec<_>>()
    );
}

/// Recovery's resolver drives a run that records its requester as them — a
/// chat its driver died in is resumed as the member who asked, not on the
/// tick's subject-less platform — and leaves every other run on the tick's.
#[tokio::test(flavor = "multi_thread")]
async fn recovery_drives_a_run_that_records_its_requester_as_them() {
    let f = fixture().await;
    let resolver = CallerRunResolver::new(f.db.clone(), Arc::new(IdentityResolver));
    let base: Arc<dyn PlatformContext> = Arc::new(
        oxy_app::server::queued_analytics::requester::requester_context(
            &f.db,
            f.workspace_id,
            &requester(&f, None),
        )
        .await
        .expect("a workspace context"),
    );

    let recorded = requester(&f, Some("member"));
    let run_id = queued_chat(&f, Some(&recorded)).await;
    let root = crud::get_run(&f.db, &run_id).await.unwrap().unwrap();
    let platform = resolver
        .platform_for(&root, base.clone())
        .await
        .expect("a platform");
    assert!(!Arc::ptr_eq(&platform, &base));
    assert_eq!(platform.requester(), Some(recorded));

    let unrecorded = queued_chat(&f, None).await;
    let root = crud::get_run(&f.db, &unrecorded).await.unwrap().unwrap();
    let platform = resolver.platform_for(&root, base.clone()).await.unwrap();
    assert!(
        Arc::ptr_eq(&platform, &base),
        "no record: the tick's platform"
    );
}

/// A member's context, wrapped to answer as a workspace preview's does.
struct PreviewOf(oxy_app::agentic_wiring::OxyProjectContext);

#[async_trait::async_trait]
impl agentic_pipeline::platform::ProjectContext for PreviewOf {
    async fn resolve_connector(&self, db: &str) -> Option<agentic_connector::ConnectorConfig> {
        self.0.resolve_connector(db).await
    }
    async fn resolve_model(
        &self,
        model_ref: Option<&str>,
        explicit: bool,
    ) -> Option<agentic_analytics::config::ResolvedModelInfo> {
        self.0.resolve_model(model_ref, explicit).await
    }
    async fn resolve_secret(&self, var: &str) -> Option<String> {
        self.0.resolve_secret(var).await
    }
    fn workspace_id(&self) -> Uuid {
        agentic_pipeline::platform::ProjectContext::workspace_id(&self.0)
    }
    fn requester(&self) -> Option<Requester> {
        self.0.requester()
    }
    fn is_workspace_preview(&self) -> bool {
        true
    }
}

#[async_trait::async_trait]
impl agentic_automation::WorkspaceContext for PreviewOf {
    fn workspace_path(&self) -> Option<&std::path::Path> {
        None
    }
    fn database_configs(&self) -> Vec<oxy_airlayer_compat::DatabaseConfig> {
        vec![]
    }
    async fn get_connector(
        &self,
        name: &str,
    ) -> Result<Arc<dyn agentic_connector::DatabaseConnector>, String> {
        Err(format!("no connector {name}"))
    }
    async fn get_integration(
        &self,
        name: &str,
    ) -> Result<agentic_automation::workspace::IntegrationConfig, String> {
        Err(format!("no integration {name}"))
    }
    async fn list_automation_files(&self) -> Result<Vec<std::path::PathBuf>, String> {
        Ok(vec![])
    }
    async fn resolve_automation_yaml(
        &self,
        _r: &str,
    ) -> Result<String, agentic_pipeline::WorkspaceReadError> {
        Err("unused".into())
    }
}

/// Which starts go on the queue: a plain chat from a request that resolved who
/// asked. The Builder domain, onboarding and auto-accept edit the working copy;
/// a preview's platform holds its writes in its own process; a start on a
/// branch is built on that branch, which a driver would not rebuild; and a
/// context with no requester could only be driven as the driver. Those stay in
/// the request's process.
#[tokio::test(flavor = "multi_thread")]
async fn only_a_plain_chat_with_a_requester_is_started_on_the_queue() {
    use agentic_pipeline::queued_run::ChatAdmission;

    let f = fixture().await;
    let member = || async {
        oxy_app::server::queued_analytics::requester::requester_context(
            &f.db,
            f.workspace_id,
            &requester(&f, Some("member")),
        )
        .await
        .expect("a member's context")
    };
    let plain = ChatAdmission::default();

    assert_eq!(
        plain.queueable(&member().await),
        Some(requester(&f, Some("member")))
    );

    for stays in [
        ChatAdmission {
            builder_domain: true,
            ..Default::default()
        },
        ChatAdmission {
            onboarding: true,
            ..Default::default()
        },
        ChatAdmission {
            auto_accept: true,
            ..Default::default()
        },
        ChatAdmission {
            working_copy_branch: Some("feat-x".into()),
            ..Default::default()
        },
    ] {
        assert_eq!(stays.queueable(&member().await), None, "{stays:?}");
    }
    assert_eq!(
        plain.queueable(&PreviewOf(member().await)),
        None,
        "a preview's platform"
    );
    let nobody = crate::run_routes_workspace_scope::Platform {
        workspace_id: f.workspace_id,
    };
    assert_eq!(plain.queueable(&nobody), None, "no requester, no queue");
}

/// The start itself, end to end: `queue_chat` seeds the run as the request's
/// member, a latency pass drives it on the queue, and the pipeline's own
/// failure reaches the run's log. An agent that does not exist is refused on
/// the start, before any run exists.
#[tokio::test(flavor = "multi_thread")]
async fn a_queued_chat_start_is_driven_by_whichever_driver_claims_it() {
    let f = fixture().await;
    let db = &f.db;
    let who = requester(&f, Some("member"));
    let platform: Arc<dyn PlatformContext> = Arc::new(
        oxy_app::server::queued_analytics::requester::requester_context(db, f.workspace_id, &who)
            .await
            .expect("a member's context"),
    );
    let chat = |agent: &str| agentic_pipeline::queued_run::ChatStart {
        workspace_id: f.workspace_id,
        agent_id: agent.into(),
        question: "how did the stores do?".into(),
        thread_id: None,
        thinking_mode: Default::default(),
    };

    let refused =
        agentic_pipeline::queued_run::queue_chat(db, &platform, chat("no_such_agent"), who.clone())
            .await
            .expect_err("an agent that does not exist");
    assert!(
        matches!(
            &refused,
            agentic_pipeline::queued_run::QueueChatError::Agent(e) if !e.is_retryable()
        ),
        "{refused:?}"
    );

    let run_id = agentic_pipeline::queued_run::queue_chat(db, &platform, chat(AGENT), who.clone())
        .await
        .expect("queued");
    let run = crud::get_run(db, &run_id).await.unwrap().unwrap();
    assert_eq!(
        Requester::recorded(run.metadata.as_ref()).unwrap(),
        Some(who)
    );
    let entry = crud::get_queue_entry(db, &run_id).await.unwrap().unwrap();
    assert_eq!(entry.queue_status, "queued");
    assert_eq!(entry.spec["kind"], json!(CHAT_RUN_KIND));

    let mut registry = CustomTaskRegistry::new();
    registry.register(CHAT_RUN_KIND, Arc::new(executor(db)));
    let registry = Arc::new(registry);
    let deadline = Instant::now() + Duration::from_secs(60);
    let run = loop {
        agentic_pipeline::recovery::recover_pending_global_runs(
            db.clone(),
            Arc::new(RuntimeState::new()),
            platform.clone(),
            Arc::new(CallerRunResolver::new(
                db.clone(),
                Arc::new(IdentityResolver),
            )),
            None,
            None,
            None,
            None,
            agentic_runtime::router::noop_router(),
            Some(f.workspace_id),
            Some(registry.clone()),
            agentic_pipeline::recovery::DrivePolicy::ALL,
        )
        .await;
        let run = crud::get_run(db, &run_id).await.unwrap().unwrap();
        if run.task_status.as_deref() != Some("running") {
            break run;
        }
        assert!(
            Instant::now() < deadline,
            "the queued chat was never driven"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    };
    assert_eq!(run.task_status.as_deref(), Some("failed"));
    assert!(began(db, &run_id).await);
}

/// No warehouse, no LLM: one inline step.
const GREET: &str = r#"
name: greet
tasks:
  - name: greeting
    type: formatter
    template: "hello"
"#;

/// `/agentic-workflows` runs, the same way: a run started for a requester is
/// seeded `Global` with who asked on its insert, nothing is registered in the
/// process that took the request, and a latency pass on any driver runs it to
/// the end — as that requester.
#[tokio::test(flavor = "multi_thread")]
async fn a_workflow_started_for_a_requester_is_driven_from_the_queue() {
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    let f = fixture().await;
    let db = &f.db;
    std::fs::create_dir_all(f._dir.path().join("automations")).unwrap();
    std::fs::write(
        f._dir.path().join("automations/greet.automation.yml"),
        GREET,
    )
    .unwrap();
    let who = requester(&f, Some("member"));
    let context = Arc::new(
        oxy_app::server::queued_analytics::requester::requester_context(db, f.workspace_id, &who)
            .await
            .expect("a member's context"),
    );
    let bridges = oxy_app::agentic_wiring::build_builder_bridges(context.clone());
    let platform: Arc<dyn PlatformContext> = context;
    let state = Arc::new(
        agentic_http::AgenticState::new(
            tokio_util::sync::CancellationToken::new(),
            db.clone(),
            Arc::new(crate::run_routes_workspace_scope::NoThreads),
        )
        .with_queued_starts(|| true),
    );
    let user = oxy_auth::types::AuthenticatedUser {
        id: f.user_id,
        email: Some("member@acme.test".into()),
        name: "Member".into(),
        picture: None,
        status: entity::users::UserStatus::Active,
        credential: None,
    };
    let app = axum::Router::new()
        .nest(
            "/agentic-workflows",
            agentic_http::automation_router(state.clone()),
        )
        .layer(axum::Extension(platform.clone()))
        .layer(axum::Extension(bridges))
        .layer(axum::Extension(user));

    let response = app
        .oneshot(
            Request::post("/agentic-workflows/runs")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({ "workflow_ref": "automations/greet.automation.yml" }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    let body: Value = serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), 1 << 20)
            .await
            .unwrap(),
    )
    .unwrap();
    let run_id = body["run_id"].as_str().expect("run_id").to_string();

    let run = crud::get_run(db, &run_id).await.unwrap().unwrap();
    assert_eq!(
        Requester::recorded(run.metadata.as_ref()).unwrap(),
        Some(who)
    );
    let entry = crud::get_queue_entry(db, &run_id).await.unwrap().unwrap();
    assert_eq!(entry.queue_status, "queued");
    assert!(!entry.scope_owned, "Global: any driver may take it");
    assert!(!state.notifiers.contains_key(&run_id));
    assert!(!state.cancel_txs.contains_key(&run_id));

    let deadline = Instant::now() + Duration::from_secs(60);
    let run = loop {
        agentic_pipeline::recovery::recover_pending_global_runs(
            db.clone(),
            Arc::new(RuntimeState::new()),
            platform.clone(),
            Arc::new(CallerRunResolver::new(
                db.clone(),
                Arc::new(IdentityResolver),
            )),
            None,
            None,
            None,
            None,
            agentic_runtime::router::noop_router(),
            Some(f.workspace_id),
            None,
            agentic_pipeline::recovery::DrivePolicy::ALL,
        )
        .await;
        let run = crud::get_run(db, &run_id).await.unwrap().unwrap();
        if !matches!(
            run.task_status.as_deref(),
            Some("running" | "delegating" | "waiting_on_child" | "waiting_on_children")
        ) {
            break run;
        }
        assert!(
            Instant::now() < deadline,
            "the queued workflow was never finished"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    };
    assert_eq!(
        run.task_status.as_deref(),
        Some("done"),
        "{:?}",
        run.error_message
    );
}

/// The `/agentic-workflows` router as a member of the fixture's workspace
/// reaches it, with the workflow `GREET` on disk, and queued starts switched on
/// or off as the host's flag would.
async fn workflow_app(
    f: &Fixture,
    queued_starts: bool,
) -> (axum::Router, Arc<agentic_http::AgenticState>) {
    std::fs::create_dir_all(f._dir.path().join("automations")).unwrap();
    std::fs::write(
        f._dir.path().join("automations/greet.automation.yml"),
        GREET,
    )
    .unwrap();
    let context = Arc::new(
        oxy_app::server::queued_analytics::requester::requester_context(
            &f.db,
            f.workspace_id,
            &requester(f, Some("member")),
        )
        .await
        .expect("a member's context"),
    );
    let bridges = oxy_app::agentic_wiring::build_builder_bridges(context.clone());
    let platform: Arc<dyn PlatformContext> = context;
    let state = Arc::new(
        agentic_http::AgenticState::new(
            tokio_util::sync::CancellationToken::new(),
            f.db.clone(),
            Arc::new(crate::run_routes_workspace_scope::NoThreads),
        )
        .with_queued_starts(move || queued_starts),
    );
    let user = oxy_auth::types::AuthenticatedUser {
        id: f.user_id,
        email: Some("member@acme.test".into()),
        name: "Member".into(),
        picture: None,
        status: entity::users::UserStatus::Active,
        credential: None,
    };
    let app = axum::Router::new()
        .nest(
            "/agentic-workflows",
            agentic_http::automation_router(state.clone()),
        )
        .layer(axum::Extension(platform))
        .layer(axum::Extension(bridges))
        .layer(axum::Extension(user));
    (app, state)
}

async fn post(app: &axum::Router, uri: &str, body: Value) -> (axum::http::StatusCode, Value) {
    use tower::ServiceExt;
    let response = app
        .clone()
        .oneshot(
            axum::http::Request::post(uri)
                .header("content-type", "application/json")
                .body(axum::body::Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// A workflow started on a `?branch=` keeps its co-located coordinator: its
/// context is that branch's, and a driver would rebuild the promoted one. So
/// does a preview's, and one with no requester.
#[tokio::test(flavor = "multi_thread")]
async fn only_a_workflow_started_for_a_requester_off_any_branch_is_queued() {
    use agentic_pipeline::queued_run::AutomationAdmission;

    let f = fixture().await;
    let member = oxy_app::server::queued_analytics::requester::requester_context(
        &f.db,
        f.workspace_id,
        &requester(&f, Some("member")),
    )
    .await
    .expect("a member's context");
    assert_eq!(
        AutomationAdmission::default().queueable(&member),
        Some(requester(&f, Some("member")))
    );
    let on_a_branch = AutomationAdmission {
        working_copy_branch: Some("feat-x".into()),
    };
    assert_eq!(on_a_branch.queueable(&member), None);
    assert_eq!(
        AutomationAdmission::default().queueable(&PreviewOf(member)),
        None
    );
    let nobody = crate::run_routes_workspace_scope::Platform {
        workspace_id: f.workspace_id,
    };
    assert_eq!(AutomationAdmission::default().queueable(&nobody), None);

    // Through the route: the same start on a branch is not queued `Global`.
    let (app, _) = workflow_app(&f, true).await;
    let (status, body) = post(
        &app,
        "/agentic-workflows/runs?branch=feat-x",
        json!({ "workflow_ref": "automations/greet.automation.yml" }),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK, "{body}");
    let run_id = body["run_id"].as_str().expect("run_id");
    let run = crud::get_run(&f.db, run_id).await.unwrap().unwrap();
    assert_eq!(Requester::recorded(run.metadata.as_ref()).unwrap(), None);
    let entry = crud::get_queue_entry(&f.db, run_id).await.unwrap().unwrap();
    assert!(entry.scope_owned, "driven by the request's own coordinator");
}

/// With queued starts switched off (the host's flag, off by default), a member's
/// workflow start keeps its co-located coordinator and records no requester, as
/// it did before the queue: a driver from an older build would run a queued one
/// on its own subject-less context.
#[tokio::test(flavor = "multi_thread")]
async fn with_queued_starts_off_a_members_workflow_keeps_its_own_coordinator() {
    let f = fixture().await;
    let (app, state) = workflow_app(&f, false).await;
    let (status, body) = post(
        &app,
        "/agentic-workflows/runs",
        json!({ "workflow_ref": "automations/greet.automation.yml" }),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK, "{body}");
    let run_id = body["run_id"].as_str().expect("run_id");
    let run = crud::get_run(&f.db, run_id).await.unwrap().unwrap();
    assert_eq!(Requester::recorded(run.metadata.as_ref()).unwrap(), None);
    let entry = crud::get_queue_entry(&f.db, run_id).await.unwrap().unwrap();
    assert!(entry.scope_owned, "driven by the request's own coordinator");
    assert!(
        state.cancel_txs.contains_key(run_id),
        "registered here, as a co-located run is"
    );
}

/// A Stop on a queued workflow is its driver's to close: the cancel writes the
/// durable flag and leaves the row, where it used to fail it under a driver
/// that may already be running it. A scoped run with nothing driving it in
/// this process is still closed on the spot, as before.
#[tokio::test(flavor = "multi_thread")]
async fn a_cancel_leaves_a_queued_workflow_to_its_driver() {
    let f = fixture().await;
    let db = &f.db;
    let (app, _) = workflow_app(&f, true).await;
    let (status, body) = post(
        &app,
        "/agentic-workflows/runs",
        json!({ "workflow_ref": "automations/greet.automation.yml" }),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK, "{body}");
    let run_id = body["run_id"].as_str().expect("run_id").to_string();

    let (status, _) = post(
        &app,
        &format!("/agentic-workflows/runs/{run_id}/cancel"),
        json!({}),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::NO_CONTENT);
    let run = crud::get_run(db, &run_id).await.unwrap().unwrap();
    assert_eq!(
        run.task_status.as_deref(),
        Some("running"),
        "left to the driver"
    );
    assert!(
        run.cancel_requested_at.is_some(),
        "the flag the driver polls"
    );
    assert!(
        agentic_pipeline::queued_run::held_by_the_queue(db, &run_id)
            .await
            .unwrap()
    );

    // A scoped run whose coordinator is gone: closed here.
    let scoped = Uuid::new_v4().to_string();
    crud::insert_run(
        db,
        &scoped,
        "workflow: x",
        None,
        "workflow",
        None,
        f.workspace_id,
    )
    .await
    .unwrap();
    crud::enqueue_task(
        db,
        &scoped,
        &scoped,
        None,
        &agentic_core::delegation::TaskSpec::Automation {
            workflow_ref: "automations/greet.automation.yml".into(),
            variables: None,
            retry_from_run_id: None,
            cache_enabled: false,
            body: None,
            initial_render_context: None,
        },
        None,
        crud::TaskScope::Scoped,
    )
    .await
    .unwrap();
    let (status, _) = post(
        &app,
        &format!("/agentic-workflows/runs/{scoped}/cancel"),
        json!({}),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::NO_CONTENT);
    let run = crud::get_run(db, &scoped).await.unwrap().unwrap();
    assert_eq!(run.task_status.as_deref(), Some("failed"));
    assert_eq!(run.error_message.as_deref(), Some("cancelled by user"));
}
