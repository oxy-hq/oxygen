use chrono::Utc;
use uuid::Uuid;

use super::*;
use crate::token::format::{generate_ci, generate_personal, generate_service_account};

fn row(kind: &str, legacy_api_key_id: Option<Uuid>) -> api_tokens::Model {
    api_tokens::Model {
        id: Uuid::new_v4(),
        kind: kind.to_string(),
        principal_user_id: Uuid::new_v4(),
        name: "laptop".into(),
        display_prefix: "oxy_pat_Ab3x".into(),
        last_four: "wxyz".into(),
        token_hash: vec![0; 32],
        all_access: false,
        platform: false,
        partner: false,
        expires_at: None,
        last_used_at: None,
        created_at: Utc::now().fixed_offset(),
        created_by: None,
        revoked_at: None,
        revoked_by: None,
        revoke_reason: None,
        source: "ui".into(),
        legacy_api_key_id,
        trust_policy_id: None,
        oidc_claims: None,
    }
}

#[test]
fn a_well_formed_new_token_is_worth_a_lookup() {
    for token in [
        generate_personal(),
        generate_service_account(),
        generate_ci(),
    ] {
        assert_eq!(classify(&token.plaintext), Presented::NewFormat);
        // Scanners hand over what they matched; surrounding blanks are not ours.
        assert_eq!(
            classify(&format!("  {}\n", token.plaintext)),
            Presented::NewFormat
        );
    }
}

#[test]
fn a_lookalike_with_a_bad_checksum_is_unknown_without_a_read() {
    let token = generate_personal().plaintext;
    let mut chars: Vec<char> = token.chars().collect();
    let last = chars.len() - 1;
    chars[last] = if chars[last] == 'a' { 'b' } else { 'a' };
    let forged: String = chars.into_iter().collect();
    assert_eq!(classify(&forged), Presented::Unknown);
    assert_eq!(classify("oxy_pat_tooshort"), Presented::Unknown);
    assert_eq!(classify("ghp_notours"), Presented::Unknown);
    assert_eq!(classify(""), Presented::Unknown);
}

#[test]
fn a_legacy_key_is_recognised_and_never_looked_up() {
    let legacy = format!("oxy_{}", "0123456789abcdef0123456789abcdef");
    assert_eq!(classify(&legacy), Presented::Legacy);
}

#[test]
fn a_found_row_is_revoked_unless_it_is_legacy_unknown_or_done() {
    assert_eq!(decide(None), Decision::Answer(LeakStatus::Unknown));
    assert_eq!(decide(Some(&row("personal", None))), Decision::Revoke);
    assert_eq!(
        decide(Some(&row("service_account", None))),
        Decision::Revoke
    );
    assert_eq!(decide(Some(&row("ci", None))), Decision::Revoke);

    let mut revoked = row("personal", None);
    revoked.revoked_at = Some(Utc::now().fixed_offset());
    assert_eq!(
        decide(Some(&revoked)),
        Decision::Answer(LeakStatus::AlreadyRevoked)
    );

    // An expired token is still revoked: Extend could revive it.
    let mut expired = row("personal", None);
    expired.expires_at = Some((Utc::now() - chrono::Duration::days(1)).fixed_offset());
    assert_eq!(decide(Some(&expired)), Decision::Revoke);

    assert_eq!(
        decide(Some(&row("legacy_publish", None))),
        Decision::Answer(LeakStatus::Unknown)
    );
}

#[test]
fn a_legacy_row_is_ignored_even_when_revoked_or_new_looking() {
    let id = Uuid::new_v4();
    // A legacy key, and a token the legacy endpoint minted (stored
    // `personal`, mirroring `api_keys`): both legacy, both left alone.
    for legacy in [row("legacy_key", Some(id)), row("personal", Some(id))] {
        assert_eq!(
            decide(Some(&legacy)),
            Decision::Answer(LeakStatus::IgnoredLegacy)
        );
    }
}

#[test]
fn the_statuses_are_the_wire_strings() {
    let wire: Vec<&str> = [
        LeakStatus::Revoked,
        LeakStatus::AlreadyRevoked,
        LeakStatus::Unknown,
        LeakStatus::IgnoredLegacy,
    ]
    .iter()
    .map(|s| s.as_str())
    .collect();
    assert_eq!(
        wire,
        ["revoked", "already_revoked", "unknown", "ignored_legacy"]
    );
}
