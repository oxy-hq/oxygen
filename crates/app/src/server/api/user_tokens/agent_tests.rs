//! What an approved agent mint is minted as, with no database.

use chrono::Duration;
use oxy_authz::{Cap, PartnerStanding, PlatformRole, PlatformStanding, Scope};
use uuid::Uuid;

use super::*;

fn member() -> PrincipalFacts {
    PrincipalFacts {
        user_id: Uuid::from_u128(1),
        member_orgs: vec![Uuid::from_u128(0xA)],
        ..Default::default()
    }
}

fn staff() -> PrincipalFacts {
    PrincipalFacts {
        platform: Some(PlatformStanding::from_role(
            PlatformRole::GlobalAdmin,
            Scope::All,
        )),
        ..member()
    }
}

fn partner() -> PrincipalFacts {
    PrincipalFacts {
        partners: vec![PartnerStanding {
            partner_id: Uuid::from_u128(0xD),
            client_orgs: vec![Uuid::from_u128(0xB)],
            caps: vec![Cap::ManageMembers],
        }],
        ..member()
    }
}

fn request(standing: bool) -> MintRequest {
    MintRequest {
        name: "agent on build-box".into(),
        standing,
        hours: 8,
    }
}

#[test]
fn the_default_name_is_the_agents_not_the_logins() {
    assert_eq!(default_name("build-box"), "agent on build-box");
    assert_ne!(
        default_name("build-box"),
        cli_login::token_name("build-box")
    );
}

#[test]
fn a_standing_is_carried_only_when_approved_and_held() {
    // (approved, holds staff, holds partner) -> (platform, partner)
    for (asked, held, carries) in [
        (true, (true, true), (true, true)),
        (true, (true, false), (true, false)),
        (true, (false, true), (false, true)),
        (true, (false, false), (false, false)),
        (false, (true, true), (false, false)),
        (false, (false, false), (false, false)),
    ] {
        assert_eq!(carried(asked, held), carries, "{asked} {held:?}");
    }
}

#[test]
fn the_token_is_all_access_for_the_hours_asked_and_carries_what_its_owner_holds() {
    let now = Utc::now();
    let flags = |standing: bool, facts: &PrincipalFacts| {
        let token = token_for(&request(standing), facts, "build-box", now);
        assert!(token.access.all_access, "always all-access");
        assert!(token.access.grants.is_empty(), "never a grant");
        assert_eq!(token.expires_at, Some(now + Duration::hours(8)));
        assert_eq!(token.source, "oxyc_agent");
        assert_eq!(token.name, "agent on build-box");
        (token.access.platform, token.access.partner)
    };
    assert_eq!(flags(true, &staff()), (true, false));
    assert_eq!(flags(true, &partner()), (false, true));
    // Asked for and not held: no error, and no standing.
    assert_eq!(flags(true, &member()), (false, false));
    // Held and not asked for: none either.
    assert_eq!(flags(false, &staff()), (false, false));
    assert_eq!(flags(false, &partner()), (false, false));
}

#[test]
fn the_audit_detail_names_the_host_and_what_was_approved() {
    let token = token_for(&request(true), &member(), "build-box", Utc::now());
    assert_eq!(token.detail["hostname"], "build-box");
    // Approved, though its owner held none: the flags beside it say so.
    assert_eq!(token.detail["standing_approved"], true);
    assert!(!token.access.platform && !token.access.partner);
}

fn row(kind: &str, source: &str) -> api_tokens::Model {
    api_tokens::Model {
        id: Uuid::new_v4(),
        kind: kind.into(),
        principal_user_id: Uuid::from_u128(1),
        name: "agent on build-box".into(),
        display_prefix: "oxy_pat_Ab3x".into(),
        last_four: "wxyz".into(),
        token_hash: vec![0; 32],
        all_access: true,
        platform: false,
        partner: false,
        expires_at: Some((Utc::now() + Duration::hours(8)).fixed_offset()),
        last_used_at: None,
        created_at: Utc::now().fixed_offset(),
        created_by: None,
        revoked_at: None,
        revoked_by: None,
        revoke_reason: None,
        source: source.into(),
        legacy_api_key_id: None,
        trust_policy_id: None,
        oidc_claims: None,
    }
}

#[test]
fn only_an_agent_token_is_refused_an_edit() {
    assert!(matches!(
        refuse_edit(&row("personal", source::OXYC_AGENT)),
        Err(TokenError::AgentTokenFixed)
    ));
    // Every other personal token is edited as it always was.
    for other in [source::UI, source::OXYC_LOGIN, source::LEGACY_ENDPOINT] {
        assert!(refuse_edit(&row("personal", other)).is_ok(), "{other}");
    }
    // A sandbox agent token is refused by its own rule, not this one.
    assert!(refuse_edit(&row("sandbox_agent", source::OXYC)).is_ok());
}

#[test]
fn a_bad_mint_is_refused_as_an_agent_tokens() {
    // With a default name, as `authorize` parses it: the hours are what is wrong.
    let body = json!({ "kind": "agent", "expires_in_hours": 169 });
    let refused = parse(&body, Some("agent on build-box"));
    assert!(
        matches!(&refused, Err(TokenError::InvalidAgentToken(why)) if why.contains("168")),
        "{refused:?}"
    );
    // With none, as a stored mint is parsed, the missing name is refused first.
    let unnamed = parse(&body, None);
    assert!(
        matches!(&unnamed, Err(TokenError::InvalidAgentToken(why)) if why.contains("'name'")),
        "{unnamed:?}"
    );
}

#[test]
fn the_limits_are_eight_hours_by_default_and_a_week_at_most() {
    let limits = serde_json::to_value(AgentLimits::current()).unwrap();
    assert_eq!(limits, json!({ "default_hours": 8, "max_hours": 168 }));
}
