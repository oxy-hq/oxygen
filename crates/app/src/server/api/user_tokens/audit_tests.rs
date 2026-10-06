use super::*;
use crate::server::api::user_tokens::system_audit::system_entry;
use chrono::Utc;

fn id(n: u128) -> Uuid {
    Uuid::from_u128(n)
}

fn token(all_access: bool) -> api_tokens::Model {
    api_tokens::Model {
        id: id(1),
        kind: "personal".into(),
        principal_user_id: id(2),
        name: "deploy".into(),
        display_prefix: "oxy_pat_Ab3x".into(),
        last_four: "9f2c".into(),
        token_hash: vec![7; 32],
        all_access,
        platform: false,
        partner: false,
        expires_at: None,
        last_used_at: None,
        created_at: Utc::now().fixed_offset(),
        created_by: Some(id(2)),
        revoked_at: None,
        revoked_by: None,
        revoke_reason: None,
        source: "ui".into(),
        legacy_api_key_id: None,
        trust_policy_id: None,
        oidc_claims: None,
    }
}

fn grant(org: u128, revoked: bool) -> api_token_grants::Model {
    api_token_grants::Model {
        id: Uuid::new_v4(),
        token_id: id(1),
        kind: api_token_grants::KIND_WORKSPACE.into(),
        org_id: id(org),
        workspace_id: None,
        role_ceiling: Some("admin".into()),
        app_id: None,
        created_at: Utc::now().fixed_offset(),
        revoked_at: revoked.then(|| Utc::now().fixed_offset()),
        revoked_by: None,
    }
}

fn event<'a>(token: &'a api_tokens::Model, orgs: Vec<Uuid>) -> Event<'a> {
    Event {
        action: CREATED,
        token,
        orgs,
        detail: json!({ "expires_at": null }),
        change: None,
    }
}

#[test]
fn an_all_access_token_reaches_every_org_its_owner_is_in() {
    let owner_orgs = [id(30), id(10), id(20), id(10)];
    // Its grants are not what it reaches.
    let reach = reach_orgs(true, &owner_orgs, &[grant(99, false)]);
    assert_eq!(reach, vec![id(10), id(20), id(30)]);
}

#[test]
fn a_narrowed_token_reaches_the_orgs_its_live_grants_name() {
    let grants = [
        grant(20, false),
        grant(10, false),
        grant(20, false),
        grant(40, true),
    ];
    // Not the owner's other orgs, and not an org that revoked its grant.
    assert_eq!(
        reach_orgs(false, &[id(10), id(77)], &grants),
        vec![id(10), id(20)]
    );
    assert!(reach_orgs(false, &[id(10)], &[]).is_empty());
}

#[test]
fn an_edit_is_recorded_where_the_token_reached_before_and_after() {
    let before = vec![id(10), id(20)];
    let after = vec![id(20), id(30)];
    assert_eq!(union(before, after), vec![id(10), id(20), id(30)]);
}

#[test]
fn one_row_per_org_and_one_unchained_row_for_no_org() {
    let token = token(true);
    assert_eq!(
        event(&token, vec![id(10), id(20)]).row_orgs(),
        vec![Some(id(10)), Some(id(20))]
    );
    assert_eq!(event(&token, vec![]).row_orgs(), vec![None]);
}

#[test]
fn the_fan_out_is_capped_and_says_so() {
    let token = token(true);
    let many: Vec<Uuid> = (0..FANOUT_MAX_ORGS as u128 + 7).map(id).collect();
    let capped = event(&token, many.clone());
    assert_eq!(capped.row_orgs().len(), FANOUT_MAX_ORGS);
    assert_eq!(capped.row_orgs()[0], Some(id(0)), "the first orgs by id");
    let metadata = capped.metadata(id(5));
    assert_eq!(metadata["fanout_truncated"], json!(true));
    assert_eq!(metadata["reach_orgs"], json!(many.len()));

    let at_the_cap = event(&token, many[..FANOUT_MAX_ORGS].to_vec());
    assert_eq!(at_the_cap.row_orgs().len(), FANOUT_MAX_ORGS);
    assert!(at_the_cap.metadata(id(5)).get("fanout_truncated").is_none());
}

#[test]
fn every_row_names_the_token_and_never_its_secret() {
    let token = token(true);
    let metadata = event(&token, vec![id(10)]).metadata(id(5));
    assert_eq!(metadata["token_id"], json!(id(1)));
    assert_eq!(metadata["token_kind"], json!("personal"));
    assert_eq!(metadata["display_prefix"], json!("oxy_pat_Ab3x"));
    assert_eq!(metadata["event_id"], json!(id(5)));
    assert_eq!(metadata["source"], json!("ui"));
    // The event's own detail rides along.
    assert!(metadata.get("expires_at").is_some_and(Value::is_null));
    let text = metadata.to_string();
    assert!(!text.contains("token_hash") && !text.contains("9f2c"));
    assert!(metadata.get("api_key_id").is_none());
}

#[test]
fn a_legacy_rows_event_names_the_key_it_mirrors() {
    let mut legacy = token(true);
    legacy.kind = "legacy_key".into();
    legacy.legacy_api_key_id = Some(id(1));
    let metadata = event(&legacy, vec![]).metadata(id(5));
    assert_eq!(metadata["api_key_id"], json!(id(1)));
}

fn rows(event: &Event<'_>, own: impl Fn(Option<Uuid>) -> Own) -> Vec<AuditEntry> {
    let context = oxy_app_core::audit::AuditContext::default();
    event.entries_with(|| system_entry(event.action, &context), own)
}

/// What `own` answers for a row is on that row and on no other: its detail
/// over the row's metadata, its change as the row's before and after.
#[test]
fn what_a_row_owns_is_on_that_row_alone() {
    let token = token(false);
    let event = event(&token, vec![id(10), id(20)]);
    let rows = rows(&event, |org| {
        if org == Some(id(10)) {
            Own {
                detail: json!({ "here": "ten" }),
                change: Some((json!({ "n": 1 }), json!({ "n": 2 }))),
            }
        } else {
            Own::default()
        }
    });
    assert_eq!(rows[0].metadata["here"], "ten");
    assert_eq!(rows[0].before, Some(json!({ "n": 1 })));
    assert_eq!(rows[0].after, Some(json!({ "n": 2 })));
    assert!(rows[1].metadata.get("here").is_none());
    assert!(rows[1].before.is_none() && rows[1].after.is_none());
    // Still one event, and the shared detail is on both.
    assert_eq!(rows[0].metadata["event_id"], rows[1].metadata["event_id"]);
    assert!(rows.iter().all(|row| row.metadata["expires_at"].is_null()));
}

/// An event's own change is every row's, unless the row has one of its own.
#[test]
fn a_rows_change_replaces_the_events() {
    let token = token(false);
    let mut event = event(&token, vec![id(10), id(20)]);
    event.change = Some((json!({ "at": "old" }), json!({ "at": "new" })));
    let rows = rows(&event, |org| match org {
        Some(org) if org == id(20) => Own::change(json!({ "own": 1 }), json!({ "own": 2 })),
        _ => Own::default(),
    });
    assert_eq!(rows[0].before, Some(json!({ "at": "old" })));
    assert_eq!(rows[0].after, Some(json!({ "at": "new" })));
    assert_eq!(rows[1].before, Some(json!({ "own": 1 })));
    assert_eq!(rows[1].after, Some(json!({ "own": 2 })));
}

#[test]
fn an_expiry_is_rfc3339_or_null() {
    assert_eq!(rfc3339(None), Value::Null);
    let at = Utc::now().fixed_offset();
    assert_eq!(rfc3339(Some(at)), json!(at.to_rfc3339()));
}
