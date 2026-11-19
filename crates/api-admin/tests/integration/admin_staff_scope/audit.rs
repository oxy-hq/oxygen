//! `GET /admin/audit` — the platform audit log, which carries a tenant's own
//! trail. Moved with the handlers from `oxy-app`'s
//! `tests/platform/admin_staff_scope/audit_and_assume.rs` (admin split,
//! 2026-11-20); the `/admin/assume/history` half stayed there, as `assume.rs`.

use axum::extract::{OriginalUri, Query};
use axum::http::{StatusCode, Uri};
use oxy_api_admin::audit::{AuditQuery, list_audit};
use oxy_app_core::audit::{self, AuditEntry};
use oxy_auth::types::AuthenticatedUser;
use sea_orm::DatabaseConnection;
use uuid::Uuid;

use super::fixture::{Reply, as_actor, reply, world};

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
                token_id: None,
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
