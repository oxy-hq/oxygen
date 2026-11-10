//! The three senders a delivery run uses, loaded once per send.
//!
//! [`Channels::load`] reads everything a run needs in a handful of queries —
//! device tokens, addresses, Slack links, the content — so the per-recipient
//! loop in [`super::delivery`] does no lookups of its own, except Slack's
//! first-time lookup of an unlinked member by email ([`super::slack_dm`]).
//! Content is read from the org's store only when email or Slack needs it: a
//! push carries none.

use std::collections::HashMap;
use std::sync::Arc;

use entity::{device_tokens, notification_sends, notifications, organizations, users};
use oxy_app_core::custom_app_environment::AppEnvironment;
use oxy_oltp::inbox::{self, InboxMessage};
use oxy_slack_client::SlackClient;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use uuid::Uuid;

use super::catalog::source::KindInfo;
use super::deliver::{Push, PushSignal, push};
use super::delivery::Channel;
use super::platform::render;
use super::settings::decide::Reason;
use super::settings::model::DeviceSettings;
use super::slack_dm::{SlackDm, SlackMessage};
use oxy_app_core::emails::app_emailer::{AppEmailer, EmailSendInput};

/// How one channel went for one person. Reasons are fixed codes, stored as-is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChannelOutcome {
    Sent,
    Skipped(&'static str),
    Failed(&'static str),
}

pub struct Channels {
    /// The kind and where it sits, which a native push carries.
    info: KindInfo,
    push: Arc<dyn Push>,
    tokens: HashMap<Uuid, Vec<device_tokens::Model>>,
    /// Present only when the send asked for email.
    email: Option<(AppEmailer, HashMap<Uuid, String>)>,
    /// Present only when the send asked for Slack and the org has it.
    slack: Option<SlackDm>,
    content: Option<InboxMessage>,
    url: Option<String>,
    /// Who the notification is from: the app's name, or "Oxygen". Email and
    /// Slack say it; a Web Push names it, encrypted to the device.
    sender: String,
}

impl Channels {
    pub async fn load(
        db: &DatabaseConnection,
        send: &notification_sends::Model,
        signals: &[notifications::Model],
        info: &KindInfo,
    ) -> Result<Self, String> {
        let users: Vec<Uuid> = signals.iter().map(|s| s.user_id).collect();
        let tokens = tokens(db, &users).await?;
        let content = content(db, send, signals).await?;
        let (app_name, url) = names_and_url(db, send, content.as_ref()).await?;
        let email = if send.email {
            Some((
                AppEmailer::from_env(app_name.clone()),
                addresses(db, &users).await?,
            ))
        } else {
            None
        };
        let slack = if send.slack {
            SlackDm::load(db, send.org_id, &users, SlackClient::new()).await?
        } else {
            None
        };
        Ok(Self {
            info: info.clone(),
            push: push(),
            tokens,
            email,
            slack,
            content,
            url,
            sender: app_name,
        })
    }

    /// `signal` is the recipient's inbox row for this send: a native push
    /// names it so the phone opens that item, and carries its thread. Email
    /// and Slack; push goes through [`Self::push_where`].
    pub async fn send(&self, channel: Channel, signal: &notifications::Model) -> ChannelOutcome {
        match channel {
            Channel::Push => self.push_where(signal, |_| None).await,
            Channel::Email => self.email(signal.user_id).await,
            Channel::Slack => self.slack(signal.user_id).await,
        }
    }

    /// The `<group>.<kind>` id a push for `signal` carries: the iOS category
    /// and Android channel a device's OS can switch off.
    pub fn channel_id(&self, signal: &notifications::Model) -> String {
        PushSignal::new(signal, &self.info, &self.sender).channel_id()
    }

    /// Push `signal` to the recipient's devices that `refuses` lets through
    /// (it answers why a device does not get it). With every device refused,
    /// the skip carries the reason of the one seen most recently: the device
    /// the person is most likely holding.
    pub async fn push_where(
        &self,
        signal: &notifications::Model,
        refuses: impl Fn(&DeviceSettings) -> Option<Reason>,
    ) -> ChannelOutcome {
        if !self.push.configured() {
            return ChannelOutcome::Skipped("not_configured");
        }
        let user = signal.user_id;
        let Some(tokens) = self.tokens.get(&user).filter(|t| !t.is_empty()) else {
            return ChannelOutcome::Skipped("no_devices");
        };
        let (allowed, refused): (Vec<_>, Vec<_>) = tokens
            .iter()
            .map(|t| (t, refuses(&DeviceSettings::of(t))))
            .partition(|(_, reason)| reason.is_none());
        if allowed.is_empty() {
            let latest = refused.iter().max_by_key(|(t, _)| t.last_seen_at);
            let reason = latest
                .and_then(|(_, r)| *r)
                .map_or("no_devices", Reason::as_str);
            return ChannelOutcome::Skipped(reason);
        }
        let tokens: Vec<device_tokens::Model> =
            allowed.into_iter().map(|(t, _)| t.clone()).collect();
        if !tokens.iter().any(|t| self.push.handles(t)) {
            return ChannelOutcome::Skipped("not_configured");
        }
        let signal = PushSignal::new(signal, &self.info, &self.sender);
        let report = self.push.send(&tokens, &signal).await;
        match (report.delivered, report.failed) {
            (0, 0) => ChannelOutcome::Skipped("no_devices"),
            (0, _) => ChannelOutcome::Failed("provider_error"),
            _ => ChannelOutcome::Sent,
        }
    }

    async fn email(&self, user: Uuid) -> ChannelOutcome {
        let Some((emailer, addresses)) = &self.email else {
            return ChannelOutcome::Skipped("not_requested");
        };
        let Some(to) = addresses.get(&user) else {
            // A frontline worker, or a deleted account: nobody to mail.
            return ChannelOutcome::Skipped("no_address");
        };
        let Some(content) = &self.content else {
            return ChannelOutcome::Failed("content_unavailable");
        };
        let subject = content.email_subject.as_deref().unwrap_or(&content.title);
        let input = EmailSendInput::to_one(
            to,
            subject,
            content.email_html.clone(),
            content
                .email_text
                .clone()
                .or_else(|| Some(content.title.clone())),
        );
        match emailer.send(input).await {
            Ok(_) => ChannelOutcome::Sent,
            Err(e) => {
                // The provider's text can carry the address, so it goes to the
                // log, never to the ledger.
                tracing::warn!(%user, error = %e, "notification email failed");
                ChannelOutcome::Failed("provider_error")
            }
        }
    }

    async fn slack(&self, user: Uuid) -> ChannelOutcome {
        let Some(dm) = &self.slack else {
            return ChannelOutcome::Skipped("not_configured");
        };
        let Some(content) = &self.content else {
            return ChannelOutcome::Failed("content_unavailable");
        };
        let message = SlackMessage {
            content,
            url: self.url.as_deref(),
            sender: &self.sender,
        };
        dm.send(user, &message).await
    }
}

/// What email and Slack say, when the send asked for either: an app's from
/// the org's store, a platform notice's rendered from Oxy's records.
async fn content(
    db: &DatabaseConnection,
    send: &notification_sends::Model,
    signals: &[notifications::Model],
) -> Result<Option<InboxMessage>, String> {
    if !(send.email || send.slack) {
        return Ok(None);
    }
    if let Some(signal) = signals.first().filter(|s| render::is_platform(s)) {
        return render::message(db, signal)
            .await
            .map(Some)
            .map_err(|e| format!("render the notice: {e}"));
    }
    Ok(inbox::read(db, send.org_id, &[send.id])
        .await
        .map_err(|e| e.to_string())?
        .into_iter()
        .next())
}

async fn tokens(
    db: &DatabaseConnection,
    users: &[Uuid],
) -> Result<HashMap<Uuid, Vec<device_tokens::Model>>, String> {
    let mut out: HashMap<Uuid, Vec<device_tokens::Model>> = HashMap::new();
    for t in device_tokens::Entity::find()
        .filter(device_tokens::Column::UserId.is_in(users.iter().copied()))
        .all(db)
        .await
        .map_err(|e| format!("load device tokens: {e}"))?
    {
        out.entry(t.user_id).or_default().push(t);
    }
    Ok(out)
}

/// The mailbox of every recipient who has one and an active account.
async fn addresses(
    db: &DatabaseConnection,
    users: &[Uuid],
) -> Result<HashMap<Uuid, String>, String> {
    Ok(users::Entity::find()
        .filter(users::Column::Id.is_in(users.iter().copied()))
        .filter(users::Column::Status.eq(users::UserStatus::Active))
        .all(db)
        .await
        .map_err(|e| format!("load addresses: {e}"))?
        .into_iter()
        .filter_map(|u| {
            let email = u.email?.trim().to_string();
            (!email.is_empty()).then_some((u.id, email))
        })
        .collect())
}

/// The sender's display name and the absolute link email and Slack carry.
async fn names_and_url(
    db: &DatabaseConnection,
    send: &notification_sends::Model,
    content: Option<&InboxMessage>,
) -> Result<(String, Option<String>), String> {
    let app = match send.app_id {
        Some(id) => entity::apps::Entity::find_by_id(id)
            .one(db)
            .await
            .map_err(|e| format!("load the app: {e}"))?,
        None => None,
    };
    let org_slug = organizations::Entity::find_by_id(send.org_id)
        .one(db)
        .await
        .map_err(|e| format!("load the org: {e}"))?
        .map(|o| o.slug);
    let environment =
        AppEnvironment::parse(&send.environment).unwrap_or(AppEnvironment::Production);
    let url = org_slug.as_deref().and_then(|org| {
        super::links::absolute_url(
            org,
            app.as_ref().map(|a| a.slug.as_str()),
            &environment,
            content.and_then(|c| c.link.as_deref()),
        )
    });
    let name = app.map(|a| a.name).unwrap_or_else(|| "Oxygen".to_string());
    Ok((name, url))
}
