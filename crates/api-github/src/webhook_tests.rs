use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;

use super::signature_is_valid;

const SECRET: &[u8] = b"whsec_test";
const BODY: &[u8] = br#"{"ref":"refs/heads/main"}"#;

fn sign(secret: &[u8], body: &[u8]) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret).unwrap();
    mac.update(body);
    format!("sha256={}", hex::encode(mac.finalize().into_bytes()))
}

#[test]
fn a_signature_made_with_the_secret_over_the_exact_body_is_valid() {
    assert!(signature_is_valid(SECRET, BODY, Some(&sign(SECRET, BODY))));
}

#[test]
fn a_different_secret_or_body_is_not() {
    let good = sign(SECRET, BODY);
    assert!(!signature_is_valid(b"another", BODY, Some(&good)));
    // One byte of whitespace changes what was signed.
    assert!(!signature_is_valid(
        SECRET,
        b"{\"ref\":\"refs/heads/main\"} ",
        Some(&good)
    ));
}

#[test]
fn a_missing_or_malformed_header_is_not() {
    let good = sign(SECRET, BODY);
    let hex_only = good.trim_start_matches("sha256=").to_string();
    for header in [
        None,
        Some(""),
        Some("sha256="),
        Some("sha256=zz"),
        // The scheme is part of the contract: the legacy SHA-1 header's form
        // and a bare digest are both refused.
        Some(hex_only.as_str()),
        Some("sha1=0123"),
    ] {
        assert!(!signature_is_valid(SECRET, BODY, header), "{header:?}");
    }
}

#[test]
fn an_empty_secret_cannot_be_used_to_sign() {
    // The handler treats an empty secret as "not configured" before this is
    // reached; this pins that the check itself does not accept a signature
    // made with no key for a body signed with a real one.
    assert!(!signature_is_valid(b"", BODY, Some(&sign(SECRET, BODY))));
}
