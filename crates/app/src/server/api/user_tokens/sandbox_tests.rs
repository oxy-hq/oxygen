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
