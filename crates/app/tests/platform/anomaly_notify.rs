//! Insights delivery against a real Postgres: what is due, the once-per-event
//! claim, and what reaches Slack.
//!
//! The rule being pinned is "an insight is announced once, to its own
//! workspace's channel, and only if the file asked for it". Every clause of
//! that is a `WHERE` in `oxy_metric_monitoring::notify::ledger`, and a `WHERE`
//! is exactly what a unit test cannot see — so each one gets a row here that
//! only that clause keeps out, beside a control that must come through.
//!
//! Slack is the one external boundary. Most cases hand `announce` a recorder;
//! two drive the real `SlackChannel` at a mock server, so the request Slack
//! would receive is asserted rather than assumed.
//!
//! Own database per test through `common::fresh_db`, so this sits in
//! `db-per-test` with the rest of the group.

use std::sync::Mutex;
use std::sync::atomic::{AtomicI64, Ordering};

use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use entity::metric_anomalies;
use oxy_metric_monitoring::Severity;
use oxy_metric_monitoring::notify::{
    AnnounceError, Destination, Due, Heading, SlackMessage, announce, ledger,
};
use sea_orm::ActiveValue::Set;
use sea_orm::{
    ConnectionTrait, DatabaseConnection, DbBackend, EntityTrait, FromQueryResult, Statement,
};
use uuid::Uuid;

mod due_rules;
mod task;

const CHANNEL: &str = "C0123ABCDEF";
const HEADING: Heading<'static> = Heading {
    workspace_name: "Acme",
    inbox_url: None,
};

/// One bucket. [`Row::due`] is a bucket that should be announced; each test
/// changes the one field its clause reads.
struct Row {
    workspace: Uuid,
    event: Option<Uuid>,
    severity: &'static str,
    status: &'static str,
    detected_ago: Duration,
    period_ended_ago: Duration,
}

impl Row {
    fn due(workspace: Uuid, event: Uuid) -> Self {
        Row {
            workspace,
            event: Some(event),
            severity: "high",
            status: "new",
            detected_ago: Duration::hours(1),
            period_ended_ago: Duration::days(1),
        }
    }
}

/// Keeps seeded rows clear of the table's unique index, which every row here
/// would otherwise collide on: same workspace, measure, grain and often period.
static SEEDED: AtomicI64 = AtomicI64::new(0);

async fn seed(db: &DatabaseConnection, row: Row) {
    let now = Utc::now();
    let period_end = now - row.period_ended_ago;
    let nth = SEEDED.fetch_add(1, Ordering::Relaxed);
    metric_anomalies::Entity::insert(metric_anomalies::ActiveModel {
        id: Set(Uuid::new_v4()),
        workspace_id: Set(row.workspace),
        measure: Set("sales.net".into()),
        time_dimension: Set("sales.day".into()),
        granularity: Set("day".into()),
        period_start: Set((period_end - Duration::days(1)).into()),
        period_end: Set(period_end.into()),
        observed: Set(1204.0),
        expected: Set(1571.0),
        lower_bound: Set(1400.0),
        upper_bound: Set(1700.0),
        z_score: Set(-4.0),
        severity: Set(row.severity.into()),
        status: Set(row.status.into()),
        label: Set(Some("Net sales".into())),
        dimension_key: Set(format!("sales.store={nth}")),
        event_id: Set(row.event),
        detected_at: Set((now - row.detected_ago).into()),
        updated_at: Set(now.into()),
        ..Default::default()
    })
    .exec(db)
    .await
    .expect("seed anomaly");
}

async fn central_db() -> DatabaseConnection {
    crate::common::fresh_db(crate::common::Schema::Central)
        .await
        .0
}

fn due_at(workspace_id: Uuid, min_severity: Severity, now: DateTime<Utc>) -> Due {
    Due {
        workspace_id,
        min_severity,
        now,
    }
}

fn due(workspace_id: Uuid) -> Due {
    due_at(workspace_id, Severity::High, Utc::now())
}

/// The ledger as a test wants to read it.
#[derive(Debug, FromQueryResult)]
struct Claim {
    event_id: Uuid,
    destination: String,
    delivered: bool,
}

async fn claims(db: &DatabaseConnection, workspace_id: Uuid) -> Vec<Claim> {
    Claim::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT event_id, destination, delivered_at IS NOT NULL AS delivered \
         FROM metric_anomaly_notifications WHERE workspace_id = $1 ORDER BY claimed_at",
        [workspace_id.into()],
    ))
    .all(db)
    .await
    .expect("read ledger")
}

/// A destination that keeps what it was handed, and can be told to refuse.
#[derive(Default)]
struct Recorder {
    refuse: Option<&'static str>,
    posts: Mutex<Vec<SlackMessage>>,
}

impl Recorder {
    fn posts(&self) -> Vec<SlackMessage> {
        self.posts.lock().unwrap().clone()
    }
}

#[async_trait]
impl Destination for Recorder {
    fn id(&self) -> &str {
        CHANNEL
    }

    async fn post(&self, message: &SlackMessage) -> Result<(), String> {
        self.posts.lock().unwrap().push(message.clone());
        match self.refuse {
            Some(why) => Err(why.to_string()),
            None => Ok(()),
        }
    }
}

#[tokio::test]
async fn an_event_is_announced_once_and_only_to_its_own_workspace() {
    let db = central_db().await;
    let (mine, theirs) = (Uuid::new_v4(), Uuid::new_v4());
    let (event, foreign) = (Uuid::new_v4(), Uuid::new_v4());
    seed(&db, Row::due(mine, event)).await;
    seed(&db, Row::due(theirs, foreign)).await;

    let slack = Recorder::default();
    let announced = announce(&db, due(mine), HEADING, &slack).await.unwrap();

    assert_eq!(announced, 1);
    let posts = slack.posts();
    assert_eq!(posts.len(), 1);
    assert_eq!(posts[0].text, "1 new insight in Acme");
    let ledger_rows = claims(&db, mine).await;
    assert_eq!(ledger_rows.len(), 1, "{ledger_rows:?}");
    assert_eq!(ledger_rows[0].event_id, event);
    assert_eq!(ledger_rows[0].destination, CHANNEL);
    assert!(ledger_rows[0].delivered);
    assert!(
        claims(&db, theirs).await.is_empty(),
        "another workspace's event must not be claimed"
    );
    assert!(
        ledger::anything_due(&db, due(theirs)).await.unwrap(),
        "and it is still theirs to announce"
    );

    // The same scan window is re-scored tomorrow, and the event grows a bucket.
    seed(&db, Row::due(mine, event)).await;
    assert!(!ledger::anything_due(&db, due(mine)).await.unwrap());
    assert_eq!(announce(&db, due(mine), HEADING, &slack).await.unwrap(), 0);
    assert_eq!(
        slack.posts().len(),
        1,
        "an announced event is not posted again"
    );
}

/// A post Slack refused must cost nothing: no row is left claiming the event,
/// so the next scan announces it without waiting out a grace period.
#[tokio::test]
async fn a_refused_post_leaves_the_event_due() {
    let db = central_db().await;
    let workspace = Uuid::new_v4();
    seed(&db, Row::due(workspace, Uuid::new_v4())).await;

    let refusing = Recorder {
        refuse: Some("channel_not_found"),
        ..Default::default()
    };
    let err = announce(&db, due(workspace), HEADING, &refusing)
        .await
        .expect_err("a refused post is a failed delivery");
    assert!(
        matches!(&err, AnnounceError::Refused(why) if why == "channel_not_found"),
        "{err:?}"
    );
    assert!(
        claims(&db, workspace).await.is_empty(),
        "the claim is released"
    );

    let slack = Recorder::default();
    assert_eq!(
        announce(&db, due(workspace), HEADING, &slack)
            .await
            .unwrap(),
        1
    );
    assert_eq!(slack.posts().len(), 1);
}

/// A ledger row written long ago, delivered, for `event` in `workspace`.
async fn seed_old_delivery(db: &DatabaseConnection, workspace: Uuid, event: Uuid) {
    let old = Utc::now() - Duration::days(120);
    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO metric_anomaly_notifications \
           (workspace_id, event_id, channel, destination, claim_id, claimed_at, delivered_at) \
         VALUES ($1, $2, 'slack', $3, $4, $5, $5)",
        [
            workspace.into(),
            event.into(),
            CHANNEL.into(),
            Uuid::new_v4().into(),
            old.fixed_offset().into(),
        ],
    ))
    .await
    .expect("seed an old ledger row");
}

/// The ledger is bounded by the inbox, not by time: a row goes when its event
/// has no bucket left, and only in the workspace the task is running for.
#[tokio::test]
async fn a_delivery_drops_the_rows_of_events_that_are_gone() {
    let db = central_db().await;
    let (mine, theirs) = (Uuid::new_v4(), Uuid::new_v4());
    seed_old_delivery(&db, mine, Uuid::new_v4()).await;
    seed_old_delivery(&db, theirs, Uuid::new_v4()).await;
    let event = Uuid::new_v4();
    seed(&db, Row::due(mine, event)).await;

    announce(&db, due(mine), HEADING, &Recorder::default())
        .await
        .unwrap();

    let kept: Vec<Uuid> = claims(&db, mine).await.iter().map(|c| c.event_id).collect();
    assert_eq!(
        kept,
        [event],
        "only the row of an event that still exists is left"
    );
    assert_eq!(
        claims(&db, theirs).await.len(),
        1,
        "another workspace's rows are not this task's"
    );
}

/// A slide on a weekly or monthly monitor gains a bucket for months. Its
/// ledger row is the only record that it was announced, so the row has to
/// outlive any age limit: four months on, a newly detected bucket of the same
/// event is still not news.
#[tokio::test]
async fn an_event_still_gaining_buckets_months_later_is_not_announced_again() {
    let db = central_db().await;
    let workspace = Uuid::new_v4();
    let (slide, fresh) = (Uuid::new_v4(), Uuid::new_v4());
    seed(
        &db,
        Row {
            detected_ago: Duration::days(120),
            period_ended_ago: Duration::days(120),
            ..Row::due(workspace, slide)
        },
    )
    .await;
    seed_old_delivery(&db, workspace, slide).await;
    // Something else is due, so a delivery runs and has its chance to prune.
    seed(&db, Row::due(workspace, fresh)).await;
    let slack = Recorder::default();
    assert_eq!(
        announce(&db, due(workspace), HEADING, &slack)
            .await
            .unwrap(),
        1
    );

    // The slide's newest bucket, detected today.
    seed(&db, Row::due(workspace, slide)).await;

    assert!(!ledger::anything_due(&db, due(workspace)).await.unwrap());
    assert_eq!(
        announce(&db, due(workspace), HEADING, &slack)
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        slack.posts().len(),
        1,
        "only the unrelated event was ever posted"
    );
    assert_eq!(claims(&db, workspace).await.len(), 2);
}
