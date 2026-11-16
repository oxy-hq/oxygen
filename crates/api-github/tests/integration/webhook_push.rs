//! The GitHub App's webhook receiver (`POST /api/webhooks/github`,
//! [`oxy_api_github::webhook`]): what it answers before it hands a push on, and
//! that a verified push does reach `oxy-app`'s compile reconciler.
//!
//! These were the receiver's half of `oxy-app`'s `platform::compile_push`,
//! which drove the handler through a bare router. The handler left `oxy-app`
//! for this crate on 2026-11-15 and `oxy-app` cannot depend on it, so they
//! moved here; what a push does to the compile queue (which workspaces match,
//! dedupe, the gate, retries) stayed there, driving `push::handle` directly.
//!
//! The signature cases run against a database holding a workspace the push
//! DOES match, so a forged delivery that got through
//! would queue a `compile_git` task: refusing it is proven by the empty queue,
//! not by the status alone.
//!
//! The handler reads `GITHUB_WEBHOOK_SECRET` from the environment, so each
//! test sets it for its own process: nextest runs
//! this binary process-per-test (`db-per-test` in `.config/nextest.toml`).

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::routing::post;
use entity::workspaces::WorkspaceStatus;
use entity::{git_namespaces, users, workspaces};
use hmac::{Hmac, KeyInit, Mac};
use oxy_api_github::webhook::github_webhook;
use sea_orm::{
    ActiveModelTrait, ActiveValue, ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement,
};
use serde_json::{Value, json};
use sha2::Sha256;
use tower::ServiceExt;
use uuid::Uuid;

use crate::common::{Schema, test_db_with};

const SECRET: &str = "whsec_webhook_push_test";
const PATH_TO_ROUTE: &str = "/api/webhooks/github";

/// Point the handler at `secret` (unset when `None`) with the compile gate on.
fn configure(secret: Option<&str>) {
    // SAFETY: nextest runs each test in its own process, and nothing in it has
    // read these yet.
    unsafe {
        match secret {
            Some(secret) => std::env::set_var("GITHUB_WEBHOOK_SECRET", secret),
            None => std::env::remove_var("GITHUB_WEBHOOK_SECRET"),
        }
    }
}

/// Deliver `body` to the route as GitHub would, signed with `secret`.
async fn deliver(event: &str, body: &Value, secret: Option<&str>) -> (StatusCode, String) {
    let raw = serde_json::to_vec(body).unwrap();
    let mut request = Request::post(PATH_TO_ROUTE)
        .header("content-type", "application/json")
        .header("x-github-event", event)
        .header("x-github-delivery", Uuid::new_v4().to_string());
    if let Some(secret) = secret {
        let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(&raw);
        request = request.header(
            "x-hub-signature-256",
            format!("sha256={}", hex::encode(mac.finalize().into_bytes())),
        );
    }
    let router = Router::new().route(PATH_TO_ROUTE, post(github_webhook));
    let response = router
        .oneshot(request.body(Body::from(raw)).unwrap())
        .await
        .expect("oneshot");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .unwrap();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

/// A workspace on `acme/analytics`, default branch `main`, at the repository
/// root — the one [`main_push`] concerns. The rows `oxy-app`'s
/// `platform::compile_from_git` seeds for the same purpose: a user, the PAT
/// namespace the workspace is connected through, and the workspace.
async fn seed_matching_workspace(db: &DatabaseConnection) -> Uuid {
    let user = Uuid::new_v4();
    users::ActiveModel {
        id: ActiveValue::Set(user),
        email: ActiveValue::Set(Some(format!("owner-{user}@example.com"))),
        name: ActiveValue::Set("Owner".into()),
        picture: ActiveValue::Set(None),
        email_verified: ActiveValue::Set(true),
        status: ActiveValue::Set(users::UserStatus::Active),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed user");
    let namespace = Uuid::new_v4();
    git_namespaces::ActiveModel {
        id: ActiveValue::Set(namespace),
        installation_id: ActiveValue::Set(0),
        name: ActiveValue::Set("acme".into()),
        owner_type: ActiveValue::Set("User".into()),
        provider: ActiveValue::Set("github".into()),
        slug: ActiveValue::Set("pat".into()),
        oauth_token: ActiveValue::Set("ghp_webhook_push_test".into()),
        created_by: ActiveValue::Set(user),
        org_id: ActiveValue::Set(None),
    }
    .insert(db)
    .await
    .expect("seed namespace");
    let ws = Uuid::new_v4();
    workspaces::ActiveModel {
        id: ActiveValue::Set(ws),
        name: ActiveValue::Set("From git".into()),
        path: ActiveValue::Set(Some(format!("/nonexistent/workspaces/{ws}"))),
        git_namespace_id: ActiveValue::Set(Some(namespace)),
        git_remote_url: ActiveValue::Set(Some("https://github.com/acme/analytics.git".into())),
        status: ActiveValue::Set(WorkspaceStatus::Ready),
        default_branch: ActiveValue::Set(Some("main".into())),
        repo_subdir: ActiveValue::Set(None),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed workspace");
    ws
}

/// The workspace ids of every queued compile task, whatever its state.
async fn compile_tasks(db: &DatabaseConnection) -> Vec<String> {
    db.query_all_raw(Statement::from_string(
        DatabaseBackend::Postgres,
        "SELECT spec->>'workspace_id' FROM agentic_task_queue \
         WHERE spec->>'type' = 'compile' ORDER BY created_at",
    ))
    .await
    .expect("read the queue")
    .iter()
    .map(|row| row.try_get_by_index(0).unwrap())
    .collect()
}

fn main_push() -> Value {
    json!({
        "ref": "refs/heads/main",
        "after": "0123456789abcdef0123456789abcdef01234567",
        "deleted": false,
        "repository": { "full_name": "acme/analytics" },
        "commits": [{ "added": [], "removed": [], "modified": ["config.yml"] }],
    })
}

/// A forged delivery is refused AND does nothing: with the gate on and a
/// workspace the push matches, an unsigned or wrongly signed push answers 401
/// and queues no compile; the same push, signed, then queues exactly one. The
/// last half is what makes the empty queue mean something.
#[tokio::test]
async fn an_unsigned_or_wrongly_signed_delivery_is_refused_and_does_nothing() {
    let db = test_db_with(Schema::All).await;
    configure(Some(SECRET));
    let ws = seed_matching_workspace(&db).await;
    let body = main_push();

    assert_eq!(
        deliver("push", &body, None).await,
        (StatusCode::UNAUTHORIZED, "invalid signature".to_string())
    );
    assert_eq!(
        deliver("push", &body, Some("not-the-secret")).await,
        (StatusCode::UNAUTHORIZED, "invalid signature".to_string())
    );
    assert!(
        compile_tasks(&db).await.is_empty(),
        "a forged delivery queued a compile: {:?}",
        compile_tasks(&db).await
    );

    // A verified push to the same repository reaches `oxy-app`'s compile
    // reconciler through the prelude and queues the one compile.
    assert_eq!(
        deliver("push", &body, Some(SECRET)).await,
        (
            StatusCode::ACCEPTED,
            "1 workspace(s) match; 1 handed to the compile queue".to_string()
        )
    );
    assert_eq!(compile_tasks(&db).await, vec![ws.to_string()]);
}

#[tokio::test]
async fn other_events_are_acknowledged_ping_is_answered_and_bad_payloads_are_refused() {
    configure(Some(SECRET));
    let (status, body) = deliver("ping", &json!({"zen": "x"}), Some(SECRET)).await;
    assert_eq!((status, body.as_str()), (StatusCode::OK, "pong"));
    let (status, body) = deliver("installation", &json!({"action": "created"}), Some(SECRET)).await;
    assert_eq!(
        (status, body.as_str()),
        (StatusCode::ACCEPTED, "ignored: installation events")
    );
    let (status, _) = deliver("push", &json!({"nope": 1}), Some(SECRET)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn without_a_configured_secret_the_route_does_not_exist() {
    configure(None);
    let (status, _) = deliver("push", &main_push(), Some(SECRET)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    // An empty secret is "not configured" too, not a key anyone can sign with.
    configure(Some(""));
    let (status, _) = deliver("push", &main_push(), Some("")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
