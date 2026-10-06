use super::*;
use chrono::Duration;
use serde_json::json;

fn id(n: u128) -> Uuid {
    Uuid::from_u128(n)
}

fn token(kind: &str, legacy_api_key_id: Option<Uuid>) -> api_tokens::Model {
    api_tokens::Model {
        id: id(1),
        kind: kind.to_string(),
        principal_user_id: id(2),
        name: "deploy".into(),
        display_prefix: "oxy_pat_Ab3x".into(),
        last_four: "9f2c".into(),
        token_hash: vec![7; 32],
        all_access: true,
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
        legacy_api_key_id,
        trust_policy_id: None,
        oidc_claims: None,
    }
}

fn grant(row_id: u128, workspace: Option<u128>) -> api_token_grants::Model {
    api_token_grants::Model {
        id: id(row_id),
        token_id: id(1),
        kind: api_token_grants::KIND_WORKSPACE.into(),
        org_id: id(10),
        workspace_id: workspace.map(id),
        role_ceiling: Some("member".into()),
        app_id: None,
        created_at: Utc::now().fixed_offset(),
        revoked_at: None,
        revoked_by: None,
    }
}

fn names() -> Names {
    Names {
        orgs: [(id(10), "Acme".to_string())].into(),
        workspaces: [(id(20), "Default".to_string())].into(),
        apps: [(id(30), "Store Ops".to_string())].into(),
    }
}

fn dto(row: &api_tokens::Model, grants: &[api_token_grants::Model]) -> TokenDto {
    token_dto(
        row,
        TokenView {
            grants,
            names: &names(),
            owner: OwnerDto::user(id(2), "ada@example.com"),
            key_inactive: false,
            now: Utc::now(),
        },
    )
}

#[test]
fn a_token_serialises_as_the_contract_spells_it() {
    let mut row = token("personal", None);
    row.all_access = false;
    let out = serde_json::to_value(dto(&row, &[grant(5, Some(20))])).unwrap();
    assert_eq!(out["kind"], "personal");
    assert_eq!(out["status"], "active");
    assert_eq!(out["source"], "ui");
    assert_eq!(out["blocked_orgs"], json!([]));
    assert_eq!(
        out["owner"],
        json!({ "type": "user", "id": id(2), "label": "ada@example.com" })
    );
    // No expiry and never used are `null`, not absent.
    for key in ["expires_at", "last_used_at", "revoked_at"] {
        assert!(out.get(key).is_some_and(|v| v.is_null()), "{key}");
    }
    assert_eq!(
        out["grants"][0],
        json!({
            "id": id(5), "kind": "workspace",
            "org_id": id(10), "org_name": "Acme",
            "workspace_id": id(20), "workspace_name": "Default",
            "role_ceiling": "member",
            "app_id": null, "app_name": null,
            "revoked_at": null,
        })
    );
}

#[test]
fn the_secret_and_its_hash_never_leave() {
    let out = serde_json::to_string(&dto(&token("personal", None), &[])).unwrap();
    assert!(!out.contains("token_hash") && !out.contains("secret"));
    // The prefix is the stored fragment: nothing appended to it.
    assert!(out.contains("\"display_prefix\":\"oxy_pat_Ab3x\""));
}

#[test]
fn an_org_wide_grant_names_no_workspace() {
    let mut row = token("personal", None);
    row.all_access = false;
    let out = dto(&row, &[grant(5, None)]);
    assert_eq!(out.grants[0].workspace_id, None);
    assert_eq!(out.grants[0].workspace_name, None);
    assert_eq!(out.grants[0].org_name, "Acme");
}

#[test]
fn an_app_publish_grant_names_its_app_and_no_ceiling() {
    let mut row = token("personal", None);
    row.all_access = false;
    let publish = api_token_grants::Model {
        kind: api_token_grants::KIND_APP_PUBLISH.into(),
        role_ceiling: None,
        app_id: Some(id(30)),
        ..grant(6, None)
    };
    let out = dto(&row, &[publish]);
    assert_eq!(out.grants[0].kind, "app_publish");
    assert_eq!(out.grants[0].role_ceiling, None);
    assert_eq!(out.grants[0].app_name.as_deref(), Some("Store Ops"));
}

#[test]
fn only_a_tokens_own_grants_are_listed() {
    let mut row = token("personal", None);
    row.all_access = false;
    let someone_elses = api_token_grants::Model {
        token_id: id(99),
        ..grant(7, None)
    };
    let out = dto(&row, &[grant(5, None), someone_elses]);
    assert_eq!(out.grants.len(), 1);
    assert_eq!(out.grants[0].id, id(5));
}

#[test]
fn a_revoked_grant_is_listed_as_revoked_even_on_an_all_access_token() {
    let mut revoked = grant(5, Some(20));
    revoked.revoked_at = Some(Utc::now().fixed_offset());
    // All-access: its live grants are not consulted, so they are not shown;
    // what an org revoked is history and stays visible.
    let all_access = dto(&token("personal", None), &[revoked.clone(), grant(6, None)]);
    assert_eq!(all_access.grants.len(), 1);
    assert!(all_access.grants[0].revoked_at.is_some());

    let mut row = token("personal", None);
    row.all_access = false;
    assert_eq!(dto(&row, &[revoked, grant(6, None)]).grants.len(), 2);
}

#[test]
fn every_row_that_mirrors_api_keys_reads_as_a_legacy_key() {
    let hex_key = token("legacy_key", Some(id(1)));
    let legacy_endpoint = token("personal", Some(id(1)));
    for row in [&hex_key, &legacy_endpoint] {
        assert!(is_legacy(row));
        let out = dto(row, &[grant(5, None)]);
        assert_eq!(out.kind, KIND_LEGACY);
        assert!(out.all_access);
        assert!(out.grants.is_empty(), "a legacy key has no grants");
    }
    let personal = token("personal", None);
    assert!(!is_legacy(&personal));
    assert_eq!(dto(&personal, &[]).kind, "personal");
}

#[test]
fn a_legacy_rows_key_id_is_the_one_it_mirrors() {
    assert_eq!(legacy_key_id(&token("legacy_key", None)), id(1));
    assert_eq!(legacy_key_id(&token("personal", Some(id(44)))), id(44));
}

#[test]
fn status_is_revoked_then_expired_then_active() {
    let now = Utc::now();
    let mut row = token("personal", None);
    assert_eq!(status_of(&row, false, now), STATUS_ACTIVE);

    row.expires_at = Some((now + Duration::days(1)).fixed_offset());
    assert_eq!(status_of(&row, false, now), STATUS_ACTIVE);

    row.expires_at = Some((now - Duration::seconds(1)).fixed_offset());
    assert_eq!(status_of(&row, false, now), STATUS_EXPIRED);

    // Revoked wins over a lapsed expiry, and stays listed with its row.
    row.revoked_at = Some(now.fixed_offset());
    assert_eq!(status_of(&row, false, now), STATUS_REVOKED);
    assert_eq!(dto(&row, &[]).status, STATUS_REVOKED);
    assert!(dto(&row, &[]).revoked_at.is_some());
}

#[test]
fn a_key_revoked_by_an_older_pod_reads_as_revoked() {
    // `api_keys.is_active = false` with no `api_tokens.revoked_at` yet.
    let row = token("legacy_key", Some(id(1)));
    assert_eq!(status_of(&row, true, Utc::now()), STATUS_REVOKED);
}
