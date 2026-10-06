//! A sandbox agent token reads the sandbox it has **now**, not an earlier one
//! that had its name (sandbox agent credential design §1, rows R1–R3, C3 and
//! F1).
//!
//! What a sandbox ran is kept under the environment's name and outlives it.
//! So once a colleague's `dev-a` is deleted and the token creates its own
//! `dev-a`, owning the name must not open what the colleague's left behind:
//! not by list, not by id, and not through a replayed idempotency key.

use axum::http::StatusCode;
use entity::app_environments;
use oxy_app::server::api::custom_apps_publish::{PublishTarget, publish_to};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde_json::{Value, json};
use uuid::Uuid;

use super::fixture::{APP, Agent, agent, send, tarball};
use crate::custom_app_functions_fixture::{FnCall, call_function_with, invocations};
use crate::sandbox_publish::{input, sandbox};
use crate::staging_functions::data;

const NAME: &str = "dev-a";
/// An idempotency key the earlier sandbox spent, and the token then uses.
const KEY: &str = "retry-1";

/// One request to `/api` in the minter's own browser session: a person.
async fn person(
    agent: &Agent,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    send(method, uri, &[("cookie", &agent.cookie)], body).await
}

/// `whoami` in `dev-a`, with a write the sandbox holds, under `KEY`.
async fn call(agent: &Agent, credential: (&str, &str)) -> FnCall {
    let headers = [
        credential,
        ("x-oxy-app-env", NAME),
        ("idempotency-key", KEY),
    ];
    let body = json!({ "write": true });
    call_function_with(&agent.t.org_slug, APP, "whoami", body, &headers).await
}

/// What the earlier `dev-a` left under the name.
struct Earlier {
    invocation: Uuid,
    run_id: String,
}

/// A person's `dev-a`: created, published to, called (an invocation and a
/// held write) and given a queued check — then deleted, and its teardown
/// finished, so the row is gone and the name is free.
async fn an_earlier_sandbox_of_the_same_name(agent: &Agent) -> Earlier {
    let app = agent.app.id;
    let environments = format!("/customer-apps/{app}/environments");
    let named = Some(json!({ "name": NAME }));
    let (status, body) = person(agent, "POST", &environments, named).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let build = input(&agent.t, APP, "earlier-1", tarball(APP, "earlier"));
    publish_to(build, PublishTarget::Sandbox(sandbox("a")))
        .await
        .expect("publish to the earlier sandbox");

    // A tag is honoured only on a bearer request, never beside a session
    // cookie: the person sends their session as a bearer, as `oxyc` does.
    let session = agent.cookie.trim_start_matches("oxy_session=");
    let bearer = format!("Bearer {session}");
    let called = call(agent, ("authorization", &bearer)).await;
    assert_eq!(called.status, StatusCode::OK, "{}", called.raw);
    assert_eq!(data(&called)["build"], "earlier");
    let invocation = invocations(&agent.t.db, app, "whoami")
        .await
        .pop()
        .expect("the earlier sandbox's invocation")
        .id;
    let runs = format!("/customer-apps/{app}/functions/smoke/runs?environment={NAME}");
    let (status, queued) = person(agent, "POST", &runs, None).await;
    assert_eq!(status, StatusCode::OK, "{queued}");
    let run_id = queued["run_id"].as_str().expect("a run id").to_string();

    // All of it is the person's to read back while the sandbox is theirs.
    let held = format!("/customer-apps/{app}/invocations/{invocation}/held");
    let (status, body) = person(agent, "GET", &held, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["held"].as_array().map(Vec::len), Some(1), "{body}");

    let (status, body) = person(agent, "DELETE", &format!("{environments}/{NAME}"), None).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    // The last step of its teardown: the row goes, and the name is free.
    app_environments::Entity::delete_many()
        .filter(app_environments::Column::AppId.eq(app))
        .filter(app_environments::Column::Name.eq(NAME))
        .exec(&agent.t.db)
        .await
        .expect("finish the teardown");
    Earlier { invocation, run_id }
}

/// The ids of the invocations a listing returned.
fn listed(rows: &Value) -> Vec<String> {
    rows.as_array()
        .unwrap_or_else(|| panic!("a list: {rows}"))
        .iter()
        .filter_map(|row| row["id"].as_str().map(str::to_string))
        .collect()
}

/// What the token reads of `dev-a`: per function (R1), per app (R2), and the
/// earlier sandbox's invocation (R3) and run (C3) by id.
async fn read_back(agent: &Agent, earlier: &Earlier) -> (Vec<String>, Vec<String>) {
    let by_function = format!("functions/whoami/invocations?environment={NAME}");
    let (status, rows) = agent.app_get(&by_function).await;
    assert_eq!(status, StatusCode::OK, "{rows}");
    let (status, all) = agent
        .app_get(&format!("invocations?environment={NAME}"))
        .await;
    assert_eq!(status, StatusCode::OK, "{all}");

    let held = format!("invocations/{}/held", earlier.invocation);
    let (status, body) = agent.app_get(&held).await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "the earlier held writes: {body}"
    );
    for query in ["", "?environment=dev-a"] {
        let run = format!("function-runs/{}{query}", earlier.run_id);
        let (status, body) = agent.app_get(&run).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "the earlier run: {body}");
    }
    (listed(&rows), listed(&all["invocations"]))
}

#[tokio::test]
async fn the_token_reads_its_own_sandbox_and_nothing_of_an_earlier_one_with_its_name() {
    let agent = agent().await;
    let earlier = an_earlier_sandbox_of_the_same_name(&agent).await;
    let (status, created) = agent.create(NAME).await;
    assert_eq!(status, StatusCode::CREATED, "the name is free: {created}");

    // Nothing ran in the token's sandbox yet, so it reads nothing at all.
    let (by_function, by_app) = read_back(&agent, &earlier).await;
    assert_eq!((by_function.len(), by_app.len()), (0, 0));

    // The key the earlier sandbox spent is unspent here: the call runs the
    // token's build, where a replay would answer with the earlier call's.
    let fields = [("build_id", "own-1"), ("environment", NAME)];
    let (status, published) = agent.publish("own", &fields).await;
    assert_eq!(status, StatusCode::OK, "{published}");
    let bearer = agent.bearer();
    let called = call(&agent, ("authorization", &bearer)).await;
    assert_eq!(called.status, StatusCode::OK, "{}", called.raw);
    assert_eq!(data(&called)["build"], "own", "not the earlier result");
    // The same key again, in the same sandbox, is its own to replay.
    let again = call(&agent, ("authorization", &bearer)).await;
    assert_eq!(data(&again)["build"], "own");
    let own: Vec<Uuid> = invocations(&agent.t.db, agent.app.id, "whoami")
        .await
        .into_iter()
        .filter(|row| row.credential_token_id == Some(agent.token_id))
        .map(|row| row.id)
        .collect();
    assert_eq!(own.len(), 1, "one run, replayed once");

    // Now it reads exactly its own invocation, by list and by id.
    let (by_function, by_app) = read_back(&agent, &earlier).await;
    let expected = vec![own[0].to_string()];
    assert_eq!((&by_function, &by_app), (&expected, &expected));
    let (status, held) = agent.app_get(&format!("invocations/{}/held", own[0])).await;
    assert_eq!(status, StatusCode::OK, "{held}");

    // A person reads by name, as before: both sandboxes' rows, and the
    // earlier one's held writes by id.
    let all = format!(
        "/customer-apps/{}/invocations?environment={NAME}",
        agent.app.id
    );
    let (status, rows) = person(&agent, "GET", &all, None).await;
    assert_eq!(status, StatusCode::OK, "{rows}");
    let mut seen = listed(&rows["invocations"]);
    seen.sort();
    let mut both = vec![own[0].to_string(), earlier.invocation.to_string()];
    both.sort();
    assert_eq!(seen, both, "unchanged for a person");
    let held = format!(
        "/customer-apps/{}/invocations/{}/held",
        agent.app.id, earlier.invocation
    );
    assert_eq!(person(&agent, "GET", &held, None).await.0, StatusCode::OK);
}
