use oxy_authz::{Cap, PlatformRole, Scope};
use sha2::{Digest, Sha256};

use super::*;

/// The one token every test here installs, so installs agree in any order.
const TOKEN: &str = "oxysvc_TestTokenForUnitTestsOnly0123456789abcdefABCDEFGHIJ";

fn sha256_hex(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

fn entry(name: &str, role: &str, token: &str) -> String {
    format!(
        r#"{{"name":"{name}","role":"{role}","sha256":"{}"}}"#,
        sha256_hex(token)
    )
}

fn installed() -> &'static ServiceToken {
    let tokens = parse(&format!("[{}]", entry("ferret", "global_admin", TOKEN))).unwrap();
    install(tokens).unwrap();
    authenticate(TOKEN).expect("the installed token authenticates")
}

#[test]
fn blank_is_no_services() {
    assert_eq!(parse("").unwrap(), Vec::new());
    assert_eq!(parse("  \n").unwrap(), Vec::new());
    assert_eq!(parse("[]").unwrap(), Vec::new());
}

#[test]
fn a_well_formed_entry_parses() {
    let tokens = parse(&format!("[{}]", entry("ferret", "global_admin", TOKEN))).unwrap();
    assert_eq!(tokens.len(), 1);
    assert_eq!(tokens[0].name, "ferret");
    assert_eq!(tokens[0].role, PlatformRole::GlobalAdmin);
    assert_eq!(tokens[0].principal(), "service:ferret");
}

#[test]
fn every_malformed_value_is_an_error_that_never_echoes_the_value() {
    let hash = sha256_hex(TOKEN);
    let upper = hash.to_ascii_uppercase();
    let cases = [
        "not json".to_string(),
        r#"{"name":"ferret"}"#.to_string(),
        format!(r#"[{{"name":"ferret","role":"global_admin"}}]"#),
        format!(r#"[{{"name":"ferret","role":"global_admin","sha256":"{hash}","x":1}}]"#),
        format!(r#"[{{"name":"Ferret","role":"global_admin","sha256":"{hash}"}}]"#),
        format!(r#"[{{"name":"","role":"global_admin","sha256":"{hash}"}}]"#),
        format!(
            r#"[{{"name":"{}","role":"global_admin","sha256":"{hash}"}}]"#,
            "a".repeat(41)
        ),
        format!(r#"[{{"name":"ferret","role":"global_owner","sha256":"{hash}"}}]"#),
        format!(r#"[{{"name":"ferret","role":"global_admin","sha256":"{upper}"}}]"#),
        format!(
            r#"[{{"name":"ferret","role":"global_admin","sha256":"{}"}}]"#,
            &hash[..63]
        ),
        format!(
            "[{},{}]",
            entry("ferret", "global_admin", TOKEN),
            entry("ferret", "app_operator", "oxysvc_other")
        ),
        format!(
            "[{},{}]",
            entry("ferret", "global_admin", TOKEN),
            entry("other", "app_operator", TOKEN)
        ),
    ];
    for raw in cases {
        let err = parse(&raw).expect_err(&raw);
        assert!(!err.contains(&hash), "the error echoes the hash: {err}");
    }
}

#[test]
fn the_standing_is_the_role_over_every_org_without_grant_management() {
    let token = installed();
    let standing = token.standing();
    assert_eq!(standing.scope, Scope::All);
    assert!(!standing.holds(Cap::ManagePlatformGrants));
    assert!(!standing.holds(Cap::ManageBilling), "billing is owner-only");
    for cap in PlatformRole::GlobalAdmin.caps() {
        if cap != Cap::ManagePlatformGrants {
            assert!(standing.holds(cap), "{cap:?}");
        }
    }
    assert_eq!(standing_of(token.token_id()), Some(standing));
}

#[test]
fn only_the_exact_token_authenticates() {
    let token = installed();
    assert_eq!(authenticate(TOKEN).map(|t| &t.name), Some(&token.name));
    let mut wrong = TOKEN.to_string();
    wrong.pop();
    wrong.push('X');
    for presented in [wrong.as_str(), "oxysvc_", PREFIX, "", &TOKEN[1..]] {
        assert!(authenticate(presented).is_none(), "{presented:?}");
    }
    // The hash alone is not the token.
    assert!(authenticate(&sha256_hex(TOKEN)).is_none());
}

#[test]
fn the_credential_carries_the_principal_and_platform_standing() {
    let token = installed();
    let credential = token.credential();
    assert_eq!(credential.kind, StoredKind::Service);
    assert_eq!(credential.name, "service:ferret");
    assert_eq!(credential.principal_user_id, token.user_id());
    assert!(credential.all_access && credential.platform && !credential.partner);
    assert!(!credential.is_legacy() && !credential.is_service_account());
    let reach = credential.reach().expect("a token narrows by its reach");
    assert!(reach.all_access && reach.platform && !reach.partner);
}

#[test]
fn ids_are_stable_and_distinct_per_name() {
    let a = ServiceToken::new("ferret", "global_admin", &sha256_hex("x")).unwrap();
    let b = ServiceToken::new("ferret", "app_operator", &sha256_hex("y")).unwrap();
    let c = ServiceToken::new("other", "global_admin", &sha256_hex("x")).unwrap();
    assert_eq!(a.user_id(), b.user_id());
    assert_eq!(a.token_id(), b.token_id());
    assert_ne!(a.user_id(), c.user_id());
    assert_ne!(a.user_id(), a.token_id());
}

#[test]
fn a_different_set_cannot_replace_the_installed_one() {
    installed();
    let other = parse(&format!("[{}]", entry("other", "app_operator", "oxysvc_x"))).unwrap();
    assert!(install(other).is_err());
}
