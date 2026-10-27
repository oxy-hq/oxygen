//! `oxy.ask_waiting` against a real Oxy database: an Ask run whose thread
//! owner asked to hear about it tells them each time it stops to ask them
//! something — once per suspension, with the prompt — but not of a suspension
//! that was already showing when they asked, and never for a run that is not
//! theirs to hear about. A run that asks and then answers tells them both.

use agentic_runtime::lifecycle::notice_request::request_notice;
use entity::notifications;
use entity::workspaces::WorkspaceStatus;
use oxy_app::server::api::notifications::inbox_view;
use oxy_app::server::api::notifications::platform::ask_finished::{READY, notify_ask_finished};
use oxy_app::server::api::notifications::platform::ask_waiting::{TITLE, notify_ask_waiting};
use oxy_app::server::api::notifications::platform::kinds::{ASK_FINISHED, ASK_WAITING};
use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder,
    Statement,
};
use uuid::Uuid;

use super::ask_finished::{QUESTION, ask, seed_run, seed_thread};
use super::{people, seed_org, seed_workspace, told};
use crate::common::{Schema, fresh_db};

/// The run stops to ask `prompt`: its suspension row, as the coordinator's
/// upsert writes it, with the time it began.
async fn suspend(db: &DatabaseConnection, run: &str, prompt: &str) {
    db.execute_raw(Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Postgres,
        "INSERT INTO agentic_run_suspensions (run_id, prompt, suggestions, resume_data, created_at) \
         VALUES ($1, $2, '[]', '{}', clock_timestamp()) \
         ON CONFLICT (run_id) DO UPDATE SET prompt = EXCLUDED.prompt, \
           created_at = EXCLUDED.created_at",
        [run.into(), prompt.into()],
    ))
    .await
    .expect("suspend");
}

async fn set_status(db: &DatabaseConnection, run: &str, status: &str) {
    db.execute_raw(Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Postgres,
        "UPDATE agentic_runs SET task_status = $2 WHERE id = $1",
        [run.into(), status.into()],
    ))
    .await
    .expect("status");
}

/// Every `oxy.ask_waiting` notice about `run`, oldest first.
async fn waiting_notices(db: &DatabaseConnection, run: &str) -> Vec<notifications::Model> {
    notifications::Entity::find()
        .filter(notifications::Column::Kind.eq(ASK_WAITING))
        .filter(notifications::Column::SubjectId.contains(run))
        .order_by_asc(notifications::Column::CreatedAt)
        .all(db)
        .await
        .expect("query notices")
}

/// Asked first: one notice per suspension, with that suspension's prompt; the
/// same suspension seen twice tells once; and the answer that follows tells
/// them too.
#[tokio::test]
async fn platform_notices_ask_waiting_tells_the_asker_of_each_question_then_the_answer() {
    let (db, _url) = fresh_db(Schema::All).await;
    let org = seed_org(&db).await;
    let ws = seed_workspace(&db, org.id, "Sales", WorkspaceStatus::Ready, None).await;
    let thread = seed_thread(&db, ws, Some(org.member)).await;
    let run = seed_run(&db, ws, thread, "analytics", "running", false).await;
    ask(&db, &run).await;

    set_status(&db, &run, "awaiting_input").await;
    suspend(&db, &run, "Which region do you mean?\nAnd which quarter?").await;
    let sent = notify_ask_waiting(&db, &run).await.unwrap().expect("sent");
    assert_eq!(sent.recipients, 1);
    let again = notify_ask_waiting(&db, &run).await.unwrap().unwrap();
    assert_eq!(
        (again.recipients, again.already_told),
        (0, 1),
        "the same suspension observed twice is one notice"
    );
    let first = waiting_notices(&db, &run).await;
    assert_eq!(people(&first), vec![org.member], "the thread's owner");
    let item = &inbox_view::items(&db, first.clone()).await.expect("inbox")[0];
    assert_eq!(item.kind, "oxy.ask_waiting");
    assert_eq!(item.kind_label, "Needs your answer");
    assert_eq!(item.title.as_deref(), Some(TITLE));
    assert_eq!(item.body.as_deref(), Some("Which region do you mean?"));
    assert_eq!(
        item.href.as_deref(),
        Some(format!("/{}/workspaces/{ws}/threads/{thread}", org.slug).as_str())
    );
    let actions: Vec<_> = item
        .actions
        .iter()
        .map(|a| (a.id.as_str(), a.runs_function))
        .collect();
    assert_eq!(actions, vec![("open", false)]);

    // Answered, and it asks again.
    set_status(&db, &run, "running").await;
    set_status(&db, &run, "awaiting_input").await;
    suspend(&db, &run, "Gross or net revenue?").await;
    notify_ask_waiting(&db, &run)
        .await
        .unwrap()
        .expect("the second question is a second notice");
    let both = waiting_notices(&db, &run).await;
    assert_eq!(both.len(), 2, "one per suspension");
    let items = inbox_view::items(&db, both).await.expect("inbox");
    assert_eq!(items[1].body.as_deref(), Some("Gross or net revenue?"));
    assert_eq!(
        items[0].body.as_deref(),
        Some(QUESTION),
        "the earlier question is gone from the run: the notice falls back to the question"
    );

    // Then it answers.
    set_status(&db, &run, "done").await;
    notify_ask_finished(&db, &run)
        .await
        .unwrap()
        .expect("the answer is told too");
    let done = told(&db, ASK_FINISHED, &run).await;
    let item = &inbox_view::items(&db, done).await.expect("inbox")[0];
    assert_eq!(
        (item.title.as_deref(), item.kind_label.as_str()),
        (Some(READY), "Answer ready")
    );
}

/// The question was already showing when they asked: they have seen it. The
/// next one, and the end of the run, still tell them.
#[tokio::test]
async fn platform_notices_ask_waiting_skips_a_question_already_showing_when_asked() {
    let (db, _url) = fresh_db(Schema::All).await;
    let org = seed_org(&db).await;
    let ws = seed_workspace(&db, org.id, "Sales", WorkspaceStatus::Ready, None).await;
    let thread = seed_thread(&db, ws, Some(org.member)).await;
    let run = seed_run(&db, ws, thread, "analytics", "awaiting_input", false).await;
    suspend(&db, &run, "Which region do you mean?").await;

    assert!(request_notice(&db, &run).await.unwrap());
    assert!(
        notify_ask_waiting(&db, &run).await.unwrap().is_none(),
        "asked while the question was on screen"
    );
    assert!(waiting_notices(&db, &run).await.is_empty());

    set_status(&db, &run, "running").await;
    set_status(&db, &run, "awaiting_input").await;
    suspend(&db, &run, "Gross or net revenue?").await;
    notify_ask_waiting(&db, &run)
        .await
        .unwrap()
        .expect("a later question");
    assert_eq!(waiting_notices(&db, &run).await.len(), 1);
}

/// Unasked, a cancel, a child run, another domain's run and an unowned thread
/// tell nobody of a question.
#[tokio::test]
async fn platform_notices_ask_waiting_skips_runs_that_are_not_the_askers() {
    let (db, _url) = fresh_db(Schema::All).await;
    let org = seed_org(&db).await;
    let ws = seed_workspace(&db, org.id, "Sales", WorkspaceStatus::Ready, None).await;
    let thread = seed_thread(&db, ws, Some(org.member)).await;
    let unowned = seed_thread(&db, ws, None).await;
    let parent = seed_run(&db, ws, thread, "analytics", "delegating", false).await;

    struct Case {
        what: &'static str,
        thread: Uuid,
        source: &'static str,
        cancelled: bool,
        asked: bool,
        child: bool,
    }
    let base = |what| Case {
        what,
        thread,
        source: "analytics",
        cancelled: false,
        asked: true,
        child: false,
    };
    for case in [
        Case {
            asked: false,
            ..base("nobody asked")
        },
        Case {
            cancelled: true,
            ..base("a cancel")
        },
        Case {
            child: true,
            ..base("a child run")
        },
        Case {
            source: "builder",
            ..base("a Builder Agent run")
        },
        Case {
            thread: unowned,
            ..base("a thread nobody owns")
        },
    ] {
        let run = seed_run(&db, ws, case.thread, case.source, "running", case.cancelled).await;
        if case.child {
            db.execute_raw(Statement::from_sql_and_values(
                sea_orm::DatabaseBackend::Postgres,
                "UPDATE agentic_runs SET parent_run_id = $2 WHERE id = $1",
                [run.clone().into(), parent.clone().into()],
            ))
            .await
            .expect("child");
        }
        if case.asked {
            ask(&db, &run).await;
        }
        set_status(&db, &run, "awaiting_input").await;
        suspend(&db, &run, "Which region do you mean?").await;
        assert!(
            notify_ask_waiting(&db, &run).await.unwrap().is_none(),
            "{}",
            case.what
        );
        assert!(waiting_notices(&db, &run).await.is_empty(), "{}", case.what);
    }
}
