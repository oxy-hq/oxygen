//! The whole sandbox loop with a minted token, through the routers
//! production mounts (`internal-docs/custom-app-sandboxes.md` §1.3; sandbox
//! agent credential design §1, rows A1–S3): introspect, create a sandbox,
//! publish to it, list its functions, set a secret, call a function, queue a
//! check and read it back, read invocations and held writes, read logs,
//! delete the secret and the sandbox, and end the token.
//!
//! Beside each step, what it records: the sandbox's creator, the invocation's
//! credential, the held row's token, and one audit row per write — as the
//! minter, stamped with the token.

use std::time::{Duration, Instant};

use axum::http::StatusCode;
use entity::{app_environments, app_function_invocations, audit_events};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde_json::{Value, json};
use uuid::Uuid;

use super::fixture::{Agent, agent};
use crate::custom_app_functions_manual_run::{platform, spawn_driver};
use crate::staging_functions::{data, held_rows};

const SANDBOX: &str = "dev-a";

/// A1, E2, E1, P1, E3: the token learns what it is, creates a sandbox that
/// records it as the creator, and publishes a build to it.
async fn create_and_publish(agent: &Agent) {
    let (status, me) = agent.get("/auth/token").await;
    assert_eq!(status, StatusCode::OK, "{me}");
    assert_eq!(me["kind"], "sandbox_agent");
    assert_eq!(me["apps"][0]["id"], json!(agent.app.id), "{me}");
    assert_eq!(me["minter"]["user_id"], json!(agent.t.guest_id), "{me}");

    let (status, created) = agent.create(SANDBOX).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    assert_eq!(
        (&created["name"], &created["build_id"]),
        (&json!(SANDBOX), &Value::Null)
    );
    let row = app_environments::Entity::find_by_id((agent.app.id, SANDBOX.to_string()))
        .one(&agent.t.db)
        .await
        .expect("read the sandbox")
        .expect("the sandbox");
    assert_eq!(row.created_by_token_id, Some(agent.token_id));
    assert_eq!(
        row.owner_user_id,
        Some(agent.t.guest_id),
        "the minter owns it"
    );

    let (status, listed) = agent.app_get("environments").await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    let names: Vec<&str> = listed["environments"]
        .as_array()
        .expect("environments")
        .iter()
        .filter_map(|e| e["name"].as_str())
        .collect();
    assert_eq!(names, vec!["production", "staging", SANDBOX]);

    let fields = [("build_id", "agent-a"), ("environment", SANDBOX)];
    let (status, published) = agent.publish("a", &fields).await;
    assert_eq!(status, StatusCode::OK, "{published}");
    assert_eq!(
        (&published["channel"], &published["environment"]),
        (&json!("sandbox"), &json!(SANDBOX))
    );
    let (status, shown) = agent.app_get(&format!("environments/{SANDBOX}")).await;
    assert_eq!(status, StatusCode::OK, "{shown}");
    assert_eq!(shown["build_id"], "agent-a", "{shown}");
}

/// C1, S1, S3, F1: the sandbox's functions are listed, a secret is set and
/// listed without its value, and a call runs the sandbox's build with that
/// secret, as the app's admin. The invocation row and the held-write row
/// both name the token.
async fn configure_and_call(agent: &Agent) {
    let (status, functions) = agent
        .app_get(&format!("functions?environment={SANDBOX}"))
        .await;
    assert_eq!(status, StatusCode::OK, "{functions}");
    let names: Vec<&str> = functions
        .as_array()
        .expect("functions")
        .iter()
        .filter_map(|f| f["name"].as_str())
        .collect();
    assert_eq!(names, vec!["smoke", "whoami"]);

    let secrets = format!("/customer-apps/{}/secrets", agent.app.id);
    let set = json!({ "key": "TOKEN", "value": "sandbox-value", "environment": SANDBOX });
    let (status, body) = agent.api("POST", &secrets, Some(set)).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let (status, listed) = agent.get(&format!("{secrets}?environment={SANDBOX}")).await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    let entry = listed["entries"]
        .as_array()
        .expect("entries")
        .iter()
        .find(|e| e["key"] == "TOKEN")
        .unwrap_or_else(|| panic!("TOKEN is listed: {listed}"));
    assert_eq!(entry["is_set"], true, "{entry}");
    assert!(
        !listed.to_string().contains("sandbox-value"),
        "no value: {listed}"
    );

    let call = agent
        .call(Some(SANDBOX), "whoami", json!({ "write": true }))
        .await;
    assert_eq!(call.status, StatusCode::OK, "{}", call.raw);
    assert_eq!(
        data(&call),
        &json!({
            "build": "a", "channel": SANDBOX, "role": "admin",
            "secret": "sandbox-value", "write": 409,
        }),
        "the sandbox's build, its secret, and the app's admin on an own sandbox"
    );
    let invocation = invocations(agent, "whoami")
        .await
        .pop()
        .expect("an invocation");
    assert_eq!(invocation.environment, SANDBOX);
    assert_eq!(invocation.credential_token_id, Some(agent.token_id));
    assert_eq!(invocation.user_id, Some(agent.t.guest_id), "as the minter");
    let held = held_rows(&agent.t).await;
    assert_eq!(held.len(), 1, "one held write: {held:?}");
    assert_eq!(held[0].metadata["token_id"], json!(agent.token_id));
    assert_eq!(held[0].actor_user_id, Some(agent.t.guest_id));
}

async fn invocations(agent: &Agent, function: &str) -> Vec<app_function_invocations::Model> {
    crate::custom_app_functions_fixture::invocations(&agent.t.db, agent.app.id, function).await
}

/// The run's detail once it leaves `queued`/`running`, read with the token.
async fn wait_for_run(agent: &Agent, run_id: &str) -> Value {
    let uri = format!("function-runs/{run_id}?environment={SANDBOX}");
    let deadline = Instant::now() + Duration::from_secs(120);
    let mut last = Value::Null;
    while Instant::now() < deadline {
        let (status, body) = agent.app_get(&uri).await;
        assert_eq!(status, StatusCode::OK, "GET {uri}: {body}");
        if !matches!(body["status"].as_str(), Some("queued" | "running")) {
            return body;
        }
        last = body;
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    panic!("run {run_id} did not finish; last: {last}");
}

/// C2, C3, R1, R2, R3: a check is queued in the sandbox and run by the
/// production executor, which admits the token again first; its run, its
/// invocation and its held write are read back with the token.
async fn check_and_read_back(agent: &Agent) {
    let (platform, _platform_dir) = platform().await;
    let driver = spawn_driver(agent.t.db.clone(), platform);
    let runs = format!(
        "/customer-apps/{}/functions/smoke/runs?environment={SANDBOX}",
        agent.app.id
    );
    let (status, queued) = agent.api("POST", &runs, None).await;
    assert_eq!(status, StatusCode::OK, "queue the check: {queued}");
    let run = wait_for_run(agent, queued["run_id"].as_str().expect("a run id")).await;
    driver.abort();
    assert_eq!(
        (&run["status"], &run["environment"]),
        (&json!("done"), &json!(SANDBOX)),
        "{run}"
    );
    let check = invocations(agent, "smoke")
        .await
        .pop()
        .expect("the check ran");
    assert_eq!(check.credential_token_id, Some(agent.token_id));
    assert_eq!(run["invocation_id"], json!(check.id), "{run}");

    let by_function = format!("functions/smoke/invocations?environment={SANDBOX}");
    let (status, rows) = agent.app_get(&by_function).await;
    assert_eq!(status, StatusCode::OK, "{rows}");
    assert_eq!(rows.as_array().map(Vec::len), Some(1), "{rows}");
    let (status, all) = agent
        .app_get(&format!("invocations?environment={SANDBOX}"))
        .await;
    assert_eq!(status, StatusCode::OK, "{all}");
    let mut ran: Vec<&str> = all["invocations"]
        .as_array()
        .expect("invocations")
        .iter()
        .filter_map(|i| i["function_name"].as_str())
        .collect();
    ran.sort_unstable();
    assert_eq!(ran, vec!["smoke", "whoami"], "{all}");

    let (status, held) = agent
        .app_get(&format!("invocations/{}/held", check.id))
        .await;
    assert_eq!(status, StatusCode::OK, "{held}");
    assert_eq!(held["environment"], SANDBOX, "{held}");
    let ops: Vec<&str> = held["held"]
        .as_array()
        .expect("held")
        .iter()
        .filter_map(|w| w["op"].as_str())
        .collect();
    assert_eq!(ops, vec!["fetch"], "{held}");
    let rows = held_rows(&agent.t).await;
    assert_eq!(rows.len(), 2, "the call's and the check's: {rows:?}");
    for row in &rows {
        assert_eq!(row.metadata["token_id"], json!(agent.token_id), "{row:?}");
    }
}

/// L1, S2, E3: the sandbox's logs are the token's to read; its secret and
/// then the sandbox itself are deleted.
async fn logs_and_cleanup(agent: &Agent) {
    let logs = format!(
        "/customer-apps/{}/{}/logs?environment={SANDBOX}",
        agent.t.org_slug, agent.app.slug
    );
    let (status, body) = agent.get(&logs).await;
    // Past both gates: the lines, or — with no log store in this process —
    // the `501` that says capture is not configured. Never a refusal.
    assert!(
        matches!(status, StatusCode::OK | StatusCode::NOT_IMPLEMENTED),
        "{status}: {body}"
    );

    let secret = format!(
        "/customer-apps/{}/secrets/TOKEN?environment={SANDBOX}",
        agent.app.id
    );
    let (status, body) = agent.api("DELETE", &secret, None).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    let sandbox = format!("/customer-apps/{}/environments/{SANDBOX}", agent.app.id);
    let (status, deleting) = agent.api("DELETE", &sandbox, None).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{deleting}");
    assert_eq!(deleting["status"], "deleting");
}

/// Every write the token made left one audit row: the minter as the actor,
/// `api_key` as the actor type, and the token's id, name and kind beside it.
async fn every_write_is_audited_as_the_minter_with_the_token(agent: &Agent) {
    let rows = audit_events::Entity::find()
        .filter(audit_events::Column::OrgId.eq(agent.t.org_id))
        .all(&agent.t.db)
        .await
        .expect("read the audit rows");
    let stamped = |action: &str| -> Vec<&audit_events::Model> {
        let token = json!(agent.token_id);
        rows.iter()
            .filter(|row| row.action == action && row.metadata["token_id"] == token)
            .collect()
    };
    for action in [
        "app.environment.created",
        "app.environment.published",
        "custom_app.secret.set",
        "app.function.run_queued",
        "custom_app.secret.deleted",
        "app.environment.deleted",
    ] {
        let written = stamped(action);
        assert_eq!(written.len(), 1, "one {action} row by the token");
        let row = written[0];
        assert_eq!(row.actor_user_id, Some(agent.t.guest_id), "{action}");
        assert_eq!(row.actor_type, "api_key", "{action}");
        assert_eq!(row.metadata["token_kind"], "sandbox_agent", "{action}");
        assert_eq!(row.environment, SANDBOX, "{action}");
    }
}

#[tokio::test]
async fn a_minted_token_runs_the_whole_loop_in_a_sandbox_it_created() {
    let agent = agent().await;
    let before = fixed_pointers(&agent).await;

    create_and_publish(&agent).await;
    configure_and_call(&agent).await;
    check_and_read_back(&agent).await;
    logs_and_cleanup(&agent).await;
    every_write_is_audited_as_the_minter_with_the_token(&agent).await;
    assert_eq!(
        fixed_pointers(&agent).await,
        before,
        "production and staging never moved"
    );

    // A2: the token ends itself, and is refused from then on.
    let (status, body) = agent.api("DELETE", "/auth/token", None).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    assert_eq!(agent.get("/auth/token").await.0, StatusCode::UNAUTHORIZED);
    let row = entity::api_tokens::Entity::find_by_id(agent.token_id)
        .one(&agent.t.db)
        .await
        .expect("read the token")
        .expect("the token");
    assert!(row.revoked_at.is_some(), "the token's row is revoked");
}

/// What production and staging serve: the app row's two pointers and the two
/// fixed environment rows.
async fn fixed_pointers(agent: &Agent) -> Value {
    let app = super::fixture::app_model(&agent.t.db, agent.app.id).await;
    let rows = app_environments::Entity::find()
        .filter(app_environments::Column::AppId.eq(app.id))
        .filter(app_environments::Column::Name.is_in(["production", "staging"]))
        .all(&agent.t.db)
        .await
        .expect("read the fixed environments");
    let mut rows: Vec<(String, Option<Uuid>)> =
        rows.into_iter().map(|r| (r.name, r.build_id)).collect();
    rows.sort();
    json!({ "published": app.published_build_id, "draft": app.draft_build_id, "rows": rows })
}
