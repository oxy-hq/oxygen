//! Find an Oxygen member in their org's Slack by email, so a notification can
//! DM someone who has never talked to the bot.
//!
//! The reverse of `oxy-app`'s `slack::linking::auto_match`: that links a Slack user who messaged
//! the bot, by the email Slack holds for them; this links an Oxygen member the
//! platform wants to reach, by the email Oxygen holds for them. The trust is
//! the same one, an address both systems verified, inside the org that
//! installed Slack. The caller passes only members with an active account.
//!
//! A match is saved as a `slack_user_links` row (`email_lookup`), so each
//! person is looked up once. A miss is remembered for an hour per pod, so a
//! daily notification to someone not in Slack costs one call a day, not one per
//! send, and someone who joins Slack later is found within the hour.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use entity::slack_installations::Model as InstallationRow;
use entity::slack_user_links;
use oxy_shared::errors::OxyError;
use oxy_slack_client::{EmailLookup, SlackClient};
use sea_orm::sea_query::OnConflict;
use sea_orm::{ActiveValue, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use uuid::Uuid;

use crate::user_links::LinkMethod;

/// The scope `users.lookupByEmail` needs. An installation granted before it
/// was requested has none, and every lookup would be refused.
const LOOKUP_SCOPE: &str = "users:read.email";

const MISS_TTL: Duration = Duration::from_secs(3600);
static MISSES: Mutex<Option<HashMap<(Uuid, Uuid), Instant>>> = Mutex::new(None);

/// Who the person is in the org's Slack.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlackIdentity {
    /// Their Slack user id, now linked.
    Linked(String),
    /// No usable Slack account has their address, or it belongs to someone
    /// already linked to another Oxygen account.
    NotFound,
}

/// Look `user` up in `installation`'s Slack by `email` and link them. `Err`
/// only for a fault a retry can fix (a rate limit, Slack or the database
/// down); every answer Slack gives is an [`SlackIdentity`].
pub async fn link_by_email(
    db: &DatabaseConnection,
    client: &SlackClient,
    installation: &InstallationRow,
    bot_token: &str,
    user: Uuid,
    email: &str,
) -> Result<SlackIdentity, OxyError> {
    if !has_lookup_scope(&installation.bot_scopes) || recently_missed(installation.id, user) {
        return Ok(SlackIdentity::NotFound);
    }
    let found = match client.users_lookup_by_email(bot_token, email).await? {
        EmailLookup::Found(u) if !(u.deleted || u.is_bot || u.is_stranger) => u.id,
        EmailLookup::Found(_) | EmailLookup::NotFound => return Ok(miss(installation.id, user)),
        EmailLookup::Refused(code) => {
            tracing::warn!(
                installation_id = %installation.id,
                code,
                "slack users.lookupByEmail refused; this member stays unlinked"
            );
            return Ok(miss(installation.id, user));
        }
    };
    let holder = save_link(db, installation.id, &found, user).await?;
    if holder == user {
        Ok(SlackIdentity::Linked(found))
    } else {
        // That Slack account was linked to someone else first (a magic link
        // from a different Oxygen account). Never move it.
        tracing::info!(
            installation_id = %installation.id,
            %user,
            "slack account with this member's email is linked to another Oxygen user"
        );
        Ok(miss(installation.id, user))
    }
}

/// Insert the link unless the Slack user already has one, and answer whose
/// it is. Two deliveries racing for one person both land on the same row.
async fn save_link(
    db: &DatabaseConnection,
    installation_id: Uuid,
    slack_user_id: &str,
    user: Uuid,
) -> Result<Uuid, OxyError> {
    let row = slack_user_links::ActiveModel {
        id: ActiveValue::Set(Uuid::new_v4()),
        installation_id: ActiveValue::Set(installation_id),
        slack_user_id: ActiveValue::Set(slack_user_id.to_string()),
        oxy_user_id: ActiveValue::Set(user),
        link_method: ActiveValue::Set(LinkMethod::EmailLookup.as_str().into()),
        linked_at: ActiveValue::NotSet,
        last_seen_at: ActiveValue::NotSet,
    };
    slack_user_links::Entity::insert(row)
        .on_conflict(
            OnConflict::columns([
                slack_user_links::Column::InstallationId,
                slack_user_links::Column::SlackUserId,
            ])
            .do_nothing()
            .to_owned(),
        )
        .exec_without_returning(db)
        .await
        .map_err(|e| OxyError::DBError(e.to_string()))?;
    slack_user_links::Entity::find()
        .filter(slack_user_links::Column::InstallationId.eq(installation_id))
        .filter(slack_user_links::Column::SlackUserId.eq(slack_user_id))
        .one(db)
        .await
        .map_err(|e| OxyError::DBError(e.to_string()))?
        .map(|l| l.oxy_user_id)
        .ok_or_else(|| OxyError::DBError("the Slack link vanished as it was written".into()))
}

fn has_lookup_scope(granted: &str) -> bool {
    granted.split(',').any(|s| s.trim() == LOOKUP_SCOPE)
}

fn recently_missed(installation: Uuid, user: Uuid) -> bool {
    let Ok(guard) = MISSES.lock() else {
        return false;
    };
    guard
        .as_ref()
        .and_then(|m| m.get(&(installation, user)))
        .is_some_and(|at| at.elapsed() < MISS_TTL)
}

fn miss(installation: Uuid, user: Uuid) -> SlackIdentity {
    if let Ok(mut guard) = MISSES.lock() {
        let map = guard.get_or_insert_with(HashMap::new);
        map.insert((installation, user), Instant::now());
        if map.len() > 10_000 {
            map.retain(|_, at| at.elapsed() < MISS_TTL);
        }
    }
    SlackIdentity::NotFound
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_lookup_scope_is_read_from_slacks_granted_list() {
        assert!(has_lookup_scope("chat:write,users:read,users:read.email"));
        assert!(has_lookup_scope("users:read.email"));
        assert!(!has_lookup_scope("chat:write,users:read"));
        assert!(!has_lookup_scope(""));
    }

    #[test]
    fn a_miss_is_remembered_per_installation_and_person() {
        let (install, user) = (Uuid::new_v4(), Uuid::new_v4());
        assert!(!recently_missed(install, user));
        assert_eq!(miss(install, user), SlackIdentity::NotFound);
        assert!(recently_missed(install, user));
        assert!(
            !recently_missed(Uuid::new_v4(), user),
            "another org's Slack"
        );
        assert!(!recently_missed(install, Uuid::new_v4()), "another person");
    }
}
