//! A build a sandbox agent token published **to a sandbox** is never shipped
//! by a blind promote either (`custom_apps_agent_built`; sandbox agent
//! credential design §10.5).
//!
//! Batch *promote latest* takes an app's newest build whatever served it, and
//! a rollback names any retained build, so a token's sandbox build was as
//! shippable as a draft. It is marked as a draft is, and both routes refuse
//! it with the same `409 draft_published_by_agent`, naming the sandbox it was
//! published to. A person's sandbox build is unmarked and both routes ship it
//! as they always did; and the sandbox itself still serves its own build.
//!
//! The token here was minted **without** staging: the mark is every sandbox
//! agent token's.

use axum::http::StatusCode;
use oxy_app::server::api::custom_apps_env_resolve::resolve_function_environment;
use oxy_app::server::api::custom_apps_publish::{PublishTarget, publish_to};
use oxy_auth::user::LOCAL_GUEST_EMAIL;
use serde_json::{Value, json};
use uuid::Uuid;

use super::fixture::{Agent, agent, app_model, tarball};
use super::staging_promote::{build_row, person, production_of};
use crate::sandbox_publish::{build_pk, input, sandbox};
use crate::staging_functions::data;

const REFUSED: &str = "draft_published_by_agent";
/// The token's own sandbox.
const OWN: &str = "dev-ship";
const PROMOTE_LATEST: &str = "/customer-apps/batch/promote-latest";

/// The token creates `name` and publishes `build_id` to it; answers the build.
async fn tokens_sandbox_build(agent: &Agent, name: &str, build_id: &str) -> Uuid {
    let (status, created) = agent.create(name).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let fields = [("build_id", build_id), ("environment", name)];
    let (status, body) = agent.publish("by-token", &fields).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    build_pk(&agent.t.db, agent.app.id, build_id)
        .await
        .expect("the token's sandbox build")
}

/// A person publishes `build_id` to their own sandbox `dev-<handle>`, which
/// they create first if `create` says so.
async fn persons_sandbox_build(agent: &Agent, handle: &str, build_id: &str, create: bool) -> Uuid {
    if create {
        let uri = format!("/customer-apps/{}/environments", agent.app.id);
        let named = Some(json!({ "name": format!("dev-{handle}") }));
        let (status, body) = person(agent, "POST", &uri, named).await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
    }
    let slug = agent.app.slug.as_str();
    let build = input(&agent.t, slug, build_id, tarball(slug, "by-person"));
    publish_to(build, PublishTarget::Sandbox(sandbox(handle)))
        .await
        .expect("a person's publish to their sandbox");
    build_pk(&agent.t.db, agent.app.id, build_id)
        .await
        .expect("the person's sandbox build")
}

async fn promote_latest(agent: &Agent) -> Value {
    let ids = Some(json!({ "ids": [agent.app.id] }));
    let (status, body) = person(agent, "POST", PROMOTE_LATEST, ids).await;
    assert_eq!(status, StatusCode::OK, "a batch is 200: {body}");
    body
}

async fn roll_back_to(agent: &Agent, build: Uuid) -> (StatusCode, Value) {
    let uri = format!("/customer-apps/{}/rollback", agent.app.id);
    person(agent, "POST", &uri, Some(json!({ "build_id": build }))).await
}

/// The token's sandbox build is the app's newest build: *promote latest*
/// refuses it and production stays where it was; a rollback that names it is
/// refused the same way.
#[tokio::test]
async fn neither_promote_latest_nor_a_rollback_ships_a_tokens_sandbox_build() {
    let agent = agent().await;
    let live = production_of(&agent).await;
    assert!(live.is_some(), "a live app");
    let build = tokens_sandbox_build(&agent, OWN, "sbx-by-token").await;
    assert_eq!(
        build_row(&agent, build).await.published_token_id,
        Some(agent.token_id),
        "a token's sandbox build is marked, as its draft is"
    );

    let body = promote_latest(&agent).await;
    assert_eq!(
        (&body["succeeded"], &body["failed"]),
        (&json!(0), &json!(1))
    );
    let row = &body["results"][0];
    assert_eq!(row["ok"], false, "{body}");
    assert_eq!(row["code"], REFUSED, "{body}");
    let error = row["error"].as_str().unwrap_or_default();
    for said in [
        "\"sbx-by-token\"",
        "published to dev-ship",
        LOCAL_GUEST_EMAIL,
        "under your own name",
    ] {
        assert!(error.contains(said), "{said} in {error}");
    }
    assert_eq!(production_of(&agent).await, live, "production never moved");

    let (status, body) = roll_back_to(&agent, build).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], REFUSED, "{body}");
    assert_eq!(body["error"], REFUSED, "{body}");
    assert_eq!(body["build_id"], "sbx-by-token", "{body}");
    assert_eq!(body["environment"], OWN, "{body}");
    assert_eq!(body["token_id"], json!(agent.token_id), "{body}");
    assert_eq!(body["minter"], LOCAL_GUEST_EMAIL, "{body}");
    let message = body["message"].as_str().unwrap_or_default();
    assert!(message.contains("published to dev-ship"), "{message}");
    assert_eq!(production_of(&agent).await, live, "production never moved");
}

/// A person's sandbox build carries no mark, and both routes do with it what
/// they always did: *promote latest* ships the newest build whatever served
/// it, and a rollback ships the build it names.
#[tokio::test]
async fn a_persons_sandbox_build_ships_on_both_routes_as_it_always_did() {
    let agent = agent().await;
    let first = persons_sandbox_build(&agent, "p1", "sbx-by-person-1", true).await;
    assert_eq!(build_row(&agent, first).await.published_token_id, None);

    let body = promote_latest(&agent).await;
    assert_eq!(
        (&body["succeeded"], &body["failed"]),
        (&json!(1), &json!(0))
    );
    assert_eq!(body["results"][0]["ok"], true, "{body}");
    assert!(body["results"][0].get("code").is_none(), "{body}");
    assert_eq!(production_of(&agent).await, Some(first));

    let second = persons_sandbox_build(&agent, "p1", "sbx-by-person-2", false).await;
    assert_eq!(build_row(&agent, second).await.published_token_id, None);
    let (status, body) = roll_back_to(&agent, second).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(production_of(&agent).await, Some(second));

    // With a token's sandbox build now the newest, promote latest stops at
    // it: production stays on the person's build, never the token's.
    let by_token = tokens_sandbox_build(&agent, OWN, "sbx-by-token").await;
    let body = promote_latest(&agent).await;
    assert_eq!(body["results"][0]["code"], REFUSED, "{body}");
    assert_eq!(production_of(&agent).await, Some(second));
    assert_ne!(Some(by_token), production_of(&agent).await);
}

/// The mark changes nothing a sandbox does with its own build: the sandbox
/// serves it, the token calls it and reads its invocations back, and the
/// environment shows it.
#[tokio::test]
async fn the_sandbox_still_serves_its_own_marked_build() {
    let agent = agent().await;
    let build = tokens_sandbox_build(&agent, OWN, "sbx-by-token").await;
    assert!(build_row(&agent, build).await.published_token_id.is_some());

    let app = app_model(&agent.t.db, agent.app.id).await;
    let environment = sandbox("ship");
    let resolved = resolve_function_environment(&agent.t.db, &app, &environment)
        .await
        .expect("resolve the sandbox");
    assert_eq!(
        resolved.build_id,
        Some(build),
        "its own build, marked or not"
    );

    let call = agent.call(Some(OWN), "whoami", json!({})).await;
    assert_eq!(call.status, StatusCode::OK, "{}", call.raw);
    assert_eq!(data(&call)["build"], "by-token", "{}", call.raw);
    assert_eq!(data(&call)["channel"], OWN, "{}", call.raw);

    let (status, shown) = agent.app_get(&format!("environments/{OWN}")).await;
    assert_eq!(status, StatusCode::OK, "{shown}");
    assert_eq!(shown["build_id"], "sbx-by-token", "{shown}");
    // Read-back is bounded by the sandbox's own creation, as before.
    let (status, rows) = agent
        .app_get(&format!("invocations?environment={OWN}"))
        .await;
    assert_eq!(status, StatusCode::OK, "{rows}");
    let listed = rows["invocations"].as_array().map(Vec::len);
    assert_eq!(listed, Some(1), "{rows}");
}
