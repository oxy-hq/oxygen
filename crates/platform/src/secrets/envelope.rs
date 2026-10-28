//! AES-256-GCM envelope sealing with the platform master key.
//!
//! The wire format is `nonce(12 bytes) || ciphertext`. Encrypted blobs are
//! self-describing for the nonce; rotation lives outside this layer (e.g.
//! `org_secrets.key_version`) when callers need it.
//!
//! Used by [`crate::secrets::OrgSecretsService`] for per-org secrets, and
//! by other crates that need to seal a small value (Airhouse SA bearers,
//! GitHub OAuth tokens, …) under the same master key.

use aes_gcm::aead::{Aead, Generate, Nonce as AeadNonce};
use aes_gcm::{Aes256Gcm, Key, KeyInit, Nonce};
use oxy_shared::errors::OxyError;

use super::encryption::get_encryption_key;

const NONCE_LEN: usize = 12;
const TAG_LEN: usize = 16;

/// Seal `plaintext` under the platform master key. Returns `nonce || ct`.
pub fn seal(plaintext: &[u8]) -> Result<Vec<u8>, OxyError> {
    seal_with_key(&get_encryption_key(), plaintext)
}

/// [`seal`] under a key the caller already holds, rather than the one the
/// loader finds. For the boot check (`super::key_check`) and its tests.
pub fn seal_with_key(key: &[u8; 32], plaintext: &[u8]) -> Result<Vec<u8>, OxyError> {
    let cipher = Aes256Gcm::new(&Key::<Aes256Gcm>::from(*key));
    let nonce = AeadNonce::<Aes256Gcm>::generate();
    let ciphertext = cipher
        .encrypt(&nonce, plaintext)
        .map_err(|e| OxyError::SecretManager(format!("encrypt failed: {e}")))?;
    let mut out = Vec::with_capacity(NONCE_LEN + ciphertext.len());
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

/// Open a blob produced by [`seal`].
pub fn open(blob: &[u8]) -> Result<Vec<u8>, OxyError> {
    open_with_key(&get_encryption_key(), blob)
}

/// [`open`] with a key the caller already holds.
pub fn open_with_key(key: &[u8; 32], blob: &[u8]) -> Result<Vec<u8>, OxyError> {
    if blob.len() < NONCE_LEN + TAG_LEN {
        return Err(OxyError::SecretManager("ciphertext too short".into()));
    }
    let (nonce_bytes, ct) = blob.split_at(NONCE_LEN);
    let cipher = Aes256Gcm::new(&Key::<Aes256Gcm>::from(*key));
    let nonce = <&Nonce<_>>::try_from(nonce_bytes)
        .map_err(|_| OxyError::SecretManager("ciphertext has a malformed nonce".to_string()))?;
    cipher
        .decrypt(nonce, ct)
        .map_err(|e| OxyError::SecretManager(format!("decrypt failed: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{Engine as _, engine::general_purpose};
    use std::sync::Mutex;

    static ENV_MUTEX: Mutex<()> = Mutex::new(());

    fn set_test_key() {
        // SAFETY: single-threaded test guarded by ENV_MUTEX.
        unsafe {
            std::env::set_var(
                "OXY_ENCRYPTION_KEY",
                general_purpose::STANDARD.encode([0u8; 32]),
            );
        }
    }

    #[test]
    fn round_trip_bytes() {
        let _g = ENV_MUTEX.lock().unwrap();
        set_test_key();
        let blob = seal(b"hello world").unwrap();
        assert_eq!(open(&blob).unwrap(), b"hello world");
    }

    #[test]
    fn nonce_is_unique_per_call() {
        let _g = ENV_MUTEX.lock().unwrap();
        set_test_key();
        assert_ne!(seal(b"x").unwrap(), seal(b"x").unwrap());
    }

    #[test]
    fn open_rejects_truncated_blob() {
        let _g = ENV_MUTEX.lock().unwrap();
        set_test_key();
        assert!(open(&[0u8; 5]).is_err());
    }

    #[test]
    fn open_rejects_tampered_blob() {
        let _g = ENV_MUTEX.lock().unwrap();
        set_test_key();
        let mut blob = seal(b"sensitive").unwrap();
        // Flip a byte in the ciphertext (after the 12-byte nonce).
        blob[NONCE_LEN] ^= 0x01;
        assert!(open(&blob).is_err());
    }
}
