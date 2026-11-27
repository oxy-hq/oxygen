//! Promote checks, phases 2 and 3 on the platform side: production attempts,
//! the files an attempt keeps, the checker on production, what follows a
//! failed production check, the seen mark and the settled seam. Through the
//! served routers, as the platform checker's service token (`x-api-key`) where
//! a door is the checker's and in a staff person's session where it is a
//! person's, on the fixture every promote test uses: an app whose production
//! serves `agent-prod` and whose staging serves `agent-stg`, both published by
//! the guest, who is the org's owner and Oxy staff.
//!
//! | Module | Proves |
//! | --- | --- |
//! | `abandoned` | a runner that died after its last check that counts, during the advisory `changes`: past the thirty minutes the row reads `error`, `GET …/checks` writes it so, the attempt settles and its notes are queued once; a runner that turns up finds the attempt closed |
//! | `attempt` | a staging claim answers the contract's shape (`screens` from the manifest under the SDK's rule, `production`, `limits`); a staging attempt settles once and hands itself on once, only with the flag on; a production attempt is claimed, reported and read back in the contract's shapes with the baseline of the build's own `changes` run, and has no functions to settle; a claim after the environment moved skips what would judge another build |
//! | `baseline` | a production attempt's baseline is the build's staging comparison only when that comparison saw the build production served right before the move: a build promoted after another replaced the production it was compared with, and a build production is rolled back to, get none |
//! | `by_hand` | a person's claim is named by the platform (`<who>, by hand`, no link, `by`), whatever label and link it sent, and their report writes `app.build_checks_reported_by_hand` once with the kinds it wrote; the checker's run keeps its own name and link and writes no row |
//! | `reach` | the checker opens production's page, an asset and the beacon and makes the app's own two reads there; a `/fn` call, a write on the serve tree, the data plane's other doors, production's other surfaces (health, logs), the console and another app's build are refused; its visits to staging and production write no view, no event row, no serve or client record and set no cookie, while a person's does |
//! | `files` | a file round-trips on the filesystem build store beside the build; each refusal; the console reads it and the checker cannot; a closed attempt keeps no more; pruned with the build's row and deleted with the app |
//! | `seen` | a person marks the changed screens seen once, audited; nothing to see is `409`; the checker and a report cannot |
//! | `after_move` | each move of production to another build queues a production attempt and its task dispatches `environment: production` to a fake GitHub; a re-stamp, the flag off and the setting off queue nothing |
//! | `failed` | a failed `production_smoke` tells the mover and the org's owners and admins, with the app, the builds and the errors; with `auto_rollback` production is rolled back once, audited, and never again for the same attempt, nor after a rollback; an `error` verdict follows nothing |
//! | `notes` | a settled staging attempt's notes task writes, with a stub in the model's place, a note on the check that failed, the note on the changed screens (both pictures of each, its ratio and its two texts), the `design_audit` row and the summary, as `GET …/checks` and the promote plan answer them, and changes no check; twice writes once; no model writes the two rows as `skipped`; a queued or running model row holds no promote and reads as nothing running |
//! | `notes_pictures` | a model that takes no pictures, and a provider that refuses them, are asked in words and the note says so; a provider that refuses one request without saying it takes none is asked that one again in words and still shown the pictures of the next; a failed call changes no status and its note says so; more pictures than one request carries go in several, every one of them once |

use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode, header};
use entity::app_build_checks;
use oxy_api_custom_apps::custom_apps_build_checks::store::AttemptKey;
use oxy_api_custom_apps::custom_apps_publish::publish;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

use crate::app_checker::{TOKEN, with_checker};
use crate::custom_app_functions_fixture::{ORG_SLUG, serve_router};
use crate::custom_apps_publish_function_artifacts::tar_gz;
use crate::promote_checks_gate::enable_gate;
use crate::promote_rollback::person;
use crate::sandbox_agent_token::fixture::{Agent, agent, send, send_raw};
use crate::sandbox_publish::{build_pk, input};

mod abandoned;
mod after_move;
mod attempt;
mod baseline;
mod by_hand;
mod failed;
mod files;
mod notes;
mod notes_pictures;
mod reach;
mod seen;

/// The fixture with the gate on for its org and the checker's service
/// installed. Authentication stays on (the fixture switched it on).
async fn gated() -> Agent {
    let agent = agent().await;
    enable_gate(&agent).await;
    with_checker(&agent.t).await;
    agent
}

/// One JSON request to the served `/api` tree as the checker.
async fn checker(method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
    send(method, uri, &[("x-api-key", TOKEN)], body).await
}

/// `…/builds/{build}/checks` of the fixture's app.
fn checks(agent: &Agent, build: Uuid) -> String {
    format!("/customer-apps/{}/builds/{build}/checks", agent.app.id)
}

/// A bundle with no functions whose manifest declares `screens`.
fn bundle_with_screens(slug: &str, mark: &str, screens: Option<Value>) -> Vec<u8> {
    let mut manifest = json!({ "schemaVersion": 2, "slug": slug });
    if let Some(screens) = screens {
        manifest["checks"] = json!({ "screens": screens });
    }
    let manifest = manifest.to_string();
    let index =
        format!("<!doctype html><html><head><title>{mark}</title></head><body></body></html>");
    tar_gz(&[
        ("index.html", index.as_bytes()),
        ("oxy-app.json", manifest.as_bytes()),
    ])
}

/// A person publishes a draft `build_id` (no functions): staging moves to it,
/// and its staging attempt 1 is queued.
async fn draft(agent: &Agent, build_id: &str, screens: Option<Value>) -> Uuid {
    let slug = agent.app.slug.as_str();
    let tarball = bundle_with_screens(slug, build_id, screens);
    publish(input(&agent.t, slug, build_id, tarball))
        .await
        .expect("a person's draft");
    build_pk(&agent.t.db, agent.app.id, build_id)
        .await
        .expect("the draft's build")
}

/// The rows of one attempt, by kind.
async fn rows(agent: &Agent, key: &AttemptKey) -> Vec<app_build_checks::Model> {
    let mut rows = app_build_checks::Entity::find()
        .filter(app_build_checks::Column::BuildId.eq(key.build_pk))
        .filter(app_build_checks::Column::Environment.eq(key.environment.as_str()))
        .filter(app_build_checks::Column::Attempt.eq(key.attempt))
        .all(&agent.t.db)
        .await
        .expect("read the rows");
    rows.sort_by(|a, b| a.kind.cmp(&b.kind));
    rows
}

fn key(build: Uuid, environment: &str, attempt: i32) -> AttemptKey {
    AttemptKey {
        build_pk: build,
        environment: environment.to_string(),
        attempt,
    }
}

/// `(kind, status)` of one attempt, by kind.
async fn statuses(agent: &Agent, key: &AttemptKey) -> Vec<(String, String)> {
    rows(agent, key)
        .await
        .into_iter()
        .map(|row| (row.kind, row.status))
        .collect()
}

fn claim_body(environment: &str) -> Value {
    json!({ "attempt": null, "environment": environment,
        "runner": { "label": "GitHub Actions run #1482",
            "url": "https://github.com/oxy-hq/oxygen-internal/actions/runs/123" } })
}

/// A passed verdict of `kind` with `detail` beside its empty findings.
fn passed(kind: &str, mut detail: Value) -> Value {
    detail["findings"] = json!([]);
    json!({ "kind": kind, "status": "passed", "summary": format!("{kind} ok"), "detail": detail })
}

/// What a `changes` run that compared two screens reports: `stores` at desk
/// differs, and production threw `errors` there.
fn changes_detail(errors: Value) -> Value {
    json!({ "production_build_id": "agent-prod", "compared": 2, "changed": 1,
        "screens": [
            { "path": "stores", "viewport": "desk", "changed": true, "ratio": 0.031,
              "staging": "shot-staging-stores-desk.png",
              "production": "shot-production-stores-desk.png", "diff": "diff-stores-desk.png",
              "production_ready": true, "production_errors": errors },
            { "path": "", "viewport": "desk", "changed": false, "ratio": 0,
              "staging": "shot-staging-root-desk.png",
              "production": "shot-production-root-desk.png", "diff": null,
              "production_ready": true, "production_errors": [] },
        ],
        "seen": null })
}

/// The checker claims `build`'s newest staging attempt and reports every
/// runner check as passed, `changes` with [`changes_detail`]. The attempt is
/// settled after it.
async fn pass_staging(agent: &Agent, build: Uuid, errors: Value) {
    let base = checks(agent, build);
    let (status, claimed) = checker(
        "POST",
        &format!("{base}/claim"),
        Some(claim_body("staging")),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{claimed}");
    let report = json!({ "attempt": claimed["attempt"], "environment": "staging", "checks": [
        passed("loads", json!({})),
        passed("design_system", json!({})),
        passed("smoke", json!({ "screens": [] })),
        passed("changes", changes_detail(errors)),
    ] });
    let (status, reported) = checker("POST", &format!("{base}/report"), Some(report)).await;
    assert_eq!(status, StatusCode::OK, "{reported}");
    assert_eq!(reported, json!({ "state": "passed", "open": [] }));
}

/// A person promotes staging's build to production; the plan it answers.
async fn promote(agent: &Agent) -> Value {
    let (status, plan) =
        crate::promote_rollback::promote(agent, "production", json!({ "from": "staging" })).await;
    assert_eq!(status, StatusCode::OK, "{plan}");
    assert_eq!(plan["moved"], true, "{plan}");
    plan
}

/// Set the app's promotion checks settings, as the console does.
async fn settings(agent: &Agent, body: Value) {
    let uri = format!("/customer-apps/{}/promotion-checks", agent.app.id);
    let (status, answered) = person(agent, "PUT", &uri, Some(body)).await;
    assert_eq!(status, StatusCode::OK, "{answered}");
}

/// One request with a raw body to the served `/api` tree. The fixture's
/// sender owns the router: this module opens no database of its own
/// (`shared_db_registry` reads a router built here as one that does).
async fn raw(
    method: &str,
    uri: &str,
    headers: &[(&str, &str)],
    body: Vec<u8>,
) -> (StatusCode, HeaderMap, Vec<u8>) {
    send_raw(method, uri, headers, body).await
}

/// How many notes tasks (`custom_app_build_checks_ai`) are queued for one
/// staging attempt: what the settled seam leaves behind.
async fn notes_queued(agent: &Agent, key: &AttemptKey) -> i64 {
    use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};
    let row = agent
        .t
        .db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT count(*) AS n FROM agentic_task_queue \
             WHERE spec->>'kind' = 'custom_app_build_checks_ai' \
             AND spec->'payload'->>'build_pk' = $1 AND spec->'payload'->>'attempt' = $2",
            [
                key.build_pk.to_string().into(),
                key.attempt.to_string().into(),
            ],
        ))
        .await
        .expect("read the queue")
        .expect("a count");
    row.try_get("", "n").expect("n")
}

/// A refusal's `(status, code)`.
fn refusal(answer: &(StatusCode, HeaderMap, Vec<u8>)) -> (StatusCode, String) {
    let body: Value = serde_json::from_slice(&answer.2).unwrap_or(Value::Null);
    (
        answer.0,
        body["code"].as_str().unwrap_or_default().to_string(),
    )
}

/// One request to the app's path URL (`rest` under its base) on the serve
/// tree, with `headers`: the status, the response headers and the body.
async fn serve(
    agent: &Agent,
    method: &str,
    rest: &str,
    headers: &[(&str, &str)],
    body: &str,
) -> (StatusCode, HeaderMap, String) {
    let mut request = Request::builder()
        .method(method)
        .uri(format!(
            "/customer-apps/{ORG_SLUG}/{}/{rest}",
            agent.app.slug
        ))
        .header(header::ACCEPT, "text/html");
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let request = request.body(Body::from(body.to_string())).expect("request");
    let response = serve_router().oneshot(request).await.expect("oneshot");
    let (status, headers) = (response.status(), response.headers().clone());
    let bytes = axum::body::to_bytes(response.into_body(), 4 * 1024 * 1024)
        .await
        .expect("body");
    (
        status,
        headers,
        String::from_utf8_lossy(&bytes).into_owned(),
    )
}
