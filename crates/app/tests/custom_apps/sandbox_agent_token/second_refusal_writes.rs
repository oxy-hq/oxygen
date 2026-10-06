//! Every other write to an app's production or staging state is refused a
//! second time (sandbox agent credential design, decision 6, carried to the
//! writes the route allow-list alone refused).
//!
//! Promote, rollback, unpublish and delete — and the rest of the staff
//! console's writes to state a sandbox is not — never reach the authorization
//! model with an environment. Each handler takes the `RefuseSandboxAgent`
//! extractor, which shares no code with the route allow-list. Here the
//! allow-list is **gone**, as in `second_refusal`: the token is authenticated
//! and handed straight to the handler.
//!
//! The cases are held to `REFUSED_WRITES`, the table the route-catalog walk
//! holds to the mounted routes: a console write that is given the extractor
//! and a row is not finished until it has a case here.

use std::collections::BTreeSet;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use axum::routing::{MethodRouter, delete, patch, post, put};
use oxy_app::server::api::admin::app_publish_tokens as publish_tokens;
use oxy_app::server::api::admin::apps::access::set_app_access;
use oxy_app::server::api::admin::apps::handlers as admin;
use oxy_app::server::api::admin::apps::storage;
use oxy_app::server::api::custom_apps_agent_refusal::REFUSED_WRITES;
use oxy_app::server::api::custom_apps_publish_oidc::{delete_publisher, register_publisher};
use sea_orm::{ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter};
use serde_json::{Value, json};
use uuid::Uuid;

use super::fixture::{Agent, agent, app_model};
use super::second_refusal::{sent, unfenced};

/// One write to an app's production or staging state: its method, its route
/// template under `/api`, and the handler production mounts there.
struct Write {
    method: &'static str,
    path: &'static str,
    handler: MethodRouter,
}

fn write(method: &'static str, path: &'static str, handler: MethodRouter) -> Write {
    Write {
        method,
        path,
        handler,
    }
}

/// Every handler that takes the extractor, on every route it is mounted at.
fn fixed_state_writes() -> Vec<Write> {
    vec![
        write("POST", "/customer-apps", post(admin::create_app)),
        write("PATCH", "/customer-apps/{id}", patch(admin::update_app)),
        write("DELETE", "/customer-apps/{id}", delete(admin::delete_app)),
        write(
            "POST",
            "/customer-apps/{id}/publish",
            post(admin::publish_app),
        ),
        write(
            "DELETE",
            "/customer-apps/{id}/publish",
            delete(admin::unpublish_app),
        ),
        write(
            "POST",
            "/customer-apps/{id}/rollback",
            post(admin::rollback_app),
        ),
        write(
            "POST",
            "/customer-apps/batch/publish",
            post(admin::batch_publish_apps),
        ),
        write(
            "POST",
            "/customer-apps/batch/promote-latest",
            post(admin::batch_promote_latest_apps),
        ),
        write(
            "POST",
            "/customer-apps/batch/unpublish",
            post(admin::batch_unpublish_apps),
        ),
        write(
            "POST",
            "/customer-apps/batch/delete",
            post(admin::batch_delete_apps),
        ),
        write(
            "POST",
            "/customer-apps/storage/sweep",
            post(storage::sweep_now),
        ),
        write(
            "POST",
            "/customer-apps/{id}/storage/delete",
            post(storage::delete_objects),
        ),
        write(
            "POST",
            "/customer-apps/{id}/publishers",
            post(register_publisher),
        ),
        write(
            "DELETE",
            "/customer-apps/{id}/publishers/{publisher_id}",
            delete(delete_publisher),
        ),
        write("POST", "/admin/apps", post(admin::create_app)),
        write("PATCH", "/admin/apps/{id}", patch(admin::update_app)),
        write("DELETE", "/admin/apps/{id}", delete(admin::delete_app)),
        write("POST", "/admin/apps/{id}/publish", post(admin::publish_app)),
        write(
            "DELETE",
            "/admin/apps/{id}/publish",
            delete(admin::unpublish_app),
        ),
        write("PUT", "/admin/apps/{id}/access", put(set_app_access)),
        write(
            "POST",
            "/admin/app-publish-tokens",
            post(publish_tokens::create_token),
        ),
        write(
            "POST",
            "/admin/app-publish-tokens/{id}/revoke",
            post(publish_tokens::revoke_token),
        ),
    ]
}

/// A well-formed body for `path`, so that only the refusal stands between
/// the request and the write.
fn body_for(agent: &Agent, path: &str, staging: Uuid) -> Option<Value> {
    let app = agent.app.id;
    let body = match path {
        "/customer-apps" | "/admin/apps" => json!({
            "name": "By Token", "org_id": agent.t.org_id,
            "project_id": agent.app.project_id, "scaffold_pr": false,
        }),
        "/customer-apps/{id}" | "/admin/apps/{id}" => json!({ "name": "Renamed By Token" }),
        "/customer-apps/{id}/rollback" => json!({ "build_id": staging }),
        "/customer-apps/{id}/storage/delete" => json!({ "keys": ["uploads/x"] }),
        "/customer-apps/{id}/publishers" => json!({
            "repo_owner": "acme", "repo_owner_id": 1, "repo_name": "app",
            "workflow_ref": ".github/workflows/oxy-publish.yml", "environment": "production",
        }),
        "/admin/apps/{id}/access" => json!({ "visibility": "restricted" }),
        "/admin/app-publish-tokens" => json!({ "name": "by token" }),
        batch if batch.starts_with("/customer-apps/batch/") => json!({ "ids": [app] }),
        _ => return None,
    };
    Some(body)
}

/// The request for `write`: `{id}` is the token's app, any other id is one
/// that names nothing.
fn request(
    write: &Write,
    body: Option<Value>,
    agent: &Agent,
    credential: (&str, &str),
) -> Request<Body> {
    let uri = write
        .path
        .replace("{id}", &agent.app.id.to_string())
        .replace("{publisher_id}", &Uuid::new_v4().to_string());
    let request = Request::builder()
        .method(write.method)
        .uri(uri)
        .header(credential.0, credential.1);
    match body {
        Some(json) => request
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(json.to_string())),
        None => request.body(Body::empty()),
    }
    .expect("request")
}

/// What the writes above would change: the app's row, its builds, the fixed
/// environments, and how many apps, publishers and publish tokens there are.
async fn fixed_state(agent: &Agent) -> Value {
    let db = &agent.t.db;
    let app = app_model(db, agent.app.id).await;
    let mut builds: Vec<Uuid> = entity::app_builds::Entity::find()
        .filter(entity::app_builds::Column::AppId.eq(app.id))
        .all(db)
        .await
        .expect("read the builds")
        .into_iter()
        .map(|build| build.id)
        .collect();
    builds.sort();
    let mut environments: Vec<(String, Option<Uuid>)> = entity::app_environments::Entity::find()
        .filter(entity::app_environments::Column::AppId.eq(app.id))
        .all(db)
        .await
        .expect("read the environments")
        .into_iter()
        .map(|row| (row.name, row.build_id))
        .collect();
    environments.sort();
    let apps = entity::apps::Entity::find().count(db).await.expect("apps");
    let publishers = entity::app_publishers::Entity::find()
        .count(db)
        .await
        .expect("publishers");
    let publish_tokens = entity::app_publish_tokens::Entity::find()
        .count(db)
        .await
        .expect("publish tokens");
    json!({
        "name": app.name, "slug": app.slug, "visibility": app.visibility, "status": app.status,
        "published": app.published_build_id, "draft": app.draft_build_id,
        "published_at": app.published_at.map(|at| at.to_rfc3339()),
        "builds": builds, "environments": environments, "apps": apps,
        "publishers": publishers, "publish_tokens": publish_tokens,
    })
}

/// The cases are exactly the table: no row without a case, no case off it.
#[test]
fn the_cases_are_every_row_of_the_refused_writes_table() {
    let cases: BTreeSet<(&str, &str)> = fixed_state_writes()
        .iter()
        .map(|write| (write.method, write.path))
        .collect();
    let table: BTreeSet<(&str, &str)> = REFUSED_WRITES.iter().copied().collect();
    assert_eq!(cases, table);
    assert_eq!(cases.len(), fixed_state_writes().len(), "no case twice");
}

/// Promote, rollback, unpublish and delete — and every other write to an
/// app's production or staging state — refuse the token in the handler. With
/// only authentication in front, each answers `403 sandbox_token_refused` and
/// nothing moves. Then the same promote, by the minter's own session, goes
/// through: the refusal was the token's and nothing else's.
#[tokio::test]
async fn every_production_write_is_refused_with_no_allow_list_in_front() {
    let agent = agent().await;
    let before = fixed_state(&agent).await;
    let staging = app_model(&agent.t.db, agent.app.id)
        .await
        .draft_build_id
        .expect("a staging build");
    let bearer = agent.bearer();

    for write in fixed_state_writes() {
        let what = format!("{} {}", write.method, write.path);
        let body = body_for(&agent, write.path, staging);
        let request = request(&write, body, &agent, ("authorization", &bearer));
        let (status, body) = sent(unfenced(write.path, write.handler), request).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{what}: {body}");
        assert_eq!(body["code"], "sandbox_token_refused", "{what}");
        assert_eq!(body["error"], "sandbox_token_refused", "{what}");
        assert_eq!(fixed_state(&agent).await, before, "{what} changed nothing");
    }

    let promote = write(
        "POST",
        "/customer-apps/{id}/publish",
        post(admin::publish_app),
    );
    let request = request(&promote, None, &agent, ("cookie", &agent.cookie));
    let (status, body) = sent(unfenced(promote.path, promote.handler), request).await;
    assert_eq!(status, StatusCode::OK, "the minter's session: {body}");
    let after = app_model(&agent.t.db, agent.app.id).await;
    assert_eq!(
        after.published_build_id,
        Some(staging),
        "promoted by a person"
    );
}
