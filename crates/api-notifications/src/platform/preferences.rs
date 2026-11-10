//! Mutes for platform notices through the old preferences route: one switch
//! per (kind, channel).
//!
//! A mute is a channel set off at the kind's node of
//! `notification_node_settings` with `app_id = NULL` (`settings::store`); an
//! unmute sets it back to inherit. The list is the kinds a person has
//! received, plus any they set. A critical kind cannot be muted at all: a
//! platform kind is critical in code, not in a manifest that might change, so
//! a mute of one could never apply. Nor can a test notice's
//! (`kinds::is_mutable`), which is left out of the list: a switch that does
//! nothing is noise.

use std::collections::BTreeMap;

use axum::http::StatusCode;
use entity::notifications;
use sea_orm::{ColumnTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter, QuerySelect};
use uuid::Uuid;

use super::kinds::{TEST, channels, is_critical, is_mutable, is_platform_kind};
use crate::delivery::Channel;
use crate::preferences::KindPreference;
use crate::settings::model::Node;
use crate::settings::store;

/// The platform kinds `user_id` has received or set, with their mutes.
pub async fn kinds(db: &DatabaseConnection, user_id: Uuid) -> Result<Vec<KindPreference>, DbErr> {
    let received: Vec<String> = notifications::Entity::find()
        .select_only()
        .column(notifications::Column::Kind)
        .filter(notifications::Column::UserId.eq(user_id))
        .filter(notifications::Column::AppId.is_null())
        .distinct()
        .into_tuple::<String>()
        .all(db)
        .await?;
    let mut by_kind: BTreeMap<String, Vec<&'static str>> = received
        .into_iter()
        .filter(|k| is_platform_kind(k))
        .map(|k| (k, Vec::new()))
        .collect();
    let set = store::nodes_for_user(db, user_id).await?;
    for (node, settings) in set.get(&None).into_iter().flatten() {
        if let Node::Kind(kind) = node {
            by_kind.insert(kind.clone(), settings.off());
        }
    }
    Ok(by_kind
        .into_iter()
        // No switch for a test notice: nothing would honour it.
        .filter(|(kind, _)| kind != TEST)
        .map(|(kind, muted)| KindPreference {
            critical: is_critical(&kind),
            channels: channels(&kind).to_vec(),
            kind,
            muted,
        })
        .collect())
}

/// Whether a platform `kind` may be set on `channel`: `false` for a kind the
/// platform does not send, a channel it is never delivered on, or muting one
/// no mute applies to (critical, or a test notice).
pub fn may_set(kind: &str, channel: Channel, muted: bool) -> bool {
    is_platform_kind(kind)
        && (!muted || is_mutable(kind))
        && channels(kind).contains(&channel.as_str())
}

/// Mute or unmute `channel` for platform `kind`. `400` where [`may_set`]
/// says no.
pub async fn set(
    db: &DatabaseConnection,
    user_id: Uuid,
    kind: &str,
    channel: Channel,
    muted: bool,
) -> Result<StatusCode, StatusCode> {
    if !may_set(kind, channel, muted) {
        return Err(StatusCode::BAD_REQUEST);
    }
    let node = Node::Kind(kind.to_string());
    store::set_channel(db, user_id, None, &node, channel, muted.then_some(false))
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(StatusCode::NO_CONTENT)
}
