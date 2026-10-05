//! A custom app's ask records who asked, and whatever drives it without its
//! request drives it as that caller:
//! `POST /api/projects/{id}/agents/{aid}/asks` writes the caller onto the run
//! in the insert that creates it, and `CallerRunResolver` — the resolver
//! `router::recovery` hands every recovery entry point — rebuilds the context
//! the handler had from that record.
//!
//! The comparison is against the context the gate builds for the same
//! request, with a driver tick's own subject-less platform as the control:
//! that is the platform `airhouse_managed` mints a system Admin for, where the
//! handler's (subject, no role) mints the caller's Reader.
//!
//! The ask here never answers: the workspace configures no database, so the
//! pipeline refuses to build. What is under test is the row the start
//! inserted first and the platform recovery would pick for it.
//!
//! **Needs** Postgres only.

use std::sync::Arc;

use agentic_automation::WorkspaceContext;
use agentic_pipeline::platform::{IdentityResolver, PlatformContext, RunPlatformResolver};
use async_trait::async_trait;
use axum::Router;
use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode};
use axum::routing::post;
use entity::{revisions, workspace_compiled_configs};
use oxy_app::agentic_wiring::thread_owner::OxyThreadOwnerLookup;
use oxy_app::server::api::custom_apps_gates::{CustomAppContext, check_custom_app_gates};
use oxy_app::server::api::projects::agent_ask::caller::{
    CallerRunResolver, RUN_CALLER_KEY, RunCaller,
};
use oxy_app::server::api::projects::agent_ask::start_ask;
use oxy_app::server::router::bare_app_state;
use sea_orm::{
    ActiveModelTrait, ActiveValue, ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement,
};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::common::read_repo_file;
use crate::custom_app_procedure_run_fixture::{Fixture, driver_platform, fixture, send};

const START_ROUTE: &str = "/projects/{project_id}/agents/{agent_id}/asks";

/// One serve replica: the production start handler over state of its own.
fn replica(db: &DatabaseConnection) -> Router {
    let mut state = bare_app_state();
    state.agentic_state = Some(Arc::new(agentic_http::AgenticState::new(
        CancellationToken::new(),
        db.clone(),
        Arc::new(OxyThreadOwnerLookup::new(db.clone())),
    )));
    Router::new()
        .route(START_ROUTE, post(start_ask))
        .with_state(state)
}

/// The fixture's workspace, plus an agent a bundle can ask.
async fn workspace_with_an_agent() -> Fixture {
    let f = fixture().await;
    std::fs::write(
        f._workspace_dir.path().join("ask.agentic.yml"),
        "llm:\n  ref: none\n",
    )
    .expect("write the agent");
    f
}

/// `POST …/agents/ask/asks`, and the run row it inserted.
///
/// The fixture's workspace configures no database, so the pipeline refuses to
/// build and the start answers `500` after marking the run failed. The row is
/// inserted before that, by the same statement a `202` goes through, and it
/// is the workspace's only run.
async fn started_run(f: &Fixture) -> agentic_runtime::entity::run::Model {
    let (status, body) = send(
        &replica(&f.t.db),
        Request::post(format!("/projects/{}/agents/ask/asks", f.workspace_id))
            .header("content-type", "application/json")
            .body(Body::from(
                json!({ "v": 1, "question": "how many orders?" }).to_string(),
            ))
            .unwrap(),
    )
    .await;
    assert!(
        status == StatusCode::ACCEPTED || status == StatusCode::INTERNAL_SERVER_ERROR,
        "start: {status} {body}"
    );
    let rows =
        f.t.db
            .query_all_raw(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                "SELECT id FROM agentic_runs WHERE workspace_id = $1",
                [f.workspace_id.into()],
            ))
            .await
            .expect("runs of the workspace");
    assert_eq!(rows.len(), 1, "the start inserts exactly one run: {body}");
    let run_id: String = rows[0].try_get("", "id").expect("id");
    agentic_runtime::crud::get_run(&f.t.db, &run_id)
        .await
        .expect("run lookup")
        .expect("the start registers a run")
}

/// A root run of `workspace` carrying `metadata`, as recovery would read it.
async fn root_with(
    db: &DatabaseConnection,
    workspace: Uuid,
    metadata: Value,
) -> agentic_runtime::entity::run::Model {
    let id = Uuid::new_v4().to_string();
    agentic_runtime::crud::insert_run(db, &id, "q", None, "analytics", Some(metadata), workspace)
        .await
        .expect("insert_run");
    agentic_runtime::crud::get_run(db, &id)
        .await
        .expect("run lookup")
        .expect("the run")
}

async fn gate(workspace: Uuid) -> CustomAppContext {
    check_custom_app_gates(&HeaderMap::new(), workspace)
        .await
        .unwrap_or_else(|_| panic!("the guest passes the gate"))
}

/// The tick's platform, as recovery is handed it.
async fn tick_platform() -> (Arc<dyn PlatformContext>, tempfile::TempDir) {
    let (platform, dir) = driver_platform().await;
    assert_eq!(
        platform.subject(),
        None,
        "the control: a driver's own platform carries no caller"
    );
    let platform: Arc<dyn PlatformContext> = platform;
    (platform, dir)
}

#[tokio::test]
async fn a_started_ask_is_driven_by_recovery_as_the_caller_the_gate_authenticated() {
    let f = workspace_with_an_agent().await;
    let root = started_run(&f).await;

    let gate = gate(f.workspace_id).await;
    let handler_context = gate
        .build_project_context()
        .await
        .unwrap_or_else(|_| panic!("the handler's context builds"));
    assert_eq!(handler_context.subject(), Some(f.t.guest_id));
    assert_eq!(handler_context.role(), None);

    // The start wrote who asked beside the pipeline's own keys, in the row it
    // inserted.
    let metadata = root.metadata.clone().expect("the run has metadata");
    assert_eq!(
        metadata[RUN_CALLER_KEY],
        RunCaller::of(&gate).to_metadata(),
        "metadata: {metadata}"
    );
    assert_eq!(
        metadata[RUN_CALLER_KEY],
        json!({ "user_id": f.t.guest_id }),
        "a live request records the caller and no pin"
    );
    assert_eq!(metadata["agent_id"], "ask", "metadata: {metadata}");

    // The identity recovery drives it with is the handler's, not the tick's.
    let resolver = CallerRunResolver::new(f.t.db.clone(), Arc::new(IdentityResolver));
    let driven = resolver
        .caller_platform(&root)
        .await
        .expect("the recorded caller's context builds")
        .expect("a run that records its caller is driven as that caller");
    assert_eq!(driven.subject(), handler_context.subject(), "subject");
    assert_eq!(driven.role(), handler_context.role(), "role");
    assert_eq!(
        driven.compiled_revision(),
        handler_context.compiled_revision(),
        "the revision it reads"
    );

    // And production's resolver hands recovery that context in place of the
    // tick's own platform, which belongs to another workspace here.
    let (tick, _dir) = tick_platform().await;
    assert_ne!(tick.workspace_id(), f.workspace_id);
    let resolved = CallerRunResolver::shared(&f.t.db)
        .platform_for(&root, tick.clone())
        .await
        .expect("resolves");
    assert!(
        !Arc::ptr_eq(&resolved, &tick),
        "a recorded ask must not be driven on the tick's subject-less platform"
    );
    assert_eq!(resolved.workspace_id(), f.workspace_id);
}

async fn seed_staging_revision(db: &DatabaseConnection, workspace: Uuid) -> Uuid {
    let now = chrono::Utc::now().fixed_offset();
    let id = Uuid::new_v4();
    revisions::ActiveModel {
        revision_id: ActiveValue::Set(id),
        workspace_id: ActiveValue::Set(workspace),
        git_sha: ActiveValue::Set("sha-feat".into()),
        branch: ActiveValue::Set(Some("feat".into())),
        schema_version: ActiveValue::Set(1),
        status: ActiveValue::Set("ready".into()),
        kind: ActiveValue::Set("staging".into()),
        owner_user_id: ActiveValue::Set(None),
        compiler_version: ActiveValue::Set("test".into()),
        started_at: ActiveValue::Set(now),
        finished_at: ActiveValue::Set(Some(now)),
        file_count_seen: ActiveValue::Set(1),
        file_count_compiled: ActiveValue::Set(1),
        file_count_failed: ActiveValue::Set(0),
        error_summary: ActiveValue::Set(None),
    }
    .insert(db)
    .await
    .expect("seed revision");
    // A revision with no compiled config reads as `Origin::Disk`: the context
    // built at it would report no revision, pinned or not.
    workspace_compiled_configs::ActiveModel {
        revision_id: ActiveValue::Set(id),
        databases: ActiveValue::Set(json!([])),
        models: ActiveValue::Set(Some(json!([]))),
        integrations: ActiveValue::Set(None),
        repositories: ActiveValue::Set(None),
        builder_agent: ActiveValue::Set(None),
        mcp: ActiveValue::Set(None),
        other: ActiveValue::Set(None),
    }
    .insert(db)
    .await
    .expect("seed compiled config");
    id
}

/// A staging request's ask reads the revision its app's draft build pins. The
/// record carries that pin, and the rebuilt context reads it too — where the
/// same caller unpinned, and the tick, read whatever is promoted.
#[tokio::test]
async fn a_recovered_ask_reads_the_staging_pin_its_request_had() {
    let f = workspace_with_an_agent().await;
    let pin = seed_staging_revision(&f.t.db, f.workspace_id).await;

    let live = gate(f.workspace_id)
        .await
        .build_project_context()
        .await
        .unwrap_or_else(|_| panic!("the live context builds"));
    let staging_gate = CustomAppContext {
        staging_pin: Some(pin),
        ..gate(f.workspace_id).await
    };
    let handler_context = staging_gate
        .build_project_context()
        .await
        .unwrap_or_else(|_| panic!("the pinned context builds"));
    assert_eq!(handler_context.compiled_revision(), Some(pin));
    assert_ne!(
        live.compiled_revision(),
        Some(pin),
        "the control: without the pin the same caller reads another revision"
    );

    let recorded = RunCaller::of(&staging_gate);
    assert_eq!(recorded.staging_pin, Some(pin));
    let root = root_with(
        &f.t.db,
        f.workspace_id,
        json!({ "agent_id": "ask", RUN_CALLER_KEY: recorded.to_metadata() }),
    )
    .await;

    let driven = CallerRunResolver::new(f.t.db.clone(), Arc::new(IdentityResolver))
        .caller_platform(&root)
        .await
        .expect("the recorded caller's context builds")
        .expect("recorded");
    assert_eq!(
        driven.compiled_revision(),
        handler_context.compiled_revision()
    );
    assert_eq!(driven.subject(), handler_context.subject());
    assert_eq!(driven.role(), handler_context.role());
}

/// Every run that is not a custom-app ask — chat, a schedule, an ask started
/// before the record existed — stays on the platform it was driven on before.
#[tokio::test]
async fn a_run_with_no_recorded_caller_keeps_the_ticks_platform() {
    let f = workspace_with_an_agent().await;
    let root = root_with(&f.t.db, f.workspace_id, json!({ "agent_id": "ask" })).await;
    let (tick, _dir) = tick_platform().await;

    let resolved = CallerRunResolver::shared(&f.t.db)
        .platform_for(&root, tick.clone())
        .await
        .expect("resolves");
    assert!(Arc::ptr_eq(&resolved, &tick));
}

/// A record that cannot be read, and a caller whose workspace is gone, leave
/// the root undriven. Neither falls back to the tick's platform.
#[tokio::test]
async fn a_caller_that_cannot_be_rebuilt_is_never_replaced_by_the_tick() {
    let f = workspace_with_an_agent().await;
    let (tick, _dir) = tick_platform().await;
    let resolver = CallerRunResolver::shared(&f.t.db);

    let unreadable = root_with(
        &f.t.db,
        f.workspace_id,
        json!({ "agent_id": "ask", RUN_CALLER_KEY: "someone" }),
    )
    .await;
    let error = resolver
        .platform_for(&unreadable, tick.clone())
        .await
        .err()
        .expect("an unreadable record is not driven");
    assert!(error.contains("unreadable"), "{error}");

    let caller = RunCaller {
        user_id: f.t.guest_id,
        staging_pin: None,
    };
    let orphan = root_with(
        &f.t.db,
        Uuid::new_v4(),
        json!({ "agent_id": "ask", RUN_CALLER_KEY: caller.to_metadata() }),
    )
    .await;
    let error = resolver
        .platform_for(&orphan, tick.clone())
        .await
        .err()
        .expect("a run whose workspace is gone is not driven");
    assert!(error.contains("no longer exists"), "{error}");
}

/// A resolver that always substitutes one platform, as the preview resolver
/// does for a preview-owned root.
struct Substitute(Arc<dyn PlatformContext>);

#[async_trait]
impl RunPlatformResolver for Substitute {
    async fn platform_for(
        &self,
        _root: &agentic_runtime::entity::run::Model,
        _base: Arc<dyn PlatformContext>,
    ) -> Result<Arc<dyn PlatformContext>, String> {
        Ok(self.0.clone())
    }
}

/// A platform the inner resolver substitutes is kept: a preview-owned root's
/// platform is what holds that run's writes, and a caller record must not
/// swap it for a production context.
#[tokio::test]
async fn a_platform_the_inner_resolver_substitutes_is_kept() {
    let f = workspace_with_an_agent().await;
    let caller = RunCaller {
        user_id: f.t.guest_id,
        staging_pin: None,
    };
    let root = root_with(
        &f.t.db,
        f.workspace_id,
        json!({ "agent_id": "ask", RUN_CALLER_KEY: caller.to_metadata() }),
    )
    .await;
    let (tick, _tick_dir) = tick_platform().await;
    let (preview, _preview_dir) = tick_platform().await;

    let resolved = CallerRunResolver::new(f.t.db.clone(), Arc::new(Substitute(preview.clone())))
        .platform_for(&root, tick)
        .await
        .expect("resolves");
    assert!(Arc::ptr_eq(&resolved, &preview));
}

/// Recovery is handed this resolver at every entry point, or a recorded ask
/// is driven as the tick on the one that was missed. Comments are dropped
/// before counting, so a mention cannot stand in for a call.
#[test]
fn every_recovery_entry_point_is_handed_the_caller_resolver() {
    let code: String = read_repo_file("crates/app/src/server/router/recovery.rs")
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    let entry_points = code.matches("recover_active_runs(").count()
        + code.matches("recover_stranded_runs(").count()
        + code.matches("recover_pending_global_runs(").count();
    assert_eq!(
        entry_points, 5,
        "recovery.rs calls a recovery entry point somewhere this test does not know about"
    );
    assert_eq!(
        code.matches("CallerRunResolver::shared(db),").count(),
        entry_points,
        "every recovery entry point must be handed `CallerRunResolver::shared(db)`"
    );
    assert_eq!(
        code.matches("PreviewRunResolver::").count(),
        0,
        "the preview resolver is composed inside `CallerRunResolver::shared`, not passed alone"
    );
}
