//! The Slack channel: who each recipient is in their org's Slack, and the DM.
//!
//! A recipient is reached through their `slack_user_links` row. A member with
//! no row is looked up by their Oxygen email the first time Slack is asked for
//! ([`link_by_email`]), so a person is told in Slack without ever having
//! talked to the bot. Frontline crew have no address and are never looked up;
//! a crew member who linked through the bot is still reached.
//!
//! The first lookup that fails with something a retry can fix (Slack's rate
//! limit, usually) stops the lookups for the rest of the run: `users.lookupByEmail`
//! allows about 50 calls a minute, and every further call would only be another
//! 429 that keeps the token throttled. Those people are `failed` and the
//! queue's retry looks them up with a fresh budget; recipients already linked
//! are still DMed. Whoever is still unlinked when the retries run out has the
//! inbox row, and is looked up again by the next notification that asks for
//! Slack.
//!
//! The DM says which app sent it: every notification comes from the same
//! Oxygen bot, and "Opening list is late" means nothing without its source.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};

use entity::{org_members, slack_installations, slack_user_links, users};
use oxy_oltp::inbox::InboxMessage;
use oxy_slack_client::SlackClient;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QuerySelect};
use serde_json::{Value, json};
use uuid::Uuid;

use super::channels::ChannelOutcome;
use oxy_slack::by_email::{SlackIdentity, link_by_email};
use oxy_slack::installations::InstallationsService;

/// Slack refuses a section whose text is longer than this.
const SECTION_LIMIT: usize = 3000;
/// …and a button whose URL is.
const BUTTON_URL_LIMIT: usize = 3000;

/// What one notification says in Slack, the same for every recipient.
pub struct SlackMessage<'a> {
    pub content: &'a InboxMessage,
    /// The absolute link, when the notification has one.
    pub url: Option<&'a str>,
    /// The app's name, or "Oxygen" for a platform notice.
    pub sender: &'a str,
}

/// The org's Slack, loaded once per delivery run.
pub struct SlackDm {
    client: SlackClient,
    db: DatabaseConnection,
    installation: slack_installations::Model,
    bot_token: String,
    /// Oxygen user → Slack user id, for the recipients already linked.
    links: HashMap<Uuid, String>,
    /// The recipients who could be looked up: unlinked members with an active
    /// account, by their Oxygen email.
    lookups: HashMap<Uuid, String>,
    /// Set by the first lookup that failed in a way a retry can fix; no
    /// lookup is made after it in this run.
    lookups_paused: AtomicBool,
}

impl SlackDm {
    /// `None` when the org has not connected Slack: every Slack delivery is
    /// then `not_configured`.
    pub async fn load(
        db: &DatabaseConnection,
        org_id: Uuid,
        recipients: &[Uuid],
        client: SlackClient,
    ) -> Result<Option<Self>, String> {
        let Some(installation) = InstallationsService::find_active_by_org(org_id)
            .await
            .map_err(|e| format!("load the Slack installation: {e}"))?
        else {
            return Ok(None);
        };
        let bot_token = InstallationsService::decrypt_bot_token(&installation)
            .await
            .map_err(|e| format!("open the Slack bot token: {e}"))?;
        let links = links(db, installation.id, recipients).await?;
        let unlinked: Vec<Uuid> = recipients
            .iter()
            .copied()
            .filter(|u| !links.contains_key(u))
            .collect();
        let lookups = member_emails(db, org_id, &unlinked).await?;
        Ok(Some(Self {
            client,
            db: db.clone(),
            installation,
            bot_token,
            links,
            lookups,
            lookups_paused: AtomicBool::new(false),
        }))
    }

    /// DM `user`. A lookup or a post that can be retried is `failed`, so the
    /// queue's retry runs it again; a person who cannot be found is
    /// `not_linked`.
    pub async fn send(&self, user: Uuid, message: &SlackMessage<'_>) -> ChannelOutcome {
        let slack_user = match self.slack_user(user).await {
            Ok(Some(id)) => id,
            Ok(None) => return ChannelOutcome::Skipped("not_linked"),
            Err(e) => {
                tracing::warn!(%user, error = %e, "notification Slack lookup failed");
                return ChannelOutcome::Failed("provider_error");
            }
        };
        match self
            .client
            .chat_post_message_with_blocks(
                &self.bot_token,
                &slack_user,
                &slack_text(message),
                None,
                Some(slack_blocks(message)),
            )
            .await
        {
            Ok(_) => ChannelOutcome::Sent,
            Err(e) => {
                tracing::warn!(%user, error = %e, "notification Slack DM failed");
                ChannelOutcome::Failed("provider_error")
            }
        }
    }

    async fn slack_user(&self, user: Uuid) -> Result<Option<String>, String> {
        if let Some(id) = self.links.get(&user) {
            return Ok(Some(id.clone()));
        }
        let Some(email) = self.lookups.get(&user) else {
            return Ok(None);
        };
        if self.lookups_paused.load(Ordering::Relaxed) {
            return Err("an earlier lookup in this run failed; left for the retry".into());
        }
        let found = link_by_email(
            &self.db,
            &self.client,
            &self.installation,
            &self.bot_token,
            user,
            email,
        )
        .await
        .map_err(|e| {
            self.lookups_paused.store(true, Ordering::Relaxed);
            e.to_string()
        })?;
        Ok(match found {
            SlackIdentity::Linked(id) => Some(id),
            SlackIdentity::NotFound => None,
        })
    }
}

async fn links(
    db: &DatabaseConnection,
    installation_id: Uuid,
    recipients: &[Uuid],
) -> Result<HashMap<Uuid, String>, String> {
    Ok(slack_user_links::Entity::find()
        .filter(slack_user_links::Column::InstallationId.eq(installation_id))
        .filter(slack_user_links::Column::OxyUserId.is_in(recipients.iter().copied()))
        .all(db)
        .await
        .map_err(|e| format!("load Slack links: {e}"))?
        .into_iter()
        .map(|l| (l.oxy_user_id, l.slack_user_id))
        .collect())
}

/// The email of each of `users` who is a member of the org with an active
/// account. Slack belongs to the org, so nobody else is looked up in it.
async fn member_emails(
    db: &DatabaseConnection,
    org_id: Uuid,
    users: &[Uuid],
) -> Result<HashMap<Uuid, String>, String> {
    if users.is_empty() {
        return Ok(HashMap::new());
    }
    let members: HashSet<Uuid> = org_members::Entity::find()
        .select_only()
        .column(org_members::Column::UserId)
        .filter(org_members::Column::OrgId.eq(org_id))
        .filter(org_members::Column::UserId.is_in(users.iter().copied()))
        .into_tuple::<Uuid>()
        .all(db)
        .await
        .map_err(|e| format!("load org members: {e}"))?
        .into_iter()
        .collect();
    if members.is_empty() {
        return Ok(HashMap::new());
    }
    Ok(users::Entity::find()
        .filter(users::Column::Id.is_in(members.iter().copied()))
        .filter(users::Column::Status.eq(users::UserStatus::Active))
        .all(db)
        .await
        .map_err(|e| format!("load member emails: {e}"))?
        .into_iter()
        .filter_map(|u| {
            let email = u.email?.trim().to_lowercase();
            (!email.is_empty()).then_some((u.id, email))
        })
        .collect())
}

/// Slack's three reserved characters, escaped as Slack documents.
fn slack_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// The notification's title and body, escaped, cut to fit a section.
fn section_text(content: &InboxMessage) -> String {
    let mut text = format!("*{}*", slack_escape(&content.title));
    if let Some(body) = &content.body {
        text.push('\n');
        text.push_str(&slack_escape(body));
    }
    if text.chars().count() > SECTION_LIMIT {
        text = text.chars().take(SECTION_LIMIT - 1).collect();
        text.push('…');
    }
    text
}

/// The fallback Slack shows in a push and where blocks cannot render.
fn slack_text(message: &SlackMessage<'_>) -> String {
    let mut text = format!("*{}*", slack_escape(&message.content.title));
    if let Some(body) = &message.content.body {
        text.push('\n');
        text.push_str(&slack_escape(body));
    }
    if let Some(url) = message.url {
        // `<url|label>` has no escape for `|`.
        text.push_str(&format!("\n<{}|Open>", url.replace('|', "%7C")));
    }
    text
}

/// The message itself: what it says, who sent it, and a button to it.
fn slack_blocks(message: &SlackMessage<'_>) -> Value {
    let mut blocks = vec![
        json!({ "type": "section", "text": { "type": "mrkdwn", "text": section_text(message.content) } }),
        json!({ "type": "context", "elements": [
            { "type": "mrkdwn", "text": slack_escape(message.sender) }
        ] }),
    ];
    if let Some(url) = message.url.filter(|u| u.len() <= BUTTON_URL_LIMIT) {
        blocks.push(json!({ "type": "actions", "elements": [{
            "type": "button",
            "text": { "type": "plain_text", "text": "Open" },
            "url": url,
        }] }));
    }
    Value::Array(blocks)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn content(title: &str, body: Option<&str>) -> InboxMessage {
        InboxMessage {
            id: Uuid::nil(),
            app_id: None,
            kind: "k".into(),
            title: title.into(),
            body: body.map(str::to_string),
            link: None,
            email_subject: None,
            email_html: None,
            email_text: None,
        }
    }

    fn message<'a>(content: &'a InboxMessage, url: Option<&'a str>) -> SlackMessage<'a> {
        SlackMessage {
            content,
            url,
            sender: "Store <Ops>",
        }
    }

    #[test]
    fn slack_text_cannot_be_turned_into_a_mention_or_a_link() {
        let c = content("<!channel> & co", Some("<https://x|y>"));
        assert_eq!(
            slack_text(&message(&c, None)),
            "*&lt;!channel&gt; &amp; co*\n&lt;https://x|y&gt;"
        );
    }

    #[test]
    fn slack_text_links_once_with_a_safe_url() {
        let c = content("Done", None);
        assert_eq!(
            slack_text(&message(&c, Some("https://a.example/x|y"))),
            "*Done*\n<https://a.example/x%7Cy|Open>"
        );
    }

    #[test]
    fn the_blocks_say_who_sent_it_and_link_with_a_button() {
        let c = content("Late", Some("Clovis"));
        let blocks = slack_blocks(&message(&c, Some("https://a.example/l/1")));
        assert_eq!(
            blocks,
            json!([
                { "type": "section", "text": { "type": "mrkdwn", "text": "*Late*\nClovis" } },
                { "type": "context", "elements": [{ "type": "mrkdwn", "text": "Store &lt;Ops&gt;" }] },
                { "type": "actions", "elements": [{
                    "type": "button",
                    "text": { "type": "plain_text", "text": "Open" },
                    "url": "https://a.example/l/1",
                }] },
            ])
        );
    }

    #[test]
    fn no_link_means_no_button_and_an_overlong_url_is_dropped() {
        let c = content("Late", None);
        assert_eq!(
            slack_blocks(&message(&c, None)).as_array().unwrap().len(),
            2
        );
        let long = format!("https://a.example/{}", "x".repeat(BUTTON_URL_LIMIT));
        assert_eq!(
            slack_blocks(&message(&c, Some(&long)))
                .as_array()
                .unwrap()
                .len(),
            2,
            "Slack refuses the whole message over a too-long button URL"
        );
    }

    #[test]
    fn a_body_that_escapes_past_slacks_limit_is_cut_to_fit() {
        // 2,000 ampersands escape to 10,000 characters.
        let c = content("T", Some(&"&".repeat(2000)));
        let text = section_text(&c);
        assert_eq!(text.chars().count(), SECTION_LIMIT);
        assert!(text.ends_with('…'));
    }
}
