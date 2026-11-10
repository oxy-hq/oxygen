//! Quiet hours' summary: the pushes quiet hours held back, told as one push
//! when the window ends — "3 notifications while quiet hours were on".
//!
//! # Why a delayed task
//!
//! The durable queue already runs a task no earlier than its `available_at`
//! (`agentic_task_queue.available_at`, which every claim honours), so a
//! summary is one task per (person, window end), queued by the delivery run
//! that held a push and due when the window ends. Its id names both, so a
//! second hold in the same window queues nothing new.
//!
//! # What it tells
//!
//! It counts the person's pushes recorded `quiet_hours` since the last
//! summary (at most a day back) that are **still unread**: anything they
//! already read in the inbox needs no nudge. Nothing unread, no push. Each
//! hold is summarized at most once (`notification_user_settings.
//! quiet_summary_through`), however the windows were edited in between.
//!
//! When it runs, the person's settings are read again:
//!
//! - **Paused** — no push; the holds count as told (the inbox has them).
//! - **Quiet again** (the window was moved or extended) — no push, and a
//!   summary is queued for the new end; the holds roll into it.
//!
//! The push is Oxygen's, kind `oxy.quiet_hours_summary` (channel id
//! `oxygen.oxy.quiet_hours_summary`), level `active`, and names the newest
//! held item, so a tap opens something real. It is not an inbox row of its
//! own. Each device's settings apply as to any push. A Web Push of it reads
//! "Oxygen" / the count and opens the notifications page
//! ([`crate::web_push_payload::href`]).

use std::collections::{BTreeSet, HashMap};

use chrono::{DateTime, Duration, Utc};
use entity::{device_tokens, notification_sends};
use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr, EntityTrait,
    FromQueryResult, QueryFilter, Statement, TransactionTrait,
};
use uuid::Uuid;

use super::decide::device_reason;
use super::model::{DeviceSettings, GlobalSettings};
use super::quiet::parse_tz;
use super::store;
use crate::catalog::model::Level;
use crate::deliver::{PushSignal, push};

/// The kind a summary push carries.
pub const SUMMARY_KIND: &str = "oxy.quiet_hours_summary";

/// How far back a summary looks, whatever it last reached: a window is
/// shorter than a day.
const LOOKBACK_HOURS: i64 = 24;

/// When the quiet-hours window `global` is in at `now` ends; `None` outside
/// one.
pub fn window_end(global: Option<&GlobalSettings>, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    let global = global?;
    let tz = parse_tz(global.timezone.as_deref()?)?;
    global.quiet_hours.as_ref()?.ends_after(now, tz)
}

/// After a delivery run: a summary for each of `held` — the people whose
/// push quiet hours held — due when their window ends. Best-effort: the hold
/// is recorded and the inbox has the notification, so a failure here is
/// logged, never the run's.
pub async fn schedule_for(
    db: &DatabaseConnection,
    send: &notification_sends::Model,
    global: &HashMap<Uuid, GlobalSettings>,
    held: &[Uuid],
    now: DateTime<Utc>,
) {
    if held.is_empty() {
        return;
    }
    // Recorded under the delivery run's workspace. An org with no workspace
    // delivers inline, with no run to borrow one from: the inbox it is.
    let workspace = match run_workspace(db, send.run_id.as_deref()).await {
        Ok(Some(workspace)) => workspace,
        Ok(None) => return,
        Err(e) => {
            tracing::warn!(send = %send.id, error = %e, "quiet-hours summary not scheduled");
            return;
        }
    };
    let people: BTreeSet<Uuid> = held.iter().copied().collect();
    for user in people {
        let Some(due) = window_end(global.get(&user), now) else {
            continue;
        };
        if let Err(e) = super::summary_task::schedule(db, user, due, workspace).await {
            tracing::warn!(%user, error = %e, "quiet-hours summary not scheduled");
        }
    }
}

async fn run_workspace(
    db: &DatabaseConnection,
    run_id: Option<&str>,
) -> Result<Option<Uuid>, DbErr> {
    let Some(run_id) = run_id else {
        return Ok(None);
    };
    let row = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT workspace_id FROM agentic_runs WHERE id = $1",
            [run_id.into()],
        ))
        .await?;
    row.map(|r| r.try_get::<Uuid>("", "workspace_id"))
        .transpose()
}

/// What a summary run came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Summary {
    /// Pushed to `devices` devices for `count` held notifications.
    Sent { count: usize, devices: usize },
    /// Nothing held is still unread, or no device takes it.
    Nothing,
    /// Paused now: the holds count as told.
    Paused,
    /// In a window again, which ends then: summarize after it.
    StillQuiet(DateTime<Utc>),
}

/// One held push.
#[derive(Debug, Clone, FromQueryResult)]
struct Held {
    id: Uuid,
    org_id: Uuid,
}

/// Summarize `user_id`'s held pushes at `now`.
pub async fn summarize(
    db: &DatabaseConnection,
    user_id: Uuid,
    now: DateTime<Utc>,
) -> Result<Summary, DbErr> {
    let txn = db.begin().await?;
    store::lock(&txn, user_id).await?;
    let (global, _) = store::person(&txn, user_id).await?;
    if global.paused_at(now) {
        store::summarized_through(&txn, user_id, now).await?;
        txn.commit().await?;
        return Ok(Summary::Paused);
    }
    if let Some(end) = window_end(Some(&global), now) {
        txn.commit().await?;
        return Ok(Summary::StillQuiet(end));
    }
    let floor = now - Duration::hours(LOOKBACK_HOURS);
    let since = store::summary_through(&txn, user_id)
        .await?
        .map_or(floor, |t| t.max(floor));
    let held = held_unread(&txn, user_id, since, now).await?;
    store::summarized_through(&txn, user_id, now).await?;
    txn.commit().await?;
    let Some(newest) = held.first() else {
        return Ok(Summary::Nothing);
    };
    let devices = tell(db, user_id, newest, held.len()).await?;
    Ok(match devices {
        0 => Summary::Nothing,
        devices => Summary::Sent {
            count: held.len(),
            devices,
        },
    })
}

/// The unread notifications whose push quiet hours held between `since` and
/// `until`, newest first.
async fn held_unread(
    db: &impl ConnectionTrait,
    user_id: Uuid,
    since: DateTime<Utc>,
    until: DateTime<Utc>,
) -> Result<Vec<Held>, DbErr> {
    Held::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "SELECT n.id, n.org_id FROM notification_deliveries d \
         JOIN notifications n ON n.id = d.notification_id \
         WHERE n.user_id = $1 AND d.channel = 'push' AND d.reason = 'quiet_hours' \
           AND d.attempted_at > $2 AND d.attempted_at <= $3 AND n.read_at IS NULL \
         ORDER BY n.created_at DESC, n.id DESC",
        [
            user_id.into(),
            since.fixed_offset().into(),
            until.fixed_offset().into(),
        ],
    ))
    .all(db)
    .await
}

/// The summary's words: a count, never what any notification said.
pub fn label(count: usize) -> String {
    match count {
        1 => "1 notification while quiet hours were on".to_string(),
        n => format!("{n} notifications while quiet hours were on"),
    }
}

/// Push the summary to the devices that take it; how many it went to.
async fn tell(
    db: &DatabaseConnection,
    user_id: Uuid,
    newest: &Held,
    count: usize,
) -> Result<usize, DbErr> {
    let transport = push();
    if !transport.configured() {
        return Ok(0);
    }
    let signal = PushSignal {
        user_id,
        kind: SUMMARY_KIND.to_string(),
        notification_id: newest.id,
        org_id: newest.org_id,
        app_id: None,
        label: label(count),
        category: crate::catalog::model::GENERAL.to_string(),
        level: Level::Active,
        thread: None,
        app_name: "Oxygen".to_string(),
    };
    let channel_id = signal.channel_id();
    let devices: Vec<device_tokens::Model> = device_tokens::Entity::find()
        .filter(device_tokens::Column::UserId.eq(user_id))
        .all(db)
        .await?
        .into_iter()
        .filter(|t| device_reason(&DeviceSettings::of(t), signal.level, &channel_id).is_none())
        .collect();
    if devices.is_empty() {
        return Ok(0);
    }
    let report = transport.send(&devices, &signal).await;
    tracing::info!(
        user = %user_id,
        held = count,
        delivered = report.delivered,
        failed = report.failed,
        "quiet-hours summary pushed"
    );
    Ok(report.delivered)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::quiet::{QuietHours, days_mask, parse_time};

    #[test]
    fn the_label_counts_and_says_nothing_else() {
        assert_eq!(label(1), "1 notification while quiet hours were on");
        assert_eq!(label(3), "3 notifications while quiet hours were on");
    }

    #[test]
    fn a_window_end_needs_quiet_hours_a_timezone_and_now_inside_it() {
        let at = |s: &str| DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc);
        let mut global = GlobalSettings {
            paused_until: None,
            quiet_hours: Some(QuietHours {
                start: parse_time("22:00").unwrap(),
                end: parse_time("07:00").unwrap(),
                days: days_mask(&[1]).unwrap(),
                allow: Level::Critical,
            }),
            timezone: Some("UTC".into()),
        };
        // Monday 23:00 UTC.
        let late = at("2026-10-19T23:00:00Z");
        assert_eq!(
            window_end(Some(&global), late),
            Some(at("2026-10-20T07:00:00Z"))
        );
        assert_eq!(window_end(Some(&global), at("2026-10-20T08:00:00Z")), None);
        assert_eq!(window_end(None, late), None);
        global.timezone = None;
        assert_eq!(window_end(Some(&global), late), None);
    }
}
