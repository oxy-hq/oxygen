//! `GET /admin/audit`, `/admin/assume/history`, `/admin/explorer/{threads,runs}` —
//! the read surfaces that carry a tenant's own content.

use axum::extract::{OriginalUri, Query};
use axum::http::{StatusCode, Uri};
use chrono::{Duration, Utc};
use entity::{admin_assume_sessions, threads};
use oxy_app::server::api::admin::assume::{HistoryQuery, history};
use oxy_app::server::api::admin::audit::{AuditQuery, list_audit};
use oxy_app::server::api::admin::explorer::{SearchQuery, search_runs, search_threads};
use oxy_app_core::audit::{self, AuditEntry};
use oxy_auth::types::AuthenticatedUser;
use sea_orm::{ActiveModelTrait, ActiveValue, DatabaseConnection};
use uuid::Uuid;

use super::fixture::{Reply, World, as_actor, reply, world};

fn uri(path: &'static str) -> OriginalUri {
    OriginalUri(Uri::from_static(path))
}

async fn audit_event(db: &DatabaseConnection, org: Option<Uuid>, label: &str) -> String {
    let mut entry =
        AuditEntry::new("someone@tenant.test", "staff_scope.probe").target("probe", label, label);
    if let Some(org) = org {
        entry = entry.org(org);
    }
    audit::record(db, entry)
        .await
        .expect("record audit event")
        .to_string()
}

async fn audit_as(actor: &AuthenticatedUser, org_id: Option<Uuid>, limit: Option<u64>) -> Reply {
    reply(
        list_audit(
            as_actor(actor),
            uri("/api/admin/audit"),
            Query(AuditQuery {
                action: Some("staff_scope.probe".into()),
                actor: None,
                org_id,
                outcome: None,
                q: None,
                limit,
                offset: None,
            }),
        )
        .await,
    )
    .await
}

/// The finding, on `list_audit`: a grant bounded to org A read org B's audit rows,
/// and the platform-level ones.
#[tokio::test]
async fn a_bounded_grant_reads_only_its_own_orgs_audit_events() {
    let w = world().await;
    let mine = audit_event(&w.db, Some(w.org_a), "a-1").await;
    let theirs = audit_event(&w.db, Some(w.org_b), "b-1").await;
    let platform = audit_event(&w.db, None, "platform-1").await;

    let listed = audit_as(&w.bounded, None, None).await;
    assert_eq!(listed.status, StatusCode::OK);
    let ids = listed.column(None, "id");
    assert_eq!(ids, vec![mine], "exactly org A's event, nothing else");
    assert!(!ids.contains(&theirs), "org B's audit row leaked");
    assert!(
        !ids.contains(&platform),
        "a platform-level audit row leaked"
    );

    // Asking for the other org by id is the same question with a filter on it.
    let asked = audit_as(&w.bounded, Some(w.org_b), None).await;
    assert_eq!(asked.status, StatusCode::OK);
    assert!(
        asked.column(None, "id").is_empty(),
        "`?org_id=<org B>` returned org B's trail to a grant bounded to org A"
    );
}

/// Filtered IN the query, before the paging — not trimmed out of a fetched page.
/// Org B's events are newer than org A's, so a post-filter would hand back an empty
/// first page with a `rel="next"`; the fix has to return a full page of A's.
#[tokio::test]
async fn a_bounded_grants_audit_pages_are_full_and_do_not_count_other_tenants() {
    let w = world().await;
    let mut mine = Vec::new();
    for i in 0..3 {
        mine.push(audit_event(&w.db, Some(w.org_a), &format!("a-{i}")).await);
    }
    for i in 0..4 {
        audit_event(&w.db, Some(w.org_b), &format!("b-{i}")).await;
    }

    let first = audit_as(&w.bounded, None, Some(2)).await;
    let ids = first.column(None, "id");
    assert_eq!(
        ids.len(),
        2,
        "a short page: the scope was applied after LIMIT"
    );
    assert!(ids.iter().all(|id| mine.contains(id)));
    assert!(first.has_next(), "three of org A's events, two shown");

    let all = audit_as(&w.bounded, None, Some(3)).await;
    assert_eq!(all.column(None, "id").len(), 3);
    assert!(
        !all.has_next(),
        "`rel=\"next\"` counted org B's events: the page claims more than the caller may see"
    );
}

/// Control: nothing changes for a grant that is not bounded.
#[tokio::test]
async fn unbounded_staff_read_every_audit_event_including_platform_level() {
    let w = world().await;
    let a = audit_event(&w.db, Some(w.org_a), "a-1").await;
    let b = audit_event(&w.db, Some(w.org_b), "b-1").await;
    let platform = audit_event(&w.db, None, "platform-1").await;

    for (who, actor) in w.everything_readers() {
        let ids = audit_as(actor, None, None).await.column(None, "id");
        for id in [&a, &b, &platform] {
            assert!(ids.contains(id), "{who} no longer reads audit event {id}");
        }
        assert_eq!(ids.len(), 3, "{who}");
    }
}

async fn assume_session(db: &DatabaseConnection, actor: &AuthenticatedUser, org: Uuid) -> String {
    let now = Utc::now().fixed_offset();
    let id = Uuid::new_v4();
    admin_assume_sessions::ActiveModel {
        id: ActiveValue::Set(id),
        actor_user_id: ActiveValue::Set(actor.id),
        actor_email: ActiveValue::Set(actor.email.clone().unwrap_or_default()),
        org_id: ActiveValue::Set(org),
        reason: ActiveValue::Set("staff-scope probe".into()),
        started_at: ActiveValue::Set(now - Duration::minutes(90)),
        expires_at: ActiveValue::Set(now - Duration::minutes(30)),
        ended_at: ActiveValue::Set(Some(now - Duration::minutes(60))),
    }
    .insert(db)
    .await
    .expect("seed assume session");
    id.to_string()
}

/// The impersonation log names the org that was entered. Found by the audit of the
/// rest of the console, not by the review.
#[tokio::test]
async fn a_bounded_grant_reads_only_assume_sessions_into_its_own_orgs() {
    let w = world().await;
    let mine = assume_session(&w.db, &w.unbounded, w.org_a).await;
    let theirs = assume_session(&w.db, &w.unbounded, w.org_b).await;
    let query = || Query(HistoryQuery::default());

    let listed = reply(
        history(
            as_actor(&w.bounded),
            uri("/api/admin/assume/history"),
            query(),
        )
        .await,
    )
    .await;
    assert_eq!(listed.status, StatusCode::OK);
    assert_eq!(
        listed.column(None, "id"),
        vec![mine.clone()],
        "a grant bounded to org A read who impersonated org B"
    );

    for (who, actor) in w.everything_readers() {
        let ids = reply(history(as_actor(actor), uri("/api/admin/assume/history"), query()).await)
            .await
            .column(None, "id");
        assert!(ids.contains(&mine) && ids.contains(&theirs), "{who}");
    }
}

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
        source: ActiveValue::Set("agents/x.agent.yml".into()),
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
