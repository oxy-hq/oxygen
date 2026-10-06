//! The parts of a sandbox agent mint that need no database: the limits the
//! dialog is told, and which rows are fixed. Who may mint, and the mint
//! itself, are exercised through the routes
//! (`crates/server/tests/integration/token_auth/sandbox_agent.rs`).

use chrono::Utc;
use serde_json::json;

use super::*;

fn row(kind: &str) -> api_tokens::Model {
    let now = Utc::now().fixed_offset();
    api_tokens::Model {
        id: Uuid::from_u128(0x5B),
        kind: kind.into(),
        principal_user_id: Uuid::from_u128(1),
        name: "agent".into(),
        display_prefix: "oxy_sbx_Ab3x".into(),
        last_four: "wxyz".into(),
        token_hash: vec![0; 32],
        all_access: false,
        platform: true,
        partner: false,
        expires_at: Some(now),
        last_used_at: None,
        created_at: now,
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

#[test]
fn the_limits_the_dialog_is_told_are_the_ones_the_mint_enforces() {
    let limits = serde_json::to_value(SandboxAgentLimits::current()).unwrap();
    assert_eq!(
        limits,
        json!({ "default_hours": 8, "max_hours": 168, "max_apps": 5 })
    );
}

#[test]
fn only_a_sandbox_agent_token_is_fixed() {
    assert!(matches!(
        refuse_edit(&row("sandbox_agent")),
        Err(TokenError::SandboxTokenFixed)
    ));
    for editable in ["personal", "legacy_key", "service_account"] {
        assert!(refuse_edit(&row(editable)).is_ok(), "{editable}");
    }
}

#[test]
fn the_mint_takes_both_capabilities() {
    assert_eq!(MINT_CAPS, [Cap::DevelopApps, Cap::ManageApps]);
}

fn org(n: u128) -> Uuid {
    Uuid::from_u128(0xA000 + n)
}

fn app(n: u128) -> Uuid {
    Uuid::from_u128(0xB000 + n)
}

/// The `app_sandbox` grant the mint stores for `app_n`, an app of `org_n`.
fn grant(org_n: u128, app_n: u128) -> entity::api_token_grants::Model {
    let now = Utc::now().fixed_offset();
    entity::api_token_grants::Model {
        id: Uuid::new_v4(),
        token_id: Uuid::from_u128(0x5B),
        kind: entity::api_token_grants::KIND_APP_SANDBOX.into(),
        org_id: org(org_n),
        workspace_id: None,
        role_ceiling: None,
        app_id: Some(app(app_n)),
        created_at: now,
        revoked_at: None,
        revoked_by: None,
    }
}

/// `token.created` for a mint naming apps of two orgs, as its rows are built:
/// each org's row names its own apps and grants, and the other org's id and
/// app ids appear nowhere in it, nor how many it has.
#[test]
fn a_mints_row_names_only_the_apps_of_its_own_org() {
    let token = row("sandbox_agent");
    let pairs = [(1, 11), (2, 21), (2, 22)];
    let stored: Vec<_> = pairs.iter().map(|(o, a)| grant(*o, *a)).collect();
    let granted: Vec<GrantedApp> = pairs
        .iter()
        .map(|(o, a)| GrantedApp {
            org_id: org(*o),
            app_id: app(*a),
        })
        .collect();
    let access = Access::of(&token, &stored);
    let context = oxy_app_core::audit::AuditContext::default();
    let rows = Event {
        action: audit::CREATED,
        token: &token,
        orgs: vec![org(1), org(2)],
        detail: json!({ "expires_at": "soon", "hostname": "build-box" }),
        change: None,
    }
    .entries_with(
        || super::super::system_audit::system_entry(audit::CREATED, &context),
        |org| mint_in(&access, &granted, org),
    );
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].metadata["event_id"], rows[1].metadata["event_id"]);

    let sides: [(u128, &[u128]); 2] = [(1, &[11]), (2, &[21, 22])];
    for ((own, own_apps), (other, other_apps)) in [(sides[0], sides[1]), (sides[1], sides[0])] {
        let row = rows.iter().find(|row| row.org_id == Some(org(own)));
        let row = row.expect("a row on the org");
        let expected: Vec<Value> = own_apps.iter().map(|n| json!(app(*n))).collect();
        assert_eq!(row.metadata["apps"], json!(expected));
        let listed: Vec<Value> = row.metadata["grants"]
            .as_array()
            .expect("grants")
            .iter()
            .map(|grant| grant["app_id"].clone())
            .collect();
        assert_eq!(listed, expected);
        // What is about the token alone is on every row.
        assert_eq!(row.metadata["hostname"], "build-box");
        assert_eq!(row.metadata["expires_at"], "soon");
        assert_eq!(row.metadata["platform"], json!(true));

        let text = format!("{row:?}");
        assert!(!text.contains(&org(other).to_string()), "{text}");
        for foreign in other_apps {
            let id = app(*foreign).to_string();
            assert!(!text.contains(&id), "org {own}'s row holds {id}");
        }
        let keys: Vec<&String> = row.metadata.as_object().expect("metadata").keys().collect();
        assert!(
            !keys
                .iter()
                .any(|key| key.contains("count") || key.contains("total")),
            "{keys:?}"
        );
    }
}
