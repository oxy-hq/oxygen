//! What a draft a sandbox agent token published can never become, and what
//! that changes for nobody else (`custom_apps_agent_built`; sandbox agent
//! credential design, "Staging option", §10.5):
//!
//! - the build remembers its publisher (`app_builds.published_token_id`),
//!   and no build a person published does (a token's sandbox build does too:
//!   `sandbox_promote`);
//! - **no blind promote ships it**: promote, the admin promote, batch
//!   promote, promote latest, a publish token's promote and a rollback to it
//!   each answer `409 draft_published_by_agent`, where a person's draft is
//!   promoted as it always was;
//! - **production never falls back to it**: an app unpublished after an
//!   agent's draft serves nothing in production, where one unpublished after
//!   a person's draft falls back to that draft, as today;
//! - the draft's guards are asked again under the app's row lock.

use std::time::Duration;

use axum::http::StatusCode;
use entity::{app_builds, apps};
use oxy_app::server::api::custom_apps_env_resolve::resolve_function_environment;
use oxy_app::server::api::custom_apps_publish::{PublishError, publish};
use oxy_app::server::api::custom_apps_sandboxes::agent_draft::{
    AgentDraftRefusal, move_draft_pointer,
};
use oxy_app_core::custom_app_environment::AppEnvironment;
use oxy_auth::user::LOCAL_GUEST_EMAIL;
use sea_orm::sea_query::Expr;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QuerySelect, TransactionTrait};
use serde_json::{Value, json};
use uuid::Uuid;

use super::fixture::{Agent, app_model, send, tarball};
use super::staging_draft::staging_agent;
use crate::sandbox_publish::{build_pk, input};

const REFUSED: &str = "draft_published_by_agent";

/// One request to `/api` in the minter's own browser session: a person.
pub(super) async fn person(
    agent: &Agent,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    send(method, uri, &[("cookie", &agent.cookie)], body).await
}

/// The token publishes `build_id` as a draft to staging; answers the build.
async fn agent_draft(agent: &Agent, build_id: &str) -> Uuid {
    let fields = [("build_id", build_id), ("environment", "staging")];
    let (status, body) = agent.publish("agent", &fields).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    build_pk(&agent.t.db, agent.app.id, build_id)
        .await
        .expect("the agent's build")
}

/// A person publishes `build_id` as a draft, under their own name.
async fn persons_draft(agent: &Agent, build_id: &str) -> Uuid {
    let slug = agent.app.slug.as_str();
    publish(input(&agent.t, slug, build_id, tarball(slug, "person")))
        .await
        .expect("a person's draft");
    build_pk(&agent.t.db, agent.app.id, build_id)
        .await
        .expect("the person's build")
}

pub(super) async fn build_row(agent: &Agent, build: Uuid) -> app_builds::Model {
    app_builds::Entity::find_by_id(build)
        .one(&agent.t.db)
        .await
        .expect("read the build")
        .expect("the build")
}

pub(super) async fn production_of(agent: &Agent) -> Option<Uuid> {
    app_model(&agent.t.db, agent.app.id)
        .await
        .published_build_id
}

/// The `409` every promote path answers an agent's draft with: the code, and
/// a sentence naming the build, where it was published, the token and its
/// minter.
fn assert_agent_refusal(status: StatusCode, body: &Value, build_id: &str, why: &str) {
    assert_eq!(status, StatusCode::CONFLICT, "{why}: {body}");
    assert_eq!(body["code"], REFUSED, "{why}: {body}");
    assert_eq!(body["error"], REFUSED, "{why}: {body}");
    assert_eq!(body["build_id"], build_id, "{why}: {body}");
    assert_eq!(body["environment"], "staging", "{why}: {body}");
    assert_eq!(body["token_name"], "stager", "{why}: {body}");
    assert_eq!(body["minter"], LOCAL_GUEST_EMAIL, "{why}: {body}");
    let message = body["message"].as_str().unwrap_or_default();
    let said = [
        "published to staging",
        "stager",
        LOCAL_GUEST_EMAIL,
        "under your own name",
    ];
    for said in said {
        assert!(message.contains(said), "{why}: {said} in {message}");
    }
}

/// A publish token a person minted in their session, as CI holds one.
async fn publish_token(agent: &Agent) -> String {
    let body = Some(json!({ "name": "ci" }));
    let (status, minted) = person(agent, "POST", "/admin/app-publish-tokens", body).await;
    assert_eq!(status, StatusCode::OK, "mint a publish token: {minted}");
    minted["token"].as_str().expect("the token").to_string()
}

#[tokio::test]
async fn a_tokens_draft_is_marked_with_the_token_and_a_persons_is_not() {
    let agent = staging_agent().await;
    let app = agent.app.id;

    let draft = agent_draft(&agent, "marked-1").await;
    assert_eq!(
        build_row(&agent, draft).await.published_token_id,
        Some(agent.token_id)
    );

    // Nothing a person published is, before the token's draft or after it.
    let theirs = persons_draft(&agent, "person-1").await;
    assert_eq!(build_row(&agent, theirs).await.published_token_id, None);
    let unmarked = app_builds::Entity::find()
        .filter(app_builds::Column::AppId.eq(app))
        .filter(app_builds::Column::PublishedTokenId.is_null())
        .all(&agent.t.db)
        .await
        .expect("read the builds");
    // The fixture's two builds and the person's.
    assert_eq!(unmarked.len(), 3, "{unmarked:?}");
}

/// Promote and the admin promote, in a person's session and with a publish
/// token; batch promote; promote latest; a rollback to the build.
#[tokio::test]
async fn no_promote_path_ships_a_draft_a_token_published() {
    let agent = staging_agent().await;
    let app = agent.app.id;
    let live = production_of(&agent).await;
    let draft = agent_draft(&agent, "agent-unapproved").await;
    assert_eq!(
        app_model(&agent.t.db, app).await.draft_build_id,
        Some(draft)
    );

    for uri in [
        format!("/customer-apps/{app}/publish"),
        format!("/admin/apps/{app}/publish"),
    ] {
        let (status, body) = person(&agent, "POST", &uri, None).await;
        assert_agent_refusal(status, &body, "agent-unapproved", &uri);
    }
    let token = publish_token(&agent).await;
    let bearer = format!("Bearer {token}");
    let promote = format!("/customer-apps/{app}/publish");
    let (status, body) = send("POST", &promote, &[("authorization", &bearer)], None).await;
    assert_agent_refusal(status, &body, "agent-unapproved", "a publish token");

    for uri in [
        "/customer-apps/batch/publish",
        "/customer-apps/batch/promote-latest",
    ] {
        let ids = Some(json!({ "ids": [app] }));
        let (status, body) = person(&agent, "POST", uri, ids).await;
        assert_eq!(status, StatusCode::OK, "{uri}: {body}");
        assert_eq!(
            (&body["succeeded"], &body["failed"]),
            (&json!(0), &json!(1))
        );
        let row = &body["results"][0];
        assert_eq!(row["ok"], false, "{uri}: {body}");
        assert_eq!(row["code"], REFUSED, "{uri}: {body}");
        let error = row["error"].as_str().unwrap_or_default();
        assert!(error.contains("stager"), "{uri}: {error}");
    }

    let rollback = format!("/customer-apps/{app}/rollback");
    let named = Some(json!({ "build_id": draft }));
    let (status, body) = person(&agent, "POST", &rollback, named).await;
    assert_agent_refusal(status, &body, "agent-unapproved", "rollback");

    assert_eq!(production_of(&agent).await, live, "production never moved");
}

/// A person who publishes the build under their own name replaces the draft
/// with one that is theirs, and that one promotes as any draft does — while
/// the token's build stays unpromotable by name.
#[tokio::test]
async fn a_persons_own_draft_promotes_as_it_always_did() {
    let agent = staging_agent().await;
    let app = agent.app.id;
    let unapproved = agent_draft(&agent, "agent-unapproved").await;

    let theirs = persons_draft(&agent, "person-approved").await;
    let promote = format!("/customer-apps/{app}/publish");
    let (status, body) = person(&agent, "POST", &promote, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(production_of(&agent).await, Some(theirs));

    // The mark is the build's, not the app's: the token's build is still
    // refused when a rollback names it, and a person's is rolled back to.
    let rollback = format!("/customer-apps/{app}/rollback");
    let named = Some(json!({ "build_id": unapproved }));
    let (status, body) = person(&agent, "POST", &rollback, named).await;
    assert_agent_refusal(status, &body, "agent-unapproved", "rollback");
    let earlier = build_pk(&agent.t.db, app, "agent-prod").await.expect("it");
    let named = Some(json!({ "build_id": earlier }));
    let (status, body) = person(&agent, "POST", &rollback, named).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(production_of(&agent).await, Some(earlier));

    // Every other path, on a person's draft: batch, promote latest, and a
    // publish token's promote.
    let next = persons_draft(&agent, "person-next").await;
    let ids = Some(json!({ "ids": [app] }));
    let (status, body) = person(&agent, "POST", "/customer-apps/batch/publish", ids).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["results"][0]["ok"], true, "{body}");
    assert!(body["results"][0].get("code").is_none(), "{body}");
    assert_eq!(production_of(&agent).await, Some(next));

    let latest = persons_draft(&agent, "person-latest").await;
    let ids = Some(json!({ "ids": [app] }));
    let uri = "/customer-apps/batch/promote-latest";
    let (status, body) = person(&agent, "POST", uri, ids).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["results"][0]["ok"], true, "{body}");
    assert_eq!(production_of(&agent).await, Some(latest));

    let by_ci = persons_draft(&agent, "person-ci").await;
    let token = publish_token(&agent).await;
    let bearer = format!("Bearer {token}");
    let (status, body) = send("POST", &promote, &[("authorization", &bearer)], None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(production_of(&agent).await, Some(by_ci));
}

async fn production_runs(agent: &Agent) -> Option<Uuid> {
    let app = app_model(&agent.t.db, agent.app.id).await;
    resolve_function_environment(&agent.t.db, &app, &AppEnvironment::Production)
        .await
        .expect("resolve production")
        .build_id
}

async fn unpublish(agent: &Agent) {
    let uri = format!("/customer-apps/{}/publish", agent.app.id);
    let (status, body) = person(agent, "DELETE", &uri, None).await;
    assert!(status.is_success(), "unpublish: {status} {body}");
    let app = app_model(&agent.t.db, agent.app.id).await;
    assert!(app.published_build_id.is_none() && app.published_at.is_none());
}

/// An app unpublished after a **person's** draft falls back to that draft on
/// the production path, as it always has. Unpublished after an **agent's**
/// draft, production runs nothing — and staging still serves the draft.
#[tokio::test]
async fn production_never_falls_back_to_a_draft_a_token_published() {
    let agent = staging_agent().await;
    let live = production_of(&agent).await;
    assert_eq!(
        production_runs(&agent).await,
        live,
        "a live app runs its own"
    );

    // A person's draft, then unpublished: today's fallback.
    let theirs = persons_draft(&agent, "person-draft").await;
    unpublish(&agent).await;
    assert_eq!(production_runs(&agent).await, Some(theirs));

    // Promoted again, an agent's draft, then unpublished: nothing.
    let promote = format!("/customer-apps/{}/publish", agent.app.id);
    let (status, body) = person(&agent, "POST", &promote, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let unapproved = agent_draft(&agent, "agent-draft").await;
    unpublish(&agent).await;
    assert_eq!(production_runs(&agent).await, None, "no fallback to it");
    let app = app_model(&agent.t.db, agent.app.id).await;
    let staging = resolve_function_environment(&agent.t.db, &app, &AppEnvironment::Staging)
        .await
        .expect("resolve staging");
    assert_eq!(
        staging.build_id,
        Some(unapproved),
        "staging still serves it"
    );

    // A person publishes over it: the fallback is theirs again.
    let replaced = persons_draft(&agent, "person-again").await;
    assert_eq!(production_runs(&agent).await, Some(replaced));
}

/// The draft's guards are asked of the app's row **under its lock**, in the
/// transaction that moves the pointer: while someone holds the row the move
/// waits, and it then decides on the row as they left it.
#[tokio::test]
async fn the_drafts_guards_are_asked_again_under_the_apps_row_lock() {
    let agent = staging_agent().await;
    let app = agent.app.id;
    let before = app_model(&agent.t.db, app).await;
    // Any build of the app the draft pointer does not already name.
    let target = before.published_build_id.expect("a live app");
    assert_ne!(before.draft_build_id, Some(target));
    let draft = || input(&agent.t, &agent.app.slug, "under-lock", Vec::new());

    let holder = agent.t.db.begin().await.expect("begin");
    apps::Entity::find_by_id(app)
        .lock_exclusive()
        .one(&holder)
        .await
        .expect("lock the app");
    let (db, publishing) = (agent.t.db.clone(), draft());
    let moving =
        tokio::spawn(async move { move_draft_pointer(&db, app, target, &publishing).await });
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(!moving.is_finished(), "the move waits for the app's row");

    // The holder unpublishes, and only then lets go.
    apps::Entity::update_many()
        .col_expr(apps::Column::PublishedBuildId, Expr::value(None::<Uuid>))
        .col_expr(
            apps::Column::PublishedAt,
            Expr::value(None::<chrono::DateTime<chrono::FixedOffset>>),
        )
        .filter(apps::Column::Id.eq(app))
        .exec(&holder)
        .await
        .expect("unpublish under the lock");
    holder.commit().await.expect("commit");

    let refused = moving.await.expect("join").expect_err("refused");
    assert!(
        matches!(
            refused,
            PublishError::AgentDraft(AgentDraftRefusal::AppNotLive { .. })
        ),
        "{refused}"
    );
    let after = app_model(&agent.t.db, app).await;
    assert_eq!(after.draft_build_id, before.draft_build_id, "nothing moved");

    // Live again but re-homed since it was admitted: refused the same way.
    apps::Entity::update_many()
        .col_expr(apps::Column::PublishedBuildId, Expr::value(Some(target)))
        .col_expr(apps::Column::PublishedAt, Expr::value(before.published_at))
        .col_expr(apps::Column::ProjectId, Expr::value(Uuid::new_v4()))
        .filter(apps::Column::Id.eq(app))
        .exec(&agent.t.db)
        .await
        .expect("re-home the app");
    let refused = move_draft_pointer(&agent.t.db, app, target, &draft())
        .await
        .expect_err("refused");
    assert!(
        matches!(
            refused,
            PublishError::AgentDraft(AgentDraftRefusal::ProjectMismatch { .. })
        ),
        "{refused}"
    );
    let after = app_model(&agent.t.db, app).await;
    assert_eq!(after.draft_build_id, before.draft_build_id, "nothing moved");
}
