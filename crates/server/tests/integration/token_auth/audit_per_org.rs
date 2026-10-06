//! A token lifecycle row holds only its own org's grants (API-tokens design
//! §3.7), asserted on the rows as they are stored and chained.
//!
//! A personal token holds grants in two orgs. `token.created` and
//! `token.grants_changed` are each written once per org, and neither org's
//! row may hold a workspace id or the org id of the other.
//!
//! The same rule for the sandbox agent kind's `token.created` is pinned next
//! to its mint (`sandbox_agent.rs`), and for `token.expired_sandboxes_queued`
//! in `sandbox_agent_sweep.rs`.

use entity::audit_events;
use entity::org_members::OrgRole;
use serde_json::{Value, json};
use uuid::Uuid;

use super::stack::{flat_api, join_org};
use super::{Fixture, audit_rows, call, fixture, seed_org, seed_workspace_in};

/// The fixture's user as an owner of two orgs, with a second workspace in
/// each: `(org, first workspace, second workspace)`.
struct TwoOrgs {
    fx: Fixture,
    a: (Uuid, Uuid, Uuid),
    b: (Uuid, Uuid, Uuid),
}

async fn two_orgs() -> TwoOrgs {
    let fx = fixture().await;
    join_org(&fx.db, fx.org_id, fx.user.id, OrgRole::Owner).await;
    let org_b = seed_org(&fx.db).await;
    join_org(&fx.db, org_b, fx.user.id, OrgRole::Owner).await;
    let a = (
        fx.org_id,
        fx.workspace_id,
        seed_workspace_in(&fx.db, fx.org_id).await,
    );
    let b = (
        org_b,
        seed_workspace_in(&fx.db, org_b).await,
        seed_workspace_in(&fx.db, org_b).await,
    );
    TwoOrgs { fx, a, b }
}

async fn in_session(fx: &Fixture, method: &str, uri: &str, body: Option<Value>) -> Value {
    let (status, body) = call(flat_api(), method, uri, &[("cookie", &fx.cookie)], body).await;
    assert!(status.is_success(), "{method} {uri}: {status} {body}");
    body
}

fn grant(org: Uuid, workspace: Uuid) -> Value {
    json!({ "org_id": org, "workspace_id": workspace, "role_ceiling": "member" })
}

/// Mint a narrowed token with `grants` in the session: its id.
async fn minted(fx: &Fixture, grants: Vec<Value>) -> String {
    let body = json!({ "name": "two orgs", "all_access": false, "grants": grants });
    let created = in_session(fx, "POST", "/user/tokens", Some(body)).await;
    created["token"]["id"].as_str().expect("an id").to_string()
}

/// The rows of `action` about `token`, one per org.
async fn rows_about(fx: &Fixture, action: &str, token: &str) -> Vec<audit_events::Model> {
    audit_rows(&fx.db, action)
        .await
        .into_iter()
        .filter(|row| row.target_id.as_deref() == Some(token))
        .collect()
}

fn row_on(rows: &[audit_events::Model], org: Uuid) -> &audit_events::Model {
    rows.iter()
        .find(|row| row.org_id == Some(org))
        .unwrap_or_else(|| panic!("a row on org {org}"))
}

/// The workspace ids an access summary lists, sorted.
fn workspaces_of(access: &Value) -> Vec<String> {
    let mut ids: Vec<String> = access["grants"]
        .as_array()
        .expect("grants")
        .iter()
        .map(|grant| {
            grant["workspace_id"]
                .as_str()
                .expect("a workspace")
                .to_string()
        })
        .collect();
    ids.sort();
    ids
}

fn sorted(ids: &[Uuid]) -> Vec<String> {
    let mut ids: Vec<String> = ids.iter().map(Uuid::to_string).collect();
    ids.sort();
    ids
}

/// The row exactly as it is stored: none of `foreign` appears anywhere in it.
fn assert_holds_none_of(row: &audit_events::Model, foreign: &[Uuid]) {
    let stored = serde_json::to_string(row).expect("serialise the row");
    for id in foreign {
        assert!(
            !stored.contains(&id.to_string()),
            "the row on org {:?} holds {id} of another org: {stored}",
            row.org_id
        );
    }
}

#[tokio::test]
async fn a_created_row_lists_only_its_own_orgs_grants() {
    let t = two_orgs().await;
    let ((org_a, a1, _), (org_b, b1, b2)) = (t.a, t.b);
    let grants = vec![grant(org_a, a1), grant(org_b, b1), grant(org_b, b2)];
    let id = minted(&t.fx, grants).await;

    let rows = rows_about(&t.fx, "token.created", &id).await;
    assert_eq!(rows.len(), 2, "one row per org the token reaches: {rows:?}");
    assert_eq!(
        rows[0].metadata["event_id"], rows[1].metadata["event_id"],
        "one event"
    );

    let on_a = row_on(&rows, org_a);
    assert_eq!(workspaces_of(&on_a.metadata), sorted(&[a1]));
    assert_holds_none_of(on_a, &[org_b, b1, b2]);

    let on_b = row_on(&rows, org_b);
    assert_eq!(workspaces_of(&on_b.metadata), sorted(&[b1, b2]));
    assert_holds_none_of(on_b, &[org_a, a1]);

    // What is about the token alone is the same on both.
    for row in [on_a, on_b] {
        assert_eq!(row.metadata["token_id"], json!(id));
        assert_eq!(row.metadata["all_access"], json!(false));
        assert_eq!(row.metadata["platform"], json!(false));
        assert_eq!(row.metadata["partner"], json!(false));
        assert!(row.metadata.get("expires_at").is_some(), "{}", row.metadata);
        // Nothing counts what the other org holds: each grant listed is its own.
        for listed in row.metadata["grants"].as_array().expect("grants") {
            assert_eq!(listed["org_id"], json!(row.org_id));
        }
    }
}

#[tokio::test]
async fn a_grants_changed_row_lists_only_its_own_orgs_before_and_after() {
    let t = two_orgs().await;
    let ((org_a, a1, a2), (org_b, b1, _)) = (t.a, t.b);
    let id = minted(&t.fx, vec![grant(org_a, a1), grant(org_b, b1)]).await;

    // The edit swaps org A's workspace and leaves org B's grant as it is.
    let edit = json!({ "grants": [grant(org_a, a2), grant(org_b, b1)] });
    in_session(&t.fx, "PATCH", &format!("/user/tokens/{id}"), Some(edit)).await;

    let rows = rows_about(&t.fx, "token.grants_changed", &id).await;
    assert_eq!(rows.len(), 2, "one row per org the token reached: {rows:?}");
    assert_eq!(
        rows[0].metadata["event_id"], rows[1].metadata["event_id"],
        "one event"
    );

    let on_a = row_on(&rows, org_a);
    let (before, after) = (on_a.before.as_ref(), on_a.after.as_ref());
    assert_eq!(workspaces_of(before.expect("before")), sorted(&[a1]));
    assert_eq!(workspaces_of(after.expect("after")), sorted(&[a2]));
    assert_holds_none_of(on_a, &[org_b, b1]);

    // Org B's row says the token changed, and that nothing of B's did.
    let on_b = row_on(&rows, org_b);
    assert_eq!(on_b.before, on_b.after);
    assert_eq!(
        workspaces_of(on_b.after.as_ref().expect("after")),
        sorted(&[b1])
    );
    assert_holds_none_of(on_b, &[org_a, a1, a2]);

    // The owner's Activity shows each event once, whichever org's row it reads.
    let uri = format!("/user/tokens/{id}/activity");
    let activity = in_session(&t.fx, "GET", &uri, None).await;
    let actions: Vec<&str> = activity["events"]
        .as_array()
        .expect("events")
        .iter()
        .map(|event| event["action"].as_str().expect("an action"))
        .collect();
    assert_eq!(actions, ["token.grants_changed", "token.created"]);
}

/// An all-access token lists no grant, so its rows differ only by org.
#[tokio::test]
async fn an_all_access_tokens_rows_list_no_grant_in_any_org() {
    let t = two_orgs().await;
    let body = json!({ "name": "everything" });
    let created = in_session(&t.fx, "POST", "/user/tokens", Some(body)).await;
    let id = created["token"]["id"].as_str().expect("an id").to_string();
    assert_eq!(created["token"]["all_access"], json!(true));

    let rows = rows_about(&t.fx, "token.created", &id).await;
    assert_eq!(rows.len(), 2, "one row per org its owner is in: {rows:?}");
    for (own, other) in [(t.a.0, t.b.0), (t.b.0, t.a.0)] {
        let row = row_on(&rows, own);
        assert_eq!(row.metadata["all_access"], json!(true));
        assert_eq!(row.metadata["grants"], json!([]));
        assert_holds_none_of(row, &[other]);
    }
}
