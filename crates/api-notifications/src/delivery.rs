//! Getting a sent notification to each person: push, email, Slack.
//!
//! The inbox row already exists — that is the notification. Everything here is
//! an attempt to draw attention to it, recorded per (recipient, channel) in
//! `notification_deliveries`:
//!
//! - **Claimed before it is sent.** An insert that finds a row owns nothing, so
//!   a re-run of the task sends nothing twice. The one exception is a row
//!   recorded `failed`: a retry re-claims it, because a refusal means it did
//!   not go.
//! - **Skips are recorded, with a code.** "Muted", "no devices", "no address",
//!   "not linked to Slack" and "push not configured" are answers someone will
//!   ask for; an absent row could mean any of them, or that nothing ran.
//! - **What the person set decides first**, through the one decision
//!   (`settings::decide`): the nearest explicit node setting, its floor,
//!   pause and quiet hours, then each device's own settings and OS state. Its
//!   reason is what the row records. A critical kind overrides the person's
//!   settings, never a device that is off or blocked by its OS.
//! - **A push held by quiet hours** is summarized when the window ends, once
//!   per person (`settings::summary`).

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use entity::{notification_sends, notifications};
use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr, EntityTrait,
    QueryFilter, Statement,
};
use uuid::Uuid;

use super::catalog::source::KindInfo;
use super::channels::{ChannelOutcome, Channels};
use super::settings::decide::{Device, KindFacts, Settings, decide};
use super::settings::model::{DeviceSettings, GlobalSettings};
use super::settings::store::{self, SourceNodes};

/// The out-of-band channels, as stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Channel {
    Push,
    Email,
    Slack,
}

impl Channel {
    pub const ALL: [Channel; 3] = [Channel::Push, Channel::Email, Channel::Slack];

    pub fn as_str(self) -> &'static str {
        match self {
            Channel::Push => "push",
            Channel::Email => "email",
            Channel::Slack => "slack",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.as_str() == s)
    }
}

/// What one delivery run did, for the run's answer and its logs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeliveryReport {
    pub sent: usize,
    pub skipped: usize,
    pub failed: usize,
    /// Set when the run could not get as far as delivering anything.
    pub error: Option<String>,
}

impl DeliveryReport {
    pub fn summary(&self) -> String {
        match &self.error {
            Some(e) => format!("delivery could not run: {e}"),
            None => format!(
                "sent {}, skipped {}, failed {}",
                self.sent, self.skipped, self.failed
            ),
        }
    }

    fn add(&mut self, outcome: &ChannelOutcome) {
        match outcome {
            ChannelOutcome::Sent => self.sent += 1,
            ChannelOutcome::Skipped(_) => self.skipped += 1,
            ChannelOutcome::Failed(_) => self.failed += 1,
        }
    }
}

/// [`deliver_send`] in the shape `oxy_app_core::notifications::inline` takes:
/// the platform-notice write path below this surface delivers a send that has
/// no workspace to queue under through it. Installed at boot by `oxy serve` and
/// `oxy worker`, which refuse to start without it.
pub fn deliver_inline(
    db: &DatabaseConnection,
    send_id: Uuid,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = String> + Send + '_>> {
    Box::pin(async move { format!("{:?}", deliver_send(db, send_id).await) })
}

/// Deliver send `send_id` to everyone it was sent to. Never fails: what went
/// wrong is in the report, and a report with `failed > 0` fails the task so
/// the queue retries.
pub async fn deliver_send(db: &DatabaseConnection, send_id: Uuid) -> DeliveryReport {
    match deliver(db, send_id).await {
        Ok(report) => report,
        Err(e) => {
            tracing::warn!(%send_id, error = %e, "notification delivery could not run");
            DeliveryReport {
                failed: 1,
                error: Some(e),
                ..DeliveryReport::default()
            }
        }
    }
}

async fn deliver(db: &DatabaseConnection, send_id: Uuid) -> Result<DeliveryReport, String> {
    let send = notification_sends::Entity::find_by_id(send_id)
        .one(db)
        .await
        .map_err(|e| format!("load the send: {e}"))?
        .ok_or("the send no longer exists (its app or org was deleted)")?;
    let signals = notifications::Entity::find()
        .filter(notifications::Column::MessageId.eq(send_id))
        .all(db)
        .await
        .map_err(|e| format!("load the recipients: {e}"))?;
    let mut report = DeliveryReport::default();
    if signals.is_empty() {
        return Ok(report);
    }
    let users: Vec<Uuid> = signals.iter().map(|s| s.user_id).collect();
    // Where the kind sits today: its category decides which category and
    // source settings reach it, and the push envelope carries it.
    let info = super::catalog::live::describe_send(db, &send)
        .await
        .map_err(|e| format!("load the catalogue: {e}"))?;
    let people = People::load(db, &send, &info, &users)
        .await
        .map_err(|e| format!("load settings: {e}"))?;
    let channels = Channels::load(db, &send, &signals, &info).await?;
    let kind = KindFacts::of(&info, send.critical);
    let now = Utc::now();
    let mut held = Vec::new();
    for signal in &signals {
        let settings = people.settings(signal.user_id, &info);
        for channel in requested(&send) {
            let one = One {
                signal,
                channel,
                settings: &settings,
                kind: &kind,
                now,
            };
            let outcome = deliver_one(db, &channels, &one)
                .await
                .map_err(|e| format!("record a delivery: {e}"))?;
            if channel == Channel::Push && outcome == ChannelOutcome::Skipped("quiet_hours") {
                held.push(signal.user_id);
            }
            report.add(&outcome);
        }
    }
    super::settings::summary::schedule_for(db, &send, &people.global, &held, now).await;
    Ok(report)
}

/// The channels a send asked for. Push always; email and Slack when asked.
fn requested(send: &notification_sends::Model) -> Vec<Channel> {
    let mut out = vec![Channel::Push];
    if send.email {
        out.push(Channel::Email);
    }
    if send.slack {
        out.push(Channel::Slack);
    }
    out
}

/// What every recipient of one send set: person-level, and on the way from
/// the send's kind up to its source.
struct People {
    global: HashMap<Uuid, GlobalSettings>,
    nodes: HashMap<Uuid, SourceNodes>,
}

impl People {
    async fn load(
        db: &DatabaseConnection,
        send: &notification_sends::Model,
        info: &KindInfo,
        users: &[Uuid],
    ) -> Result<Self, DbErr> {
        Ok(Self {
            global: store::people_settings(db, users).await?,
            nodes: store::nodes_for_kind(db, users, send.app_id, &info.kind, &info.category)
                .await?,
        })
    }

    fn settings<'a>(&'a self, user: Uuid, info: &'a KindInfo) -> Settings<'a> {
        static NOTHING_SET: GlobalSettings = GlobalSettings {
            paused_until: None,
            quiet_hours: None,
            timezone: None,
        };
        let global = self.global.get(&user).unwrap_or(&NOTHING_SET);
        // Labels only name a deciding node in the settings document; the
        // ledger records the reason alone.
        Settings::for_kind(global, self.nodes.get(&user), info, "")
    }
}

/// One (recipient, channel) to deliver.
struct One<'a> {
    signal: &'a notifications::Model,
    channel: Channel,
    settings: &'a Settings<'a>,
    kind: &'a KindFacts,
    now: DateTime<Utc>,
}

async fn deliver_one(
    db: &DatabaseConnection,
    channels: &Channels,
    one: &One<'_>,
) -> Result<ChannelOutcome, DbErr> {
    let (signal, channel) = (one.signal, one.channel);
    if !claim(db, signal.id, channel).await? {
        // Already sent, skipped, or claimed by a run that may have sent it.
        return Ok(ChannelOutcome::Skipped("already_attempted"));
    }
    let decision = decide(one.settings, one.kind, channel, None, one.now);
    let outcome = match decision.reason {
        Some(reason) => ChannelOutcome::Skipped(reason.as_str()),
        None if channel == Channel::Push => {
            let channel_id = channels.channel_id(signal);
            let refuses = |device: &DeviceSettings| {
                let target = Device {
                    settings: device,
                    channel_id: &channel_id,
                };
                decide(one.settings, one.kind, channel, Some(&target), one.now).reason
            };
            channels.push_where(signal, refuses).await
        }
        None => channels.send(channel, signal).await,
    };
    finish(db, signal.id, channel, &outcome).await?;
    Ok(outcome)
}

/// Take (signal, channel) for this attempt. `true` when it is ours to send: a
/// new row, or a `failed` one being retried.
async fn claim(
    db: &DatabaseConnection,
    notification_id: Uuid,
    channel: Channel,
) -> Result<bool, DbErr> {
    let row = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "INSERT INTO notification_deliveries (notification_id, channel, status) \
             VALUES ($1, $2, 'claimed') \
             ON CONFLICT (notification_id, channel) DO UPDATE \
               SET status = 'claimed', reason = NULL, attempted_at = now() \
               WHERE notification_deliveries.status = 'failed' \
             RETURNING notification_id",
            [notification_id.into(), channel.as_str().into()],
        ))
        .await?;
    Ok(row.is_some())
}

async fn finish(
    db: &DatabaseConnection,
    notification_id: Uuid,
    channel: Channel,
    outcome: &ChannelOutcome,
) -> Result<(), DbErr> {
    let (status, reason) = match outcome {
        ChannelOutcome::Sent => ("sent", None),
        ChannelOutcome::Skipped(r) => ("skipped", Some(*r)),
        ChannelOutcome::Failed(r) => ("failed", Some(*r)),
    };
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "UPDATE notification_deliveries SET status = $3, reason = $4, attempted_at = now() \
         WHERE notification_id = $1 AND channel = $2",
        [
            notification_id.into(),
            channel.as_str().into(),
            status.into(),
            reason.map(str::to_string).into(),
        ],
    ))
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channels_round_trip_through_their_stored_names() {
        for c in Channel::ALL {
            assert_eq!(Channel::parse(c.as_str()), Some(c));
        }
        assert_eq!(Channel::parse("sms"), None, "no SMS channel exists yet");
    }

    #[test]
    fn a_report_with_a_failure_says_so() {
        let mut r = DeliveryReport::default();
        r.add(&ChannelOutcome::Sent);
        r.add(&ChannelOutcome::Skipped("muted"));
        r.add(&ChannelOutcome::Failed("provider_error"));
        assert_eq!(r.summary(), "sent 1, skipped 1, failed 1");
    }
}
