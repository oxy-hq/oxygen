//! What a sandbox agent token granted an app's staging does **on staging**
//! besides publishing a draft, what it still may not do anywhere, and that a
//! token minted without staging is answered on every staging route exactly as
//! it always was (sandbox agent credential design, "Staging option").
//!
//! Sent through the routers production mounts, with a token minted through
//! the real route.

use std::time::{Duration, Instant};

use axum::http::StatusCode;
use entity::{api_token_grants, app_function_invocations};
use sea_orm::{ActiveModelTrait, ActiveValue, ColumnTrait, EntityTrait, QueryFilter};
use serde_json::{Value, json};
use uuid::Uuid;

use super::fixture::{Agent, agent, app_model, published};
use super::staging_draft::staging_agent;
use crate::custom_app_functions_manual_run::{platform, spawn_driver};
use crate::staging_functions::data;

const STAGING: &str = "staging";

/// When the token was granted the app's staging: its `app_staging` row.
async fn granted_at(agent: &Agent) -> chrono::DateTime<chrono::Utc> {
    let grant = api_token_grants::Entity::find()
        .filter(api_token_grants::Column::TokenId.eq(agent.token_id))
        .filter(api_token_grants::Column::Kind.eq(api_token_grants::KIND_APP_STAGING))
        .filter(api_token_grants::Column::AppId.eq(agent.app.id))
        .one(&agent.t.db)
        .await
        .expect("read the grant")
        .expect("an app_staging grant");
    grant.created_at.with_timezone(&chrono::Utc)
}

/// A staging invocation of `whoami` written at `at`, as a colleague's call
/// left one.
async fn staging_invocation(agent: &Agent, at: chrono::DateTime<chrono::Utc>) -> Uuid {
    let app = app_model(&agent.t.db, agent.app.id).await;
    let id = Uuid::new_v4();
    app_function_invocations::ActiveModel {
        id: ActiveValue::Set(id),
        app_id: ActiveValue::Set(app.id),
        build_id: ActiveValue::Set(app.draft_build_id.expect("a staging build")),
        function_name: ActiveValue::Set("whoami".into()),
        mode: ActiveValue::Set("route".into()),
        user_id: ActiveValue::Set(Some(agent.t.guest_id)),
        status: ActiveValue::Set("success".into()),
        duration_ms: ActiveValue::Set(Some(1)),
        error: ActiveValue::Set(None),
        cancel_requested_at: ActiveValue::Set(None),
        created_at: ActiveValue::Set(at.into()),
        idempotency_key: ActiveValue::Set(None),
        result_body: ActiveValue::Set(None),
        result_status: ActiveValue::Set(None),
        request_hash: ActiveValue::Set(None),
        failure_fingerprint: ActiveValue::Set(None),
        environment: ActiveValue::Set(STAGING.into()),
        credential_token_id: ActiveValue::Set(None),
    }
    .insert(&agent.t.db)
    .await
    .expect("seed a staging invocation");
    id
}

fn listed_ids(rows: &Value) -> Vec<String> {
    let rows = rows
        .as_array()
        .or_else(|| rows["invocations"].as_array())
        .expect("a list of invocations");
    rows.iter()
        .filter_map(|row| row["id"].as_str().map(str::to_string))
        .collect()
}

async fn wait_for_run(agent: &Agent, run_id: &str) -> Value {
    let uri = format!("function-runs/{run_id}?environment={STAGING}");
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

/// F1, C1, E3: a call runs staging's build as `ctx.channel = "staging"`, with
/// the app's admin role and every write held; staging's functions and the
/// environment itself are read by name.
async fn call_and_look(agent: &Agent) -> Uuid {
    let call = agent
        .call(Some(STAGING), "whoami", json!({ "write": true }))
        .await;
    assert_eq!(call.status, StatusCode::OK, "{}", call.raw);
    assert_eq!(
        data(&call),
        &json!({
            "build": "staging", "channel": STAGING, "role": "admin",
            "secret": null, "write": 409,
        }),
        "staging's build, under staging's holds, as the app's admin"
    );
    let rows =
        crate::custom_app_functions_fixture::invocations(&agent.t.db, agent.app.id, "whoami").await;
    let own = rows
        .iter()
        .find(|row| row.credential_token_id == Some(agent.token_id))
        .expect("the token's invocation");
    assert_eq!(own.environment, STAGING);
    assert_eq!(own.user_id, Some(agent.t.guest_id), "as the minter");

    let (status, functions) = agent
        .app_get(&format!("functions?environment={STAGING}"))
        .await;
    assert_eq!(status, StatusCode::OK, "{functions}");
    let names: Vec<&str> = functions
        .as_array()
        .expect("functions")
        .iter()
        .filter_map(|f| f["name"].as_str())
        .collect();
    assert_eq!(names, vec!["smoke", "whoami"]);

    let (status, shown) = agent.app_get("environments/staging").await;
    assert_eq!(status, StatusCode::OK, "{shown}");
    assert_eq!(shown["name"], STAGING, "{shown}");
    own.id
}

/// C2, C3: a check is queued on staging and run by the production executor,
/// which admits the token again first.
async fn run_a_check(agent: &Agent) -> Uuid {
    let (platform, _platform_dir) = platform().await;
    let driver = spawn_driver(agent.t.db.clone(), platform);
    let runs = format!(
        "/customer-apps/{}/functions/smoke/runs?environment={STAGING}",
        agent.app.id
    );
    let (status, queued) = agent.api("POST", &runs, None).await;
    assert_eq!(status, StatusCode::OK, "queue the check: {queued}");
    assert_eq!(queued["environment"], STAGING, "{queued}");
    let run = wait_for_run(agent, queued["run_id"].as_str().expect("a run id")).await;
    driver.abort();
    assert_eq!(
        (&run["status"], &run["environment"]),
        (&json!("done"), &json!(STAGING)),
        "{run}"
    );
    let check =
        crate::custom_app_functions_fixture::invocations(&agent.t.db, agent.app.id, "smoke")
            .await
            .pop()
            .expect("the check ran");
    assert_eq!(check.credential_token_id, Some(agent.token_id));
    assert_eq!(check.environment, STAGING);
    check.id
}

#[tokio::test]
async fn a_staging_token_calls_checks_and_reads_staging_from_its_grant_on() {
    let agent = staging_agent().await;
    let since = granted_at(&agent).await;
    // What staging ran before the token was granted it: a colleague's.
    let earlier = staging_invocation(&agent, since - chrono::Duration::hours(1)).await;

    let called = call_and_look(&agent).await;
    let checked = run_a_check(&agent).await;

    // R1, R2: staging's invocations, from the grant on. The colleague's
    // earlier row is staging's too, and is not the token's to read.
    let (status, all) = agent
        .app_get(&format!("invocations?environment={STAGING}"))
        .await;
    assert_eq!(status, StatusCode::OK, "{all}");
    let mut listed = listed_ids(&all);
    listed.sort();
    let mut own = vec![called.to_string(), checked.to_string()];
    own.sort();
    assert_eq!(listed, own, "{all}");
    let by_function = format!("functions/whoami/invocations?environment={STAGING}");
    let (status, rows) = agent.app_get(&by_function).await;
    assert_eq!(status, StatusCode::OK, "{rows}");
    assert_eq!(listed_ids(&rows), vec![called.to_string()], "{rows}");

    // R3: the check's held write is read; the earlier invocation's is not.
    let (status, held) = agent.app_get(&format!("invocations/{checked}/held")).await;
    assert_eq!(status, StatusCode::OK, "{held}");
    assert_eq!(held["environment"], STAGING, "{held}");
    let (status, body) = agent.app_get(&format!("invocations/{earlier}/held")).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["code"], "invocation_not_found", "{body}");

    // L1: staging's logs — the lines, or the `501` of a process with no log
    // store. Never a refusal.
    let logs = format!(
        "/customer-apps/{}/{}/logs?environment={STAGING}",
        agent.t.org_slug, agent.app.slug
    );
    let (status, body) = agent.get(&logs).await;
    assert!(
        matches!(status, StatusCode::OK | StatusCode::NOT_IMPLEMENTED),
        "{status}: {body}"
    );
}

/// With staging granted, everything else is refused as it always was:
/// production in any form, staging's and production's secrets, creating or
/// deleting staging, promote and rollback and every other console write, and
/// another app's staging. Production's pointer never moves.
#[tokio::test]
async fn the_staging_grant_opens_nothing_but_staging() {
    let agent = staging_agent().await;
    let sibling = published(&agent.t, "stg-reach-sibling").await;
    let before = app_model(&agent.t.db, agent.app.id).await;
    let app = agent.app.id;

    // Production: named, or meant by naming nothing.
    for environment in [None, Some("production")] {
        let call = agent.call(environment, "whoami", json!({})).await;
        assert_eq!(call.status, StatusCode::NOT_FOUND, "{}", call.raw);
    }
    let mut refused: Vec<(&str, String, Option<Value>)> = vec![
        ("GET", "functions".into(), None),
        ("GET", "functions?environment=production".into(), None),
        ("GET", "invocations".into(), None),
        ("GET", "invocations?environment=production".into(), None),
        ("GET", "environments/production".into(), None),
        ("POST", "functions/smoke/runs".into(), None),
        (
            "POST",
            "functions/smoke/runs?environment=production".into(),
            None,
        ),
        ("DELETE", "environments/production".into(), None),
        // Staging is not the token's to delete, whatever it may open there.
        ("DELETE", "environments/staging".into(), None),
    ];
    // Secrets are a sandbox's alone: staging's and production's are refused,
    // list and write.
    for environment in ["staging", "production"] {
        let set = json!({ "key": "K", "value": "v", "environment": environment });
        refused.push(("GET", format!("secrets?environment={environment}"), None));
        refused.push(("POST", "secrets".into(), Some(set)));
        refused.push((
            "DELETE",
            format!("secrets/TOKEN?environment={environment}"),
            None,
        ));
    }
    for (method, rest, body) in refused {
        let uri = format!("/customer-apps/{app}/{rest}");
        let (status, answer) = agent.api(method, &uri, body).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{method} {rest}: {answer}");
    }
    let secrets = format!("/customer-apps/{app}/secrets?environment=staging");
    let (_, answer) = agent.get(&secrets).await;
    assert_eq!(answer["code"], "environment_not_found", "{answer}");

    // Staging is never created: the name is not a sandbox's, for anyone.
    let (status, answer) = agent.create(STAGING).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{answer}");
    assert_eq!(answer["code"], "not_a_sandbox", "{answer}");

    // Promote, rollback, unpublish and the rest of the console's writes, and
    // another app — its staging included.
    let other = sibling.id;
    let outside = [
        ("POST", format!("/customer-apps/{app}/publish")),
        ("DELETE", format!("/customer-apps/{app}/publish")),
        ("POST", format!("/customer-apps/{app}/rollback")),
        ("DELETE", format!("/customer-apps/{app}")),
        ("PATCH", format!("/customer-apps/{app}")),
        ("POST", "/customer-apps/batch/promote-latest".to_string()),
        ("POST", format!("/admin/apps/{app}/publish")),
        ("GET", format!("/customer-apps/{app}/secrets/TOKEN/value")),
        (
            "GET",
            format!("/customer-apps/{other}/environments/staging"),
        ),
        (
            "GET",
            format!("/customer-apps/{other}/functions?environment=staging"),
        ),
        (
            "GET",
            format!("/customer-apps/{other}/invocations?environment=staging"),
        ),
    ];
    for (method, uri) in outside {
        let body = (method == "POST").then(|| json!({ "build_id": "x" }));
        let (status, answer) = agent.api(method, &uri, body).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{method} {uri}: {answer}");
    }
    let call = agent
        .call_app(&sibling.slug, Some(STAGING), "whoami", json!({}))
        .await;
    assert_eq!(call.status, StatusCode::NOT_FOUND, "{}", call.raw);

    assert_eq!(
        app_model(&agent.t.db, agent.app.id).await,
        before,
        "nothing of the app moved"
    );
}

/// A token minted **without** staging, on every staging route: the `404` it
/// always got, in the code that route always gave it — where the token of
/// the same minter granted staging is answered.
#[tokio::test]
async fn a_token_without_staging_gets_todays_answers_on_every_staging_route() {
    let plain = agent().await;
    let app = plain.app.id;
    assert!(
        api_token_grants::Entity::find()
            .filter(api_token_grants::Column::TokenId.eq(plain.token_id))
            .filter(api_token_grants::Column::Kind.eq(api_token_grants::KIND_APP_STAGING))
            .one(&plain.t.db)
            .await
            .expect("read the grants")
            .is_none(),
        "a mint that does not ask for staging stores no staging grant"
    );
    let earlier = staging_invocation(&plain, chrono::Utc::now()).await;

    let console: Vec<(&str, String)> = vec![
        ("GET", "environments/staging".into()),
        ("DELETE", "environments/staging".into()),
        ("GET", "functions?environment=staging".into()),
        ("POST", "functions/smoke/runs?environment=staging".into()),
        ("GET", "invocations?environment=staging".into()),
        (
            "GET",
            "functions/whoami/invocations?environment=staging".into(),
        ),
        ("GET", "secrets?environment=staging".into()),
        ("DELETE", "secrets/TOKEN?environment=staging".into()),
    ];
    for (method, rest) in console {
        let uri = format!("/customer-apps/{app}/{rest}");
        let (status, answer) = plain.api(method, &uri, None).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{method} {rest}: {answer}");
        assert_eq!(
            answer["code"], "environment_not_found",
            "{method} {rest}: {answer}"
        );
    }
    let held = format!("/customer-apps/{app}/invocations/{earlier}/held");
    let (status, answer) = plain.get(&held).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{answer}");
    assert_eq!(answer["code"], "invocation_not_found", "{answer}");

    // `/fn` and `/logs` name the app by slugs: one body for everything that
    // is not the token's.
    let call = plain.call(Some(STAGING), "whoami", json!({})).await;
    assert_eq!(call.status, StatusCode::NOT_FOUND, "{}", call.raw);
    let told: Value = serde_json::from_str(&call.raw).expect("the token's one body");
    assert_eq!(told["code"], "not_found", "{told}");
    let logs = format!(
        "/customer-apps/{}/{}/logs?environment={STAGING}",
        plain.t.org_slug, plain.app.slug
    );
    let (status, answer) = plain.get(&logs).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{answer}");
    assert_eq!(answer["code"], "not_found", "{answer}");

    // The same minter's token granted staging is answered on each.
    let (_, secret) = super::staging_draft::mint_with_staging(&plain.cookie, &[app]).await;
    let staged = Agent { secret, ..plain };
    for rest in [
        "environments/staging",
        "functions?environment=staging",
        "invocations?environment=staging",
    ] {
        let (status, answer) = staged.app_get(rest).await;
        assert_eq!(status, StatusCode::OK, "GET {rest}: {answer}");
    }
    let call = staged.call(Some(STAGING), "whoami", json!({})).await;
    assert_eq!(call.status, StatusCode::OK, "{}", call.raw);
}
