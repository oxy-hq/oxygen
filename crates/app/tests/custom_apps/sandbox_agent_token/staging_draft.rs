//! A draft publish by a sandbox agent token granted an app's staging
//! (`custom_apps_sandboxes::agent_draft`; sandbox agent credential design,
//! "Staging option"), through the routers production mounts.
//!
//! The draft moves staging's pointer and nothing else of the app: the row
//! production shares is left byte for byte as it was, bar the draft pointer.
//! Each of its guards refuses with its own code and moves nothing, and a
//! token minted without staging is answered exactly as it always was.

use axum::http::StatusCode;
use entity::{app_builds, app_environments, apps, audit_events, workspaces};
use oxy_app::server::api::custom_apps_publish::publish;
use sea_orm::{ActiveModelTrait, ActiveValue, ColumnTrait, EntityTrait, QueryFilter};
use serde_json::{Value, json};
use uuid::Uuid;

use super::fixture::{Agent, agent, app_model, mint, publish_as, published, send, tarball};
use crate::sandbox_publish::{app_row, build_pk, input};

/// A sandbox agent token for `apps`, minted with `staging` under `cookie`.
pub(super) async fn mint_with_staging(cookie: &str, apps: &[Uuid]) -> (Uuid, String) {
    let body = json!({ "name": "stager", "kind": "sandbox_agent", "apps": apps, "staging": true });
    let (status, minted) = send("POST", "/user/tokens", &[("cookie", cookie)], Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "mint with staging: {minted}");
    let id = minted["token"]["id"].as_str().expect("a token id");
    let secret = minted["secret"].as_str().expect("the secret").to_string();
    (Uuid::parse_str(id).expect("a uuid"), secret)
}

/// The fixture's agent, holding a token granted its app's staging instead of
/// the plain one: the same tenant, the same live app, the same minter.
pub(super) async fn staging_agent() -> Agent {
    let plain = agent().await;
    let (token_id, secret) = mint_with_staging(&plain.cookie, &[plain.app.id]).await;
    Agent {
        token_id,
        secret,
        ..plain
    }
}

async fn build_labels(agent: &Agent, app: Uuid) -> Vec<String> {
    let mut labels: Vec<String> = app_builds::Entity::find()
        .filter(app_builds::Column::AppId.eq(app))
        .all(&agent.t.db)
        .await
        .expect("read the builds")
        .into_iter()
        .map(|build| build.build_id)
        .collect();
    labels.sort();
    labels
}

async fn environment_build(agent: &Agent, name: &str) -> Option<Uuid> {
    app_environments::Entity::find_by_id((agent.app.id, name.to_string()))
        .one(&agent.t.db)
        .await
        .expect("read the environment")
        .and_then(|row| row.build_id)
}

/// The app, its builds and its two fixed pointers: what a refused publish
/// must leave exactly as it was.
async fn state(agent: &Agent) -> (apps::Model, Vec<String>, Option<Uuid>, Option<Uuid>) {
    (
        app_model(&agent.t.db, agent.app.id).await,
        build_labels(agent, agent.app.id).await,
        environment_build(agent, "staging").await,
        environment_build(agent, "production").await,
    )
}

fn assert_refused(answer: &(StatusCode, Value), status: StatusCode, code: &str, why: &str) {
    let (answered, body) = answer;
    assert_eq!(*answered, status, "{why}: {body}");
    assert_eq!(body["code"], code, "{why}: {body}");
    assert_eq!(body["error"], code, "{why}: {body}");
    let message = body["message"].as_str().unwrap_or_default();
    assert!(!message.is_empty(), "{why}: a sentence, {body}");
}

#[tokio::test]
async fn a_draft_moves_stagings_pointer_and_leaves_the_app_row_as_it_was() {
    let agent = staging_agent().await;
    let before = app_model(&agent.t.db, agent.app.id).await;
    let production = environment_build(&agent, "production").await;
    assert!(before.published_build_id.is_some() && before.published_at.is_some());

    let fields = [("build_id", "agent-draft-1"), ("environment", "staging")];
    let (status, published) = agent.publish("draft", &fields).await;
    assert_eq!(status, StatusCode::OK, "{published}");
    assert_eq!(published["channel"], "draft", "{published}");
    assert_eq!(published["environment"], "staging", "{published}");
    assert_eq!(published["build_id"], "agent-draft-1", "{published}");
    assert_eq!(published["app_id"], json!(agent.app.id), "{published}");
    assert_eq!(published["is_new_app"], false, "{published}");

    // Staging serves the draft: the pointer and its mirror, together.
    let draft = build_pk(&agent.t.db, agent.app.id, "agent-draft-1")
        .await
        .expect("the draft's build");
    let after = app_model(&agent.t.db, agent.app.id).await;
    assert_eq!(after.draft_build_id, Some(draft));
    assert_eq!(environment_build(&agent, "staging").await, Some(draft));
    // Production does not: its pointer and its mirror are where they were.
    assert_eq!(after.published_build_id, before.published_build_id);
    assert_eq!(environment_build(&agent, "production").await, production);

    // And the row production shares is byte for byte what it was — its name,
    // its branch, its workspace, when it was synced and updated, when it was
    // published and by whom — with the draft pointer the only column moved.
    let but_the_pointer = apps::Model {
        draft_build_id: before.draft_build_id,
        ..after.clone()
    };
    assert_eq!(but_the_pointer, before);
    assert_ne!(after.draft_build_id, before.draft_build_id);

    // The token reads its staging back by name.
    let (status, shown) = agent.app_get("environments/staging").await;
    assert_eq!(status, StatusCode::OK, "{shown}");
    assert_eq!(shown["name"], "staging", "{shown}");
    assert_eq!(shown["build_id"], "agent-draft-1", "{shown}");
}

/// One audit row says which token published the draft: the minter as the
/// actor, the token's id and kind beside it, staging as the environment.
#[tokio::test]
async fn a_draft_is_audited_as_the_minter_with_the_token() {
    let agent = staging_agent().await;
    let fields = [("build_id", "agent-draft-a"), ("environment", "staging")];
    let (status, published) = agent.publish("draft", &fields).await;
    assert_eq!(status, StatusCode::OK, "{published}");

    let rows = audit_events::Entity::find()
        .filter(audit_events::Column::OrgId.eq(agent.t.org_id))
        .filter(audit_events::Column::Action.eq("app.environment.published"))
        .all(&agent.t.db)
        .await
        .expect("read the audit rows");
    let token = json!(agent.token_id);
    let by_token: Vec<&audit_events::Model> = rows
        .iter()
        .filter(|row| row.metadata["token_id"] == token)
        .collect();
    assert_eq!(by_token.len(), 1, "one row by the token: {rows:?}");
    let row = by_token[0];
    assert_eq!(row.environment, "staging");
    assert_eq!(row.actor_user_id, Some(agent.t.guest_id));
    assert_eq!(row.actor_type, "api_key");
    assert_eq!(row.metadata["token_kind"], "sandbox_agent");
    assert_eq!(row.metadata["build_id"], "agent-draft-a");
}

/// Guards 1 and 5: what the request itself says. Each is refused before
/// anything is stored, with its code, and nothing moved.
#[tokio::test]
async fn a_draft_never_promotes_and_never_carries_a_field_that_rewrites_the_app() {
    let agent = staging_agent().await;
    let before = state(&agent).await;

    // 1: no environment, a promote, the published channel.
    let unnamed: &[(&str, &str)] = &[("build_id", "g1")];
    let promote: &[(&str, &str)] = &[
        ("build_id", "g1"),
        ("environment", "staging"),
        ("promote", "true"),
    ];
    let live: &[(&str, &str)] = &[
        ("build_id", "g1"),
        ("environment", "staging"),
        ("channel", "published"),
    ];
    let bare_promote: &[(&str, &str)] = &[("build_id", "g1"), ("promote", "true")];
    for fields in [unnamed, promote, live, bare_promote] {
        let answer = agent.publish("x", fields).await;
        let why = format!("{fields:?}");
        assert_refused(
            &answer,
            StatusCode::FORBIDDEN,
            "sandbox_token_refused",
            &why,
        );
    }
    // Production is not an environment a publish names, for anyone.
    let production = [("build_id", "g1"), ("environment", "production")];
    let (status, body) = agent.publish("x", &production).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");

    // 5: each field that rewrites the app, or pins what staging reads.
    let revision = Uuid::new_v4().to_string();
    for (field, value) in [
        ("name", "Renamed By An Agent"),
        ("branch", "feature/agent"),
        ("semantic_revision_id", revision.as_str()),
    ] {
        let fields = [
            ("build_id", "g5"),
            ("environment", "staging"),
            (field, value),
        ];
        let answer = agent.publish("x", &fields).await;
        assert_refused(
            &answer,
            StatusCode::BAD_REQUEST,
            "publish_field_refused",
            field,
        );
        let message = answer.1["message"].as_str().unwrap_or_default();
        assert!(message.contains(&format!("'{field}'")), "{message}");
    }

    assert_eq!(state(&agent).await, before, "nothing moved");
}

async fn seed_workspace(agent: &Agent, name: &str) -> Uuid {
    let id = Uuid::new_v4();
    workspaces::ActiveModel {
        id: ActiveValue::Set(id),
        name: ActiveValue::Set(name.into()),
        org_id: ActiveValue::Set(Some(agent.t.org_id)),
        ..Default::default()
    }
    .insert(&agent.t.db)
    .await
    .expect("seed a second workspace");
    id
}

/// Guards 2, 3 and 4: the app the draft names. Another app, an app that is
/// not live and another workspace are each refused with their own code.
#[tokio::test]
async fn a_draft_is_held_to_a_live_app_it_was_granted_in_its_own_workspace() {
    let agent = staging_agent().await;
    let before = state(&agent).await;
    let bearer = agent.bearer();
    let draft = [("build_id", "g2"), ("environment", "staging")];

    // 2: a live app of the same workspace the token was not granted, and an
    // app that does not exist, read the same.
    let sibling = published(&agent.t, "stg-sibling").await;
    let sibling_before = app_row(&agent.t.db, sibling.id).await;
    for slug in ["stg-sibling", "stg-nobody"] {
        let answer = publish_as(&bearer, &agent.t, slug, "x", &draft).await;
        assert_refused(
            &answer,
            StatusCode::NOT_FOUND,
            "environment_not_found",
            slug,
        );
    }
    assert_eq!(app_row(&agent.t.db, sibling.id).await, sibling_before);

    // 4: the app's own workspace, or nothing — no re-home for the token.
    let elsewhere = seed_workspace(&agent, "another workspace")
        .await
        .to_string();
    let moving = [
        ("build_id", "g4"),
        ("environment", "staging"),
        ("project", elsewhere.as_str()),
    ];
    let answer = agent.publish("x", &moving).await;
    assert_refused(&answer, StatusCode::CONFLICT, "project_mismatch", "re-home");
    assert_eq!(state(&agent).await, before, "nothing moved");

    // 3: an app with no production build. Its draft is what production's
    // functions would run, so the token publishes none there.
    let never_promoted = publish(input(
        &agent.t,
        "stg-draft-only",
        "only-draft",
        tarball("stg-draft-only", "draft"),
    ))
    .await
    .expect("a draft-only app")
    .app_id;
    let row = app_row(&agent.t.db, never_promoted).await;
    assert!(row.published_build_id.is_none() && row.published_at.is_none());
    let (_, secret) = mint_with_staging(&agent.cookie, &[never_promoted]).await;
    let bearer = format!("Bearer {secret}");
    let fields = [("build_id", "g3"), ("environment", "staging")];
    let answer = publish_as(&bearer, &agent.t, "stg-draft-only", "x", &fields).await;
    assert_refused(&answer, StatusCode::CONFLICT, "app_not_live", "not live");
    assert_eq!(app_row(&agent.t.db, never_promoted).await, row);
    assert_eq!(
        build_labels(&agent, never_promoted).await,
        vec!["only-draft".to_string()]
    );
}

/// A token minted **without** staging gets today's answers when it names
/// staging on a publish: the route's own `400`, and nothing moved. So does a
/// second token of the same minter, granted staging of another app only.
#[tokio::test]
async fn a_token_without_staging_publishes_no_draft() {
    let plain = agent().await;
    let before = state(&plain).await;
    let draft = [("build_id", "p1"), ("environment", "staging")];
    let (status, body) = plain.publish("x", &draft).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "bad_request", "{body}");
    let message = body["message"].as_str().unwrap_or_default();
    assert!(message.contains("is not a sandbox name"), "{message}");

    let promoting = [
        ("build_id", "p1"),
        ("environment", "staging"),
        ("promote", "true"),
    ];
    let (status, body) = plain.publish("x", &promoting).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "bad_request", "{body}");

    // Granted staging of another app: this app's staging is not its own.
    let other = published(&plain.t, "stg-other").await;
    let (_, secret) = mint(&plain.cookie, &[plain.app.id]).await;
    let (_, staged_elsewhere) = mint_with_staging(&plain.cookie, &[other.id]).await;
    for (secret, status, code) in [
        (secret, StatusCode::BAD_REQUEST, "bad_request"),
        (
            staged_elsewhere,
            StatusCode::NOT_FOUND,
            "environment_not_found",
        ),
    ] {
        let bearer = format!("Bearer {secret}");
        let (answered, body) = publish_as(&bearer, &plain.t, &plain.app.slug, "x", &draft).await;
        assert_eq!(answered, status, "{body}");
        assert_eq!(body["code"], code, "{body}");
    }
    assert_eq!(state(&plain).await, before, "nothing moved");
}
