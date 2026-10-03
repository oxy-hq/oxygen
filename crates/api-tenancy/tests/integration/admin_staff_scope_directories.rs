//! The staff console's org and workspace directories (`/admin/orgs-meta`,
//! `/admin/workspaces-meta`) under a bounded grant. Moved from `oxy-app`'s
//! `tests/platform/admin_staff_scope/directories.rs` with the handlers; the
//! fixture is that suite's, included by path so the two cannot drift.

use axum::extract::{OriginalUri, Query};
use axum::http::{StatusCode, Uri};
use oxy_api_tenancy::admin::orgs::{ListMetaQuery, list_orgs_meta};
use oxy_api_tenancy::admin::workspaces::{ListWorkspacesQuery, list_workspaces};
use oxy_auth::types::AuthenticatedUser;
use uuid::Uuid;

#[path = "../../../app/tests/platform/admin_staff_scope/fixture.rs"]
#[allow(dead_code)]
mod fixture;
use fixture::{Reply, as_actor, reply, world};

fn uri(path: &'static str) -> OriginalUri {
    OriginalUri(Uri::from_static(path))
}

async fn orgs_as(actor: &AuthenticatedUser, page_size: Option<u64>) -> Reply {
    reply(
        list_orgs_meta(
            as_actor(actor),
            uri("/api/admin/orgs-meta"),
            Query(ListMetaQuery {
                search: None,
                page: None,
                page_size,
            }),
        )
        .await,
    )
    .await
}

async fn workspaces_as(actor: &AuthenticatedUser, org_id: Option<Uuid>) -> Reply {
    reply(
        list_workspaces(
            as_actor(actor),
            uri("/api/admin/workspaces-meta"),
            Query(ListWorkspacesQuery {
                search: None,
                status: None,
                org_id,
                page: None,
                page_size: None,
            }),
        )
        .await,
    )
    .await
}

/// The directory that undid the 404 policy: every by-id route on these routers
/// already refused an out-of-scope org, and the listing named all of them.
#[tokio::test]
async fn a_bounded_grant_lists_only_its_own_orgs_and_their_workspaces() {
    let w = world().await;

    let orgs = orgs_as(&w.bounded, None).await;
    assert_eq!(orgs.status, StatusCode::OK);
    assert_eq!(orgs.column(None, "id"), vec![w.org_a.to_string()]);
    // Org A sorts first by name, so a page of one shows it either way — what the
    // scope must change is whether the response claims a second page.
    let first = orgs_as(&w.bounded, Some(1)).await;
    assert_eq!(first.column(None, "id"), vec![w.org_a.to_string()]);
    assert!(
        !first.has_next(),
        "`rel=\"next\"` tells a bounded grant another org exists"
    );

    let workspaces = workspaces_as(&w.bounded, None).await;
    assert_eq!(workspaces.status, StatusCode::OK);
    assert_eq!(
        workspaces.column(None, "id"),
        vec![w.ws_a.to_string()],
        "org B's workspace, or the org-less one, is listed"
    );
    assert!(
        workspaces_as(&w.bounded, Some(w.org_b))
            .await
            .column(None, "id")
            .is_empty(),
        "`?org_id=<org B>` listed org B's workspaces"
    );

    for (who, actor) in w.everything_readers() {
        let orgs = orgs_as(actor, None).await.column(None, "id");
        assert!(orgs.contains(&w.org_a.to_string()) && orgs.contains(&w.org_b.to_string()));
        let workspaces = workspaces_as(actor, None).await.column(None, "id");
        for ws in [w.ws_a, w.ws_b, w.ws_orphan] {
            assert!(workspaces.contains(&ws.to_string()), "{who} lost {ws}");
        }
    }
}
