//! The audit row of a bundle publish — `app.environment.published` — for
//! **every** credential, and what such a row says of an app publish token.
//!
//! `POST /api/customer-apps/publish` moves an environment's pointer: staging's,
//! production's with `promote`, or a sandbox's. The build row has always said
//! who (`app_builds.published_by`, a user id), but nothing said *with what*,
//! and nothing was in the audit log at all unless a sandbox agent token made
//! the publish. Now every publish that succeeds leaves one row, built with
//! [`AuditEntry::for_request`]: the environment as the target, the build id in
//! the metadata, and the key or token that authenticated the request stamped
//! by id, name and kind. A refused publish moved nothing and leaves none.
//!
//! Two shapes of credential reach the route:
//!
//! * **A session, a legacy API key or an API token.** `for_request` names it,
//!   so the row is the one the sandbox agent token's publish already wrote
//!   (`custom_apps_sandboxes::agent_publish::audit`), unchanged and shared.
//! * **An app publish token** (`oxypublish_`). It is not an API token: the
//!   request carries an [`AppPublishTokenAuth`] marker and no
//!   `CredentialContext`, so `for_request` alone would record its minter as a
//!   plain user — or, for an OIDC-minted one, the machine principal's nil id.
//!   [`by_publish_token`] says a key acted and names the token under the same
//!   four metadata keys, with `token_kind = app_publish_token`, so one filter
//!   (`metadata.token_id`) finds what either kind of credential did.
//!
//! `admin::apps::run_audit` writes the sibling row for "run now" the same way.

use entity::prelude::AppPublishTokens;
use entity::{app_publish_tokens, apps};
use oxy_app_core::audit::{self, ActorType, AuditEntry, RequestActor, TOKEN_ID_KEY};
use oxy_app_core::custom_app_environment::AppEnvironment;
use oxy_auth::types::AppPublishTokenAuth;
use sea_orm::{DatabaseConnection, EntityTrait};
use serde_json::{Map, Value, json};

use super::custom_apps_publish::PublishResult;
use super::custom_apps_sandboxes::agent_publish;

pub(crate) const PUBLISHED: &str = "app.environment.published";
/// `metadata.token_kind` of a row an `oxypublish_` token wrote. Not one of
/// `api_tokens.kind`'s values: the id beside it is an `app_publish_tokens` id.
pub(crate) const PUBLISH_TOKEN_KIND: &str = "app_publish_token";

/// One audit row for a publish that succeeded, whoever made it. Best effort,
/// as every row written after the action it records: the publish stands.
pub(crate) async fn published(
    db: &DatabaseConnection,
    actor: &RequestActor,
    marker: Option<&AppPublishTokenAuth>,
    result: &PublishResult,
) {
    let Some(marker) = marker else {
        return agent_publish::audit(db, actor, result).await;
    };
    let app = match apps::Entity::find_by_id(result.app_id).one(db).await {
        Ok(Some(app)) => app,
        Ok(None) => return,
        Err(e) => {
            tracing::warn!(app_id = %result.app_id, error = %e, "publish: audit skipped");
            return;
        }
    };
    let Some(environment) = AppEnvironment::parse(&result.environment) else {
        return;
    };
    let entry = AuditEntry::for_request(actor, PUBLISHED)
        .org(app.org_id)
        .workspace(app.project_id)
        .target(
            "custom_app_environment",
            format!("{}/{environment}", app.id),
            format!("{}/{environment}", app.slug),
        )
        .environment(environment.name());
    let detail = json!({ "build_id": result.build_id });
    audit::record_best_effort(db, by_publish_token(db, entry, marker, detail).await).await;
}

/// `entry`, as the `oxypublish_` token behind `marker` wrote it, with `detail`
/// as the rest of its metadata. One primary-key read for the token's name and
/// display prefix; a row that cannot be read still names the token by id.
pub(crate) async fn by_publish_token(
    db: &DatabaseConnection,
    entry: AuditEntry,
    marker: &AppPublishTokenAuth,
    detail: Value,
) -> AuditEntry {
    let token = AppPublishTokens::find_by_id(marker.token_id)
        .one(db)
        .await
        .unwrap_or_else(|e| {
            tracing::warn!(token_id = %marker.token_id, error = %e, "publish token: name not read");
            None
        });
    stamped(entry, marker, token.as_ref(), detail)
}

/// The stamp itself: a key acted, and which. Never the token or its hash.
fn stamped(
    mut entry: AuditEntry,
    marker: &AppPublishTokenAuth,
    token: Option<&app_publish_tokens::Model>,
    detail: Value,
) -> AuditEntry {
    entry.actor_type = ActorType::ApiKey;
    // An OIDC-minted token's principal has no `users` row; its nil id is never
    // recorded. The workflow identity the exchange verified is the token's name.
    if entry.actor_user_id.is_some_and(|id| id.is_nil()) {
        entry.actor_user_id = None;
    }
    let mut metadata = match detail {
        Value::Object(map) => map,
        _ => Map::new(),
    };
    let name = token
        .map(|t| t.name.clone())
        .or_else(|| marker.machine_identity.clone());
    metadata.insert(TOKEN_ID_KEY.into(), json!(marker.token_id));
    metadata.insert("token_kind".into(), json!(PUBLISH_TOKEN_KIND));
    if let Some(name) = name {
        metadata.insert("token_name".into(), json!(name));
    }
    if let Some(token) = token {
        metadata.insert("display_prefix".into(), json!(token.token_prefix));
    }
    entry.metadata(Value::Object(metadata))
}

#[cfg(test)]
#[path = "custom_apps_publish_audit_tests.rs"]
mod tests;
