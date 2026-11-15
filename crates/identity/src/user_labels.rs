//! A user's display label (`users::Model::label`, the email where there is
//! one) for a set of ids, in one query: the "created by" and "updated by"
//! columns a listing shows beside its rows.
//!
//! Lowered out of `oxy-app`'s `server/api/secrets.rs` on 2026-11-15, when that
//! handler module left for `oxy-api-workspace-config`: the custom-app secrets
//! listing (`oxy-api-custom-apps`) shares it, and two surfaces may not reach
//! each other (S3). A read of the User aggregate, so Identity's.

use std::collections::HashMap;

use entity::users::Entity as Users;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use uuid::Uuid;

/// Resolve a set of user UUIDs to their emails in one query.
pub async fn resolve_user_emails(
    db: &impl sea_orm::ConnectionTrait,
    ids: &[Uuid],
) -> HashMap<Uuid, String> {
    if ids.is_empty() {
        return HashMap::new();
    }
    match Users::find()
        .filter(entity::users::Column::Id.is_in(ids.to_vec()))
        .all(db)
        .await
    {
        Ok(users) => users
            .into_iter()
            .map(|u| (u.id, u.label().to_string()))
            .collect(),
        Err(e) => {
            tracing::warn!("Failed to resolve user emails: {}", e);
            HashMap::new()
        }
    }
}
