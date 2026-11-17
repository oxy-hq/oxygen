//! The audit row of a data read: who, which app, which environment, which
//! table and page — and a read whose row could not be written is withheld.

use axum::http::StatusCode;
use oxy_app_core::audit::ActorType;
use serde_json::json;

use super::super::page::Dir;
use super::*;
use crate::custom_apps_agent_fixture as fixture;

fn app() -> apps::Model {
    fixture::app(
        Uuid::from_u128(0xA1),
        Uuid::from_u128(0x0A),
        Uuid::from_u128(0x3A),
    )
}

#[test]
fn a_rows_read_names_who_the_app_the_environment_and_the_table() {
    let actor = fixture::actor(None);
    let order = [OrderTerm {
        column: "created_at".into(),
        dir: Dir::Desc,
    }];
    let read = Read::Rows {
        table: "orders",
        limit: 50,
        offset: 100,
        order: &order,
    };
    let staging = AppEnvironment::Staging;
    let entry = entry(&actor, &app(), &staging, "app_ops", read);

    assert_eq!(entry.action, DATA_READ);
    assert_eq!(entry.actor_user_id, Some(actor.user.id));
    assert_eq!(entry.actor_email, "minter@oxy.tech");
    assert_eq!(entry.actor_type, ActorType::User);
    assert_eq!(entry.org_id, Some(app().org_id));
    assert_eq!(entry.workspace_id, Some(app().project_id));
    assert_eq!(entry.target_type.as_deref(), Some("custom_app_environment"));
    assert_eq!(
        entry.target_id,
        Some(format!("{}/staging", app().id)),
        "the app and the environment"
    );
    assert_eq!(entry.target_label.as_deref(), Some("ops/staging"));
    assert_eq!(entry.environment.as_deref(), Some("staging"));
    assert_eq!(
        entry.metadata,
        json!({
            "read": "rows",
            "environment": "staging",
            "schema": "app_ops",
            "table": "orders",
            "limit": 50,
            "offset": 100,
            "order": [{ "column": "created_at", "dir": "desc" }],
        })
    );
}

#[test]
fn a_tables_read_in_production_is_recorded_too() {
    let entry = entry(
        &fixture::actor(None),
        &app(),
        &AppEnvironment::Production,
        "app_ops",
        Read::Tables,
    );
    assert_eq!(entry.action, DATA_READ);
    // Production is the column's default, stored as `None`.
    assert_eq!(entry.environment, None);
    assert_eq!(entry.metadata["read"], "tables");
    assert_eq!(entry.metadata["environment"], "production");
    assert_eq!(entry.metadata["schema"], "app_ops");
}

/// A read made with a token is the token's on the record, not a hand-made
/// session read: `for_request` stamps it.
#[test]
fn a_token_read_is_stamped_with_its_token() {
    let app = app();
    let token = Uuid::from_u128(0x70);
    let credential = fixture::credential(token, app.org_id, app.id, app.project_id);
    let actor = fixture::actor(Some(credential));
    let entry = entry(
        &actor,
        &app,
        &AppEnvironment::Production,
        "app_ops",
        Read::Tables,
    );
    assert_eq!(entry.actor_type, ActorType::ApiKey);
    let metadata = entry.effective_metadata();
    assert_eq!(metadata["token_id"], token.to_string(), "{metadata}");
}

#[test]
fn a_read_whose_row_was_written_is_returned() {
    let read = withhold_unless_recorded("rows", Ok(Uuid::new_v4()));
    assert_eq!(read.ok(), Some("rows"));
}

/// No row, no result: the read happened, but nothing of it leaves.
#[test]
fn a_read_whose_row_was_not_written_is_withheld() {
    let refused =
        withhold_unless_recorded("rows", Err(DbErr::Custom("audit_events is gone".into())))
            .expect_err("withheld");
    assert_eq!(refused.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(refused.code, "audit_unavailable");
}
