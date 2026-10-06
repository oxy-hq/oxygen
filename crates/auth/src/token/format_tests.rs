//! Unit tests for token format, checksum and parse.

use super::*;

#[test]
fn crc32_matches_the_standard_check_value() {
    // The published check value for CRC-32/ISO-HDLC. If this drifts, every
    // checksum a scanner computes disagrees with ours.
    assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    assert_eq!(crc32(b""), 0);
}

#[test]
fn base62_is_fixed_width_and_covers_u32() {
    assert_eq!(base62_fixed(0, 6), "000000");
    assert_eq!(base62_fixed(61, 6), "00000z");
    assert_eq!(base62_fixed(62, 6), "000010");
    assert_eq!(base62_fixed(u32::MAX, 6).len(), 6);
}

#[test]
fn a_generated_pat_has_the_documented_shape() {
    let t = generate_personal();
    assert!(t.plaintext.starts_with(PAT_PREFIX));
    assert_eq!(
        t.plaintext.len(),
        PAT_PREFIX.len() + RANDOM_LEN + CHECKSUM_LEN
    );
    let body = &t.plaintext[PAT_PREFIX.len()..];
    assert!(body.bytes().all(|b| b.is_ascii_alphanumeric()));
    // The secret-scanning pattern design §8 Phase 5 registers.
    assert_eq!(body.len(), 36);
}

#[test]
fn a_generated_token_verifies_and_hashes_to_its_stored_hash() {
    let t = generate_personal();
    assert!(verify_checksum(&t.plaintext));
    assert_eq!(t.token_hash, hash_token(&t.plaintext));
    assert_eq!(t.token_hash.len(), 32);
}

#[test]
fn two_tokens_never_collide() {
    let a = generate_personal();
    let b = generate_personal();
    assert_ne!(a.plaintext, b.plaintext);
    assert_ne!(a.token_hash, b.token_hash);
}

#[test]
fn every_new_prefix_round_trips_its_checksum() {
    for prefix in [PAT_PREFIX, SAT_PREFIX, CI_PREFIX, SBX_PREFIX] {
        let t = generate_with_prefix(prefix);
        assert!(verify_checksum(&t.plaintext), "{prefix}");
    }
}

#[test]
fn a_single_changed_character_fails_the_checksum() {
    let t = generate_personal();
    let mut bytes = t.plaintext.clone().into_bytes();
    let i = PAT_PREFIX.len() + 3;
    bytes[i] = if bytes[i] == b'a' { b'b' } else { b'a' };
    let tampered = String::from_utf8(bytes).unwrap();
    assert!(!verify_checksum(&tampered));
}

#[test]
fn wrong_length_or_alphabet_fails_the_checksum() {
    let t = generate_personal();
    assert!(!verify_checksum(&t.plaintext[..t.plaintext.len() - 1]));
    assert!(!verify_checksum(&format!("{}x", t.plaintext)));
    let with_dash = format!("{}-{}", PAT_PREFIX, &t.plaintext[PAT_PREFIX.len() + 1..]);
    assert!(!verify_checksum(&with_dash));
    assert!(!verify_checksum(PAT_PREFIX));
}

#[test]
fn a_checksum_computed_without_the_prefix_is_refused() {
    // The CRC covers the prefix, so a body lifted from a pat and replayed under
    // another prefix does not verify.
    let t = generate_personal();
    let swapped = format!("{SAT_PREFIX}{}", &t.plaintext[PAT_PREFIX.len()..]);
    assert!(!verify_checksum(&swapped));
}

#[test]
fn legacy_formats_never_pass_the_new_checksum() {
    let legacy_key = format!("oxy_{}", "a".repeat(32));
    let legacy_publish = format!("oxypublish_{}", "0".repeat(64));
    assert!(!verify_checksum(&legacy_key));
    assert!(!verify_checksum(&legacy_publish));
}

#[test]
fn parse_recognises_every_kind() {
    assert_eq!(
        parse_format(&generate_personal().plaintext),
        Some(TokenFormat::Personal)
    );
    assert_eq!(
        parse_format(&generate_with_prefix(SAT_PREFIX).plaintext),
        Some(TokenFormat::ServiceAccount)
    );
    assert_eq!(
        parse_format(&generate_with_prefix(CI_PREFIX).plaintext),
        Some(TokenFormat::Ci)
    );
    let legacy = format!("oxy_{}", uuid::Uuid::new_v4().simple());
    assert_eq!(parse_format(&legacy), Some(TokenFormat::LegacyKey));
    let publish = format!("oxypublish_{}", "ab".repeat(32));
    assert_eq!(parse_format(&publish), Some(TokenFormat::LegacyPublish));
}

#[test]
fn parse_is_strict_about_legacy_shapes() {
    assert_eq!(parse_format("oxy_ABCDEF"), None);
    assert_eq!(parse_format(&format!("oxy_{}", "g".repeat(32))), None);
    assert_eq!(parse_format(&format!("oxy_{}", "a".repeat(31))), None);
    assert_eq!(
        parse_format(&format!("oxypublish_{}", "a".repeat(63))),
        None
    );
    assert_eq!(parse_format("eyJhbGciOiJIUzI1NiJ9.e30.x"), None);
    assert_eq!(parse_format(""), None);
}

#[test]
fn a_malformed_new_prefix_still_claims_its_family() {
    // The dispatch relies on this: a broken oxy_pat_ must 401, never fall
    // through to the cookie.
    assert_eq!(new_prefix_format("oxy_pat_"), Some(TokenFormat::Personal));
    assert_eq!(new_prefix_format("oxy_pat_!!"), Some(TokenFormat::Personal));
    assert_eq!(new_prefix_format("oxy_ci_x"), Some(TokenFormat::Ci));
    assert_eq!(
        new_prefix_format("oxy_sbx_x"),
        Some(TokenFormat::SandboxAgent)
    );
    assert_eq!(
        new_prefix_format("oxy_sat_x"),
        Some(TokenFormat::ServiceAccount)
    );
    // A legacy key is never mistaken for a new prefix: `p`, `s` and `i` are
    // not hex, so `oxy_<hex>` cannot start with any of them.
    assert_eq!(new_prefix_format(&format!("oxy_{}", "c".repeat(32))), None);
    assert_eq!(new_prefix_format("oxypublish_00"), None);
}

#[test]
fn only_new_formats_are_new() {
    assert!(TokenFormat::Personal.is_new());
    assert!(TokenFormat::ServiceAccount.is_new());
    assert!(TokenFormat::Ci.is_new());
    assert!(TokenFormat::SandboxAgent.is_new());
    assert!(!TokenFormat::LegacyKey.is_new());
    assert!(!TokenFormat::LegacyPublish.is_new());
}

#[test]
fn display_prefix_and_last_four() {
    let t = generate_personal();
    assert_eq!(t.display_prefix.len(), PAT_PREFIX.len() + 4);
    assert!(t.plaintext.starts_with(&t.display_prefix));
    assert!(t.plaintext.ends_with(&t.last_four));
    assert_eq!(t.last_four.len(), 4);

    let legacy = "oxy_0123456789abcdef0123456789abcdef";
    assert_eq!(display_prefix(legacy), "oxy_");
    assert_eq!(last_four(legacy), "cdef");
}

#[test]
fn short_values_reveal_nothing_at_the_tail() {
    assert_eq!(last_four("abcdefgh"), "");
    assert_eq!(last_four("abc"), "");
    assert_eq!(display_prefix("ab"), "ab");
    assert_eq!(last_four("abcdefghi"), "fghi");
}

#[test]
fn the_hash_is_sha256_of_the_exact_string() {
    // The migration backfills `sha256(convert_to(key_hash, 'UTF8'))`; lookup
    // must hash the presented string the same way, byte for byte.
    assert_eq!(
        hex::encode(hash_token("abc")),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}

#[test]
fn a_minted_legacy_key_has_the_legacy_shape_and_is_never_a_token() {
    let key = generate_legacy_key();
    assert_eq!(parse_format(&key), Some(TokenFormat::LegacyKey), "{key}");
    assert_eq!(
        new_prefix_format(&key),
        None,
        "a legacy key has no token prefix"
    );
    assert!(!key.starts_with(PAT_PREFIX));
    assert_ne!(key, generate_legacy_key(), "each mint is a new key");
    // What the list shows of it: `oxy_` and the last four, as before.
    assert_eq!(display_prefix(&key), "oxy_");
    assert_eq!(last_four(&key).len(), 4);
}

#[test]
fn a_sandbox_agent_token_has_its_own_prefix_and_the_shared_shape() {
    let token = generate_sandbox_agent();
    assert!(token.plaintext.starts_with("oxy_sbx_"));
    assert_eq!(
        token.plaintext.len(),
        SBX_PREFIX.len() + RANDOM_LEN + CHECKSUM_LEN
    );
    assert!(verify_checksum(&token.plaintext));
    assert_eq!(
        parse_format(&token.plaintext),
        Some(TokenFormat::SandboxAgent)
    );
    assert_eq!(token.token_hash, hash_token(&token.plaintext));
    // The display prefix keeps the family and four characters of the body.
    assert_eq!(token.display_prefix.len(), SBX_PREFIX.len() + 4);
    assert!(token.display_prefix.starts_with(SBX_PREFIX));
    // The checksum covers the prefix: the same body under another family's
    // prefix does not verify.
    let body = &token.plaintext[SBX_PREFIX.len()..];
    assert!(!verify_checksum(&format!("{PAT_PREFIX}{body}")));
}

/// Every new-format kind, by an exhaustive match: a new `TokenFormat` variant
/// does not compile here until it says whether a scanner must know it.
fn scanned_prefix(format: TokenFormat) -> Option<&'static str> {
    match format {
        TokenFormat::Personal
        | TokenFormat::ServiceAccount
        | TokenFormat::Ci
        | TokenFormat::SandboxAgent => format.new_prefix(),
        TokenFormat::LegacyKey | TokenFormat::LegacyPublish => None,
    }
}

const NEW_FORMATS: [TokenFormat; 4] = [
    TokenFormat::Personal,
    TokenFormat::ServiceAccount,
    TokenFormat::Ci,
    TokenFormat::SandboxAgent,
];

/// The scanner pattern is exactly the four prefixes and the 36-character
/// body. A kind left out of it leaks without the leak endpoint ever hearing.
#[test]
fn the_scanner_pattern_names_every_new_format_prefix_and_the_body_length() {
    let families: Vec<&str> = NEW_FORMATS
        .into_iter()
        .map(|format| {
            let prefix = scanned_prefix(format).expect("a new format");
            prefix
                .strip_prefix("oxy_")
                .and_then(|rest| rest.strip_suffix('_'))
                .expect("oxy_<family>_")
        })
        .collect();
    assert_eq!(families, ["pat", "sat", "ci", "sbx"]);
    let body = RANDOM_LEN + CHECKSUM_LEN;
    assert_eq!(body, 36);
    assert_eq!(
        SCANNER_PATTERN,
        format!("oxy_({})_[0-9A-Za-z]{{{body}}}", families.join("|"))
    );
}

/// What the pattern says, checked by hand against a minted token of each
/// kind: its prefix, then exactly 36 base62 characters.
#[test]
fn a_minted_token_of_every_kind_matches_the_scanner_pattern() {
    let minted = [
        generate_personal(),
        generate_service_account(),
        generate_ci(),
        generate_sandbox_agent(),
    ];
    for (token, format) in minted.iter().zip(NEW_FORMATS) {
        let prefix = scanned_prefix(format).expect("a new format");
        let body = token
            .plaintext
            .strip_prefix(prefix)
            .unwrap_or_else(|| panic!("a {prefix} token"));
        assert_eq!(body.len(), 36, "{prefix}");
        assert!(body.bytes().all(|b| b.is_ascii_alphanumeric()), "{prefix}");
    }
}
