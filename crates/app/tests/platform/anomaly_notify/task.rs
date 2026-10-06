//! The edges of the feature: the request Slack receives, the task a scan
//! queues, and the executor that runs it.

use agentic_core::delegation::{TaskAssignment, TaskOutcome, TaskSpec};
use agentic_runtime::worker::TaskExecutor;
use chrono::Utc;
use entity::workspaces::WorkspaceStatus;
use entity::{organizations, workspaces};
use oxy_app::server::anomaly_notify::executor::{AnomalyNotifyExecutor, SlackChannel};
use oxy_app::server::anomaly_notify::{ANOMALY_NOTIFY_KIND, enqueue_if_due};
use oxy_metric_monitoring::notify::{Heading, announce, ledger};
use oxy_metric_monitoring::{NotifyConfig, Severity};
use oxy_slack_client::SlackClient;
use sea_orm::ActiveValue::Set;
use sea_orm::{ActiveModelTrait, DatabaseConnection, DbBackend, FromQueryResult, Statement};
use serde_json::{Value, json};
use uuid::Uuid;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::{CHANNEL, HEADING, Row, central_db, claims, due, seed};

fn slack_channel(server: &MockServer) -> SlackChannel {
    SlackChannel {
        client: SlackClient::with_base_url(server.uri()),
        bot_token: "xoxb-test".into(),
        channel: CHANNEL.into(),
    }
}

#[tokio::test]
async fn slack_receives_the_message_as_the_orgs_app() {
    let db = central_db().await;
    let workspace = Uuid::new_v4();
    seed(&db, Row::due(workspace, Uuid::new_v4())).await;
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat.postMessage"))
        .and(header("authorization", "Bearer xoxb-test"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "ok": true })))
        .expect(1)
        .mount(&server)
        .await;

    let heading = Heading {
        workspace_name: "Acme",
        inbox_url: Some("https://app.example.com/acme/workspaces/w/ide/semantic?view=anomalies"),
    };
    let announced = announce(&db, due(workspace), heading, &slack_channel(&server))
        .await
        .unwrap();

    assert_eq!(announced, 1);
    let requests = server.received_requests().await.expect("recorded requests");
    let body: Value = requests[0].body_json().expect("a JSON body");
    assert_eq!(body["channel"], CHANNEL);
    assert_eq!(body["text"], "1 new insight in Acme");
    let line = body["blocks"][1]["text"]["text"].as_str().unwrap();
    assert!(line.starts_with("🔴 *Net sales* · store="), "{line}");
    assert!(line.contains("23.4% below expected"), "{line}");
    let footer = body["blocks"][2]["elements"][0]["text"].as_str().unwrap();
    assert!(footer.contains("|Open the Insights Inbox>"), "{footer}");
    assert!(claims(&db, workspace).await[0].delivered);
}

/// Slack answers a refusal with HTTP 200 and `ok: false`. That has to fail the
/// delivery — a client that read the status code would record it as sent and
/// the insight would never be announced.
#[tokio::test]
async fn a_slack_refusal_fails_the_delivery_and_says_what_to_fix() {
    let db = central_db().await;
    let workspace = Uuid::new_v4();
    seed(&db, Row::due(workspace, Uuid::new_v4())).await;
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat.postMessage"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({ "ok": false, "error": "not_in_channel" })),
        )
        .mount(&server)
        .await;

    let err = announce(&db, due(workspace), HEADING, &slack_channel(&server))
        .await
        .expect_err("ok:false is a refusal");

    let said = err.to_string();
    assert!(
        said.contains(CHANNEL) && said.contains("invite it"),
        "{said}"
    );
    assert!(claims(&db, workspace).await.is_empty());
    assert!(ledger::anything_due(&db, due(workspace)).await.unwrap());
}

async fn seed_workspace(db: &DatabaseConnection) -> Uuid {
    let now = Utc::now().fixed_offset();
    let org_id = Uuid::new_v4();
    organizations::ActiveModel {
        id: Set(org_id),
        name: Set("insights-org".into()),
        slug: Set(format!("insights-{}", org_id.simple())),
        created_at: Set(now),
        updated_at: Set(now),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed org");
    let workspace_id = Uuid::new_v4();
    workspaces::ActiveModel {
        id: Set(workspace_id),
        name: Set("insights-ws".into()),
        created_at: Set(now),
        updated_at: Set(now),
        org_id: Set(Some(org_id)),
        status: Set(WorkspaceStatus::Ready),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed workspace");
    workspace_id
}

#[derive(Debug, FromQueryResult)]
struct Queued {
    task_id: String,
    run_id: String,
    source_type: Option<String>,
    payload: Value,
}

async fn queued(db: &DatabaseConnection) -> Vec<Queued> {
    Queued::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT q.task_id, q.run_id, r.source_type, q.spec->'payload' AS payload \
         FROM agentic_task_queue q JOIN agentic_runs r ON r.id = q.run_id \
         WHERE q.spec->>'kind' = $1",
        [ANOMALY_NOTIFY_KIND.into()],
    ))
    .all(db)
    .await
    .expect("read the queue")
}

fn notify_block() -> NotifyConfig {
    NotifyConfig {
        slack_channel: CHANNEL.into(),
        min_severity: Severity::High,
    }
}

/// A scan queues a delivery only when the file asked and there is news. A
/// quiet scan must leave no run behind, or every workspace grows a daily
/// "nothing new" row.
#[tokio::test]
async fn a_scan_queues_a_delivery_only_when_there_is_something_to_say() {
    let (db, _url) = crate::common::fresh_db(crate::common::Schema::All).await;
    let workspace = seed_workspace(&db).await;
    let block = notify_block();

    assert_eq!(enqueue_if_due(&db, workspace, Some(&block)).await, None);
    seed(&db, Row::due(workspace, Uuid::new_v4())).await;
    assert_eq!(
        enqueue_if_due(&db, workspace, None).await,
        None,
        "no `notify:` block"
    );
    assert!(queued(&db).await.is_empty());

    let run_id = enqueue_if_due(&db, workspace, Some(&block))
        .await
        .expect("a due event and a block queue a task");

    let tasks = queued(&db).await;
    assert_eq!(tasks.len(), 1, "{tasks:?}");
    assert_eq!(tasks[0].run_id, run_id);
    assert_eq!(tasks[0].task_id, run_id, "a root task carries its run's id");
    assert_eq!(tasks[0].source_type.as_deref(), Some(ANOMALY_NOTIFY_KIND));
    assert_eq!(
        tasks[0].payload,
        json!({
            "workspace_id": workspace,
            "notify": { "slack_channel": CHANNEL, "min_severity": "high" },
        })
    );
}

fn assignment(kind: &str, payload: Value) -> TaskAssignment {
    TaskAssignment {
        task_id: "t".into(),
        parent_task_id: None,
        run_id: "r".into(),
        spec: TaskSpec::Custom {
            kind: kind.into(),
            payload,
        },
        policy: None,
    }
}

#[tokio::test]
async fn the_executor_refuses_what_is_not_its_task() {
    let executor = AnomalyNotifyExecutor {
        db: central_db().await,
    };
    let payload = json!({ "workspace_id": Uuid::new_v4(), "notify": notify_block() });

    let wrong_kind = executor
        .execute(assignment("preagg_cycle", payload))
        .await
        .err()
        .expect("another kind is refused");
    assert!(wrong_kind.contains("preagg_cycle"), "{wrong_kind}");
    let bad_payload = executor
        .execute(assignment(
            ANOMALY_NOTIFY_KIND,
            json!({ "workspace_id": "nope" }),
        ))
        .await
        .err()
        .expect("an unreadable payload is refused");
    assert!(bad_payload.contains("payload"), "{bad_payload}");
}

/// Whatever goes wrong inside the task, the runtime is owed a terminal
/// outcome. A workspace deleted between the scan and the task is the cheapest
/// way to make it go wrong.
#[tokio::test]
async fn a_task_for_a_vanished_workspace_ends_failed_rather_than_hanging() {
    let executor = AnomalyNotifyExecutor {
        db: central_db().await,
    };
    let payload = json!({ "workspace_id": Uuid::new_v4(), "notify": notify_block() });

    let mut task = executor
        .execute(assignment(ANOMALY_NOTIFY_KIND, payload))
        .await
        .expect("a well-formed task starts");
    let outcome = tokio::time::timeout(std::time::Duration::from_secs(30), task.outcomes.recv())
        .await
        .expect("an outcome arrives")
        .expect("the channel carries one");

    assert!(
        matches!(&outcome, TaskOutcome::Failed(why) if why.contains("no longer exists")),
        "{outcome:?}"
    );
}
