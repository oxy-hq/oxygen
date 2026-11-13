//! `POST …/environments` with `from` copies the parent's build pointer
//! (`internal-docs/custom-app-sandboxes.md` §3, Lifecycle): the new
//! sandbox serves the build its parent serves, and that build's migrations are
//! queued into the sandbox's own homes through the task a sandbox publish
//! queues. A parent with no build gives a sandbox with none, as before. Copying
//! a build is not publishing it, so a token that forks its own sandbox marks
//! nothing. A sandbox agent token forks only from an environment it may open:
//! its own sandboxes, never staging, production or a colleague's.

use axum::http::StatusCode;
use entity::{app_builds, app_environment_events, app_environments};
use oxy_api_custom_apps::custom_apps_env_resolve::resolve_environment;
use oxy_api_custom_apps::custom_apps_publish::{PublishTarget, publish, publish_to};
use oxy_api_custom_apps::custom_apps_sandboxes::branch::NewBranch;
use oxy_api_custom_apps::custom_apps_sandboxes::migrations_task::{
    SANDBOX_MIGRATIONS_KIND, SandboxMigrationsTask,
};
use oxy_api_custom_apps::custom_apps_sandboxes::ops;
use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseBackend, DatabaseConnection, EntityTrait, QueryFilter,
    Statement,
};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::custom_app_functions_fixture::seeded_tenant;
use crate::sandbox_agent_token::fixture::agent;
use crate::sandbox_publish::{
    app_row, app_with_two_sandboxes, build_pk, bundle, input, noop, queued, sandbox,
};

/// A bundle that declares one Airhouse migration, in the app's own schema.
fn with_migration(slug: &str) -> Vec<u8> {
    let schema = format!("app_{}", slug.replace('-', "_"));
    let sql = format!(
        "CREATE SCHEMA IF NOT EXISTS {schema};\nCREATE TABLE {schema}.visits (visit_id VARCHAR NOT NULL);"
    );
    bundle(
        slug,
        &[noop("noop", json!({ "route": true }))],
        json!({ "airhouseMigrations": { "dir": "airhouse-migrations" } }),
        &[("airhouse-migrations/0001_visits.sql", sql.as_bytes())],
    )
}

fn from(parent: &str) -> NewBranch {
    NewBranch {
        from: Some(parent.to_string()),
        ..NewBranch::default()
    }
}

/// The build `name` serves, by its pointer row.
async fn serving(db: &DatabaseConnection, app: Uuid, name: &str) -> Option<Uuid> {
    let app = app_row(db, app).await;
    let environment = oxy_app_core::custom_app_environment::AppEnvironment::parse(name)
        .expect("an environment name");
    resolve_environment(db, &app, &environment)
        .await
        .expect("resolve")
        .build_id
}

/// The queued migrations task of `environment`, if any.
async fn queued_task(
    db: &DatabaseConnection,
    app: Uuid,
    environment: &str,
) -> Option<SandboxMigrationsTask> {
    let row = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT spec FROM agentic_task_queue \
             WHERE spec->>'kind' = $1 AND spec->'payload'->>'app_id' = $2 \
               AND spec->'payload'->>'environment' = $3",
            [
                SANDBOX_MIGRATIONS_KIND.into(),
                app.to_string().into(),
                environment.into(),
            ],
        ))
        .await
        .expect("read the queue")?;
    let spec: Value = row.try_get("", "spec").expect("spec");
    Some(
        SandboxMigrationsTask::from_spec(&serde_json::from_value(spec).expect("a spec"))
            .expect("a sandbox migrations task"),
    )
}

async fn publish_events(
    db: &DatabaseConnection,
    app: Uuid,
    environment: &str,
) -> Vec<Option<Uuid>> {
    app_environment_events::Entity::find()
        .filter(app_environment_events::Column::AppId.eq(app))
        .filter(app_environment_events::Column::Environment.eq(environment))
        .filter(app_environment_events::Column::Action.eq("publish"))
        .all(db)
        .await
        .expect("read the events")
        .into_iter()
        .map(|event| event.build_id)
        .collect()
}

/// Staging serves a build that declares a migration; a sandbox created from
/// staging serves that build, records the move as a publish does, and has the
/// build's Airhouse migration queued for its own sibling — staging's homes
/// are not queued again.
#[tokio::test]
async fn a_sandbox_created_from_staging_starts_on_stagings_build_and_queues_its_migrations() {
    let t = seeded_tenant().await;
    let slug = "sbx-from-stg";
    let app = app_with_two_sandboxes(&t, slug).await;
    publish(input(&t, slug, "stg-mig", with_migration(slug)))
        .await
        .expect("a draft to staging");
    let staged = build_pk(&t.db, app.id, "stg-mig").await.expect("the build");
    assert_eq!(serving(&t.db, app.id, "staging").await, Some(staged));
    let staging_tasks = queued(&t.db, app.id, "custom_app_staging_migrations").await;

    let created = ops::create_with(&t.db, &app, &sandbox("c3"), &t.guest(), from("staging"))
        .await
        .expect("create from staging");
    assert_eq!(created.build_id, Some(staged), "the row answers its build");
    assert_eq!(serving(&t.db, app.id, "dev-c3").await, Some(staged));
    assert_eq!(
        publish_events(&t.db, app.id, "dev-c3").await,
        vec![Some(staged)]
    );

    let task = queued_task(&t.db, app.id, "dev-c3")
        .await
        .expect("the build's migrations are queued for the sandbox");
    assert_eq!(task.build_pk, staged);
    let files: Vec<&str> = task
        .migrations
        .iter()
        .map(|m| m.filename.as_str())
        .collect();
    assert_eq!(files, vec!["0001_visits.sql"]);
    assert_eq!(
        queued(&t.db, app.id, "custom_app_staging_migrations").await,
        staging_tasks,
        "nothing is queued for staging's homes"
    );
    assert_eq!(
        serving(&t.db, app.id, "staging").await,
        Some(staged),
        "staging did not move"
    );
}

/// From a sandbox: the same, with the sandbox's own build.
#[tokio::test]
async fn a_sandbox_created_from_a_sandbox_starts_on_its_build() {
    let t = seeded_tenant().await;
    let slug = "sbx-from-sbx";
    let app = app_with_two_sandboxes(&t, slug).await;
    publish_to(
        input(&t, slug, "a1-mig", with_migration(slug)),
        PublishTarget::Sandbox(sandbox("a1")),
    )
    .await
    .expect("publish to dev-a1");
    let built = build_pk(&t.db, app.id, "a1-mig").await.expect("the build");
    let staging = serving(&t.db, app.id, "staging").await;
    assert_ne!(staging, Some(built), "staging serves another build");

    ops::create_with(&t.db, &app, &sandbox("c3"), &t.guest(), from("dev-a1"))
        .await
        .expect("create from dev-a1");
    assert_eq!(serving(&t.db, app.id, "dev-c3").await, Some(built));
    let task = queued_task(&t.db, app.id, "dev-c3")
        .await
        .expect("queued for dev-c3");
    assert_eq!(task.build_pk, built);
    assert_eq!(
        serving(&t.db, app.id, "dev-a1").await,
        Some(built),
        "the parent is unchanged"
    );
}

/// A parent that serves nothing gives a sandbox that serves nothing, with
/// nothing queued, and the create answers as it always did.
#[tokio::test]
async fn a_parent_with_no_build_gives_a_sandbox_with_none() {
    let t = seeded_tenant().await;
    let app = app_with_two_sandboxes(&t, "sbx-from-none").await;
    assert_eq!(serving(&t.db, app.id, "dev-a1").await, None);

    let created = ops::create_with(&t.db, &app, &sandbox("c3"), &t.guest(), from("dev-a1"))
        .await
        .expect("create from a sandbox with no build");
    assert_eq!(created.build_id, None);
    assert_eq!(serving(&t.db, app.id, "dev-c3").await, None);
    assert!(publish_events(&t.db, app.id, "dev-c3").await.is_empty());
    assert_eq!(queued(&t.db, app.id, SANDBOX_MIGRATIONS_KIND).await, 0);
}

/// A create with no `from` is what it was: no pointer, nothing queued.
#[tokio::test]
async fn a_create_with_no_from_copies_nothing() {
    let t = seeded_tenant().await;
    let slug = "sbx-no-from";
    let app = app_with_two_sandboxes(&t, slug).await;
    publish(input(&t, slug, "stg-mig", with_migration(slug)))
        .await
        .expect("a draft to staging");
    let created = ops::create_with(
        &t.db,
        &app,
        &sandbox("c3"),
        &t.guest(),
        NewBranch::default(),
    )
    .await
    .expect("create");
    assert_eq!(created.build_id, None);
    assert_eq!(queued(&t.db, app.id, SANDBOX_MIGRATIONS_KIND).await, 0);
}

/// A sandbox agent token forks only from an environment it may open, and its
/// reach is the `dev-*` sandboxes it created: staging and production are
/// answered exactly as a parent that does not exist, no row is created and
/// nothing is queued.
#[tokio::test]
async fn a_sandbox_agent_token_may_not_fork_from_staging_or_production() {
    let agent = agent().await;
    let uri = format!("/customer-apps/{}/environments", agent.app.id);
    for (parent, name) in [("staging", "dev-copy-s"), ("production", "dev-copy-p")] {
        let (status, refused) = agent
            .api("POST", &uri, Some(json!({ "name": name, "from": parent })))
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{parent}: {refused}");
        assert_eq!(refused["code"], "invalid_parent", "{parent}: {refused}");
        let row = app_environments::Entity::find_by_id((agent.app.id, name.to_string()))
            .one(&agent.t.db)
            .await
            .expect("read the sandbox");
        assert!(row.is_none(), "{parent}: no sandbox was created");
    }
    let (status, missing) = agent
        .api(
            "POST",
            &uri,
            Some(json!({ "name": "dev-copy-n", "from": "dev-nobody" })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{missing}");
    assert_eq!(missing["code"], "invalid_parent", "{missing}");
    assert_eq!(
        queued(&agent.t.db, agent.app.id, SANDBOX_MIGRATIONS_KIND).await,
        0,
        "nothing was queued"
    );
}

/// A token may fork from a sandbox it created: the new sandbox serves the
/// parent's build, and copying it is not publishing it, so the build's mark
/// stays the token that published it.
#[tokio::test]
async fn a_sandbox_agent_token_forks_from_its_own_sandbox() {
    let agent = agent().await;
    let (status, parent) = agent.create("dev-mine").await;
    assert_eq!(status, StatusCode::CREATED, "{parent}");
    let fields = [("build_id", "agent-mine"), ("environment", "dev-mine")];
    let (status, published) = agent.publish("mine", &fields).await;
    assert_eq!(status, StatusCode::OK, "{published}");

    let uri = format!("/customer-apps/{}/environments", agent.app.id);
    let (status, forked) = agent
        .api(
            "POST",
            &uri,
            Some(json!({ "name": "dev-fork", "from": "dev-mine" })),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{forked}");
    assert_eq!(forked["parent"], "dev-mine");
    assert_eq!(forked["build_id"], "agent-mine", "{forked}");

    let pk = build_pk(&agent.t.db, agent.app.id, "agent-mine")
        .await
        .expect("the build");
    assert_eq!(
        serving(&agent.t.db, agent.app.id, "dev-fork").await,
        Some(pk)
    );
    let row = app_builds::Entity::find_by_id(pk)
        .one(&agent.t.db)
        .await
        .expect("read the build")
        .expect("the build");
    assert_eq!(
        row.published_token_id,
        Some(agent.token_id),
        "copying a build is not publishing it"
    );
}
