//! `oxyc promote` / `oxyc rollback` (environments design §5.5a), through the
//! real routers in a staff person's session, and the one production move every
//! other writer of production's pointer now goes through.
//!
//! | Module | Proves |
//! | --- | --- |
//! | `paths` | staging → production and a sandbox → staging move their pointer, write their event and audit row, and answer the plan; every other pair is `400 promote_path_refused`; `already_serving`; `target_moved`; `--build` must be a build the source served; a promoting publish records staging's event first and warns it is deprecated |
//! | `invariant` | a build staging never served is refused on the new route, Make Live, the console's rollback and promote latest; the move re-asks under the app's row lock, so a pointer or a validation that changed while it waited refuses it |
//! | `lock_resolved` | a move aimed at the draft or the newest build names it under the app's row lock: a draft publish (or a newer build) landing between the caller's read and the lock is the build made live |
//! | `migrations` | a production move applies the build's pending OLTP files before the pointer moves — the new route and Make Live alike — and a failing file leaves the pointer where it was, answered with the migration's own status |
//! | `rollback` | the default target is the previous build (twice swaps back), `--to` names one the environment served, a sandbox is `not_rollbackable`, an environment with nothing before is `nothing_to_roll_back_to` |
//! | `blockers` | an unset `required` key blocks a promote and only warns a rollback; retiring a registered schedule needs the function named; rollback re-registers its target's schedules |
//! | `batch_audit` | the batch publish, promote-latest and unpublish write the single handlers' audit row for every app they move, for the same actor, in the move's transaction; an app whose move is refused has none |
//! | `second_person` | with the app's second-person rule on, its publisher's Promote to production and console rollback to a build production never served are `409 same_person_promote`; their rollback to a build production served stays open; `PUT …/promotion-rule` sets the rule |
//! | `agent_built` | a build a sandbox agent token published is `409 draft_published_by_agent` on every new path — promote from staging, a sandbox's promote to staging, a staging rollback — dry runs included |

use axum::http::StatusCode;
use entity::{app_environment_events, audit_events};
use oxy_api_custom_apps::custom_apps_publish::{PublishTarget, publish, publish_to};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::sandbox_agent_token::fixture::{Agent, send};
use crate::sandbox_publish::{build_pk, input, sandbox};

mod agent_built;
mod batch_audit;
mod blockers;
mod invariant;
mod lock_resolved;
mod migrations;
mod paths;
mod rollback;
mod second_person;

/// One request to `/api` in the minter's own browser session: a staff person.
pub(crate) async fn person(
    agent: &Agent,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    send(method, uri, &[("cookie", &agent.cookie)], body).await
}

/// `POST …/environments/{to}/promote` with `body`.
pub(crate) async fn promote(agent: &Agent, to: &str, body: Value) -> (StatusCode, Value) {
    let uri = format!("/customer-apps/{}/environments/{to}/promote", agent.app.id);
    person(agent, "POST", &uri, Some(body)).await
}

/// `POST …/environments/{environment}/rollback` with `body`.
pub(crate) async fn rollback(agent: &Agent, environment: &str, body: Value) -> (StatusCode, Value) {
    let uri = format!(
        "/customer-apps/{}/environments/{environment}/rollback",
        agent.app.id
    );
    person(agent, "POST", &uri, Some(body)).await
}

/// `(production, staging)` as the environment rows hold them now.
pub(crate) async fn pointers(agent: &Agent) -> (Option<Uuid>, Option<Uuid>) {
    let serving = crate::app_environments::serving(&agent.t.db, agent.app.id).await;
    (serving.production, serving.staging)
}

/// `(environment, build, action)` of the app's events, oldest first.
pub(crate) async fn events(agent: &Agent) -> Vec<(String, Option<Uuid>, String)> {
    app_environment_events::Entity::find()
        .filter(app_environment_events::Column::AppId.eq(agent.app.id))
        .order_by_asc(app_environment_events::Column::At)
        .all(&agent.t.db)
        .await
        .expect("read the events")
        .into_iter()
        .map(|e| (e.environment, e.build_id, e.action))
        .collect()
}

/// The app's audit rows with `action`, oldest first.
pub(crate) async fn audit_rows(agent: &Agent, action: &str) -> Vec<audit_events::Model> {
    audit_events::Entity::find()
        .filter(audit_events::Column::Action.eq(action))
        .filter(audit_events::Column::OrgId.eq(agent.app.org_id))
        .order_by_asc(audit_events::Column::Seq)
        .all(&agent.t.db)
        .await
        .expect("read the audit rows")
}

/// A person publishes `tarball` as a draft `build_id`: staging moves to it.
pub(crate) async fn draft(agent: &Agent, build_id: &str, tarball: Vec<u8>) -> Uuid {
    let slug = agent.app.slug.as_str();
    publish(input(&agent.t, slug, build_id, tarball))
        .await
        .expect("a person's draft");
    build_pk(&agent.t.db, agent.app.id, build_id)
        .await
        .expect("the draft's build")
}

/// A person publishes `tarball` as `build_id` to their sandbox
/// `dev-<handle>`, created first by route when `create`.
pub(crate) async fn sandbox_build(
    agent: &Agent,
    handle: &str,
    build_id: &str,
    tarball: Vec<u8>,
    create: bool,
) -> Uuid {
    if create {
        let uri = format!("/customer-apps/{}/environments", agent.app.id);
        let named = Some(json!({ "name": format!("dev-{handle}") }));
        let (status, body) = person(agent, "POST", &uri, named).await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
    }
    let slug = agent.app.slug.as_str();
    publish_to(
        input(&agent.t, slug, build_id, tarball),
        PublishTarget::Sandbox(sandbox(handle)),
    )
    .await
    .expect("a person's sandbox publish");
    build_pk(&agent.t.db, agent.app.id, build_id)
        .await
        .expect("the sandbox build")
}

/// The build the fixture's production serves (`agent-prod`) and staging's
/// (`agent-stg`).
pub(crate) async fn fixture_builds(agent: &Agent) -> (Uuid, Uuid) {
    let db = &agent.t.db;
    let prod = build_pk(db, agent.app.id, "agent-prod").await;
    let stg = build_pk(db, agent.app.id, "agent-stg").await;
    (prod.expect("agent-prod"), stg.expect("agent-stg"))
}
