//! A lifecycle row lists the grants of its own org and holds nothing of
//! another's: `token.created` in its metadata, `token.grants_changed` in its
//! before and after. The stored rows are read back in
//! `crates/server/tests/integration/token_auth/audit_per_org.rs`.

use chrono::Utc;
use oxy_app_core::audit::{AuditContext, AuditEntry};

use super::*;
use crate::server::api::user_tokens::audit::{CREATED, Event, GRANTS_CHANGED};
use crate::server::api::user_tokens::system_audit::system_entry;

fn org(n: u128) -> Uuid {
    Uuid::from_u128(0xA000 + n)
}

fn workspace(n: u128) -> Uuid {
    Uuid::from_u128(0xC000 + n)
}

fn token(all_access: bool) -> api_tokens::Model {
    api_tokens::Model {
        id: Uuid::from_u128(0x70),
        kind: "personal".into(),
        principal_user_id: Uuid::from_u128(1),
        name: "deploy".into(),
        display_prefix: "oxy_pat_Ab3x".into(),
        last_four: "9f2c".into(),
        token_hash: vec![7; 32],
        all_access,
        platform: true,
        partner: false,
        expires_at: None,
        last_used_at: None,
        created_at: Utc::now().fixed_offset(),
        created_by: None,
        revoked_at: None,
        revoked_by: None,
        revoke_reason: None,
        source: "ui".into(),
        legacy_api_key_id: None,
        trust_policy_id: None,
        oidc_claims: None,
    }
}

/// A workspace grant of org `org_n` on workspace `workspace_n`.
fn grant(org_n: u128, workspace_n: u128, ceiling: &str) -> api_token_grants::Model {
    api_token_grants::Model {
        id: Uuid::new_v4(),
        token_id: Uuid::from_u128(0x70),
        kind: api_token_grants::KIND_WORKSPACE.into(),
        org_id: org(org_n),
        workspace_id: Some(workspace(workspace_n)),
        role_ceiling: Some(ceiling.into()),
        app_id: None,
        created_at: Utc::now().fixed_offset(),
        revoked_at: None,
        revoked_by: None,
    }
}

fn revoked(mut grant: api_token_grants::Model) -> api_token_grants::Model {
    grant.revoked_at = Some(Utc::now().fixed_offset());
    grant
}

/// The rows of an event of `action` over `orgs`, as a request would write
/// them but for the actor.
fn rows(
    action: &'static str,
    token: &api_tokens::Model,
    orgs: &[u128],
    own: impl Fn(Option<Uuid>) -> Own,
) -> Vec<AuditEntry> {
    let context = AuditContext::default();
    Event {
        action,
        token,
        orgs: orgs.iter().map(|n| org(*n)).collect(),
        detail: json!({ "expires_at": null }),
        change: None,
    }
    .entries_with(|| system_entry(action, &context), own)
}

/// Everything a row holds: every field, its metadata, its before and after.
fn serialised(row: &AuditEntry) -> String {
    format!("{row:?}")
}

fn workspaces_of(access: &Value) -> Vec<Value> {
    access["grants"]
        .as_array()
        .expect("grants")
        .iter()
        .map(|grant| grant["workspace_id"].clone())
        .collect()
}

#[test]
fn a_row_lists_the_grants_of_its_own_org_and_no_other() {
    let grants = [
        grant(1, 11, "admin"),
        grant(2, 21, "member"),
        grant(2, 22, "viewer"),
    ];
    let narrowed = token(false);
    let access = Access::of(&narrowed, &grants);

    let first = access.in_org(Some(org(1)));
    assert_eq!(
        first["grants"],
        json!([{
            "kind": "workspace",
            "org_id": org(1),
            "workspace_id": workspace(11),
            "role_ceiling": "admin",
            "app_id": null,
        }])
    );
    assert_eq!(
        workspaces_of(&access.in_org(Some(org(2)))),
        vec![json!(workspace(21)), json!(workspace(22))]
    );
    // An org the token holds nothing in, and a row with no org, list none.
    assert_eq!(access.in_org(Some(org(3)))["grants"], json!([]));
    assert_eq!(access.in_org(None)["grants"], json!([]));
}

#[test]
fn the_flags_are_the_tokens_and_the_same_on_every_row() {
    let grants = [grant(1, 11, "admin"), grant(2, 21, "member")];
    let narrowed = token(false);
    let access = Access::of(&narrowed, &grants);
    for row in [Some(org(1)), Some(org(2)), None] {
        let said = access.in_org(row);
        assert_eq!(said["all_access"], json!(false));
        assert_eq!(said["platform"], json!(true));
        assert_eq!(said["partner"], json!(false));
        // Flags and grants, and nothing that counts what another org holds.
        let mut keys: Vec<&String> = said.as_object().expect("an object").keys().collect();
        keys.sort();
        assert_eq!(keys, ["all_access", "grants", "partner", "platform"]);
    }
}

#[test]
fn a_revoked_grant_and_an_all_access_tokens_grants_are_not_listed() {
    let grants = [grant(1, 11, "admin"), revoked(grant(1, 12, "member"))];
    let narrowed = token(false);
    let live = Access::of(&narrowed, &grants).in_org(Some(org(1)));
    assert_eq!(workspaces_of(&live), vec![json!(workspace(11))]);

    // An all-access token's grants are not part of what it reaches.
    let everything = token(true);
    let all = Access::of(&everything, &grants).in_org(Some(org(1)));
    assert_eq!(all["all_access"], json!(true));
    assert_eq!(all["grants"], json!([]));
}

/// `token.created` for a token with grants in two orgs: each org's row lists
/// its own grants, and the other org's id and workspace ids appear nowhere in
/// it. The rows are one event and say the same of the token itself.
#[test]
fn a_created_row_holds_nothing_of_another_org() {
    let grants = [
        grant(1, 11, "admin"),
        grant(2, 21, "member"),
        grant(2, 22, "viewer"),
    ];
    let narrowed = token(false);
    let access = Access::of(&narrowed, &grants);
    let rows = rows(CREATED, &narrowed, &[1, 2], created(&access));
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].metadata["event_id"], rows[1].metadata["event_id"]);

    let sides: [(u128, &[u128]); 2] = [(1, &[11]), (2, &[21, 22])];
    for ((own, own_workspaces), (other, other_workspaces)) in
        [(sides[0], sides[1]), (sides[1], sides[0])]
    {
        let row = rows.iter().find(|row| row.org_id == Some(org(own)));
        let row = row.expect("a row on the org");
        let expected: Vec<Value> = own_workspaces
            .iter()
            .map(|n| json!(workspace(*n)))
            .collect();
        assert_eq!(workspaces_of(&row.metadata), expected);
        assert!(row.before.is_none() && row.after.is_none());
        // What is about the token alone is on every row.
        assert_eq!(row.metadata["all_access"], json!(false));
        assert_eq!(row.metadata["platform"], json!(true));
        assert!(row.metadata["expires_at"].is_null());
        assert_eq!(row.metadata["token_id"], json!(narrowed.id));

        let text = serialised(row);
        assert!(!text.contains(&org(other).to_string()), "{text}");
        for foreign in other_workspaces {
            let id = workspace(*foreign).to_string();
            assert!(!text.contains(&id), "org {own}'s row holds {id}");
        }
    }
}

/// `token.grants_changed`, an edit that drops org 1's grant for another and
/// leaves org 2's alone: each row's before and after are its own org's, and
/// the row of the org the edit did not touch reads the same on both sides.
#[test]
fn a_grants_changed_row_holds_only_its_own_orgs_before_and_after() {
    let held = [grant(1, 11, "admin"), grant(2, 21, "member")];
    let edited = [grant(1, 12, "viewer"), grant(2, 21, "member")];
    let narrowed = token(false);
    let (before, after) = (Access::of(&narrowed, &held), Access::of(&narrowed, &edited));
    let rows = rows(GRANTS_CHANGED, &narrowed, &[1, 2], changed(&before, &after));
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].metadata["event_id"], rows[1].metadata["event_id"]);

    let touched = &rows[0];
    assert_eq!(touched.org_id, Some(org(1)));
    let (was, is) = (touched.before.as_ref(), touched.after.as_ref());
    assert_eq!(
        workspaces_of(was.expect("before")),
        vec![json!(workspace(11))]
    );
    assert_eq!(
        workspaces_of(is.expect("after")),
        vec![json!(workspace(12))]
    );
    let text = serialised(touched);
    for foreign in [org(2).to_string(), workspace(21).to_string()] {
        assert!(!text.contains(&foreign), "org 1's row holds {foreign}");
    }

    let untouched = &rows[1];
    assert_eq!(untouched.org_id, Some(org(2)));
    assert_eq!(untouched.before, untouched.after);
    assert_eq!(
        workspaces_of(untouched.after.as_ref().expect("after")),
        vec![json!(workspace(21))]
    );
    let text = serialised(untouched);
    for foreign in [
        org(1).to_string(),
        workspace(11).to_string(),
        workspace(12).to_string(),
    ] {
        assert!(!text.contains(&foreign), "org 2's row holds {foreign}");
    }
    // The grants are in the change, not repeated in the metadata.
    assert!(untouched.metadata.get("grants").is_none());
}

/// Narrowed to all access: the row of an org the token held a grant in shows
/// that grant going, and the flag that replaced it.
#[test]
fn widening_to_all_access_reads_as_the_orgs_grants_going() {
    let held = [grant(1, 11, "admin")];
    let (narrowed, everything) = (token(false), token(true));
    let (before, after) = (Access::of(&narrowed, &held), Access::of(&everything, &held));
    let rows = rows(
        GRANTS_CHANGED,
        &everything,
        &[1, 3],
        changed(&before, &after),
    );

    let was_granted = rows[0].before.as_ref().expect("before");
    assert_eq!(workspaces_of(was_granted), vec![json!(workspace(11))]);
    assert_eq!(rows[0].after.as_ref().expect("after")["all_access"], true);
    assert_eq!(rows[0].after.as_ref().expect("after")["grants"], json!([]));
    // An org the owner belongs to, reached only now: nothing of org 1.
    assert_eq!(
        rows[1].before.as_ref().expect("before")["grants"],
        json!([])
    );
    assert!(!serialised(&rows[1]).contains(&workspace(11).to_string()));
}
