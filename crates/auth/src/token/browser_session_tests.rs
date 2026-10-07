//! Unit tests for the token session's JWT and the ticket's hash. The database
//! side — the ticket's single use, and a session deciding as its token — is
//! `oxy-server`'s `token_auth::browser_session`.

use super::*;
use crate::constants::AUTHENTICATION_SECRET_KEY;
use crate::token::credential::source;
use crate::token::format::hash_token;

fn token_row(secret: &str) -> api_tokens::Model {
    api_tokens::Model {
        id: Uuid::new_v4(),
        kind: "personal".into(),
        principal_user_id: Uuid::new_v4(),
        name: "agent".into(),
        display_prefix: "oxy_pat_Ab3x".into(),
        last_four: "wxyz".into(),
        token_hash: hash_token(secret),
        all_access: true,
        platform: false,
        partner: false,
        expires_at: None,
        last_used_at: None,
        created_at: Utc::now().fixed_offset(),
        created_by: None,
        revoked_at: None,
        revoked_by: None,
        revoke_reason: None,
        source: source::UI.into(),
        legacy_api_key_id: None,
        trust_policy_id: None,
        oidc_claims: None,
    }
}

/// A JWT with `claims`, signed with `key`, naming `kid` — what an attacker who
/// holds a key (or guesses one) can produce.
fn forged(row: &api_tokens::Model, key: &[u8], kid: Option<String>, exp_in: i64) -> String {
    let now = Utc::now().timestamp();
    let claims = Claims {
        sub: row.principal_user_id.to_string(),
        email: "a@example.com".into(),
        exp: (now + exp_in) as usize,
        iat: now as usize,
        tid: row.id,
    };
    let header = Header {
        kid,
        ..Header::default()
    };
    encode(&header, &claims, &EncodingKey::from_secret(key)).expect("sign")
}

/// Now, on a whole second: a JWT's `exp` holds no fraction, so a fixture that
/// does compares unequal to what comes back by up to a second.
fn whole_second_now() -> DateTime<Utc> {
    DateTime::from_timestamp(Utc::now().timestamp(), 0).expect("now")
}

fn kid_of(row: &api_tokens::Model) -> Option<String> {
    Some(format!("{KID_PREFIX}{}", row.id))
}

#[test]
fn a_minted_session_names_its_token_and_verifies_against_its_row() {
    let row = token_row("oxy_pat_one");
    let now = whole_second_now();
    let session = mint(&row, "a@example.com", now).expect("mint");

    assert_eq!(token_id_of(&session.jwt), Some(row.id));
    assert_eq!(verify(&session.jwt, &row), Ok(session.expires_at));
    // Twelve hours, to the second the JWT carries.
    assert_eq!(
        session.expires_at,
        now + Duration::seconds(SESSION_TTL_SECS)
    );
    assert_eq!(session.max_age_secs(now), SESSION_TTL_SECS);
}

#[test]
fn a_session_never_outlives_its_token() {
    let now = whole_second_now();
    let lapses = now + Duration::minutes(20);
    let row = api_tokens::Model {
        expires_at: Some(lapses.fixed_offset()),
        ..token_row("oxy_pat_short")
    };
    let session = mint(&row, "a@example.com", now).expect("mint");
    assert_eq!(session.expires_at, lapses);
    assert_eq!(session.max_age_secs(now), 20 * 60);
}

#[test]
fn a_login_session_and_anything_that_is_not_a_jwt_name_no_token() {
    let row = token_row("oxy_pat_one");
    // A login session: the shared key, no `kid`.
    let login = forged(&row, AUTHENTICATION_SECRET_KEY.as_bytes(), None, 3600);
    assert_eq!(token_id_of(&login), None);
    for other in ["", "oxy_pat_abc", "not.a.jwt", "a.b"] {
        assert_eq!(token_id_of(other), None, "{other}");
    }
    // A `kid` that is not a token's is not routed here either.
    let foreign = forged(
        &row,
        AUTHENTICATION_SECRET_KEY.as_bytes(),
        Some("key-1".into()),
        3600,
    );
    assert_eq!(token_id_of(&foreign), None);
    let bad_id = forged(
        &row,
        AUTHENTICATION_SECRET_KEY.as_bytes(),
        Some(format!("{KID_PREFIX}not-a-uuid")),
        3600,
    );
    assert_eq!(token_id_of(&bad_id), None);
}

#[test]
fn knowing_a_tokens_id_is_not_enough_to_forge_its_session() {
    // The token's id is on every audit row, and the login-session key is a
    // constant in the source. A JWT that names the token and is signed with
    // that key is routed here by its `kid` and refused by its signature.
    let row = token_row("oxy_pat_one");
    let jwt = forged(
        &row,
        AUTHENTICATION_SECRET_KEY.as_bytes(),
        kid_of(&row),
        3600,
    );
    assert_eq!(token_id_of(&jwt), Some(row.id));
    assert!(verify(&jwt, &row).is_err());
}

#[test]
fn one_tokens_session_is_not_anothers() {
    let mine = token_row("oxy_pat_mine");
    let theirs = token_row("oxy_pat_theirs");
    let session = mint(&mine, "a@example.com", Utc::now()).expect("mint");
    // Their row, my JWT: the signature is under my token's key.
    assert!(verify(&session.jwt, &theirs).is_err());

    // My key, their token named in the claims: refused on the claims.
    let key = signing_key(&mine.token_hash);
    let crossed = forged(&theirs, &key, kid_of(&mine), 3600);
    assert_eq!(
        verify(&crossed, &mine),
        Err("a token session that names another token")
    );
}

#[test]
fn a_regenerated_token_ends_the_sessions_of_its_old_secret() {
    // Regenerating keeps the row's id and changes its hash, so the key does
    // not survive it: a session of the old secret stops with the old secret.
    let row = token_row("oxy_pat_before");
    let session = mint(&row, "a@example.com", Utc::now()).expect("mint");
    let regenerated = api_tokens::Model {
        token_hash: hash_token("oxy_pat_after"),
        ..row
    };
    assert!(verify(&session.jwt, &regenerated).is_err());
}

#[test]
fn an_expired_session_is_refused_with_no_leeway() {
    let row = token_row("oxy_pat_one");
    let key = signing_key(&row.token_hash);
    let lapsed = forged(&row, &key, kid_of(&row), -5);
    assert!(verify(&lapsed, &row).is_err());
    let live = forged(&row, &key, kid_of(&row), 60);
    assert!(verify(&live, &row).is_ok());
}

#[test]
fn a_ticket_is_stored_under_a_hash_no_login_exchange_computes() {
    // The login exchange looks a code up under these two hashes and no other,
    // in this release and the one before: a ticket is a row it cannot find.
    let ticket = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
    let stored = hash_ticket(ticket);
    assert_eq!(stored.len(), 32);
    assert_ne!(stored, cli_login::hash_code(ticket));
    assert_ne!(stored, cli_login::hash_mint_code(ticket));
}

#[test]
fn only_a_tickets_row_names_a_token() {
    let token_id = Uuid::new_v4();
    let ticket = json!({ "kind": TICKET_KIND, "token_id": token_id });
    assert_eq!(ticketed_token(Some(&ticket)), Some(token_id));

    // A login code has no `mint`; a sandbox mint is another kind.
    assert_eq!(ticketed_token(None), None);
    for other in [
        json!({ "kind": "sandbox_agent", "token_id": token_id }),
        json!({ "kind": TICKET_KIND }),
        json!({ "kind": TICKET_KIND, "token_id": "not-a-uuid" }),
        json!("browser_session"),
    ] {
        assert_eq!(ticketed_token(Some(&other)), None, "{other}");
    }
}

#[test]
fn a_session_is_cached_apart_from_every_presented_key() {
    // A presented key is cached under `hash_token` of a header value, which
    // cannot hold a NUL: no key, whatever it says, lands on a session's entry.
    let jwt = "eyJhbGciOiJIUzI1NiJ9.e30.sig";
    assert_ne!(cache_key(jwt), hash_token(jwt));
    assert_ne!(cache_key(jwt), cache_key("eyJhbGciOiJIUzI1NiJ9.e30.other"));
}
