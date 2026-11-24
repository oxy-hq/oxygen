//! The staff console's Explorer (`/admin/explorer/{threads,runs}`) under a
//! bounded grant: a run's question and error, and a thread's title and input,
//! for every tenant. Moved from `oxy-app`'s
//! `tests/platform/admin_staff_scope/audit_and_explorer.rs` with the handlers
//! (admin split, PR 1); the fixture is that suite's, included by path so the two
//! cannot drift.

use axum::extract::Query;
use axum::http::StatusCode;
use entity::threads;
use oxy_api_threads::admin::explorer::{SearchQuery, search_runs, search_threads};
use sea_orm::{ActiveModelTrait, ActiveValue, DatabaseConnection};
use uuid::Uuid;

#[path = "../../../test-utils/harness/admin_staff_scope_fixture.rs"]
#[allow(dead_code)]
pub(crate) mod fixture;
use fixture::{World, as_actor, reply, world};

async fn thread(db: &DatabaseConnection, workspace: Uuid, title: &str) -> String {
    let id = Uuid::new_v4();
    threads::ActiveModel {
        id: ActiveValue::Set(id),
        user_id: ActiveValue::Set(None),
        created_at: ActiveValue::NotSet,
        title: ActiveValue::Set(title.to_string()),
        input: ActiveValue::Set(format!("question from {title}")),
        output: ActiveValue::Set(String::new()),
        source_type: ActiveValue::Set("agent".into()),
        source: ActiveValue::Set("agents/x.agentic.yml".into()),
        references: ActiveValue::Set("[]".into()),
        is_processing: ActiveValue::Set(false),
        project_id: ActiveValue::Set(workspace),
        sandbox_info: ActiveValue::Set(None),
    }
    .insert(db)
    .await
    .expect("seed thread");
    id.to_string()
}

async fn run(db: &DatabaseConnection, workspace: Uuid, question: &str) -> String {
    let id = format!("run-{}", Uuid::new_v4());
    agentic_runtime::crud::runs::insert_run(db, &id, question, None, "analytics", None, workspace)
        .await
        .expect("seed run");
    id
}

struct Explorer {
    thread_a: String,
    thread_b: String,
    thread_orphan: String,
    run_a: String,
    run_b: String,
    run_orphan: String,
}

async fn seed_explorer(w: &World) -> Explorer {
    Explorer {
        thread_a: thread(&w.db, w.ws_a, "thread-a").await,
        thread_b: thread(&w.db, w.ws_b, "thread-b").await,
        thread_orphan: thread(&w.db, w.ws_orphan, "thread-orphan").await,
        run_a: run(&w.db, w.ws_a, "why did org a fail").await,
        run_b: run(&w.db, w.ws_b, "why did org b fail").await,
        run_orphan: run(&w.db, w.ws_orphan, "why did the orphan fail").await,
    }
}

fn search(org: Option<Uuid>, term: Option<&str>) -> Query<SearchQuery> {
    Query(SearchQuery {
        org_id: org.map(|o| o.to_string()),
        search: term.map(str::to_string),
        ..SearchQuery::default()
    })
}

/// The finding, on the explorer: a run's question and error, and a thread's title
/// and input, for every tenant.
#[tokio::test]
async fn a_bounded_grant_searches_only_its_own_orgs_threads_and_runs() {
    let w = world().await;
    let x = seed_explorer(&w).await;

    let threads = reply(search_threads(as_actor(&w.bounded), search(None, None)).await).await;
    assert_eq!(threads.status, StatusCode::OK);
    assert_eq!(
        threads.column(Some("items"), "id"),
        vec![x.thread_a.clone()],
        "org B's (or an org-less workspace's) thread leaked"
    );
    assert_eq!(
        threads.body["total"], 1,
        "the total counted threads the caller cannot see"
    );

    let runs = reply(search_runs(as_actor(&w.bounded), search(None, None)).await).await;
    assert_eq!(runs.status, StatusCode::OK);
    assert_eq!(
        runs.column(Some("items"), "id"),
        vec![x.run_a.clone()],
        "org B's (or an org-less workspace's) run leaked"
    );
    assert_eq!(runs.body["total"], 1);
}

/// "Requests org B's row by id": the explorer's by-id read is its search box, which
/// matches a thread or run id exactly — and `?org_id=` is the org-360 Activity tab.
#[tokio::test]
async fn a_bounded_grant_cannot_reach_another_orgs_thread_or_run_by_id_or_org_filter() {
    let w = world().await;
    let x = seed_explorer(&w).await;

    for term in [&x.thread_b, &x.thread_orphan] {
        let found =
            reply(search_threads(as_actor(&w.bounded), search(None, Some(term))).await).await;
        assert_eq!(found.body["total"], 0, "thread {term} found by id");
        assert!(found.column(Some("items"), "id").is_empty());
    }
    for term in [&x.run_b, &x.run_orphan] {
        let found = reply(search_runs(as_actor(&w.bounded), search(None, Some(term))).await).await;
        assert_eq!(found.body["total"], 0, "run {term} found by id");
        assert!(found.column(Some("items"), "id").is_empty());
    }

    let by_org = reply(search_runs(as_actor(&w.bounded), search(Some(w.org_b), None)).await).await;
    assert_eq!(by_org.status, StatusCode::OK);
    assert_eq!(
        by_org.body["total"], 0,
        "`?org_id=<org B>` listed org B's runs to a grant bounded to org A"
    );
    let by_org =
        reply(search_threads(as_actor(&w.bounded), search(Some(w.org_b), None)).await).await;
    assert_eq!(by_org.body["total"], 0);
}

/// Control: the cross-tenant explorer is unchanged for unbounded staff, org-less
/// rows included.
#[tokio::test]
async fn unbounded_staff_search_every_tenants_threads_and_runs() {
    let w = world().await;
    let x = seed_explorer(&w).await;

    for (who, actor) in w.everything_readers() {
        let threads = reply(search_threads(as_actor(actor), search(None, None)).await).await;
        let ids = threads.column(Some("items"), "id");
        for id in [&x.thread_a, &x.thread_b, &x.thread_orphan] {
            assert!(ids.contains(id), "{who} no longer finds thread {id}");
        }
        assert_eq!(threads.body["total"], 3, "{who}");

        let runs = reply(search_runs(as_actor(actor), search(None, None)).await).await;
        let ids = runs.column(Some("items"), "id");
        for id in [&x.run_a, &x.run_b, &x.run_orphan] {
            assert!(ids.contains(id), "{who} no longer finds run {id}");
        }
        assert_eq!(runs.body["total"], 3, "{who}");

        let one = reply(search_runs(as_actor(actor), search(Some(w.org_b), None)).await).await;
        assert_eq!(
            one.column(Some("items"), "id"),
            vec![x.run_b.clone()],
            "{who}"
        );
    }
}
