//! "When the minter loses access the token stops at once" (sandbox agent
//! credential design §4), asserted on a condition and never on a duration:
//! no sleep, no cache dropped by hand.
//!
//! - The minter's grant row is deleted while the 60 s grant cache still
//!   holds it — the minter's own browser session goes on answering from that
//!   cache — and the token's very next request is refused.
//! - The minter is deactivated, or the token revoked: the next request is
//!   `401`.
//! - A check the token queued is cancelled when its minter loses the grant
//!   before a worker picks it up.
//! - The token's decisions and its minter's cached ones never meet.

use std::time::{Duration, Instant};

use axum::http::StatusCode;
use entity::users::{self, UserStatus};
use oxy_app::server::api::custom_apps_env_resolve::{
    may_open_environment, may_open_non_production,
};
use oxy_app::server::authz::Caller;
use oxy_app_core::custom_app_environment::AppEnvironment;
use oxy_auth::user::LOCAL_GUEST_EMAIL;
use sea_orm::{ActiveModelTrait, ActiveValue, ConnectionTrait, DatabaseBackend, EntityTrait};
use sea_orm::{IntoActiveModel, Statement};
use serde_json::json;

use super::fixture::{Agent, agent, revoke_staff, send, token_actor};
use crate::custom_app_functions_fixture::invocations;
use crate::custom_app_functions_manual_run::{platform, spawn_driver};

const OWN: &str = "dev-own";

/// The token with a sandbox of its own on a build, and a call there that ran.
async fn with_a_working_sandbox() -> Agent {
    let agent = agent().await;
    let (status, created) = agent.create(OWN).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let fields = [("build_id", "own-1"), ("environment", OWN)];
    let (status, body) = agent.publish("own", &fields).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let call = agent.call(Some(OWN), "whoami", json!({})).await;
    assert_eq!(call.status, StatusCode::OK, "{}", call.raw);
    agent
}

fn refused(status: StatusCode) -> bool {
    matches!(status, StatusCode::FORBIDDEN | StatusCode::NOT_FOUND)
}

/// The minter's own console read, under their browser session.
async fn minter_lists_environments(agent: &Agent) -> StatusCode {
    let uri = format!("/customer-apps/{}/environments", agent.app.id);
    send("GET", &uri, &[("cookie", &agent.cookie)], None)
        .await
        .0
}

#[tokio::test]
async fn the_token_stops_on_its_next_request_when_its_minter_loses_the_grant() {
    let agent = with_a_working_sandbox().await;
    // The minter's session reads the grant, which caches it for 60 s.
    assert_eq!(minter_lists_environments(&agent).await, StatusCode::OK);

    revoke_staff(&agent.t.db, LOCAL_GUEST_EMAIL).await;

    // The cache was not dropped: the session still answers from it. So what
    // refuses the token below is its own uncached read, not an expiry.
    assert_eq!(
        minter_lists_environments(&agent).await,
        StatusCode::OK,
        "the session's cached grant is still warm"
    );
    let call = agent.call(Some(OWN), "whoami", json!({})).await;
    assert!(refused(call.status), "/fn: {} {}", call.status, call.raw);
    let (status, body) = agent.app_get("environments").await;
    assert!(refused(status), "the console: {status} {body}");
    let (status, body) = agent.app_get(&format!("functions?environment={OWN}")).await;
    assert!(refused(status), "a read-back: {status} {body}");
    let fields = [("build_id", "after"), ("environment", OWN)];
    let (status, body) = agent.publish("after", &fields).await;
    assert!(refused(status), "a publish: {status} {body}");
    let secret = json!({ "key": "K", "value": "v", "environment": OWN });
    let secrets = format!("/customer-apps/{}/secrets", agent.app.id);
    let (status, body) = agent.api("POST", &secrets, Some(secret)).await;
    assert!(refused(status), "a secret: {status} {body}");
    let logs = format!(
        "/customer-apps/{}/{}/logs?environment={OWN}",
        agent.t.org_slug, agent.app.slug
    );
    let (status, body) = agent.get(&logs).await;
    assert!(refused(status), "logs: {status} {body}");
}

#[tokio::test]
async fn the_token_stops_when_its_minter_is_deactivated() {
    let agent = with_a_working_sandbox().await;
    let minter = users::Entity::find_by_id(agent.t.guest_id)
        .one(&agent.t.db)
        .await
        .expect("read the minter")
        .expect("the minter");
    let mut minter = minter.into_active_model();
    minter.status = ActiveValue::Set(UserStatus::Deleted);
    minter
        .update(&agent.t.db)
        .await
        .expect("deactivate the minter");

    let call = agent.call(Some(OWN), "whoami", json!({})).await;
    assert_eq!(call.status, StatusCode::UNAUTHORIZED, "{}", call.raw);
    assert_eq!(agent.get("/auth/token").await.0, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn the_token_stops_when_its_minter_revokes_it() {
    let agent = with_a_working_sandbox().await;
    let uri = format!("/user/tokens/{}", agent.token_id);
    let (status, body) = send("DELETE", &uri, &[("cookie", &agent.cookie)], None).await;
    assert!(
        status.is_success(),
        "the minter revokes it: {status} {body}"
    );

    let call = agent.call(Some(OWN), "whoami", json!({})).await;
    assert_eq!(call.status, StatusCode::UNAUTHORIZED, "{}", call.raw);
    assert_eq!(agent.get("/auth/token").await.0, StatusCode::UNAUTHORIZED);
}

async fn run_status(agent: &Agent, run_id: &str) -> Option<String> {
    let row = agent
        .t
        .db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT task_status FROM agentic_runs WHERE id = $1",
            [run_id.into()],
        ))
        .await
        .expect("read the run")?;
    row.try_get("", "task_status").ok()
}

/// A check queued by the token, with no worker running yet; the minter then
/// loses the grant; a worker then picks the run up. The executor admits the
/// token again before starting, finds it no longer reaches the sandbox, and
/// cancels the run: the function never runs.
#[tokio::test]
async fn a_queued_check_is_cancelled_when_its_minter_loses_the_grant_before_it_starts() {
    let agent = with_a_working_sandbox().await;
    let runs = format!(
        "/customer-apps/{}/functions/smoke/runs?environment={OWN}",
        agent.app.id
    );
    let (status, queued) = agent.api("POST", &runs, None).await;
    assert_eq!(status, StatusCode::OK, "queue the check: {queued}");
    let run_id = queued["run_id"].as_str().expect("a run id").to_string();

    revoke_staff(&agent.t.db, LOCAL_GUEST_EMAIL).await;

    let (platform, _platform_dir) = platform().await;
    let driver = spawn_driver(agent.t.db.clone(), platform);
    let deadline = Instant::now() + Duration::from_secs(120);
    let ended = loop {
        let status = run_status(&agent, &run_id).await;
        let terminal = ["done", "failed", "cancelled", "timed_out"];
        if status.as_deref().is_some_and(|s| terminal.contains(&s)) {
            break status.expect("a status");
        }
        assert!(Instant::now() < deadline, "the run never ended: {status:?}");
        tokio::time::sleep(Duration::from_millis(250)).await;
    };
    driver.abort();

    assert!(
        matches!(ended.as_str(), "cancelled" | "failed"),
        "the run did not complete: {ended}"
    );
    assert_eq!(
        invocations(&agent.t.db, agent.app.id, "smoke").await.len(),
        0,
        "the check never ran"
    );
}

/// The minter's session decision is cached per (user, app); a token request
/// authenticates as that user. The token neither reads that cache — staging
/// is `true` for the minter and `false` for the token — nor writes it: the
/// minter's next decision is still `true`.
#[tokio::test]
async fn a_token_decision_never_meets_its_minters_cached_one() {
    let agent = agent().await;
    let db = &agent.t.db;
    let minter = Caller::without_credential(agent.t.guest_id, LOCAL_GUEST_EMAIL);
    let token = Caller::from_user(&token_actor(&agent.t, agent.token_id, &agent.app).user);
    let staging = AppEnvironment::Staging;

    assert!(may_open_non_production(db, &minter, &agent.app).await);
    assert!(!may_open_environment(db, &token, &agent.app, &staging).await);
    assert!(!may_open_non_production(db, &token, &agent.app).await);
    assert!(
        may_open_non_production(db, &minter, &agent.app).await,
        "the token's refusal did not lock the minter out of staging"
    );
    assert!(may_open_environment(db, &minter, &agent.app, &staging).await);
}
