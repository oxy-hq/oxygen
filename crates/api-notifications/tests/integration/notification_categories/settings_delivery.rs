//! Delivery records the one decision's reason — a floor, a device switched
//! off, OS-denied or OS-disabled, a pause, quiet hours — with a push
//! transport registered for real (process-per-test makes the global safe),
//! and quiet hours' held pushes come back as one summary when the window
//! ends.

use std::sync::{Arc, Mutex};

use chrono::{Duration, Timelike, Utc};
use entity::{device_tokens, notification_deliveries, notifications};
use oxy_api_notifications::deliver::{Push, PushReport, PushSignal, set_push};
use oxy_api_notifications::delivery::deliver_send;
use oxy_api_notifications::devices::register::{Registration, upsert_device};
use oxy_api_notifications::devices::{self, DevicePatch};
use oxy_api_notifications::send::{AppRef, Outgoing, send};
use oxy_api_notifications::settings::patch::apply;
use oxy_api_notifications::settings::store;
use oxy_api_notifications::settings::summary::{Summary, summarize, window_end};
use oxy_api_notifications::settings::summary_task::run_id;
use oxy_auth::types::AuthenticatedUser;
use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseBackend, DatabaseConnection, EntityTrait, QueryFilter,
    Statement,
};
use serde_json::json;
use uuid::Uuid;

use super::fixture::{World, seed};
use crate::common::Schema;
use crate::oltp_fx::with_fx_on;

/// Takes every push, and remembers each banner and how many devices it went
/// to.
#[derive(Default)]
struct Recorder {
    seen: Mutex<Vec<(String, usize)>>,
}

#[async_trait::async_trait]
impl Push for Recorder {
    async fn send(&self, tokens: &[device_tokens::Model], signal: &PushSignal) -> PushReport {
        self.seen
            .lock()
            .unwrap()
            .push((signal.label.clone(), tokens.len()));
        PushReport {
            delivered: tokens.len(),
            failed: 0,
        }
    }
    fn name(&self) -> &'static str {
        "recorder"
    }
}

fn outgoing(org: Uuid, w: &World, kind: &str, slack: bool, to: Vec<Uuid>) -> Outgoing {
    Outgoing {
        org_id: org,
        app: Some(AppRef {
            id: w.app.id,
            slug: w.app.slug.clone(),
            name: w.app.name.clone(),
        }),
        environment: "production".into(),
        workspace_id: Some(w.workspace),
        kind: kind.into(),
        critical: false,
        title: "Heads up".into(),
        body: None,
        link: None,
        email: None,
        slack,
        idempotency_key: None,
        thread_key: None,
        recipients: to,
    }
}

/// Send, deliver, and the recorded reason per (recipient, channel).
async fn reasons(
    db: &DatabaseConnection,
    out: Outgoing,
) -> Vec<(Uuid, String, String, Option<String>)> {
    let sent = send(db, out).await.expect("send");
    let report = deliver_send(db, sent.id).await;
    assert_eq!(report.failed, 0, "{report:?}");
    let signals = notifications::Entity::find()
        .filter(notifications::Column::MessageId.eq(sent.id))
        .all(db)
        .await
        .unwrap();
    let mut out = Vec::new();
    for s in signals {
        for d in notification_deliveries::Entity::find()
            .filter(notification_deliveries::Column::NotificationId.eq(s.id))
            .all(db)
            .await
            .unwrap()
        {
            out.push((s.user_id, d.channel, d.status, d.reason));
        }
    }
    out
}

fn of(
    rows: &[(Uuid, String, String, Option<String>)],
    user: Uuid,
    channel: &str,
) -> (String, Option<String>) {
    let row = rows
        .iter()
        .find(|r| r.0 == user && r.1 == channel)
        .unwrap_or_else(|| panic!("no {channel} row"));
    (row.2.clone(), row.3.clone())
}

fn skipped(reason: &str) -> (String, Option<String>) {
    ("skipped".into(), Some(reason.into()))
}

fn sent() -> (String, Option<String>) {
    ("sent".into(), None)
}

async fn device(db: &DatabaseConnection, user: Uuid, token: &str) -> Uuid {
    let registration = Registration {
        platform: "web",
        token,
        name: None,
        app_version: None,
        keys: None,
    };
    upsert_device(db, user, &registration, Utc::now().fixed_offset())
        .await
        .expect("register")
}

async fn set_device(db: &DatabaseConnection, user: Uuid, id: Uuid, v: serde_json::Value) {
    let patch: DevicePatch = serde_json::from_value(v).unwrap();
    devices::update(db, user, id, patch)
        .await
        .expect("device patch");
}

async fn settings(db: &DatabaseConnection, user: &AuthenticatedUser, v: serde_json::Value) {
    apply(db, user, None, v.to_string().as_bytes())
        .await
        .expect("settings patch");
}

#[tokio::test]
async fn notification_categories_delivery_records_the_decided_reason() {
    with_fx_on(Schema::All, |fx| async move {
        fx.provisioner
            .provision(fx.org_id)
            .await
            .expect("provision");
        let recorder = Arc::new(Recorder::default());
        set_push(recorder.clone());
        let w = seed(&fx.db, fx.org_id).await;
        let (db, org) = (&fx.db, fx.org_id);
        let (me, other) = (w.member.id, w.other.id);
        device(db, me, "https://push.example/me").await;
        let theirs = device(db, other, "https://push.example/other").await;

        // A floor at a category; the other member's only device off.
        settings(
            db,
            &w.member,
            json!({ "nodes": [{ "app_id": w.app.id,
            "scope": "operations/receiving", "min_level": "time_sensitive" }] }),
        )
        .await;
        set_device(db, other, theirs, json!({ "enabled": false })).await;
        let rows = reasons(
            db,
            outgoing(org, &w, "delivery_late", false, vec![me, other]),
        )
        .await;
        assert_eq!(of(&rows, me, "push"), skipped("below_level"));
        assert_eq!(of(&rows, other, "push"), skipped("device_off"));

        // Blocked by the OS: everything, or this kind's channel only.
        set_device(
            db,
            other,
            theirs,
            json!({ "enabled": true, "os_permission": "denied" }),
        )
        .await;
        let rows = reasons(db, outgoing(org, &w, "stock_low", false, vec![other])).await;
        assert_eq!(of(&rows, other, "push"), skipped("os_denied"));
        let overdue = format!("{}.list_overdue", w.app.id);
        set_device(
            db,
            other,
            theirs,
            json!({ "os_permission": "granted", "os_disabled": [overdue] }),
        )
        .await;
        let rows = reasons(db, outgoing(org, &w, "list_overdue", false, vec![other])).await;
        assert_eq!(of(&rows, other, "push"), skipped("os_disabled"));
        let rows = reasons(db, outgoing(org, &w, "stock_low", false, vec![other])).await;
        assert_eq!(of(&rows, other, "push"), sent());

        // Paused: push and Slack held, critical still pushed.
        let pause = (Utc::now() + Duration::hours(1)).to_rfc3339();
        settings(
            db,
            &w.member,
            json!({ "global": { "paused_until": pause } }),
        )
        .await;
        let rows = reasons(db, outgoing(org, &w, "stock_low", true, vec![me])).await;
        assert_eq!(of(&rows, me, "push"), skipped("paused"));
        assert_eq!(of(&rows, me, "slack"), skipped("paused"));
        let rows = reasons(db, outgoing(org, &w, "temp_alarm", false, vec![me])).await;
        assert_eq!(of(&rows, me, "push"), sent(), "critical overrides a pause");
    })
    .await;
}

#[tokio::test]
async fn notification_categories_quiet_hours_hold_pushes_and_summarize_them_once() {
    with_fx_on(Schema::All, |fx| async move {
        fx.provisioner.provision(fx.org_id).await.expect("provision");
        let recorder = Arc::new(Recorder::default());
        set_push(recorder.clone());
        let w = seed(&fx.db, fx.org_id).await;
        let (db, org, me) = (&fx.db, fx.org_id, w.member.id);
        device(db, me, "https://push.example/me").await;

        // A window from an hour ago to an hour from now, UTC, every day.
        let hm = |t: chrono::DateTime<Utc>| format!("{:02}:{:02}", t.hour(), t.minute());
        let now = Utc::now();
        let quiet = json!({ "start": hm(now - Duration::hours(1)), "end": hm(now + Duration::hours(1)),
                            "days": [0, 1, 2, 3, 4, 5, 6], "allow": "critical" });
        settings(db, &w.member, json!({ "global": { "quiet_hours": quiet, "timezone": "UTC" } })).await;
        let rows = reasons(db, outgoing(org, &w, "stock_low", true, vec![me])).await;
        assert_eq!(of(&rows, me, "push"), skipped("quiet_hours"));
        assert_eq!(of(&rows, me, "slack"), skipped("quiet_hours"));
        let rows = reasons(db, outgoing(org, &w, "temp_alarm", false, vec![me])).await;
        assert_eq!(of(&rows, me, "push"), sent(), "allow: critical lets it through");

        // One summary is queued, due when the window ends.
        let (global, _) = store::person(db, me).await.unwrap();
        let due = window_end(Some(&global), Utc::now()).expect("inside the window");
        let queued = db
            .query_one_raw(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                "SELECT available_at FROM agentic_task_queue WHERE task_id = $1",
                [run_id(me, due).into()],
            ))
            .await
            .unwrap()
            .expect("the summary task");
        let available: chrono::DateTime<chrono::FixedOffset> =
            queued.try_get("", "available_at").unwrap();
        assert_eq!(available, due.fixed_offset(), "not claimable before the window ends");
        // A second hold in the same window queues nothing new.
        reasons(db, outgoing(org, &w, "list_overdue", false, vec![me])).await;
        let count = db
            .query_one_raw(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                "SELECT count(*) AS n FROM agentic_task_queue WHERE task_id LIKE $1",
                [format!("notification-quiet-summary-{me}-%").into()],
            ))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(count.try_get::<i64>("", "n").unwrap(), 1);

        // Still inside the window: nothing is told yet.
        assert_eq!(summarize(db, me, Utc::now()).await.unwrap(), Summary::StillQuiet(due));
        let pushed = || recorder.seen.lock().unwrap().clone();
        let before = pushed().len();
        // At the end: one push for both holds, and never again for them.
        let after = due + Duration::seconds(1);
        assert_eq!(
            summarize(db, me, after).await.unwrap(),
            Summary::Sent { count: 2, devices: 1 }
        );
        let seen = pushed();
        assert_eq!(seen.len(), before + 1);
        assert_eq!(seen.last().unwrap().0, "2 notifications while quiet hours were on");
        assert_eq!(summarize(db, me, after).await.unwrap(), Summary::Nothing);
    })
    .await;
}
