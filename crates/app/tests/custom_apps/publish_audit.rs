//! Every bundle publish is audited, to every environment and for every
//! credential (`custom_apps_publish_audit`), through the real handler.
//!
//! Before, `POST /api/customer-apps/publish` left an `audit_events` row only
//! when a sandbox agent token made it (`sandbox_agent_token/the_loop.rs`
//! still asserts that one). A staging or production publish, and a sandbox
//! publish by anyone else, said who only on the build row — a user id, with
//! nothing about the key that carried it.

use axum::http::StatusCode;
use entity::audit_events;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder};
use uuid::Uuid;

use crate::custom_app_functions_fixture::{Tenant, seeded_tenant};
use crate::sandbox_publish::app_with_two_sandboxes;
use crate::sandbox_publish_route::{json_of, publish};

const PUBLISHED: &str = "app.environment.published";

async fn rows(t: &Tenant) -> Vec<audit_events::Model> {
    audit_events::Entity::find()
        .filter(audit_events::Column::Action.eq(PUBLISHED))
        .order_by_asc(audit_events::Column::Seq)
        .all(&t.db)
        .await
        .expect("read the audit rows")
}

#[tokio::test]
async fn every_publish_leaves_one_audit_row_naming_its_environment_and_its_credential() {
    let t = seeded_tenant().await;
    let slug = "audit-publish";
    let app = app_with_two_sandboxes(&t, slug).await;
    let before = rows(&t).await.len();

    // On the guest's own login: a sandbox, staging, then production.
    // With a publish token: staging, the one place it may publish.
    let publishes: [(&[(&str, &str)], bool, &str); 4] = [
        (
            &[("build_id", "to-a1"), ("environment", "dev-a1")],
            false,
            "dev-a1",
        ),
        (&[("build_id", "draft-1")], false, "staging"),
        (
            &[("build_id", "live-1"), ("promote", "true")],
            false,
            "production",
        ),
        (&[("build_id", "by-token")], true, "staging"),
    ];
    for (fields, token, environment) in publishes {
        let (status, body) = publish(slug, fields, token).await;
        assert_eq!(status, StatusCode::OK, "{fields:?}: {body}");
        assert_eq!(json_of(&body)["environment"], environment, "{body}");
    }

    let written = rows(&t).await;
    let written = &written[before..];
    assert_eq!(written.len(), 4, "one row per publish");
    for (row, (fields, _, environment)) in written.iter().zip(publishes) {
        assert_eq!(row.environment, environment);
        assert_eq!(row.org_id, Some(t.org_id));
        assert_eq!(row.actor_user_id, Some(t.guest_id));
        assert_eq!(row.target_type.as_deref(), Some("custom_app_environment"));
        assert_eq!(row.target_id, Some(format!("{}/{environment}", app.id)));
        assert_eq!(row.target_label, Some(format!("{slug}/{environment}")));
        assert_eq!(row.metadata["build_id"], fields[0].1);
    }

    // A login is a user, and names no key.
    for row in &written[..3] {
        assert_eq!(row.actor_type, "user");
        assert!(row.metadata.get("token_id").is_none());
    }
    // A publish token is a key, and the row says which.
    let by_token = &written[3];
    assert_eq!(by_token.actor_type, "api_key");
    assert_eq!(by_token.metadata["token_kind"], "app_publish_token");
    let named = by_token.metadata["token_id"].as_str().expect("a token id");
    assert!(Uuid::parse_str(named).is_ok(), "{named}");
}

#[tokio::test]
async fn a_refused_publish_leaves_no_audit_row() {
    let t = seeded_tenant().await;
    let slug = "audit-refused";
    app_with_two_sandboxes(&t, slug).await;
    let before = rows(&t).await.len();

    // A sandbox that does not exist, and a publish token naming a sandbox.
    let (status, body) = publish(slug, &[("environment", "dev-nobody")], false).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    let (status, body) = publish(slug, &[("environment", "dev-a1")], true).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");

    assert_eq!(rows(&t).await.len(), before);
}
