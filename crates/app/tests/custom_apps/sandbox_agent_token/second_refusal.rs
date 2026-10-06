//! Production writes are refused a second time (sandbox agent credential
//! design, decision 6; §3.3).
//!
//! Nothing in the authorization model sees the operations that change
//! production, so each has a refusal of its own that shares no code with the
//! route allow-list. Here the allow-list is **gone**: the token is
//! authenticated and handed straight to the handler, or to the op behind it,
//! with neither `app_grant_scope_middleware` nor `serve_tree_refuses` in
//! front. Every one is still refused, and production does not move.
//!
//! Here: a channel publish, a production call and a production run, each
//! refused where the operation decides its environment. Every other write to
//! state a sandbox is not is in `second_refusal_writes`.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{HeaderMap, Method, Request, StatusCode, header};
use axum::routing::post;
use axum::{Router, middleware};
use oxy_app::server::api::admin::apps::handlers::run_function_job;
use oxy_app::server::api::custom_apps_functions::handle_function_request;
use oxy_app::server::api::custom_apps_functions::seam::FunctionQueryExecutor;
use oxy_app::server::api::custom_apps_publish::{
    PublishError, PublishTarget, publish, publish_handler, publish_to,
};
use oxy_app::server::api::projects::query::DataPlaneQueryExecutor;
use oxy_auth::middleware::{AuthState, auth_middleware};
use oxy_auth::token::SandboxAgent;
use serde_json::Value;
use tower::ServiceExt;

use super::fixture::{APP, Agent, BOUNDARY, agent, app_model, credential, multipart, tarball};
use crate::custom_app_functions_fixture::invocations;
use crate::sandbox_publish::{input, queued, sandbox};

/// A handler behind authentication alone: no route allow-list.
pub(super) fn unfenced(path: &str, handler: axum::routing::MethodRouter) -> Router {
    Router::new()
        .route(path, handler)
        .layer(middleware::from_fn_with_state(
            AuthState::built_in(SandboxAgent::Admit),
            auth_middleware,
        ))
}

pub(super) async fn sent(router: Router, request: Request<Body>) -> (StatusCode, Value) {
    let response = router.oneshot(request).await.expect("oneshot");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .expect("read the body");
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// The publish route with no allow-list in front, as the token.
async fn publish_unfenced(agent: &Agent, fields: &[(&str, &str)]) -> (StatusCode, Value) {
    let (workspace, org) = (agent.app.project_id.to_string(), agent.t.org_id.to_string());
    let mut all = vec![
        ("app", APP),
        ("project", workspace.as_str()),
        ("org_id", org.as_str()),
    ];
    all.extend_from_slice(fields);
    let request = Request::post("/customer-apps/publish")
        .header("authorization", agent.bearer())
        .header(
            header::CONTENT_TYPE,
            format!("multipart/form-data; boundary={BOUNDARY}"),
        )
        .body(Body::from(multipart(&all, &tarball(APP, "x"))))
        .expect("request");
    sent(
        unfenced("/customer-apps/publish", post(publish_handler)),
        request,
    )
    .await
}

/// A channel publish and a promote, at the route and at the op under it.
#[tokio::test]
async fn a_channel_publish_is_refused_with_no_allow_list_in_front() {
    let agent = agent().await;
    let before = app_model(&agent.t.db, agent.app.id).await;

    let draft: &[(&str, &str)] = &[("build_id", "unfenced-draft")];
    let promote: &[(&str, &str)] = &[("build_id", "unfenced-live"), ("promote", "true")];
    for fields in [draft, promote] {
        let (status, body) = publish_unfenced(&agent, fields).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{fields:?}: {body}");
        assert_eq!(body["code"], "sandbox_token_refused", "{body}");
    }

    // The op itself refuses, whoever calls it: the route's own check is not
    // what holds this.
    let token = credential(&agent.t, agent.token_id, &agent.app);
    let caller = oxy_app::server::authz::Caller::from_user(&oxy_auth::types::AuthenticatedUser {
        id: agent.t.guest_id,
        email: Some(oxy_auth::user::LOCAL_GUEST_EMAIL.to_string()),
        name: "Guest".into(),
        picture: None,
        status: entity::users::UserStatus::Active,
        credential: Some(token),
    });
    let by_token = |build: &str, promote: bool| {
        let mut input = input(&agent.t, APP, build, tarball(APP, "x"));
        input.publisher = Some(caller.clone());
        input.promote = promote;
        input
    };
    for refused in [
        publish(by_token("op-draft", false)).await,
        publish(by_token("op-live", true)).await,
        publish_to(by_token("op-channels", false), PublishTarget::Channels).await,
        publish_to(
            by_token("op-both", true),
            PublishTarget::Sandbox(sandbox("own")),
        )
        .await,
    ] {
        let refused = refused.map(|published| published.build_id);
        assert!(
            matches!(refused, Err(PublishError::SandboxTokenRefused)),
            "{refused:?}"
        );
    }

    let after = app_model(&agent.t.db, agent.app.id).await;
    assert_eq!(
        (after.published_build_id, after.draft_build_id),
        (before.published_build_id, before.draft_build_id),
        "neither pointer moved"
    );
}

/// `/fn` as `serve_dispatch` hands it on, with the serve tree's fence skipped.
async fn call_unfenced(agent: &Agent, environment: Option<&str>) -> StatusCode {
    let mut headers = HeaderMap::new();
    headers.insert("authorization", agent.bearer().parse().expect("a header"));
    if let Some(environment) = environment {
        headers.insert("x-oxy-app-env", environment.parse().expect("a header"));
    }
    let uri = format!("/customer-apps/{}/{APP}/fn/whoami", agent.t.org_slug);
    handle_function_request(
        &agent.t.org_slug,
        APP,
        "whoami",
        Method::POST,
        uri.parse().expect("a uri"),
        headers,
        axum::body::Bytes::from_static(b"{}"),
        Arc::new(DataPlaneQueryExecutor) as Arc<dyn FunctionQueryExecutor>,
        Default::default(),
    )
    .await
    .status()
}

/// A production call, and a staging one: the function gate refuses the
/// token's entrance wherever the environment is not a sandbox it created.
/// No function ran, so no invocation was recorded.
#[tokio::test]
async fn a_production_call_is_refused_with_no_allow_list_in_front() {
    let agent = agent().await;
    for environment in [None, Some("production"), Some("staging")] {
        assert_eq!(
            call_unfenced(&agent, environment).await,
            StatusCode::NOT_FOUND,
            "{environment:?}"
        );
    }
    assert_eq!(
        invocations(&agent.t.db, agent.app.id, "whoami").await.len(),
        0,
        "nothing ran in production or staging"
    );
}

/// Run now with no `?environment=` queues a run in production. With no
/// allow-list in front, the handler refuses the token and queues nothing.
#[tokio::test]
async fn a_production_run_is_refused_with_no_allow_list_in_front() {
    let agent = agent().await;
    let path = "/customer-apps/{id}/functions/{name}/runs";
    for query in ["", "?environment=production", "?environment=staging"] {
        let uri = format!(
            "/customer-apps/{}/functions/smoke/runs{query}",
            agent.app.id
        );
        let request = Request::post(&uri)
            .header("authorization", agent.bearer())
            .body(Body::empty())
            .expect("request");
        let (status, body) = sent(unfenced(path, post(run_function_job)), request).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{query:?}: {body}");
    }
    assert_eq!(
        queued(&agent.t.db, agent.app.id, "app_function").await,
        0,
        "no run was queued"
    );
}
