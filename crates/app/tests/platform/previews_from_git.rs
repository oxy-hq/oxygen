//! Previews and custom-app staging compile a pushed branch on a process with
//! no working copy.
//!
//! Creating or refreshing a preview needed the one pod that holds workspace
//! files for one thing: finding the branch head in `.git` and compiling its
//! worktree. A branch that is on GitHub is now looked up there and compiled
//! from the commit GitHub has (`oxy_app::server::compile_request`), so these
//! routes are `FleetOk`.
//!
//! Every test runs as `OXY_ROLE=serve` over the real handlers behind the
//! workspace access check: `workspaces.path` names a directory that does not
//! exist and the workspace-path probe is armed. GitHub is a `wiremock` server
//! (`GITHUB_API_URL`), and so is the Factory where a test gives the replica
//! one to ask (`OXY_IDE_UPSTREAM`). A worker's selection-and-drive loop runs
//! the queued compile, as in [`super::compile_from_git`].
//!
//! Run with:
//! `cargo nextest run -p oxy-app --test platform -E 'test(previews_from_git)'`

mod fixture;

use axum::http::StatusCode;
use oxy::workspace_fs_probe::leaks;
use serde_json::{Value, json};
use uuid::Uuid;
use wiremock::matchers::{body_json, header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

use fixture::{BRANCH, HEAD, MOVED, World, encode};

use super::compile_from_git::workspace_tree;

// ── A pushed branch, with no Factory anywhere ────────────────────────────────

#[tokio::test]
async fn a_preview_of_a_pushed_branch_is_created_and_refreshed_with_no_working_copy() {
    let w = World::new(None).await;
    w.fx.branch_is_at(BRANCH, HEAD).await;
    w.fx.serve_commit(HEAD, workspace_tree()).await;

    let created = w.create(BRANCH).await;
    assert_eq!(created.status, StatusCode::ACCEPTED, "{}", created.text);
    let item = &created.json()["item"];
    assert_eq!(item["sha"], HEAD);
    assert_eq!(item["status"], "compiling");
    assert_eq!(item["revision_id"], Value::Null);

    w.fx.drive_until_settled().await;

    let ready = w.listed(BRANCH).await;
    assert_eq!(ready["status"], "ready", "{ready}");
    let revision: Uuid = ready["revision_id"].as_str().unwrap().parse().unwrap();
    assert_eq!(w.fx.revisions_of(HEAD).await[0].kind, "staging");
    assert_eq!(
        w.fx.promoted().await,
        Some(w.fx.served),
        "a preview must never move what the workspace serves"
    );
    // The compile worker asked for the change check when the revision landed.
    assert_eq!(w.checks_queued().await, [revision]);
    let checks = w
        .send(
            "GET",
            format!("{}/checks?branch={}", w.previews(), encode(BRANCH)),
            None,
        )
        .await;
    assert_eq!(checks.status, StatusCode::OK, "{}", checks.text);
    assert_eq!(checks.json()["revision_id"], revision.to_string());

    // The branch moves on GitHub; a refresh follows it. Nothing was pulled.
    w.fx.github.reset().await;
    w.fx.branch_is_at(BRANCH, MOVED).await;
    w.fx.serve_commit(MOVED, workspace_tree()).await;
    let refreshed = w.refresh(BRANCH).await;
    assert_eq!(refreshed.status, StatusCode::ACCEPTED, "{}", refreshed.text);
    assert_eq!(refreshed.json()["item"]["sha"], MOVED);
    assert_eq!(refreshed.json()["item"]["status"], "compiling");

    w.fx.drive_until_settled().await;

    let moved = w.listed(BRANCH).await;
    assert_eq!(moved["status"], "ready", "{moved}");
    assert_ne!(moved["revision_id"], ready["revision_id"]);
    assert_eq!(w.fx.promoted().await, Some(w.fx.served));
    let moved_revision: Uuid = moved["revision_id"].as_str().unwrap().parse().unwrap();
    assert!(
        w.checks_queued().await.contains(&moved_revision),
        "the new revision gets its own check"
    );
    assert_eq!(leaks(), 0, "nothing reached for a working copy");
}

#[tokio::test]
async fn a_second_preview_at_the_same_commit_reuses_the_revision_and_still_gets_its_check() {
    let w = World::new(None).await;
    w.fx.branch_is_at(BRANCH, HEAD).await;
    w.fx.branch_is_at("feat/y", HEAD).await;
    let staged = w.fx.ready_revision(HEAD, "staging").await;

    for branch in [BRANCH, "feat/y"] {
        let created = w.create(branch).await;
        assert_eq!(created.status, StatusCode::ACCEPTED, "{}", created.text);
        let item = &created.json()["item"];
        assert_eq!(item["status"], "ready", "{branch}: {item}");
        assert_eq!(item["revision_id"], staged.to_string());
    }
    assert!(w.fx.queued().await.is_empty(), "nothing was compiled again");
    // No compile will finish to ask for the check, so the create did.
    assert_eq!(w.checks_queued().await, [staged]);
}

/// The server side of `oxyc publish --semantic-branch`: POST, then poll the
/// status by the commit the POST named.
#[tokio::test]
async fn custom_app_staging_compiles_a_pushed_branch_with_no_working_copy() {
    let w = World::new(None).await;
    w.fx.branch_is_at(BRANCH, HEAD).await;
    w.fx.serve_commit(HEAD, workspace_tree()).await;

    let asked = w.stage(BRANCH).await;
    assert_eq!(asked.status, StatusCode::OK, "{}", asked.text);
    assert_eq!(asked.json()["status"], "pending");
    assert_eq!(asked.json()["git_sha"], HEAD);
    assert!(asked.json()["task_id"].is_string(), "this call queued it");

    w.fx.drive_until_settled().await;

    let uri = format!("/api/{}/compile/staging/status?git_sha={HEAD}", w.fx.ws);
    let status = w.send("GET", uri, None).await;
    assert_eq!(status.status, StatusCode::OK, "{}", status.text);
    assert_eq!(status.json()["status"], "ready");
    let revision = w.fx.revisions_of(HEAD).await.remove(0);
    assert_eq!(
        status.json()["revision_id"],
        revision.revision_id.to_string()
    );
    assert_eq!(w.fx.promoted().await, Some(w.fx.served));
}

#[tokio::test]
async fn the_default_branch_is_refused_from_the_recorded_branch() {
    let w = World::new(None).await;

    let refused = w.create("main").await;
    assert_eq!(refused.status, StatusCode::BAD_REQUEST, "{}", refused.text);
    assert_eq!(refused.json()["code"], "default_branch");
    assert_eq!(leaks(), 0, "the default branch was read from the row");
}

// ── A branch GitHub does not have ────────────────────────────────────────────

/// No Factory to ask: the refusal says what is wrong and what to do, and
/// nothing is queued or recorded.
#[tokio::test]
async fn an_unpushed_branch_gets_a_typed_refusal_on_a_pod_with_no_working_copy() {
    let w = World::new(None).await;
    w.fx.github_answers(BRANCH, ResponseTemplate::new(404))
        .await;

    let refused = w.create(BRANCH).await;
    assert_eq!(refused.status, StatusCode::CONFLICT, "{}", refused.text);
    let body = refused.json();
    assert_eq!(body["code"], "cannot_compile");
    assert_eq!(body["reason"], "branch_not_pushed");
    let message = body["message"].as_str().expect("a message");
    assert!(message.contains("push the branch"), "{message}");

    let staged = w.stage(BRANCH).await;
    assert_eq!(staged.status, StatusCode::CONFLICT, "{}", staged.text);
    assert!(
        staged.text.contains("[branch_not_pushed]"),
        "{}",
        staged.text
    );

    assert!(w.fx.queued().await.is_empty(), "a refusal queues nothing");
    assert!(w.preview_rows().await.is_empty(), "and records no preview");
    assert_eq!(leaks(), 0);
}

/// The case that must not change while the Factory exists: the request is
/// sent on to it unchanged, and its answer is the answer.
#[tokio::test]
async fn an_unpushed_branch_is_replayed_to_the_factory_when_there_is_one() {
    let factory = MockServer::start().await;
    let w = World::new(Some(&factory.uri())).await;
    w.fx.github_answers("local-only", ResponseTemplate::new(404))
        .await;
    w.fx.branch_is_at(BRANCH, HEAD).await;
    let answer = json!({ "item": { "branch": "local-only", "status": "compiling" } });
    Mock::given(method("POST"))
        .and(path(w.previews()))
        .and(header("x-oxy-forwarded-by", "serve"))
        .and(body_json(json!({ "branch": "local-only" })))
        .respond_with(
            ResponseTemplate::new(202)
                .insert_header("x-oxy-served-by", "ide@factory#1")
                .set_body_json(&answer),
        )
        .mount(&factory)
        .await;
    Mock::given(method("POST"))
        .and(path(format!("/api/{}/compile/staging", w.fx.ws)))
        .and(query_param("branch", "local-only"))
        .and(header("x-oxy-forwarded-by", "serve"))
        .respond_with(ResponseTemplate::new(200).set_body_string("staged by the factory"))
        .mount(&factory)
        .await;

    let relayed = w.create("local-only").await;
    assert_eq!(relayed.status, StatusCode::ACCEPTED, "{}", relayed.text);
    assert_eq!(relayed.json(), answer);
    assert_eq!(relayed.headers["x-oxy-served-by"], "ide@factory#1");
    assert!(
        relayed.headers.contains_key("x-oxy-forwarded-via"),
        "the hop is visible"
    );
    assert_eq!(w.stage("local-only").await.text, "staged by the factory");
    assert!(
        w.fx.queued().await.is_empty(),
        "this replica queued nothing"
    );
    assert!(w.preview_rows().await.is_empty(), "and recorded nothing");

    // A branch GitHub has never goes near the Factory.
    let before = factory.received_requests().await.unwrap_or_default().len();
    let pushed = w.create(BRANCH).await;
    assert_eq!(pushed.status, StatusCode::ACCEPTED, "{}", pushed.text);
    assert_eq!(pushed.json()["item"]["sha"], HEAD);
    let after = factory.received_requests().await.unwrap_or_default().len();
    assert_eq!(after, before, "the factory was not asked");
}

/// Configured but stopped or restarting: the same refusal as having none, not
/// the generic ide-down 502 — the branch is the thing to fix.
#[tokio::test]
async fn a_factory_that_does_not_answer_gets_the_same_refusal_as_none() {
    // Nothing listens on port 9 (discard) here; the connect is refused.
    let w = World::new(Some("http://127.0.0.1:9")).await;
    w.fx.github_answers(BRANCH, ResponseTemplate::new(404))
        .await;

    let refused = w.create(BRANCH).await;
    assert_eq!(refused.status, StatusCode::CONFLICT, "{}", refused.text);
    assert_eq!(refused.json()["reason"], "branch_not_pushed");
    assert!(!refused.headers.contains_key("x-oxy-forwarded-via"));
}

#[tokio::test]
async fn github_being_down_is_a_503_and_is_not_replayed() {
    let factory = MockServer::start().await;
    let w = World::new(Some(&factory.uri())).await;
    w.fx.github_answers(BRANCH, ResponseTemplate::new(502))
        .await;

    let refused = w.create(BRANCH).await;
    assert_eq!(
        refused.status,
        StatusCode::SERVICE_UNAVAILABLE,
        "{}",
        refused.text
    );
    assert_eq!(refused.json()["code"], "github_unavailable");
    assert!(
        factory
            .received_requests()
            .await
            .unwrap_or_default()
            .is_empty()
    );
}
