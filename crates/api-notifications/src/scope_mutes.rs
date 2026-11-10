//! Mutes above a kind: a whole source (`scope = ""`) or a category subtree,
//! through the old `PUT /api/notifications/preferences` scope form.
//!
//! A mute is now a channel set off at a scope node of
//! `notification_node_settings` (`settings::store`), which delivery reads;
//! unmuting sets it back to inherit. [`check`] is the rule both this route
//! and `PATCH /api/notifications/settings` apply before writing one.

use std::collections::BTreeSet;

use axum::http::StatusCode;
use entity::{apps, notifications};
use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter, QuerySelect,
};
use uuid::Uuid;

use super::catalog::model::{is_within, valid_path};
use super::catalog::platform::oxygen;
use super::catalog::source::SourceCatalogue;
use super::delivery::Channel;
use super::settings::model::Node;

/// Whether muting `channel` at `scope` is a mute that can mean something.
///
/// `400` for a path that is malformed, or that this source does not have,
/// and for a subtree where no kind could ever be muted on that channel —
/// every one critical or never muted (a test notice), or none delivered on it
/// (an Oxygen kind's channels are fixed). A subtree with a critical kind
/// beside others is fine: that kind just ignores the mute. Unmuting is always allowed for a well-formed path,
/// so a mute the catalogue has since dropped can still be cleared.
pub fn check(
    catalogue: &SourceCatalogue,
    kinds: &BTreeSet<String>,
    scope: &str,
    channel: Channel,
    muted: bool,
) -> Result<(), StatusCode> {
    if !(scope.is_empty() || valid_path(scope)) {
        return Err(StatusCode::BAD_REQUEST);
    }
    if !muted {
        return Ok(());
    }
    if !(scope.is_empty() || catalogue.knows_path(scope)) {
        return Err(StatusCode::BAD_REQUEST);
    }
    let inside: Vec<_> = kinds
        .iter()
        .map(|k| catalogue.describe(k))
        .filter(|k| is_within(&k.category, scope))
        .collect();
    let applies = inside
        .iter()
        .any(|k| k.mutable && k.channels.contains(&channel.as_str()));
    match inside.is_empty() || applies {
        true => Ok(()),
        false => Err(StatusCode::BAD_REQUEST),
    }
}

/// `PUT /api/notifications/preferences` with `scope`: mute or unmute one
/// channel of a whole source or a category subtree. `app_id: None` is Oxygen.
pub async fn set(
    db: &DatabaseConnection,
    user: &oxy_auth::types::AuthenticatedUser,
    app_id: Option<Uuid>,
    scope: &str,
    channel: Channel,
    muted: bool,
) -> Result<StatusCode, StatusCode> {
    let internal = |_| StatusCode::INTERNAL_SERVER_ERROR;
    let catalogue = match app_id {
        None => oxygen(),
        Some(app_id) => {
            // 404 for an app this person cannot be told by, as for a kind mute.
            let app = apps::Entity::find_by_id(app_id)
                .one(db)
                .await
                .map_err(internal)?
                .filter(|a| user.reaches_org(a.org_id))
                .ok_or(StatusCode::NOT_FOUND)?;
            if !super::preferences::may_be_told_by(db, user.id, &app)
                .await
                .map_err(internal)?
            {
                return Err(StatusCode::NOT_FOUND);
            }
            super::catalog::live::load(db, &app)
                .await
                .map_err(internal)?
        }
    };
    let received = received_kinds(db, user.id, app_id)
        .await
        .map_err(internal)?;
    check(
        &catalogue,
        &catalogue.kinds_with(&received),
        scope,
        channel,
        muted,
    )?;
    let value = muted.then_some(false);
    let node = Node::Scope(scope.to_string());
    super::settings::store::set_channel(db, user.id, app_id, &node, channel, value)
        .await
        .map_err(internal)?;
    Ok(StatusCode::NO_CONTENT)
}

/// The kinds `user_id` has received from `app` (`None`: Oxygen).
pub(crate) async fn received_kinds(
    db: &impl ConnectionTrait,
    user_id: Uuid,
    app: Option<Uuid>,
) -> Result<Vec<String>, DbErr> {
    let find = notifications::Entity::find()
        .select_only()
        .column(notifications::Column::Kind)
        .filter(notifications::Column::UserId.eq(user_id))
        .distinct();
    let find = match app {
        Some(app) => find.filter(notifications::Column::AppId.eq(app)),
        None => find.filter(notifications::Column::AppId.is_null()),
    };
    find.into_tuple::<String>().all(db).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::live::from_manifest;
    use serde_json::json;

    fn store_ops() -> SourceCatalogue {
        from_manifest(Some(&json!({
            "functions": { "f": { "notify": { "kinds": {
                "list_overdue": {}, "temp_alarm": { "critical": true } } } } },
            "notifications": {
                "categories": { "operations/food-safety": {}, "safety-only/alarms": {} },
                "kinds": {
                    "list_overdue": { "category": "operations" },
                    "temp_alarm": { "category": "safety-only/alarms" }
                }
            }
        })))
    }

    #[test]
    fn a_scope_mute_needs_a_known_path_and_a_kind_it_can_reach() {
        let c = store_ops();
        let kinds = c.kinds_with(std::iter::empty());
        let push = Channel::Push;
        assert_eq!(check(&c, &kinds, "", push, true), Ok(()));
        assert_eq!(check(&c, &kinds, "operations", push, true), Ok(()));
        assert_eq!(
            check(&c, &kinds, "operations/food-safety", push, true),
            Ok(()),
            "an empty declared category can be muted ahead of its kinds"
        );
        assert_eq!(
            check(&c, &kinds, "finance", push, true),
            Err(StatusCode::BAD_REQUEST),
            "unknown path"
        );
        assert_eq!(
            check(&c, &kinds, "safety-only", push, true),
            Err(StatusCode::BAD_REQUEST),
            "every kind below is critical: the mute could never apply"
        );
        assert_eq!(
            check(&c, &kinds, "Bad Path", push, false),
            Err(StatusCode::BAD_REQUEST)
        );
        assert_eq!(
            check(&c, &kinds, "finance", push, false),
            Ok(()),
            "a stale mute can always be cleared"
        );
    }

    #[test]
    fn oxygen_refuses_a_channel_its_subtree_never_uses_and_a_billing_mute() {
        use crate::platform::kinds::{KINDS, channels, is_critical};
        let c = oxygen();
        let kinds = c.kinds_with(std::iter::empty());
        // The whole source takes a channel some non-critical kind is
        // delivered on (`kinds::channels`), and refuses any other.
        for channel in Channel::ALL {
            let used = KINDS
                .iter()
                .any(|k| !is_critical(k) && channels(k).contains(&channel.as_str()));
            let expected = if used {
                Ok(())
            } else {
                Err(StatusCode::BAD_REQUEST)
            };
            assert_eq!(
                check(&c, &kinds, "", channel, true),
                expected,
                "{channel:?}"
            );
        }
        assert_eq!(check(&c, &kinds, "", Channel::Push, true), Ok(()));
        assert_eq!(
            check(&c, &kinds, "workspaces", Channel::Email, true),
            Err(StatusCode::BAD_REQUEST),
            "a ready workspace is never emailed"
        );
        assert_eq!(
            check(&c, &kinds, "billing", Channel::Push, true),
            Err(StatusCode::BAD_REQUEST),
            "billing is critical-only"
        );
        assert_eq!(check(&c, &kinds, "runs", Channel::Email, true), Ok(()));
    }
}
