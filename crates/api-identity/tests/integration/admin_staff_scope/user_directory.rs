//! `GET /admin/users` — the user directory, as a bounded grant sees it.
//!
//! The org and workspace directories were narrowed first, and this one is the same
//! rule over a person: a bounded grant lists the users who are members of an org it
//! reaches, narrowed in the query ahead of the paging, and each row's org-owned
//! summary (membership count, top role, partners operated) counts only those orgs.

use axum::extract::{OriginalUri, Query};
use axum::http::{StatusCode, Uri};
use entity::org_members::OrgRole;
use oxy_api_identity::admin::users_admin::{ListUsersQuery, list_users};
use oxy_auth::types::AuthenticatedUser;
use serde_json::Value;
use uuid::Uuid;

use super::fixture::{Reply, as_actor, member_of, reply, seed_user, world};
use super::user_detail::operate_partner;

async fn list_as(actor: &AuthenticatedUser, page: u64, page_size: u64) -> Reply {
    let uri = OriginalUri(Uri::from_static("/admin/users"));
    let query = Query(ListUsersQuery {
        search: None,
        status: None,
        role: None,
        page: Some(page),
        page_size: Some(page_size),
    });
    reply(list_users(as_actor(actor), uri, query).await).await
}

fn row<'a>(page: &'a Reply, user: Uuid) -> &'a Value {
    page.body
        .as_array()
        .expect("a row array")
        .iter()
        .find(|r| r["id"] == user.to_string())
        .unwrap_or_else(|| panic!("{user} is not listed: {}", page.body))
}

fn sorted(mut ids: Vec<String>) -> Vec<String> {
    ids.sort();
    ids
}

#[tokio::test]
async fn a_bounded_grants_user_directory_lists_only_members_of_its_orgs() {
    let w = world().await;
    let in_a = seed_user(&w.db, "in-a@tenant.test").await;
    member_of(&w.db, in_a.id, w.org_a, OrgRole::Member).await;
    let in_b = seed_user(&w.db, "in-b@tenant.test").await;
    member_of(&w.db, in_b.id, w.org_b, OrgRole::Member).await;
    let in_both = seed_user(&w.db, "in-both@tenant.test").await;
    member_of(&w.db, in_both.id, w.org_a, OrgRole::Member).await;
    let in_b_membership = member_of(&w.db, in_both.id, w.org_b, OrgRole::Owner).await;
    operate_partner(&w.db, w.org_b, in_b_membership).await;
    let in_none = seed_user(&w.db, "in-none@tenant.test").await;

    let page = list_as(&w.bounded, 0, 50).await;
    assert_eq!(page.status, StatusCode::OK, "{}", page.body);
    assert_eq!(
        sorted(page.column(None, "id")),
        sorted(vec![in_a.id.to_string(), in_both.id.to_string()]),
        "the directory lists users a grant bounded to org A has no reach to"
    );
    // The row's summary is org-owned too: org B's membership is not this grant's.
    let shared = row(&page, in_both.id);
    assert_eq!(shared["org_count"], 1, "{shared}");
    assert_eq!(shared["top_org_role"], "member", "{shared}");
    assert_eq!(shared["partners"], Value::Array(Vec::new()), "{shared}");

    // Narrowed ahead of the paging: two rows at one a page are two full pages, and
    // the second is the last.
    let first = list_as(&w.bounded, 0, 1).await;
    assert_eq!(first.column(None, "id").len(), 1);
    assert!(first.has_next(), "link: {}", first.link);
    let second = list_as(&w.bounded, 1, 1).await;
    assert_eq!(second.column(None, "id").len(), 1);
    assert!(!second.has_next(), "link: {}", second.link);

    for (who, actor) in w.everything_readers() {
        let page = list_as(actor, 0, 50).await;
        let ids = page.column(None, "id");
        for user in [
            &in_a,
            &in_b,
            &in_both,
            &in_none,
            &w.bounded,
            &w.unbounded,
            &w.owner,
        ] {
            assert!(
                ids.contains(&user.id.to_string()),
                "{who} misses {:?}",
                user.email
            );
        }
        let shared = row(&page, in_both.id);
        assert_eq!(shared["org_count"], 2, "{who}: {shared}");
        assert_eq!(shared["top_org_role"], "owner", "{who}: {shared}");
        assert_eq!(
            shared["partners"][0]["id"],
            w.org_b.to_string(),
            "{who}: {shared}"
        );
    }
}
