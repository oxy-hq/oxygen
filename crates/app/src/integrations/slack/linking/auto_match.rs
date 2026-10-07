use crate::integrations::slack::client::SlackClient;
use crate::integrations::slack::services::installations::InstallationsService;
use crate::integrations::slack::services::user_links::{CreateLink, LinkMethod, UserLinksService};
use entity::org_members;
use entity::prelude::Users;
use entity::slack_installations::Model as InstallationRow;
use oxy::database::client::establish_connection;
use oxy::database::filters::UserQueryFilterExt;
use oxy_shared::errors::OxyError;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use uuid::Uuid;

pub enum AutoMatchResult {
    Linked(entity::slack_user_links::Model),
    NoEmail,
    EmailNotInOrg,
}

// In-memory negative cache: avoids repeated `users.info` calls for Slack users
// whose emails will never match an Oxy account. TTL is 5 minutes.
// Only negative results (NoEmail, EmailNotInOrg) are cached; Linked results
// go through the normal UserLinksService path on subsequent events.
const NEGATIVE_CACHE_TTL: Duration = Duration::from_secs(300);
static NEGATIVE_CACHE: Mutex<Option<HashMap<(Uuid, String), Instant>>> = Mutex::new(None);

fn is_cached_negative(inst_id: Uuid, slack_user_id: &str) -> bool {
    let Ok(guard) = NEGATIVE_CACHE.lock() else {
        return false;
    };
    let Some(map) = guard.as_ref() else {
        return false;
    };
    map.get(&(inst_id, slack_user_id.to_string()))
        .is_some_and(|ts| ts.elapsed() < NEGATIVE_CACHE_TTL)
}

fn cache_negative(inst_id: Uuid, slack_user_id: &str) {
    let Ok(mut guard) = NEGATIVE_CACHE.lock() else {
        return;
    };
    let map = guard.get_or_insert_with(HashMap::new);
    map.insert((inst_id, slack_user_id.to_string()), Instant::now());
    // Opportunistic cleanup to bound memory usage.
    if map.len() > 1000 {
        map.retain(|_, ts| ts.elapsed() < NEGATIVE_CACHE_TTL);
    }
}

/// Attempt to auto-link a Slack user to an Oxy account by matching email.
///
/// Flow:
/// 1. Check in-memory negative cache (skip `users.info` for known-unlinked users).
/// 2. Fetch the Slack user's profile via `users.info`.
/// 3. Look up the email in the `users` table.
/// 4. Verify the user is a member of the installation's org.
/// 5. Create a `slack_user_links` row with `LinkMethod::EmailAuto`.
pub async fn try_auto_match(
    installation: &InstallationRow,
    slack_user_id: &str,
) -> Result<AutoMatchResult, OxyError> {
    // Skip users.info for known-unlinked users (saves Slack API quota).
    if is_cached_negative(installation.id, slack_user_id) {
        return Ok(AutoMatchResult::EmailNotInOrg);
    }

    let bot_token = InstallationsService::decrypt_bot_token(installation).await?;
    let info = SlackClient::new()
        .users_info(&bot_token, slack_user_id)
        .await?;
    let raw_email = info.user.and_then(|u| u.profile).and_then(|p| p.email);
    let Some(email) = raw_email
        .map(|e| e.trim().to_lowercase())
        .filter(|e| !e.is_empty())
    else {
        tracing::info!(
            slack_user_id,
            "slack auto-match: users.info returned no email (check users:read.email scope on the app)"
        );
        cache_negative(installation.id, slack_user_id);
        return Ok(AutoMatchResult::NoEmail);
    };

    // Not every stored address is lowercase — one made by Google or GitHub
    // sign-in kept the provider's capitals — so the lookup ignores case.
    let conn = establish_connection().await?;
    let user = Users::find()
        .filter_by_email(&email)
        .one(&conn)
        .await
        .map_err(|e| OxyError::DBError(e.to_string()))?;
    let Some(user) = user else {
        tracing::info!(
            slack_user_id,
            email,
            org_id = %installation.org_id,
            "slack auto-match: no Oxy user with this email — falling back to magic-link"
        );
        cache_negative(installation.id, slack_user_id);
        return Ok(AutoMatchResult::EmailNotInOrg);
    };

    let mem = org_members::Entity::find()
        .filter(org_members::Column::OrgId.eq(installation.org_id))
        .filter(org_members::Column::UserId.eq(user.id))
        .one(&conn)
        .await
        .map_err(|e| OxyError::DBError(e.to_string()))?;
    if mem.is_none() {
        tracing::info!(
            slack_user_id,
            email,
            oxy_user_id = %user.id,
            org_id = %installation.org_id,
            "slack auto-match: user exists but isn't a member of this install's org"
        );
        cache_negative(installation.id, slack_user_id);
        return Ok(AutoMatchResult::EmailNotInOrg);
    }

    let link = UserLinksService::create(CreateLink {
        installation_id: installation.id,
        slack_user_id: slack_user_id.to_string(),
        oxy_user_id: user.id,
        link_method: LinkMethod::EmailAuto,
    })
    .await?;
    Ok(AutoMatchResult::Linked(link))
}
