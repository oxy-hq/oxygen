//! A sandbox agent token reads the outbox of the instance it created and no
//! earlier one (`custom_apps_sandbox_instance`): a message captured before the
//! sandbox existed — a deleted sandbox's name taken again, a grant made later
//! — is not returned, and the page is filled from the messages that are. A
//! person (no `since`) reads everything. Also: a token reads only a sandbox it
//! created.

use axum::http::StatusCode;
use chrono::{Duration, Utc};
use entity::app_environments;
use oxy_api_custom_apps::custom_apps_outbox::{self as outbox, Message};
use oxy_api_custom_apps::custom_apps_storage::RetentionPolicy;
use oxy_app_core::custom_app_environment::AppEnvironment;
use sea_orm::EntityTrait;
use serde_json::{Map, Value, json};
use uuid::Uuid;

use crate::sandbox_agent_token::fixture::{agent, send};

const SANDBOX: &str = "dev-a";

fn message(subject: &str) -> Message {
    Message {
        kind: "email",
        to: json!(["cfo@customer.example"]),
        subject: Some(subject.to_string()),
        title: None,
        body: Some("posted".to_string()),
        invocation_id: Uuid::nil(),
        function: "send".to_string(),
        captured_at: String::new(),
        details: Map::new(),
    }
}

async fn keep(app: Uuid, subject: &str, at: chrono::DateTime<Utc>) {
    let environment = AppEnvironment::parse(SANDBOX).expect("a sandbox name");
    let (pathname, bytes) = message(subject)
        .encode(at, Uuid::new_v4())
        .expect("encodes");
    outbox::write(
        &outbox::silo(app, &environment),
        &pathname,
        bytes,
        &RetentionPolicy::default(),
    )
    .await
    .expect("written");
}

fn subjects(page: &Value) -> Vec<&str> {
    page["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .filter_map(|m| m["subject"].as_str())
        .collect()
}

#[tokio::test]
async fn a_token_reads_only_what_its_sandbox_instance_captured() {
    let agent = agent().await;
    let (status, created) = agent.create(SANDBOX).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let row = app_environments::Entity::find_by_id((agent.app.id, SANDBOX.to_string()))
        .one(&agent.t.db)
        .await
        .expect("read the sandbox")
        .expect("the sandbox");
    let born = row.created_at.with_timezone(&Utc);
    for (subject, before) in [("before-1", 3), ("before-2", 2), ("before-3", 1)] {
        keep(agent.app.id, subject, born - Duration::hours(before)).await;
    }
    keep(agent.app.id, "after-1", Utc::now()).await;
    keep(agent.app.id, "after-2", Utc::now() + Duration::seconds(1)).await;

    let uri = format!(
        "/customer-apps/{}/environments/{SANDBOX}/outbox",
        agent.app.id
    );
    let (status, page) = agent.get(&format!("{uri}?limit=2")).await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert_eq!(
        subjects(&page),
        ["after-2", "after-1"],
        "the page is full of this instance's messages"
    );
    assert_eq!(page["has_more"], false, "earlier instances' are not 'more'");
    let (_, all) = agent.get(&uri).await;
    assert_eq!(subjects(&all).len(), 2, "{all}");

    let (status, staff) = send("GET", &uri, &[("cookie", &agent.cookie)], None).await;
    assert_eq!(status, StatusCode::OK, "{staff}");
    assert_eq!(subjects(&staff).len(), 5, "a person reads the whole outbox");
}

/// The door is the tail's: a sandbox the token did not create, staging it was
/// not granted, and production are all `404 environment_not_found`.
#[tokio::test]
async fn a_token_reads_no_outbox_but_its_own_sandbox() {
    let agent = agent().await;
    crate::app_environments::seed_sandbox(&agent.t.db, agent.app.id, "dev-b", agent.t.guest_id)
        .await;
    for name in ["dev-b", "staging", "production"] {
        let (status, refused) = agent
            .get(&format!(
                "/customer-apps/{}/environments/{name}/outbox",
                agent.app.id
            ))
            .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{name}: {refused}");
        assert_eq!(refused["error"], "environment_not_found", "{name}");
    }
}
