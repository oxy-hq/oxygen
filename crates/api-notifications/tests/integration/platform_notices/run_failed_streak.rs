//! `oxy.run_failed` once per failing streak, then daily: a pipeline failing
//! every hour tells its people on the first failure, stays quiet, and reminds
//! them a day later with how long it has been failing. A success ends the
//! streak; a cancel neither ends nor starts one; each source has its own.

use chrono::{DateTime, Duration, SubsecRound, Utc};
use entity::workspaces::WorkspaceStatus;
use oxy_api_notifications::inbox_view;
use oxy_api_notifications::platform::kinds::RUN_FAILED;
use oxy_api_notifications::platform::run_failed::notify_run_failed;
use sea_orm::{ConnectionTrait, DatabaseConnection, Statement};
use serde_json::json;
use uuid::Uuid;

use super::{people, seed_org, seed_workspace, sorted, told};
use crate::common::{Schema, fresh_db};

const PIPELINE: &str = "pipelines/restaurant_analytics.airway.yml";
const AUTOMATION: &str = "automations/nightly.automation.yml";

/// How a seeded run ended.
#[derive(Clone, Copy)]
enum Ended {
    Done,
    Failed,
    /// Written `failed`, as several cancel paths do, with the cancel flag.
    Cancelled,
}

/// A root run of `file` in `ws`, created `at`. Neither started by a person
/// nor scheduled, so the workspace's admins are told: here the org's owners
/// and admin.
async fn seed(
    db: &DatabaseConnection,
    ws: Uuid,
    file: &str,
    ended: Ended,
    at: DateTime<Utc>,
) -> String {
    let (source, metadata) = match file.ends_with(".airway.yml") {
        true => (
            "airway",
            json!({ "pipeline_ref": file, "pipeline_name": "restaurant_analytics" }),
        ),
        false => ("workflow", json!({ "workflow_ref": file })),
    };
    let (status, error, cancelled) = match ended {
        Ended::Done => ("done", None, false),
        Ended::Failed => ("failed", Some("step failed"), false),
        Ended::Cancelled => ("failed", Some("step failed"), true),
    };
    let id = Uuid::new_v4().to_string();
    db.execute_raw(Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Postgres,
        "INSERT INTO agentic_runs (id, question, source_type, metadata, task_status, \
           error_message, workspace_id, cancel_requested_at, attempt, created_at, updated_at) \
         VALUES ($1, 'q', $2, $3, $4, $5, $6, CASE WHEN $7 THEN $8 END, 0, $8, $8)",
        [
            id.clone().into(),
            source.into(),
            metadata.into(),
            status.into(),
            error.map(str::to_string).into(),
            ws.into(),
            cancelled.into(),
            at.fixed_offset().into(),
        ],
    ))
    .await
    .expect("seed run");
    id
}

/// How many people `run`'s failure told now.
async fn tells(db: &DatabaseConnection, run: &str) -> usize {
    notify_run_failed(db, run)
        .await
        .expect("notify")
        .map(|s| s.recipients)
        .unwrap_or(0)
}

/// Move every notice about `run` `hours` into the past.
async fn age_notices(db: &DatabaseConnection, run: &str, hours: i32) {
    db.execute_raw(Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Postgres,
        "UPDATE notifications SET created_at = created_at - make_interval(hours => $2) \
         WHERE subject_id = $1",
        [run.into(), hours.into()],
    ))
    .await
    .expect("age notices");
}

/// The words `run`'s notice reads as in the inbox.
async fn words(db: &DatabaseConnection, run: &str) -> (String, String) {
    let rows = told(db, RUN_FAILED, run).await;
    let items = inbox_view::items(db, rows).await.expect("inbox");
    let item = items.first().expect("a notice");
    (
        item.title.clone().unwrap_or_default(),
        item.body.clone().unwrap_or_default(),
    )
}

/// Whole seconds, `days` ago: a Postgres round trip keeps microseconds only.
fn days_ago(days: i64) -> DateTime<Utc> {
    (Utc::now() - Duration::days(days)).trunc_subsecs(0)
}

/// The first failure after a success tells; the next two do not; one after the
/// last notice is a day old reminds, counting the streak from its start; the
/// one after that is quiet again.
#[tokio::test]
async fn platform_notices_run_failed_tells_once_per_streak_then_daily() {
    let (db, _url) = fresh_db(Schema::All).await;
    let org = seed_org(&db).await;
    let ws = seed_workspace(&db, org.id, "Ops", WorkspaceStatus::Ready, None).await;
    let start = days_ago(2);
    let hour = |n: i64| start + Duration::hours(n);
    seed(&db, ws, PIPELINE, Ended::Done, hour(-1)).await;

    let first = seed(&db, ws, PIPELINE, Ended::Failed, hour(0)).await;
    assert_eq!(tells(&db, &first).await, 3, "the first failure tells");
    let admins = sorted(vec![org.owner, org.second_owner, org.admin]);
    assert_eq!(people(&told(&db, RUN_FAILED, &first).await), admins);
    for n in 1..=2 {
        let again = seed(&db, ws, PIPELINE, Ended::Failed, hour(n)).await;
        assert_eq!(tells(&db, &again).await, 0, "failure {} is quiet", n + 1);
    }

    age_notices(&db, &first, 25).await;
    let reminder = seed(&db, ws, PIPELINE, Ended::Failed, hour(3)).await;
    assert_eq!(tells(&db, &reminder).await, 3, "a day later: one reminder");
    let (title, body) = words(&db, &reminder).await;
    assert_eq!(title, "restaurant_analytics is still failing");
    assert_eq!(
        body,
        format!(
            "It has failed 4 times since {}. Open the latest run to see why.",
            start.format("%B %-d at %H:%M UTC")
        )
    );
    // One page holding both: each counts its streak only through its own run.
    let mut page = told(&db, RUN_FAILED, &first).await;
    page.truncate(1);
    page.extend(told(&db, RUN_FAILED, &reminder).await.into_iter().take(1));
    let mut titles: Vec<String> = inbox_view::items(&db, page)
        .await
        .expect("inbox")
        .into_iter()
        .filter_map(|i| i.title)
        .collect();
    titles.sort();
    assert_eq!(
        titles,
        vec![
            "restaurant_analytics failed".to_string(),
            "restaurant_analytics is still failing".to_string(),
        ],
        "the first reads as one failure, the reminder as the streak"
    );

    let next = seed(&db, ws, PIPELINE, Ended::Failed, hour(4)).await;
    assert_eq!(tells(&db, &next).await, 0, "quiet again after the reminder");
}

/// A success ends the streak, so the next failure tells again and reads as a
/// first failure. A cancel neither breaks a streak nor starts one.
#[tokio::test]
async fn platform_notices_run_failed_streak_ends_on_success_not_on_cancel() {
    let (db, _url) = fresh_db(Schema::All).await;
    let org = seed_org(&db).await;
    let ws = seed_workspace(&db, org.id, "Ops", WorkspaceStatus::Ready, None).await;
    let start = days_ago(1);
    let hour = |n: i64| start + Duration::hours(n);

    let first = seed(&db, ws, PIPELINE, Ended::Failed, hour(0)).await;
    assert_eq!(tells(&db, &first).await, 3, "no history: the first failure");
    let cancel = seed(&db, ws, PIPELINE, Ended::Cancelled, hour(1)).await;
    assert_eq!(tells(&db, &cancel).await, 0, "a cancel tells nobody");
    let second = seed(&db, ws, PIPELINE, Ended::Failed, hour(2)).await;
    assert_eq!(tells(&db, &second).await, 0, "a cancel does not break it");

    seed(&db, ws, PIPELINE, Ended::Done, hour(3)).await;
    let cancel = seed(&db, ws, PIPELINE, Ended::Cancelled, hour(4)).await;
    assert_eq!(tells(&db, &cancel).await, 0, "a cancel does not start one");
    let after = seed(&db, ws, PIPELINE, Ended::Failed, hour(5)).await;
    assert_eq!(tells(&db, &after).await, 3, "a success ended the streak");
    let (title, body) = words(&db, &after).await;
    assert_eq!(title, "restaurant_analytics failed");
    assert!(body.starts_with("The run that started "), "{body}");
}

/// Each source has its own streak: another pipeline, an automation, and the
/// same file in another workspace each tell on their first failure.
#[tokio::test]
async fn platform_notices_run_failed_streaks_are_per_source() {
    let (db, _url) = fresh_db(Schema::All).await;
    let org = seed_org(&db).await;
    let ws = seed_workspace(&db, org.id, "Ops", WorkspaceStatus::Ready, None).await;
    let other_ws = seed_workspace(&db, org.id, "Sales", WorkspaceStatus::Ready, None).await;
    let start = days_ago(1);
    let hour = |n: i64| start + Duration::hours(n);
    let sources = [
        (ws, PIPELINE),
        (ws, "pipelines/toast.airway.yml"),
        (ws, AUTOMATION),
        (other_ws, PIPELINE),
    ];

    for (ws, file) in sources {
        let run = seed(&db, ws, file, Ended::Failed, hour(0)).await;
        assert_eq!(tells(&db, &run).await, 3, "{file} in {ws}: its first");
    }
    for (ws, file) in sources {
        let run = seed(&db, ws, file, Ended::Failed, hour(1)).await;
        assert_eq!(tells(&db, &run).await, 0, "{file} in {ws}: its second");
    }
}

/// Two failures of one source decided out of order, or at once on two pods,
/// tell each person once: the decision reads the whole streak under the
/// source's lock.
#[tokio::test]
async fn platform_notices_run_failed_two_failures_at_once_tell_once() {
    let (db, _url) = fresh_db(Schema::All).await;
    let org = seed_org(&db).await;
    let ws = seed_workspace(&db, org.id, "Ops", WorkspaceStatus::Ready, None).await;
    let start = days_ago(1);
    let hour = |n: i64| start + Duration::hours(n);

    let earlier = seed(&db, ws, PIPELINE, Ended::Failed, hour(0)).await;
    let later = seed(&db, ws, PIPELINE, Ended::Failed, hour(1)).await;
    assert_eq!(tells(&db, &later).await, 3, "the later one decided first");
    assert_eq!(
        tells(&db, &earlier).await,
        0,
        "the earlier one is the same streak"
    );

    let a = seed(&db, ws, AUTOMATION, Ended::Failed, hour(0)).await;
    let b = seed(&db, ws, AUTOMATION, Ended::Failed, hour(0)).await;
    let (x, y) = tokio::join!(tells(&db, &a), tells(&db, &b));
    assert_eq!(x + y, 3, "one of them told, once each");
}
