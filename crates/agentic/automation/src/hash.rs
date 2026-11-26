//! Stable content hashing for automation state.
//!
//! These hashes are written to the database and compared across runs to
//! decide which steps can be reused on retry ("resume only unchanged steps").
//! Two properties are non-negotiable:
//!
//! 1. **Stable across Rust versions, platforms, and serializer revs.** A hash
//!    persisted today must match the same input hashed years from now on a
//!    different machine. `std::collections::hash_map::DefaultHasher` does not
//!    give this — it is an unspecified algorithm that may change between Rust
//!    releases. We use SHA-256 (FIPS 180-4) which is frozen.
//!
//! 2. **Insensitive to serialization noise.** Reformatting a YAML file or
//!    re-ordering map keys must not invalidate the cache. We canonicalize
//!    every value through RFC 8785 (JSON Canonicalization Scheme) before
//!    hashing — sorted keys, normalized numbers, deterministic Unicode.
//!
//! Use [`canonical_hash`] for any value that will end up persisted and later
//! compared. Use [`canonical_hash_pairs`] when composing several inputs
//! (step config + render context + variables + ...) into a single digest.

use serde::Serialize;
use sha2::{Digest, Sha256};

/// Compute the SHA-256 hex digest of a value's RFC 8785 canonical JSON form.
///
/// Returns lowercase hex (64 chars). Errors only if `value` is not
/// JSON-representable (e.g. a map with non-string keys, a non-finite float).
pub fn canonical_hash<T: Serialize>(value: &T) -> Result<String, HashError> {
    let canonical = serde_jcs::to_vec(value).map_err(HashError::Canonicalize)?;
    let mut hasher = Sha256::new();
    hasher.update(&canonical);
    Ok(hex_lower(hasher.finalize().as_slice()))
}

/// Compose several named inputs into a single SHA-256 digest.
///
/// The named-tuple form makes the hash self-describing in code review:
/// reading the call site tells you exactly what's in the digest. Keys are
/// sorted by JCS canonicalization, so order at the call site doesn't matter.
pub fn canonical_hash_pairs<T: Serialize>(pairs: &[(&str, &T)]) -> Result<String, HashError> {
    let map: serde_json::Map<String, serde_json::Value> = pairs
        .iter()
        .map(|(k, v)| {
            let json = serde_json::to_value(v).map_err(HashError::Canonicalize)?;
            Ok(((*k).to_string(), json))
        })
        .collect::<Result<_, HashError>>()?;
    canonical_hash(&serde_json::Value::Object(map))
}

#[derive(Debug, thiserror::Error)]
pub enum HashError {
    #[error("canonicalize: {0}")]
    Canonicalize(#[from] serde_json::Error),
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn deterministic_across_calls() {
        let v = json!({"b": 1, "a": [2, 3]});
        let h1 = canonical_hash(&v).unwrap();
        let h2 = canonical_hash(&v).unwrap();
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 64);
    }

    #[test]
    fn key_order_does_not_matter() {
        let a = json!({"b": 1, "a": 2});
        let b = json!({"a": 2, "b": 1});
        assert_eq!(canonical_hash(&a).unwrap(), canonical_hash(&b).unwrap());
    }

    #[test]
    fn whitespace_in_source_does_not_leak() {
        // Two equivalent values constructed differently — both hash the same.
        let a: serde_json::Value = serde_json::from_str("{\"x\":   1 }").unwrap();
        let b: serde_json::Value = serde_json::from_str("{\"x\":1}").unwrap();
        assert_eq!(canonical_hash(&a).unwrap(), canonical_hash(&b).unwrap());
    }

    #[test]
    fn distinct_values_distinct_hashes() {
        let a = json!({"x": 1});
        let b = json!({"x": 2});
        assert_ne!(canonical_hash(&a).unwrap(), canonical_hash(&b).unwrap());
    }

    #[test]
    fn pairs_match_explicit_object() {
        let step = json!({"sql": "select 1"});
        let ctx = json!({"foo": "bar"});

        let pairs = canonical_hash_pairs(&[("step", &step), ("ctx", &ctx)]).unwrap();
        let explicit = canonical_hash(&json!({"step": step, "ctx": ctx})).unwrap();
        assert_eq!(pairs, explicit);
    }

    #[test]
    fn pairs_order_independent() {
        let step = json!({"sql": "select 1"});
        let ctx = json!({"foo": "bar"});

        let h1 = canonical_hash_pairs(&[("step", &step), ("ctx", &ctx)]).unwrap();
        let h2 = canonical_hash_pairs(&[("ctx", &ctx), ("step", &step)]).unwrap();
        assert_eq!(h1, h2);
    }

    /// Ground-truth: hash of `{}` under JCS+SHA-256.
    ///
    /// JCS encodes `{}` as the two bytes `{}` (no whitespace). SHA-256 of
    /// those two bytes is the well-known constant below. If this test ever
    /// fails, the hash function has changed shape and every persisted hash
    /// in the database has been silently invalidated.
    #[test]
    fn empty_object_hash_is_frozen() {
        let h = canonical_hash(&json!({})).unwrap();
        assert_eq!(
            h,
            "44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a"
        );
    }

    /// An integer past 2^53 - 1 (a warehouse BIGINT id in a step's context, say)
    /// must still hash, to the digest it always had: serde_jcs 0.2 formats it
    /// through f64 (`9007199254740993` -> `9007199254740992`). serde_jcs 0.4
    /// refuses such integers instead, which would fail the automation run with
    /// "failed to hash automation config". See the pin in the workspace Cargo.toml.
    #[test]
    fn unsafe_integer_hashes_as_before() {
        let h = canonical_hash(&json!({"id": 9_007_199_254_740_993_u64})).unwrap();
        assert_eq!(
            h,
            "24bb430971eb50f964e63784a7ad4f3411bc7cdb1659188e371150793e872da1"
        );
    }
}
