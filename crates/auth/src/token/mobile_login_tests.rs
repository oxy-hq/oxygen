//! Unit tests for the mobile sign-in code: the redirect allowlist, the hash
//! domain, and what a spent row must say to sign anyone in. The database side
//! — single use under concurrent exchanges — is `cli_login::spend`, shared
//! with `oxyc login` and covered by `oxy-server`'s `token_auth` group.

use super::*;

/// RFC 7636 appendix B.
const VERIFIER: &str = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
const REDIRECT: &str = "oxygen://auth";

fn row(mint: Option<Value>, expires_in_secs: i64) -> cli_auth_codes::Model {
    let now = Utc::now();
    cli_auth_codes::Model {
        code_hash: hash_code("code"),
        user_id: Uuid::from_u128(7),
        code_challenge: challenge_of(VERIFIER),
        hostname: String::new(),
        created_at: now.fixed_offset(),
        expires_at: (now + Duration::seconds(expires_in_secs)).fixed_offset(),
        consumed_at: Some(now.fixed_offset()),
        mint,
    }
}

fn mobile_row(redirect_uri: &str) -> cli_auth_codes::Model {
    row(
        Some(json!({ "kind": CODE_KIND, "redirect_uri": redirect_uri })),
        CODE_TTL_SECS,
    )
}

#[test]
fn only_the_oxygen_scheme_is_a_redirect_uri() {
    for ok in [
        "oxygen://auth",
        // The Oxygen Dev Android app's callback.
        "oxygen://auth-dev",
        "  oxygen://auth  ",
        "oxygen://auth/callback",
        "OXYGEN://auth",
        "oxygen://",
    ] {
        assert_eq!(
            clean_redirect_uri(ok),
            Some(ok.trim().to_string()),
            "{ok:?} should be accepted"
        );
    }
    for bad in [
        "",
        "   ",
        // Expo's schemes: the apps are native now, and these are not theirs.
        "exp://192.168.1.4:8081/--/auth",
        "exp+oxygen://auth",
        "https://evil.example.com/callback",
        "http://127.0.0.1:53124/callback",
        "javascript:alert(1)",
        "oxygen:auth",
        "oxygen:/auth",
        "oxygenx://auth",
        "xoxygen://auth",
        "oxygen.evil://auth",
        // The page appends the query; one already there would come back broken.
        "oxygen://auth?next=https://evil.example.com",
        "oxygen://auth#frag",
        "oxygen://user@auth",
        "oxygen://auth\\x",
        "oxygen://a b",
        "oxygen://au\nth",
        "oxygen://au\tth",
        "oxygen://é",
    ] {
        assert_eq!(clean_redirect_uri(bad), None, "{bad:?} should be refused");
    }
    let long = format!("oxygen://{}", "a".repeat(MAX_REDIRECT_URI_LEN));
    assert_eq!(clean_redirect_uri(&long), None, "an overlong URI");
}

#[test]
fn a_mobile_code_is_never_found_as_an_oxyc_code_or_the_reverse() {
    // `oxyc login`'s exchange looks a code up by these two hashes alone; a
    // mobile code must be under neither, and a CLI code never under this one.
    let code = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
    assert_ne!(hash_code(code), cli_login::hash_code(code));
    assert_ne!(hash_code(code), cli_login::hash_mint_code(code));
    assert_eq!(hash_code(code).len(), 32);
}

#[test]
fn the_right_verifier_and_redirect_sign_the_user_in() {
    let now = Utc::now();
    let signed_in = honour(&mobile_row(REDIRECT), VERIFIER, REDIRECT, now);
    assert_eq!(signed_in, Some(Uuid::from_u128(7)));
    // The app may send the URI with stray whitespace; it is cleaned the same way.
    let padded = honour(&mobile_row(REDIRECT), VERIFIER, " oxygen://auth ", now);
    assert_eq!(padded, Some(Uuid::from_u128(7)));
}

#[test]
fn a_verifier_that_does_not_hash_to_the_challenge_signs_no_one_in() {
    let now = Utc::now();
    let row = mobile_row(REDIRECT);
    let other = "a".repeat(43);
    for bad in [other.as_str(), "", "short", &VERIFIER[1..]] {
        assert_eq!(honour(&row, bad, REDIRECT, now), None, "{bad:?}");
    }
    // The challenge itself is not the verifier.
    let challenge = challenge_of(VERIFIER);
    assert_eq!(honour(&row, &challenge, REDIRECT, now), None);
}

#[test]
fn the_code_is_bound_to_the_redirect_uri_it_was_issued_for() {
    let now = Utc::now();
    let row = mobile_row(REDIRECT);
    for other in [
        "oxygen://auth/other",
        "oxygen://evil",
        "https://evil.example.com",
        "",
    ] {
        assert_eq!(honour(&row, VERIFIER, other, now), None, "{other:?}");
    }
}

#[test]
fn an_expired_code_signs_no_one_in() {
    let mut lapsed = mobile_row(REDIRECT);
    lapsed.expires_at = (Utc::now() - Duration::seconds(1)).fixed_offset();
    assert_eq!(honour(&lapsed, VERIFIER, REDIRECT, Utc::now()), None);
}

#[test]
fn another_kind_of_row_is_no_mobile_code() {
    let now = Utc::now();
    let others = [
        // An `oxyc login` code, a sandbox mint, and a browser ticket.
        row(None, CODE_TTL_SECS),
        row(
            Some(json!({ "kind": "sandbox_agent", "redirect_uri": REDIRECT })),
            CODE_TTL_SECS,
        ),
        row(
            Some(json!({ "kind": "browser_session", "token_id": Uuid::nil() })),
            CODE_TTL_SECS,
        ),
        // A mobile kind that names no redirect URI.
        row(Some(json!({ "kind": CODE_KIND })), CODE_TTL_SECS),
    ];
    for other in others {
        assert_eq!(
            honour(&other, VERIFIER, REDIRECT, now),
            None,
            "{:?}",
            other.mint
        );
    }
}
