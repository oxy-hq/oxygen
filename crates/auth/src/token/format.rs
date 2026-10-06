//! Token formats: generate, recognise, checksum, hash and display.
//!
//! A new-format token is `<prefix><30 base62 random><6 base62 CRC32>` (design
//! §3.1), where the CRC32 covers everything before it, prefix included. The
//! checksum lets a secret scanner — or this server, before any database work —
//! reject a lookalike offline. It is integrity, not authenticity: the random
//! part is what makes a token unguessable.
//!
//! Two legacy formats stay valid: `oxy_<32 hex>` (the API key the legacy
//! endpoint minted until Phase 1) and `oxypublish_<64 hex>` (the custom-app
//! publish token). Only a SHA-256 of any token is ever stored.

use sha2::{Digest, Sha256};

/// Personal access token prefix.
pub const PAT_PREFIX: &str = "oxy_pat_";
/// Service-account token prefix.
pub const SAT_PREFIX: &str = "oxy_sat_";
/// Trusted-access (OIDC-exchanged) token prefix (Phase 4; recognised only).
pub const CI_PREFIX: &str = "oxy_ci_";
/// Sandbox agent token prefix (sandbox agent credential design §2).
pub const SBX_PREFIX: &str = "oxy_sbx_";

/// The pattern a secret scanner registers for every new-format token (API-tokens
/// design §8 Phase 5; sandbox agent credential design §2): each checksummed
/// prefix, then the [`RANDOM_LEN`] + [`CHECKSUM_LEN`] base62 characters. A new
/// kind is added here with its prefix; `format_tests` fails until it is.
pub const SCANNER_PATTERN: &str = "oxy_(pat|sat|ci|sbx)_[0-9A-Za-z]{36}";

const LEGACY_KEY_PREFIX: &str = "oxy_";
const LEGACY_KEY_HEX_LEN: usize = 32;
const LEGACY_PUBLISH_PREFIX: &str = "oxypublish_";
const LEGACY_PUBLISH_HEX_LEN: usize = 64;

/// Base62 characters of randomness in a new-format token: ~178 bits.
pub const RANDOM_LEN: usize = 30;
/// Base62 characters of checksum. 62^6 > 2^32, so any CRC32 fits.
pub const CHECKSUM_LEN: usize = 6;
/// How many leading body characters the display prefix keeps.
const DISPLAY_BODY_CHARS: usize = 4;

const BASE62: &[u8; 62] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

/// What a presented credential claims to be, judged by its shape alone.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TokenFormat {
    /// `oxy_pat_…`
    Personal,
    /// `oxy_sat_…`
    ServiceAccount,
    /// `oxy_ci_…`
    Ci,
    /// `oxy_sbx_…`
    SandboxAgent,
    /// `oxy_<32 hex>`, and — for lookup purposes — any other value presented
    /// as an API key, since the legacy endpoint matched raw `api_keys` rows.
    LegacyKey,
    /// `oxypublish_<64 hex>`
    LegacyPublish,
}

impl TokenFormat {
    /// True for the four checksummed prefixes. Only these carry the
    /// no-fallthrough rule (design §4.1); legacy keys keep today's precedence.
    pub fn is_new(self) -> bool {
        matches!(
            self,
            Self::Personal | Self::ServiceAccount | Self::Ci | Self::SandboxAgent
        )
    }

    /// The literal prefix for a new format; `None` for the legacy ones.
    pub fn new_prefix(self) -> Option<&'static str> {
        match self {
            Self::Personal => Some(PAT_PREFIX),
            Self::ServiceAccount => Some(SAT_PREFIX),
            Self::Ci => Some(CI_PREFIX),
            Self::SandboxAgent => Some(SBX_PREFIX),
            Self::LegacyKey | Self::LegacyPublish => None,
        }
    }
}

/// The new-format family a value claims by prefix, malformed or not.
///
/// Deliberately a prefix test only: a value that *claims* `oxy_pat_` and fails
/// any later check must still be answered 401 rather than retried as a cookie,
/// so this must not reject a malformed body.
pub fn new_prefix_format(candidate: &str) -> Option<TokenFormat> {
    [
        TokenFormat::Personal,
        TokenFormat::ServiceAccount,
        TokenFormat::Ci,
        TokenFormat::SandboxAgent,
    ]
    .into_iter()
    .find(|f| f.new_prefix().is_some_and(|p| candidate.starts_with(p)))
}

/// Strict recognition of every known format. New prefixes are recognised by
/// prefix (see [`new_prefix_format`]); the legacy formats by their exact shape.
pub fn parse_format(candidate: &str) -> Option<TokenFormat> {
    if let Some(format) = new_prefix_format(candidate) {
        return Some(format);
    }
    if is_hex_after(candidate, LEGACY_PUBLISH_PREFIX, LEGACY_PUBLISH_HEX_LEN) {
        return Some(TokenFormat::LegacyPublish);
    }
    if is_hex_after(candidate, LEGACY_KEY_PREFIX, LEGACY_KEY_HEX_LEN) {
        return Some(TokenFormat::LegacyKey);
    }
    None
}

/// True when `candidate` is exactly `prefix` + `len` lowercase hex characters.
fn is_hex_after(candidate: &str, prefix: &str, len: usize) -> bool {
    candidate.strip_prefix(prefix).is_some_and(|body| {
        body.len() == len
            && body
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}

/// Verify a new-format token offline: known prefix, exact length, base62 body,
/// and a checksum that matches. False for every legacy format.
pub fn verify_checksum(token: &str) -> bool {
    let Some(prefix) = new_prefix_format(token).and_then(TokenFormat::new_prefix) else {
        return false;
    };
    let body = &token[prefix.len()..];
    if body.len() != RANDOM_LEN + CHECKSUM_LEN || !body.bytes().all(|b| BASE62.contains(&b)) {
        return false;
    }
    let (signed, checksum) = token.split_at(token.len() - CHECKSUM_LEN);
    base62_fixed(crc32(signed.as_bytes()), CHECKSUM_LEN) == checksum
}

/// A freshly minted token. The plaintext is handed to the caller once and
/// never persisted; the rest is what gets stored.
#[derive(Clone, Debug)]
pub struct GeneratedToken {
    pub plaintext: String,
    /// SHA-256 of `plaintext`.
    pub token_hash: Vec<u8>,
    pub display_prefix: String,
    pub last_four: String,
}

/// Mint a personal access token (`oxy_pat_…`).
pub fn generate_personal() -> GeneratedToken {
    generate_with_prefix(PAT_PREFIX)
}

/// Mint a service-account token (`oxy_sat_…`).
pub fn generate_service_account() -> GeneratedToken {
    generate_with_prefix(SAT_PREFIX)
}

/// Mint a trusted-access token (`oxy_ci_…`).
pub fn generate_ci() -> GeneratedToken {
    generate_with_prefix(CI_PREFIX)
}

/// Mint a sandbox agent token (`oxy_sbx_…`).
pub fn generate_sandbox_agent() -> GeneratedToken {
    generate_with_prefix(SBX_PREFIX)
}

/// Mint a legacy API key: `oxy_` + 32 lowercase hex characters, the shape the
/// legacy endpoint has always returned. A legacy key is never a token — it has
/// no checksum and is recognised by this exact shape ([`parse_format`]).
pub fn generate_legacy_key() -> String {
    format!("{LEGACY_KEY_PREFIX}{}", uuid::Uuid::new_v4().simple())
}

pub(crate) fn generate_with_prefix(prefix: &str) -> GeneratedToken {
    let mut signed = String::with_capacity(prefix.len() + RANDOM_LEN + CHECKSUM_LEN);
    signed.push_str(prefix);
    signed.push_str(&random_base62(RANDOM_LEN));
    let checksum = base62_fixed(crc32(signed.as_bytes()), CHECKSUM_LEN);
    let plaintext = signed + &checksum;
    GeneratedToken {
        token_hash: hash_token(&plaintext),
        display_prefix: display_prefix(&plaintext),
        last_four: last_four(&plaintext),
        plaintext,
    }
}

/// `len` base62 characters from the thread-local CSPRNG (ChaCha seeded from the
/// OS — the same source the publish tokens use). Rejection sampling keeps the
/// distribution uniform: 248 is the largest multiple of 62 below 256.
fn random_base62(len: usize) -> String {
    let mut out = String::with_capacity(len);
    while out.len() < len {
        let bytes: [u8; 32] = rand::random();
        for b in bytes {
            if b < 248 && out.len() < len {
                out.push(BASE62[usize::from(b % 62)] as char);
            }
        }
    }
    out
}

/// SHA-256 of the whole token: the only form any table stores, and the value
/// an audit row may carry as `hashed_token`.
pub fn hash_token(token: &str) -> Vec<u8> {
    Sha256::digest(token.as_bytes()).to_vec()
}

/// Non-secret leading fragment for lists and audit rows.
///
/// New format: the prefix plus four body characters (`oxy_pat_Ab3x`). Anything
/// else: its first four characters, which for a legacy key is just `oxy_` —
/// the same exposure the legacy endpoint's masked key already had. Mirrored by
/// the backfill SQL (`left(key_hash, 4)`), so the two must change together.
pub fn display_prefix(token: &str) -> String {
    let keep = match new_prefix_format(token).and_then(TokenFormat::new_prefix) {
        Some(prefix) => prefix.len() + DISPLAY_BODY_CHARS,
        None => DISPLAY_BODY_CHARS,
    };
    token.chars().take(keep).collect()
}

/// The last four characters, or nothing for a value of eight characters or
/// fewer (it would give away too much of it). Mirrored by the backfill SQL.
pub fn last_four(token: &str) -> String {
    let count = token.chars().count();
    if count <= 8 {
        return String::new();
    }
    token.chars().skip(count - 4).collect()
}

/// CRC-32/ISO-HDLC (the zlib/PNG CRC), bitwise. Small and dependency-free; a
/// token is ~40 bytes, so a lookup table would buy nothing measurable.
fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

/// `n` in base62, left-padded with `0` to exactly `width` characters.
fn base62_fixed(mut n: u32, width: usize) -> String {
    let mut digits = vec![b'0'; width];
    for slot in digits.iter_mut().rev() {
        *slot = BASE62[(n % 62) as usize];
        n /= 62;
    }
    String::from_utf8(digits).expect("base62 alphabet is ASCII")
}

#[cfg(test)]
#[path = "format_tests.rs"]
mod tests;
