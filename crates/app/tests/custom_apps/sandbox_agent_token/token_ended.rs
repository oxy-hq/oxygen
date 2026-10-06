//! The maintenance sweep's token-ended pass (sandbox agent credential design
//! §2 and §7.2 "Maintenance sweep"): a sandbox whose creating token was
//! revoked or expired more than a day ago is torn down as `token_ended`, and
//! one whose token row is gone falls back to idle expiry.
//!
//! Each test has its own database, so a sweep at a later `now` is the passing
//! of time and touches nothing of another test's.

use axum::http::StatusCode;
use chrono::{DateTime, Duration, Utc};
use entity::{api_tokens, audit_events};
use oxy_app::server::api::custom_apps_sandboxes::maintenance::sweep;
use oxy_app::server::api::custom_apps_sandboxes::{idle_ttl, ops, token_ended};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde_json::{Value, json};
use uuid::Uuid;

use super::fixture::{Agent, agent, send};
use crate::sandbox_sweep::{sandboxes, teardowns};
use crate::sandbox_teardown::sandbox;

async fn token_row(agent: &Agent, id: Uuid) -> api_tokens::Model {
    api_tokens::Entity::find_by_id(id)
        .one(&agent.t.db)
        .await
        .expect("read the token")
        .expect("the token")
}

/// When the token `id` ended, by the rule the pass states in Rust.
async fn ended_at(agent: &Agent, id: Uuid) -> DateTime<Utc> {
    let row = token_row(agent, id).await;
    token_ended::ended_at(
        row.expires_at.map(DateTime::<Utc>::from),
        row.revoked_at.map(DateTime::<Utc>::from),
    )
    .expect("the token ends")
}

async fn create_all(agent: &Agent, names: &[&str]) {
    for name in names {
        let (status, body) = agent.create(name).await;
        assert_eq!(status, StatusCode::CREATED, "create {name}: {body}");
    }
}

async fn audit_rows(agent: &Agent, action: &str) -> Vec<audit_events::Model> {
    audit_events::Entity::find()
        .filter(audit_events::Column::Action.eq(action))
        .all(&agent.t.db)
        .await
        .expect("read the audit rows")
}

/// The app's sandboxes that are being deleted, by name.
async fn deleting(agent: &Agent) -> Vec<String> {
    sandboxes(&agent.t.db, agent.app.id)
        .await
        .into_iter()
        .filter_map(|(name, deleting)| deleting.then_some(name))
        .collect()
}

/// A revoked token's sandboxes stay for a day and then go, both in one pass:
/// marked, a teardown each with the reason `token_ended`, and one
/// `token.expired_sandboxes_queued` event on the app's org that names both. A
/// colleague's sandbox of the same app is not the token's and stays. What the
/// pass selects is exactly what the rule in Rust says is due.
#[tokio::test]
async fn a_revoked_tokens_sandboxes_are_torn_down_a_day_later_and_recorded_once() {
    let agent = agent().await;
    create_all(&agent, &["dev-a", "dev-b"]).await;
    ops::create(&agent.t.db, &agent.app, &sandbox("p"), &agent.t.guest())
        .await
        .expect("a colleague's sandbox");
    let (status, body) = agent.api("DELETE", "/auth/token", None).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    let ended = ended_at(&agent, agent.token_id).await;
    let row = token_row(&agent, agent.token_id).await;
    assert_eq!(
        Some(ended),
        row.revoked_at.map(DateTime::<Utc>::from),
        "the revocation ended it, hours before its expiry"
    );

    let early = ended + Duration::hours(23);
    assert!(!token_ended::is_due(Some(ended), early));
    assert_eq!(sweep(&agent.t.db, early).await.expect("sweep"), 0);
    assert!(deleting(&agent).await.is_empty(), "a day is not up yet");

    let due = ended + token_ended::grace() + Duration::seconds(1);
    assert!(token_ended::is_due(Some(ended), due));
    assert_eq!(sweep(&agent.t.db, due).await.expect("sweep"), 2);
    assert_eq!(deleting(&agent).await, vec!["dev-a", "dev-b"]);
    let mut queued = teardowns(&agent.t.db, agent.app.id).await;
    queued.sort();
    let reason = "token_ended".to_string();
    assert_eq!(
        queued,
        vec![("dev-a".into(), reason.clone()), ("dev-b".into(), reason)]
    );

    let events = audit_rows(&agent, "token.expired_sandboxes_queued").await;
    assert_eq!(events.len(), 1, "once per token: {events:?}");
    let event = &events[0];
    assert_eq!(event.org_id, Some(agent.t.org_id), "the granted app's org");
    assert_eq!(event.actor_type, "system");
    assert_eq!(event.target_id, Some(agent.token_id.to_string()));
    let metadata = &event.metadata;
    assert_eq!(metadata["token_kind"], "sandbox_agent", "{metadata}");
    assert_eq!(metadata["reason"], "token_ended", "{metadata}");
    assert_eq!(metadata["ended_by"], "revoked", "{metadata}");
    let named: Vec<(&Value, &Value)> = metadata["sandboxes"]
        .as_array()
        .expect("sandboxes")
        .iter()
        .map(|s| (&s["app_id"], &s["environment"]))
        .collect();
    let app = json!(agent.app.id);
    assert_eq!(
        named,
        vec![(&app, &json!("dev-a")), (&app, &json!("dev-b"))]
    );
    let deleted: Vec<_> = audit_rows(&agent, "app.environment.deleted")
        .await
        .into_iter()
        .filter(|row| row.reason.as_deref() == Some("token_ended"))
        .collect();
    assert_eq!(deleted.len(), 2, "each sandbox's own row: {deleted:?}");
    assert!(deleted.iter().all(|row| row.actor_type == "system"));

    // A second pass, and another replica's, finds nothing left to do.
    assert_eq!(sweep(&agent.t.db, due).await.expect("sweep again"), 0);
    let events = audit_rows(&agent, "token.expired_sandboxes_queued").await;
    assert_eq!(events.len(), 1, "still one event");
}

/// A token nobody revoked ends at its expiry, and its sandbox goes a day
/// after that. A second token of the same minter, still alive at that moment,
/// keeps its sandbox.
#[tokio::test]
async fn an_expired_tokens_sandbox_is_torn_down_a_day_after_the_expiry() {
    let agent = agent().await;
    create_all(&agent, &["dev-a"]).await;
    let body = json!({
        "name": "longer", "kind": "sandbox_agent", "apps": [agent.app.id], "expires_in_hours": 72,
    });
    let headers = [("cookie", agent.cookie.as_str())];
    let (status, minted) = send("POST", "/user/tokens", &headers, Some(body)).await;
    assert_eq!(status, StatusCode::CREATED, "{minted}");
    let longer = Uuid::parse_str(minted["token"]["id"].as_str().expect("an id")).expect("a uuid");
    let bearer = format!("Bearer {}", minted["secret"].as_str().expect("the secret"));
    let uri = format!("/customer-apps/{}/environments", agent.app.id);
    let headers = [("authorization", bearer.as_str())];
    let (status, body) = send("POST", &uri, &headers, Some(json!({ "name": "dev-l" }))).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    let ended = ended_at(&agent, agent.token_id).await;
    let row = token_row(&agent, agent.token_id).await;
    assert_eq!(row.revoked_at, None);
    assert_eq!(Some(ended), row.expires_at.map(DateTime::<Utc>::from));

    let at_expiry = ended + Duration::minutes(1);
    assert_eq!(sweep(&agent.t.db, at_expiry).await.expect("sweep"), 0);
    let due = ended + token_ended::grace() + Duration::seconds(1);
    let still_alive = ended_at(&agent, longer).await;
    assert!(!token_ended::is_due(Some(still_alive), due));
    assert_eq!(sweep(&agent.t.db, due).await.expect("sweep"), 1);
    assert_eq!(deleting(&agent).await, vec!["dev-a"], "not dev-l");
    let events = audit_rows(&agent, "token.expired_sandboxes_queued").await;
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].target_id, Some(agent.token_id.to_string()));
    assert_eq!(events[0].metadata["ended_by"], "expired");
}

/// A sandbox whose token row no longer exists has nothing to say when its
/// token ended. It is not this pass's: it goes when it has sat idle for the
/// idle TTL, with the reason `expired`, like a sandbox a person created.
#[tokio::test]
async fn a_sandbox_whose_token_row_is_gone_falls_back_to_idle_expiry() {
    let agent = agent().await;
    create_all(&agent, &["dev-a"]).await;
    api_tokens::Entity::delete_by_id(agent.token_id)
        .exec(&agent.t.db)
        .await
        .expect("delete the token row");

    let now = Utc::now();
    let long_after = now + token_ended::grace() * 3;
    assert!(long_after < now + idle_ttl());
    assert_eq!(sweep(&agent.t.db, long_after).await.expect("sweep"), 0);
    assert!(deleting(&agent).await.is_empty(), "no token row, no end");

    let idle = now + idle_ttl() + Duration::hours(1);
    assert_eq!(sweep(&agent.t.db, idle).await.expect("sweep"), 1);
    assert_eq!(
        teardowns(&agent.t.db, agent.app.id).await,
        vec![("dev-a".to_string(), "expired".to_string())]
    );
    let events = audit_rows(&agent, "token.expired_sandboxes_queued").await;
    assert!(events.is_empty(), "no token to record it for: {events:?}");
}
